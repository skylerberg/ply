//! The region allocator: a bump arena whose scopes are the program's regions.
//!
//! ADR 0017 §1 and §3. A region is a lexical allocation scope; the values
//! allocated in it live in a bump arena that is freed at the region's close, and
//! a region is one of two kinds:
//!
//! - **`unique`** — no continuation is captured across it. Nothing can reach its
//!   slots after its close, so allocation is a bump pointer, close is a
//!   truncation, and nothing is copied or counted.
//! - **`shared`** — a continuation may be captured across it and resumed after
//!   its close, so its slots may not go back to the bump pointer there.
//!
//! [`crate::region_kind`] decides which, conservatively.
//!
//! # What a slot is, and why it is not a pointer
//!
//! [`Slot`] is an index plus a generation, not an address. ADR 0005 §2 made a
//! cell a key rather than a pointer so that it could not dangle into freed
//! memory; a region that *does* free needs that property more, not less. A slot
//! whose region has closed reads `None` — deterministically, on every run —
//! rather than aliasing whatever the next region allocated in its place. The
//! generation is what makes that true: a physical position's generation only
//! ever rises, so a stale slot can never match the value now living there.
//!
//! # What `snapshot` and `restore` are, and what they are not
//!
//! They are a save-and-restore primitive over the arena's state. **They are not
//! on the continuation-capture path**, and putting them there would change what
//! programs mean.
//!
//! ADR 0017 §3 originally asked for snapshot-at-capture, so that each resumption
//! observed the region as of the capture. That reading was retracted: ADR 0005
//! §3 threads one state — resumption *n* observes resumption *n−1*'s writes —
//! and restoring at a resumption discards the clause's own write before the
//! computation that asked for it runs, which makes `put(5); get()` answer `0`
//! and the canonical cell-backed state handler unwritable.
//! `crates/ply-eval/tests/region_meaning_audit.rs` holds the programs that
//! distinguish the two, and every one of them asserts the threaded answer.
//!
//! What a `shared` region buys is therefore reclamation safety rather than
//! per-resumption state: its slots outlive its close because a continuation may
//! still reach them.
//!
//! Where an explicit checkpoint *is* wanted, it is [`Arena::snapshot_open`] and
//! never [`Arena::snapshot`]: a resumption may write any region open at the
//! point of the save, so covering only the innermost leaves the enclosing
//! regions' writes in place.

use crate::value::Value;
use ply_span::Span;
use std::fmt;

/// Slots per chunk.
///
/// Fixed rather than doubling so that a slot's index decomposes into (chunk,
/// offset) by a shift and a mask, and so that a chunk's `Vec` is allocated at
/// its final capacity once and never reallocates — a growing `Vec` would move
/// the values a live region is holding and would put an allocation back on the
/// path this module exists to take it off.
const CHUNK: usize = 256;

const fn chunk_of(index: usize) -> usize {
    index / CHUNK
}

const fn offset_of(index: usize) -> usize {
    index % CHUNK
}

/// Which of ADR 0017 §3's two kinds a region is.
///
/// The default is [`RegionKind::Shared`], and that is load-bearing rather than
/// arbitrary: inferring `unique` where a capture is reachable frees memory a
/// continuation can still reach, and the resulting defect is a use-after-free
/// through a resumption. Every path that does not *know* must land here.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub enum RegionKind {
    /// The compiler proved no continuation is captured across this region.
    Unique,
    #[default]
    Shared,
}

impl RegionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RegionKind::Unique => "unique",
            RegionKind::Shared => "shared",
        }
    }

    pub fn parse(s: &str) -> Option<RegionKind> {
        match s {
            "unique" => Some(RegionKind::Unique),
            "shared" => Some(RegionKind::Shared),
            _ => None,
        }
    }

    /// Whether a capture inside this region has to snapshot it.
    pub fn snapshots(self) -> bool {
        matches!(self, RegionKind::Shared)
    }
}

impl fmt::Display for RegionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A handle on an open region. Ordinal within one arena, so an inner region's
/// id is always greater than the region enclosing it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RegionId(pub u32);

impl fmt::Display for RegionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "r{}", self.0)
    }
}

/// A value allocated in a region. An index and a generation, never an address.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Slot {
    index: u32,
    generation: u32,
}

impl Slot {
    /// Ascending allocation order within one arena, so a caller that iterates
    /// slots iterates them identically on every run.
    pub fn index(self) -> u32 {
        self.index
    }

    pub fn generation(self) -> u32 {
        self.generation
    }
}

impl fmt::Display for Slot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@{}.{}", self.index, self.generation)
    }
}

#[derive(Clone, Copy)]
struct Scope {
    id: RegionId,
    kind: RegionKind,
    /// The bump pointer when the region opened. Closing truncates back to it.
    mark: usize,
    span: Span,
}

/// What the arena has cost, so "a bump pointer is free" stays a measurement.
///
/// `chunks_allocated` is the only field that counts a call to the global
/// allocator: everything else is a bump, a truncation or a generation bump. A
/// steady state that opens and closes regions of a bounded size allocates
/// nothing after the first pass, and [`Stats::chunks_allocated`] going flat is
/// what says so.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Stats {
    /// Chunks taken from the global allocator over the arena's whole life.
    pub chunks_allocated: usize,
    /// Slot allocations — bumps, not allocations.
    pub allocations: u64,
    /// Regions opened.
    pub regions_opened: u64,
    /// Snapshots taken.
    pub snapshots: u64,
    /// Slots copied by those snapshots. The cost §3 says is paid at the capture.
    pub slots_copied: u64,
    /// Snapshots restored.
    pub restores: u64,
    /// The high-water mark of live slots.
    pub peak_live: usize,
}

impl Stats {
    /// Bytes the arena holds from the global allocator: the chunks' value
    /// storage and their generation storage.
    pub fn bytes_reserved(&self) -> usize {
        self.chunks_allocated * CHUNK * (std::mem::size_of::<Value>() + std::mem::size_of::<u32>())
    }
}

/// A region's extent as it stood at some earlier point, and the scopes that
/// were open there.
///
/// Both halves are required. Restoring the values without the scope stack
/// resurrects the slots of a region that has closed since — with no region open
/// to free them again — and leaves a region opened since the snapshot holding a
/// `mark` above the bump pointer, where its close frees nothing and its extent
/// is a subtraction that underflows.
pub struct Snapshot {
    region: RegionId,
    /// The bump pointer at the snapshot's floor — where restoring truncates to.
    base: usize,
    /// The bump pointer when the snapshot was taken.
    top: usize,
    /// `values[i]` and `generations[i]` belong to index `base + i`.
    values: Vec<Value>,
    generations: Vec<u32>,
    /// The scopes at and above the snapshot's floor, innermost last. The scopes
    /// below it are untouched: a snapshot cannot outlive them, because closing
    /// one closes `region` too and [`Arena::restore`] then refuses.
    scopes: Vec<Scope>,
    /// Where `scopes` sits in the arena's own stack.
    depth: usize,
}

impl Snapshot {
    /// The region the snapshot is rooted at: the one whose close would discard
    /// it, and the outermost one it covers.
    pub fn region(&self) -> RegionId {
        self.region
    }

    /// Slots the snapshot copied. The cost of a capture, as a number.
    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Regions the snapshot covers: the one it names and everything nested
    /// inside it at the moment it was taken.
    pub fn regions(&self) -> usize {
        self.scopes.len()
    }
}

impl fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Snapshot({}, {}..{}, {} slots)",
            self.region,
            self.base,
            self.top,
            self.values.len()
        )
    }
}

/// A bump arena whose scopes are regions.
///
/// One per task: ADR 0017 §5 gives every task its own region stack, and a
/// [`Value`] is thread-confined already, so an arena is never shared.
pub struct Arena {
    /// `chunks[c][o]` is the value at index `c * CHUNK + o`. Every chunk below
    /// the live one is full, the live one holds `live % CHUNK`, and every chunk
    /// above it is empty but keeps its capacity — which is the whole of "close
    /// frees the region without giving the memory back".
    chunks: Vec<Vec<Value>>,
    /// Parallel to `chunks`, and **never truncated**: a physical position's
    /// generation only rises, so a slot from a closed region cannot match the
    /// value now living at its index.
    generations: Vec<Vec<u32>>,
    /// The bump pointer.
    live: usize,
    scopes: Vec<Scope>,
    next_region: u32,
    stats: Stats,
}

impl Default for Arena {
    fn default() -> Arena {
        Arena::new()
    }
}

impl Arena {
    pub fn new() -> Arena {
        Arena {
            chunks: Vec::new(),
            generations: Vec::new(),
            live: 0,
            scopes: Vec::new(),
            next_region: 0,
            stats: Stats::default(),
        }
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Live slots across every open region.
    pub fn live(&self) -> usize {
        self.live
    }

    /// Open regions, innermost last.
    pub fn depth(&self) -> usize {
        self.scopes.len()
    }

    /// Opens a region of the given kind.
    ///
    /// Regions nest: an inner region may read and write an outer region's slots,
    /// because closing the inner one truncates only back to its own mark. The
    /// converse — an outer region reading an inner one's slot after it closed —
    /// answers `None` rather than a stale value, and ADR 0017 §1 makes it a
    /// compile error before it can get here.
    pub fn open(&mut self, kind: RegionKind, span: Span) -> RegionId {
        let id = RegionId(self.next_region);
        self.next_region = self.next_region.wrapping_add(1);
        self.scopes.push(Scope {
            id,
            kind,
            mark: self.live,
            span,
        });
        self.stats.regions_opened += 1;
        id
    }

    pub fn kind(&self, region: RegionId) -> Option<RegionKind> {
        self.scope(region).map(|s| self.scopes[s].kind)
    }

    pub fn span(&self, region: RegionId) -> Option<Span> {
        self.scope(region).map(|s| self.scopes[s].span)
    }

    /// The innermost open region, or `None` outside every region.
    pub fn current(&self) -> Option<RegionId> {
        self.scopes.last().map(|s| s.id)
    }

    /// Slots this region and everything nested inside it are holding.
    pub fn extent(&self, region: RegionId) -> Option<usize> {
        self.scope(region).map(|s| {
            let mark = self.scopes[s].mark;
            debug_assert!(
                mark <= self.live,
                "an open region's mark sits above the bump pointer"
            );
            self.live.saturating_sub(mark)
        })
    }

    /// Allocates in the innermost open region. A bump: no allocation once the
    /// arena has been through a region of this size before.
    ///
    /// `None` outside every region, because a value with no region has no
    /// lifetime to be freed at — a caller that reaches this has lost its scope
    /// and must say so rather than allocate into nothing.
    pub fn alloc(&mut self, value: Value) -> Option<Slot> {
        if self.scopes.is_empty() {
            return None;
        }
        let index = self.live;
        let c = chunk_of(index);
        if c == self.chunks.len() {
            self.chunks.push(Vec::with_capacity(CHUNK));
            self.generations.push(vec![0; CHUNK]);
            self.stats.chunks_allocated += 1;
        }
        // Guaranteed not to reallocate: the chunk was created with `CHUNK`
        // capacity and holds `offset_of(index) < CHUNK` values.
        self.chunks[c].push(value);
        let generation = self.generations[c][offset_of(index)];
        self.live += 1;
        self.stats.allocations += 1;
        self.stats.peak_live = self.stats.peak_live.max(self.live);
        Some(Slot {
            index: index as u32,
            generation,
        })
    }

    pub fn get(&self, slot: Slot) -> Option<&Value> {
        let index = self.resolve(slot)?;
        self.chunks[chunk_of(index)].get(offset_of(index))
    }

    /// `false` when the slot's region has closed, which the caller must report
    /// rather than ignore — the same rule [`World::set`](crate::World::set)
    /// follows, and for the same reason.
    pub fn set(&mut self, slot: Slot, value: Value) -> bool {
        let Some(index) = self.resolve(slot) else {
            return false;
        };
        self.chunks[chunk_of(index)][offset_of(index)] = value;
        true
    }

    pub fn contains(&self, slot: Slot) -> bool {
        self.resolve(slot).is_some()
    }

    /// Closes `region` and every region nested inside it, freeing their slots.
    ///
    /// Nested regions are closed too rather than refused: a `perform` whose
    /// handler discards the continuation abandons every region the body had
    /// open, and the enclosing region's close is the only place left to reclaim
    /// them. Closing an already-closed region is a no-op, so a teardown that
    /// runs twice is not a second free.
    ///
    /// The chunks keep their capacity. Freeing a region drops the values it
    /// held — an `Arc` payload's refcount falls here, which is the whole of
    /// "freed at the region's close" — and gives the *slots* back to the bump
    /// pointer, not the memory back to the allocator.
    pub fn close(&mut self, region: RegionId) {
        let Some(at) = self.scope(region) else {
            return;
        };
        let mark = self.scopes[at].mark;
        self.scopes.truncate(at);
        self.truncate(mark, true);
    }

    /// Closes the innermost open region.
    pub fn close_current(&mut self) -> Option<RegionId> {
        let id = self.scopes.last()?.id;
        self.close(id);
        Some(id)
    }

    /// The extent of `region` and of everything nested inside it, as it stands
    /// now.
    ///
    /// **This is not what a capture takes.** A capture is reachable across every
    /// region open at it, not only across the one it names, so a snapshot of an
    /// inner region leaves an enclosing region's writes in place and the next
    /// resumption reads them; [`Arena::snapshot_open`] is the operation that
    /// covers them, and the one a capture path must call.
    ///
    /// `None` for a [`RegionKind::Unique`] region, and that refusal is the
    /// mechanism rather than a nicety: a unique region is one the compiler
    /// proved has no capture reachable across it, so a snapshot of one means the
    /// inference and the machine disagree about what the program does. The
    /// caller reports it instead of paying for a copy that should have been
    /// impossible.
    pub fn snapshot(&mut self, region: RegionId) -> Option<Snapshot> {
        let at = self.scope(region)?;
        if self.scopes[at].kind == RegionKind::Unique {
            return None;
        }
        Some(self.snapshot_from(at))
    }

    /// Every region open at this point — the snapshot a continuation capture
    /// has to take.
    ///
    /// A resumption may write any region open at the capture, not only the
    /// innermost, so covering less than the whole live arena leaves one
    /// resumption reading another's writes. That makes the cost of a capture
    /// the arena's *whole* live extent rather than one region's, which is what
    /// [`Stats::slots_copied`] reports and what
    /// `covering_every_open_region_costs_the_whole_live_arena` pins.
    ///
    /// `Err` names the innermost open region the compiler called `unique`. A
    /// capture is happening across it, so the inference and the machine
    /// disagree; the caller reports that rather than silently snapshotting a
    /// region whose kind said no snapshot could be needed. `Ok(None)` outside
    /// every region, where there is nothing to snapshot.
    pub fn snapshot_open(&mut self) -> Result<Option<Snapshot>, RegionId> {
        if let Some(scope) = self
            .scopes
            .iter()
            .rev()
            .find(|s| s.kind == RegionKind::Unique)
        {
            return Err(scope.id);
        }
        if self.scopes.is_empty() {
            return Ok(None);
        }
        Ok(Some(self.snapshot_from(0)))
    }

    fn snapshot_from(&mut self, at: usize) -> Snapshot {
        let base = self.scopes[at].mark;
        let region = self.scopes[at].id;
        let mut values = Vec::with_capacity(self.live - base);
        let mut generations = Vec::with_capacity(self.live - base);
        for index in base..self.live {
            let (c, o) = (chunk_of(index), offset_of(index));
            values.push(self.chunks[c][o].clone());
            generations.push(self.generations[c][o]);
        }
        self.stats.snapshots += 1;
        self.stats.slots_copied += values.len() as u64;
        Snapshot {
            region,
            base,
            top: self.live,
            values,
            generations,
            scopes: self.scopes[at..].to_vec(),
            depth: at,
        }
    }

    /// Re-installs a snapshot: the covered slots and the scopes that were open
    /// over them.
    ///
    /// Three halves, and all three are required:
    ///
    /// - the slots that existed at the snapshot get their values **and their
    ///   generations** back, so a `Slot` a captured continuation is holding
    ///   still resolves;
    /// - everything allocated after it is dropped **and its generation is
    ///   bumped**, so a slot allocated between the snapshot and the restore
    ///   cannot be read through an index that has been handed out again;
    /// - **the scope stack goes back to what it was.** A restore is a restore of
    ///   the arena's state, not of a bump range: without this a region closed
    ///   since the snapshot has its slots resurrected with nothing left to free
    ///   them, and a region opened since it survives with a `mark` above the
    ///   bump pointer, where its close truncates to a mark the arena is already
    ///   below and frees nothing at all.
    ///
    /// `false` when the snapshot's region has closed, which is a caller error
    /// rather than a silent no-op.
    pub fn restore(&mut self, snapshot: &Snapshot) -> bool {
        // The region must still be open *at the depth it was taken from*.
        // Anything else means an enclosing scope closed and reopened under a
        // fresh id, which cannot happen, or that the caller is restoring into an
        // arena this snapshot did not come from.
        if self.scopes.get(snapshot.depth).map(|s| s.id) != Some(snapshot.region) {
            return false;
        }
        // Everything allocated above the snapshot is freed with its generation
        // bumped; the snapshot's own slots are freed without one, because they
        // are about to be written back under the very identities a continuation
        // holds.
        self.truncate(snapshot.top, true);
        self.truncate(snapshot.base, false);
        for (i, value) in snapshot.values.iter().enumerate() {
            let index = snapshot.base + i;
            let (c, o) = (chunk_of(index), offset_of(index));
            self.chunks[c].push(value.clone());
            self.generations[c][o] = snapshot.generations[i];
            debug_assert_eq!(self.chunks[c].len(), o + 1);
        }
        self.live = snapshot.top;
        self.scopes.truncate(snapshot.depth);
        self.scopes.extend_from_slice(&snapshot.scopes);
        self.stats.restores += 1;
        true
    }

    /// Ascending by index — the order a differential comparison and a rendered
    /// artifact both need, and the same guarantee [`World::cells`] gives.
    ///
    /// [`World::cells`]: crate::World::cells
    pub fn slots(&self) -> impl Iterator<Item = (Slot, &Value)> {
        (0..self.live).map(move |index| {
            let (c, o) = (chunk_of(index), offset_of(index));
            (
                Slot {
                    index: index as u32,
                    generation: self.generations[c][o],
                },
                &self.chunks[c][o],
            )
        })
    }

    fn scope(&self, region: RegionId) -> Option<usize> {
        self.scopes.iter().position(|s| s.id == region)
    }

    /// The live index a slot names, or `None` if its region has closed.
    fn resolve(&self, slot: Slot) -> Option<usize> {
        let index = slot.index as usize;
        if index >= self.live {
            return None;
        }
        let (c, o) = (chunk_of(index), offset_of(index));
        (self.generations[c][o] == slot.generation).then_some(index)
    }

    /// Drops every slot at or above `mark`, keeping the chunks.
    ///
    /// `invalidate` bumps the freed positions' generations. It is `false` in
    /// exactly one place — [`Arena::restore`] undoing a resumption's writes over
    /// slots it is about to reinstate — where the identities must survive.
    fn truncate(&mut self, mark: usize, invalidate: bool) {
        if mark >= self.live {
            return;
        }
        if invalidate {
            for index in mark..self.live {
                let (c, o) = (chunk_of(index), offset_of(index));
                self.generations[c][o] = self.generations[c][o].wrapping_add(1);
            }
        }
        let first = chunk_of(mark);
        let last = chunk_of(self.live.saturating_sub(1));
        for c in first..=last.min(self.chunks.len().saturating_sub(1)) {
            let keep = if c == first { offset_of(mark) } else { 0 };
            self.chunks[c].truncate(keep);
        }
        self.live = mark;
    }
}

impl fmt::Debug for Arena {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Arena({} live, {} regions open, {} chunks)",
            self.live,
            self.scopes.len(),
            self.chunks.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A value whose payload is behind an `Arc`, so `strong_count` reports
    /// whether the arena freed it or is still holding it.
    fn payload(n: i64) -> (Arc<Vec<Value>>, Value) {
        let items = Arc::new(vec![Value::Int(n)]);
        (Arc::clone(&items), Value::List(items))
    }

    fn int_of(arena: &Arena, slot: Slot) -> i64 {
        match arena.get(slot) {
            Some(Value::Int(i)) => *i,
            other => panic!("expected an Int in {slot}, found {other:?}"),
        }
    }

    #[test]
    fn allocation_is_a_bump_and_close_gives_the_slots_back() {
        let mut arena = Arena::new();
        let r = arena.open(RegionKind::Unique, Span::DUMMY);
        for i in 0..64 {
            arena.alloc(Value::Int(i));
        }
        assert_eq!(arena.live(), 64);
        assert_eq!(arena.extent(r), Some(64));

        arena.close(r);

        assert_eq!(arena.live(), 0);
        assert_eq!(arena.depth(), 0);
        assert_eq!(arena.extent(r), None);
    }

    /// The free at the region's close is a real free: the values are dropped,
    /// which is what an `Arc` payload's refcount says.
    #[test]
    fn closing_a_region_drops_the_values_it_held() {
        let mut arena = Arena::new();
        let (arc, value) = payload(1);
        let r = arena.open(RegionKind::Unique, Span::DUMMY);
        arena.alloc(value);
        assert_eq!(
            Arc::strong_count(&arc),
            2,
            "the arena and this test hold it"
        );

        arena.close(r);

        assert_eq!(Arc::strong_count(&arc), 1, "the region's close freed it");
    }

    /// The point of a bump arena: the second region through costs the allocator
    /// nothing. `chunks_allocated` is the only field that counts a call to the
    /// global allocator, so it going flat is the claim.
    #[test]
    fn a_second_region_of_the_same_size_allocates_nothing() {
        let mut arena = Arena::new();
        for _ in 0..4 {
            let r = arena.open(RegionKind::Unique, Span::DUMMY);
            for i in 0..1_000 {
                arena.alloc(Value::Int(i));
            }
            arena.close(r);
        }
        let after_warm = arena.stats().chunks_allocated;

        for _ in 0..1_000 {
            let r = arena.open(RegionKind::Unique, Span::DUMMY);
            for i in 0..1_000 {
                arena.alloc(Value::Int(i));
            }
            arena.close(r);
        }

        assert_eq!(
            arena.stats().chunks_allocated,
            after_warm,
            "a thousand more regions of a size the arena has already seen took no chunk"
        );
        assert_eq!(arena.stats().allocations, 1_004_000);
    }

    #[test]
    fn a_slot_from_a_closed_region_reads_nothing_rather_than_the_value_after_it() {
        let mut arena = Arena::new();
        let first = arena.open(RegionKind::Unique, Span::DUMMY);
        let stale = arena.alloc(Value::Int(1)).expect("inside a region");
        arena.close(first);

        let second = arena.open(RegionKind::Unique, Span::DUMMY);
        let fresh = arena.alloc(Value::Int(2)).expect("inside a region");

        assert_eq!(
            stale.index(),
            fresh.index(),
            "the bump pointer reused the position, which is why the generation matters"
        );
        assert!(arena.get(stale).is_none());
        assert!(!arena.set(stale, Value::Int(99)));
        assert_eq!(int_of(&arena, fresh), 2);
        arena.close(second);
    }

    #[test]
    fn allocating_outside_every_region_is_refused() {
        let mut arena = Arena::new();
        assert!(arena.alloc(Value::Int(1)).is_none());
        assert_eq!(arena.stats().allocations, 0);
    }

    // ------------------------------------------------------------- nesting

    #[test]
    fn an_inner_region_reads_and_writes_an_outer_regions_values() {
        let mut arena = Arena::new();
        let outer = arena.open(RegionKind::Unique, Span::DUMMY);
        let a = arena.alloc(Value::Int(1)).expect("inside a region");

        let inner = arena.open(RegionKind::Unique, Span::DUMMY);
        let b = arena.alloc(Value::Int(2)).expect("inside a region");
        assert_eq!(int_of(&arena, a), 1);
        assert!(arena.set(a, Value::Int(10)));

        arena.close(inner);

        assert_eq!(int_of(&arena, a), 10, "the outer region kept its write");
        assert!(arena.get(b).is_none(), "the inner region's slot is gone");
        assert_eq!(arena.extent(outer), Some(1));
        arena.close(outer);
    }

    #[test]
    fn closing_an_outer_region_closes_the_inner_regions_still_open_inside_it() {
        let mut arena = Arena::new();
        let outer = arena.open(RegionKind::Unique, Span::DUMMY);
        arena.alloc(Value::Int(1));
        let mid = arena.open(RegionKind::Shared, Span::DUMMY);
        arena.alloc(Value::Int(2));
        let inner = arena.open(RegionKind::Unique, Span::DUMMY);
        let deep = arena.alloc(Value::Int(3)).expect("inside a region");

        arena.close(outer);

        assert_eq!(arena.depth(), 0);
        assert_eq!(arena.live(), 0);
        assert!(arena.get(deep).is_none());
        assert_eq!(arena.kind(mid), None);
        assert_eq!(arena.kind(inner), None);
    }

    #[test]
    fn closing_a_region_twice_is_not_a_second_free() {
        let mut arena = Arena::new();
        let outer = arena.open(RegionKind::Unique, Span::DUMMY);
        let kept = arena.alloc(Value::Int(7)).expect("inside a region");
        let inner = arena.open(RegionKind::Unique, Span::DUMMY);
        arena.alloc(Value::Int(8));

        arena.close(inner);
        arena.close(inner);

        assert_eq!(int_of(&arena, kept), 7);
        assert_eq!(arena.depth(), 1);
        arena.close(outer);
    }

    #[test]
    fn nesting_deeper_than_one_chunk_keeps_every_level_addressable() {
        let mut arena = Arena::new();
        let mut regions = Vec::new();
        let mut slots = Vec::new();
        for level in 0..32 {
            regions.push(arena.open(RegionKind::Unique, Span::DUMMY));
            for i in 0..40 {
                slots.push((
                    arena
                        .alloc(Value::Int(level * 100 + i))
                        .expect("inside a region"),
                    level * 100 + i,
                ));
            }
        }
        assert!(arena.live() > CHUNK * 4, "the test spans several chunks");
        for (slot, expected) in &slots {
            assert_eq!(int_of(&arena, *slot), *expected);
        }
        for region in regions.iter().rev() {
            arena.close(*region);
        }
        assert_eq!(arena.live(), 0);
        for (slot, _) in &slots {
            assert!(arena.get(*slot).is_none());
        }
    }

    // ------------------------------------------------------------ snapshots

    #[test]
    fn a_unique_region_refuses_to_be_snapshotted() {
        let mut arena = Arena::new();
        let r = arena.open(RegionKind::Unique, Span::DUMMY);
        arena.alloc(Value::Int(1));
        assert!(arena.snapshot(r).is_none());
        assert_eq!(arena.stats().snapshots, 0);
        arena.close(r);
    }

    /// The primitive's contract, stated over the sequence a checkpoint would
    /// use: after a restore the arena is exactly as the snapshot found it, and
    /// a slot allocated between the two is not readable through it.
    ///
    /// This is a property of [`Arena::restore`] and **not** the language's
    /// resumption semantics, which thread one state — see the module doc.
    #[test]
    fn a_restore_undoes_every_write_and_every_allocation_since_the_snapshot() {
        let mut arena = Arena::new();
        let r = arena.open(RegionKind::Shared, Span::DUMMY);
        let c = arena.alloc(Value::Int(0)).expect("inside a region");

        let at_capture = arena.snapshot(r).expect("a shared region snapshots");

        assert!(arena.set(c, Value::Int(1)));
        let made_after = arena.alloc(Value::Int(111)).expect("inside a region");
        assert_eq!(int_of(&arena, c), 1);

        assert!(arena.restore(&at_capture));
        assert_eq!(int_of(&arena, c), 0, "the write is undone");
        assert!(
            arena.get(made_after).is_none(),
            "and a slot allocated since the snapshot is not readable through it"
        );

        arena.set(c, Value::Int(2));
        assert_eq!(int_of(&arena, c), 2);
        arena.close(r);
    }

    /// A slot allocated after the snapshot and one allocated after the restore
    /// take the same physical position. They must not be one slot.
    #[test]
    fn allocations_either_side_of_a_restore_are_different_slots() {
        let mut arena = Arena::new();
        let r = arena.open(RegionKind::Shared, Span::DUMMY);
        arena.alloc(Value::Int(0));
        let at_capture = arena.snapshot(r).expect("a shared region snapshots");

        let first = arena.alloc(Value::Int(1)).expect("inside a region");
        arena.restore(&at_capture);
        let second = arena.alloc(Value::Int(2)).expect("inside a region");

        assert_eq!(first.index(), second.index());
        assert_ne!(first.generation(), second.generation());
        assert!(arena.get(first).is_none());
        assert_eq!(int_of(&arena, second), 2);
        arena.close(r);
    }

    /// The slot identities held from before the snapshot survive a restore, or
    /// whoever holds one reads `None` where it left a value.
    #[test]
    fn a_restore_keeps_the_slots_the_capture_was_holding() {
        let mut arena = Arena::new();
        let r = arena.open(RegionKind::Shared, Span::DUMMY);
        let held: Vec<Slot> = (0..300)
            .map(|i| arena.alloc(Value::Int(i)).expect("inside a region"))
            .collect();
        let at_capture = arena.snapshot(r).expect("a shared region snapshots");

        for slot in &held {
            arena.set(*slot, Value::Int(-1));
        }
        arena.restore(&at_capture);

        for (i, slot) in held.iter().enumerate() {
            assert_eq!(int_of(&arena, *slot), i as i64);
        }
        arena.close(r);
    }

    /// A snapshot of a region covers the regions nested inside it, because their
    /// slots sit above its mark. A capture that crosses an inner region must
    /// restore the inner region's writes too.
    #[test]
    fn a_snapshot_covers_the_regions_nested_inside_the_one_it_names() {
        let mut arena = Arena::new();
        let outer = arena.open(RegionKind::Shared, Span::DUMMY);
        let a = arena.alloc(Value::Int(1)).expect("inside a region");
        let inner = arena.open(RegionKind::Unique, Span::DUMMY);
        let b = arena.alloc(Value::Int(2)).expect("inside a region");

        let at_capture = arena.snapshot(outer).expect("a shared region snapshots");
        assert_eq!(at_capture.len(), 2);

        arena.set(a, Value::Int(10));
        arena.set(b, Value::Int(20));
        arena.restore(&at_capture);

        assert_eq!(int_of(&arena, a), 1);
        assert_eq!(int_of(&arena, b), 2);
        arena.close(inner);
        arena.close(outer);
    }

    /// The reason [`Arena::snapshot_open`] exists, written as the failure it
    /// prevents: a snapshot of the region a capture is *lexically* inside
    /// covers that region and no enclosing one, so a write the first resumption
    /// makes to an enclosing region survives the restore and the second
    /// resumption reads it.
    ///
    /// This is the canonical Ply shape — a handler's own state in `a`, the
    /// body's scratch state in `b` — and `b` is the region `Arena::current`
    /// names.
    #[test]
    fn a_snapshot_of_the_inner_region_leaves_the_enclosing_regions_writes_in_place() {
        let mut arena = Arena::new();
        let outer = arena.open(RegionKind::Shared, Span::DUMMY);
        let x = arena.alloc(Value::Int(0)).expect("inside a region");
        let inner = arena.open(RegionKind::Shared, Span::DUMMY);
        let y = arena.alloc(Value::Int(0)).expect("inside a region");

        let of_inner = arena.snapshot(inner).expect("a shared region snapshots");
        arena.set(x, Value::Int(1));
        arena.set(y, Value::Int(1));
        arena.restore(&of_inner);

        assert_eq!(int_of(&arena, y), 0, "the named region came back");
        assert_eq!(
            int_of(&arena, x),
            1,
            "and the enclosing region's write did not, which is why a capture may not use this"
        );

        // What a capture takes instead.
        arena.set(y, Value::Int(0));
        arena.set(x, Value::Int(0));
        let of_every = arena
            .snapshot_open()
            .expect("no open region is unique")
            .expect("two regions are open");
        assert_eq!(of_every.region(), outer, "rooted at the outermost");
        assert_eq!(of_every.regions(), 2);
        arena.set(x, Value::Int(1));
        arena.set(y, Value::Int(1));
        arena.restore(&of_every);
        assert_eq!(int_of(&arena, x), 0);
        assert_eq!(int_of(&arena, y), 0);
        arena.close(outer);
    }

    /// The cost, stated as the number it actually is rather than the one ADR
    /// 0017 §3 advertised. "Cost is paid at the capture, and only in regions
    /// where a capture actually happens" reads as the innermost region's size;
    /// the snapshot that isolates the resumptions is the whole live arena.
    #[test]
    fn covering_every_open_region_costs_the_whole_live_arena() {
        let mut arena = Arena::new();
        let outer = arena.open(RegionKind::Shared, Span::DUMMY);
        for i in 0..1_000 {
            arena.alloc(Value::Int(i));
        }
        let inner = arena.open(RegionKind::Shared, Span::DUMMY);
        arena.alloc(Value::Int(-1));

        let of_inner = arena.snapshot(inner).expect("a shared region snapshots");
        let of_every = arena
            .snapshot_open()
            .expect("no open region is unique")
            .expect("two regions are open");

        assert_eq!(of_inner.len(), 1, "the region the capture is written in");
        assert_eq!(of_every.len(), 1_001, "the one that actually isolates it");
        arena.close(outer);
    }

    /// A `unique` region open at a capture is the inference and the machine
    /// disagreeing, and the capture path has to be told which region so it can
    /// name it. Silently snapshotting it would hide the disagreement; silently
    /// skipping it would be the use-after-free.
    #[test]
    fn a_capture_across_a_unique_region_is_refused_and_names_it() {
        let mut arena = Arena::new();
        let outer = arena.open(RegionKind::Shared, Span::DUMMY);
        arena.alloc(Value::Int(1));
        let unique = arena.open(RegionKind::Unique, Span::DUMMY);
        arena.alloc(Value::Int(2));
        let inner = arena.open(RegionKind::Shared, Span::DUMMY);

        assert_eq!(arena.snapshot_open().err(), Some(unique));
        assert_eq!(arena.stats().snapshots, 0, "and nothing was copied");

        arena.close(inner);
        arena.close(unique);
        assert!(
            arena
                .snapshot_open()
                .expect("every open region is shared")
                .is_some()
        );
        arena.close(outer);
    }

    #[test]
    fn a_capture_outside_every_region_has_nothing_to_snapshot() {
        let mut arena = Arena::new();
        assert!(arena.snapshot_open().expect("nothing is unique").is_none());
        assert_eq!(arena.stats().snapshots, 0);
    }

    // ------------------------------------------------ restore and the scopes

    /// A restore restores the arena's state, not a bump range. A region that
    /// closed between the snapshot and the restore was open at the snapshot, so
    /// it comes back open — otherwise its slots are live with nothing left to
    /// free them, and a value that read `None` between two resumptions reads a
    /// live cell again during the second.
    #[test]
    fn a_restore_brings_a_closed_regions_scope_back_with_its_slots() {
        let mut arena = Arena::new();
        let r = arena.open(RegionKind::Shared, Span::DUMMY);
        arena.alloc(Value::Int(1));
        let inner = arena.open(RegionKind::Shared, Span::DUMMY);
        let x = arena.alloc(Value::Int(7)).expect("inside a region");

        let at_capture = arena.snapshot(r).expect("a shared region snapshots");

        arena.set(x, Value::Int(8));
        arena.close(inner);
        assert!(arena.get(x).is_none(), "the close freed it");
        assert_eq!(arena.depth(), 1);

        assert!(arena.restore(&at_capture));

        assert_eq!(int_of(&arena, x), 7, "the slot is back");
        assert_eq!(
            arena.kind(inner),
            Some(RegionKind::Shared),
            "and so is the region that owns it, or nothing frees it again"
        );
        assert_eq!(arena.depth(), 2);
        assert_eq!(arena.current(), Some(inner));

        arena.close(inner);
        assert!(arena.get(x).is_none());
        arena.close(r);
        assert_eq!(arena.live(), 0);
    }

    /// The other direction: a region opened after the snapshot did not exist at
    /// it, so it is gone after the restore. Left behind, its `mark` sits above
    /// the bump pointer, its close truncates to a mark the arena is already
    /// below and frees nothing, and its extent is a subtraction that underflows.
    #[test]
    fn a_region_opened_after_the_snapshot_does_not_survive_the_restore() {
        let mut arena = Arena::new();
        let r = arena.open(RegionKind::Shared, Span::DUMMY);
        arena.alloc(Value::Int(1));

        let at_capture = arena.snapshot(r).expect("a shared region snapshots");

        arena.alloc(Value::Int(2));
        let q = arena.open(RegionKind::Shared, Span::DUMMY);
        let stranded = arena.alloc(Value::Int(3)).expect("inside a region");

        assert!(arena.restore(&at_capture));

        assert_eq!(arena.live(), 1);
        assert_eq!(arena.depth(), 1, "`q` did not exist at the snapshot");
        assert_eq!(arena.current(), Some(r));
        assert_eq!(arena.kind(q), None);
        assert_eq!(arena.extent(q), None, "and its extent is not a subtraction");
        assert!(arena.get(stranded).is_none());

        // The allocation the next resumption makes is reclaimed by the region it
        // is actually in, rather than escaping to a mark nothing reaches.
        let again = arena.alloc(Value::Int(4)).expect("inside a region");
        assert_eq!(arena.extent(r), Some(2));
        arena.close(r);
        assert_eq!(arena.live(), 0);
        assert!(arena.get(again).is_none());
    }

    /// Every open region's mark stays at or below the bump pointer, which is
    /// the invariant `extent` and `snapshot` subtract under. Driven through the
    /// open/alloc/close/snapshot/restore cycle rather than asserted once.
    #[test]
    fn no_sequence_of_restores_strands_a_regions_mark_above_the_bump_pointer() {
        let mut arena = Arena::new();
        let root = arena.open(RegionKind::Shared, Span::DUMMY);
        let mut snaps = Vec::new();
        for round in 0..6 {
            arena.alloc(Value::Int(round));
            let inner = arena.open(RegionKind::Shared, Span::DUMMY);
            arena.alloc(Value::Int(round * 10));
            snaps.push(arena.snapshot(root).expect("a shared region snapshots"));
            if round % 2 == 0 {
                arena.close(inner);
            }
        }
        for snap in snaps.iter().rev() {
            assert!(arena.restore(snap));
            assert!(arena.extent(root).unwrap() <= arena.live());
            assert!(arena.snapshot_open().is_ok());
            arena.alloc(Value::Int(-1));
        }
        arena.close(root);
        assert_eq!(arena.live(), 0);
        assert_eq!(arena.depth(), 0);
    }

    /// The cost §3 says is paid at the capture, stated as a number: one copy per
    /// live slot of the region, and nothing per allocation.
    #[test]
    fn a_snapshot_copies_the_regions_slots_and_no_others() {
        let mut arena = Arena::new();
        let outer = arena.open(RegionKind::Shared, Span::DUMMY);
        for i in 0..500 {
            arena.alloc(Value::Int(i));
        }
        let inner = arena.open(RegionKind::Shared, Span::DUMMY);
        for i in 0..40 {
            arena.alloc(Value::Int(i));
        }

        let of_inner = arena.snapshot(inner).expect("a shared region snapshots");
        let of_outer = arena.snapshot(outer).expect("a shared region snapshots");

        assert_eq!(of_inner.len(), 40, "the inner region's own slots");
        assert_eq!(of_outer.len(), 540, "the outer region's, nesting included");
        assert_eq!(arena.stats().slots_copied, 580);
        arena.close(outer);
    }

    /// Linear in the region's size and in nothing else, so "cost is paid at the
    /// capture" is a measurement rather than a slogan.
    #[test]
    fn snapshot_cost_is_linear_in_the_regions_size() {
        for size in [0usize, 1, 100, 1_000, 10_000] {
            let mut arena = Arena::new();
            let r = arena.open(RegionKind::Shared, Span::DUMMY);
            for i in 0..size {
                arena.alloc(Value::Int(i as i64));
            }
            let before = arena.stats().slots_copied;
            let snap = arena.snapshot(r).expect("a shared region snapshots");
            assert_eq!(snap.len(), size);
            assert_eq!(arena.stats().slots_copied - before, size as u64);
            arena.close(r);
        }
    }

    /// A snapshot holds the payload rather than copying it: a `Value` clone is a
    /// refcount bump, so a snapshot of a region full of lists is proportional to
    /// the *slot count* and not to what the slots point at.
    #[test]
    fn a_snapshot_shares_payloads_rather_than_deep_copying_them() {
        let mut arena = Arena::new();
        let r = arena.open(RegionKind::Shared, Span::DUMMY);
        let mut kept = Vec::new();
        for i in 0..64 {
            let (arc, value) = payload(i);
            arena.alloc(value);
            kept.push(arc);
        }
        for arc in &kept {
            assert_eq!(Arc::strong_count(arc), 2);
        }

        let snap = arena.snapshot(r).expect("a shared region snapshots");

        for arc in &kept {
            assert_eq!(
                Arc::strong_count(arc),
                3,
                "the arena, the snapshot and this test — a deep copy would be more"
            );
        }
        drop(snap);
        arena.close(r);
        for arc in &kept {
            assert_eq!(Arc::strong_count(arc), 1);
        }
    }

    /// A snapshot that is dropped unused costs only itself: the region closes at
    /// its lexical end and its arena is freed as if nothing had been saved.
    #[test]
    fn a_snapshot_that_is_never_restored_costs_only_itself() {
        let mut arena = Arena::new();
        let (arc, value) = payload(1);
        let r = arena.open(RegionKind::Shared, Span::DUMMY);
        arena.alloc(value);
        let snap = arena.snapshot(r).expect("a shared region snapshots");

        drop(snap);
        arena.close(r);

        assert_eq!(Arc::strong_count(&arc), 1);
        assert_eq!(arena.live(), 0);
        assert_eq!(arena.stats().restores, 0);
    }

    #[test]
    fn restoring_into_a_closed_region_is_refused() {
        let mut arena = Arena::new();
        let r = arena.open(RegionKind::Shared, Span::DUMMY);
        arena.alloc(Value::Int(1));
        let snap = arena.snapshot(r).expect("a shared region snapshots");
        arena.close(r);

        assert!(!arena.restore(&snap));
        assert_eq!(arena.live(), 0);
    }

    #[test]
    fn slots_iterate_in_ascending_index_order() {
        let mut arena = Arena::new();
        let r = arena.open(RegionKind::Unique, Span::DUMMY);
        for i in 0..600 {
            arena.alloc(Value::Int(i));
        }
        let seen: Vec<u32> = arena.slots().map(|(slot, _)| slot.index()).collect();
        assert_eq!(seen, (0..600).collect::<Vec<u32>>());
        arena.close(r);
    }

    #[test]
    fn the_default_kind_is_shared() {
        assert_eq!(RegionKind::default(), RegionKind::Shared);
        assert!(RegionKind::default().snapshots());
        assert!(!RegionKind::Unique.snapshots());
        assert_eq!(RegionKind::parse("unique"), Some(RegionKind::Unique));
        assert_eq!(RegionKind::parse("shared"), Some(RegionKind::Shared));
        assert_eq!(RegionKind::parse("Unique"), None);
    }
}
