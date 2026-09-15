//! The region allocator: a bump arena whose scopes are the program's regions.

use crate::value::Value;
use ply_span::{Diagnostic, Span, codes};
use std::fmt;
use std::rc::Rc;

/// Slots per chunk.
pub const CHUNK: usize = 256;

/// Live pins tolerated before [`Arena::pin`] sweeps the dead ones.
const PIN_PRUNE_AT: usize = 256;

const fn chunk_of(index: usize) -> usize {
    index / CHUNK
}

const fn offset_of(index: usize) -> usize {
    index % CHUNK
}

/// Which of the region-kind rule's two kinds a region is.
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

/// A handle on an open region.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RegionId(pub u32);

impl fmt::Display for RegionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "r{}", self.0)
    }
}

/// A value allocated in a region.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Slot {
    index: u32,
    generation: u32,
}

impl Slot {
    /// A slot naming a physical position directly.
    pub fn new(index: u32, generation: u32) -> Slot {
        Slot { index, generation }
    }

    /// Ascending allocation order within one arena, so a caller that iterates slots iterates them
    /// identically on every run.
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
    /// The bump pointer when the region opened.
    mark: usize,
    /// Regions opened before this one, over the arena's whole life.
    ordinal: u64,
    span: Span,
}

/// A live continuation's claim on every region that was open when it was captured — the region-kind rule's
/// "reference counted, and reclaimed when the last continuation that can reach them dies".
#[derive(Clone)]
pub struct Pin(Rc<PinCore>);

struct PinCore {
    /// [`Stats::regions_opened`] at the capture.
    frontier: u64,
    /// The bump pointer at the capture, for [`Arena::restore`]'s guard and for reporting.
    top: usize,
}

impl Pin {
    /// Slots that were live when this continuation was captured.
    pub fn extent(&self) -> usize {
        self.0.top
    }

    /// Regions the arena had opened when this continuation was captured.
    pub fn frontier(&self) -> u64 {
        self.0.frontier
    }
}

impl fmt::Debug for Pin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Pin({} slots, {} regions, {} holders)",
            self.0.top,
            self.0.frontier,
            Rc::strong_count(&self.0).saturating_sub(1)
        )
    }
}

/// A run of slots whose regions have closed and whose memory a live [`Pin`] still covers.
struct Retained {
    lo: usize,
    hi: usize,
    /// The ordinal of the outermost region in the run — the one whose close created it, and the one
    /// a pin is tested against.
    ordinal: u64,
    /// Every region in the run, ascending, so a report of what is still held is byte-identical run
    /// to run.
    regions: Vec<RegionId>,
}

/// What a close did with the region's slots.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reclaim {
    /// The slots went back to the bump pointer.
    Freed(usize),
    /// A live continuation can still reach them, so they did not.
    Retained(usize),
    /// The region was not open — a teardown running twice, which is not a second free.
    NotOpen,
}

impl Reclaim {
    /// Slots the close accounted for, either way.
    pub fn slots(self) -> usize {
        match self {
            Reclaim::Freed(n) | Reclaim::Retained(n) => n,
            Reclaim::NotOpen => 0,
        }
    }

    pub fn freed(self) -> bool {
        matches!(self, Reclaim::Freed(_))
    }
}

/// What the arena has cost, so "a bump pointer is free" stays a measurement.
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
    /// Slots copied by those snapshots.
    pub slots_copied: u64,
    /// Snapshots restored.
    pub restores: u64,
    /// The high-water mark of live slots.
    pub peak_live: usize,
    /// Pins taken — one per continuation capture, on a capture path that follows the rule.
    pub pins_taken: u64,
    /// Closes that handed their slots straight back.
    pub closes_freed: u64,
    /// Closes that retained their slots rather than freeing them, because a continuation captured
    /// across the region was still live.
    pub closes_deferred: u64,
    /// Slots handed back late, after the last continuation that could reach them died.
    pub slots_reclaimed_late: u64,
    /// The size of one stored value, which the arena's type decides.
    pub element: usize,
}

impl Stats {
    /// Bytes the arena holds from the global allocator: the chunks' value storage and their
    /// generation storage.
    pub fn bytes_reserved(&self) -> usize {
        self.chunks_allocated * CHUNK * (self.element + std::mem::size_of::<u32>())
    }
}

/// A region's extent as it stood at some earlier point, and the scopes that were open there.
pub struct Snapshot<V = Value> {
    region: RegionId,
    /// The bump pointer at the snapshot's floor — where restoring truncates to.
    base: usize,
    /// The bump pointer when the snapshot was taken.
    top: usize,
    /// `values[i]` and `generations[i]` belong to index `base + i`.
    values: Vec<V>,
    generations: Vec<u32>,
    /// The scopes at and above the snapshot's floor, innermost last.
    scopes: Vec<Scope>,
    /// Where `scopes` sits in the arena's own stack.
    depth: usize,
}

impl<V> Snapshot<V> {
    /// The region the snapshot is rooted at: the one whose close would discard it, and the
    /// outermost one it covers.
    pub fn region(&self) -> RegionId {
        self.region
    }

    /// Slots the snapshot copied.
    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Regions the snapshot covers: the one it names and everything nested inside it at the moment
    /// it was taken.
    pub fn regions(&self) -> usize {
        self.scopes.len()
    }
}

impl<V> fmt::Debug for Snapshot<V> {
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
/// The arena over whatever a cell holds: the interpreter's values by default, and the tier's
/// heap words on the tier, so that a cell's contents never cross the seam.
pub struct Arena<V = Value> {
    /// `chunks[c][o]` is the value at index `c * CHUNK + o`.
    chunks: Vec<Vec<V>>,
    /// Parallel to `chunks`, and **never truncated**: a physical position's generation only rises,
    /// so a slot from a closed region cannot match the value now living at its index.
    generations: Vec<Vec<u32>>,
    /// The bump pointer.
    live: usize,
    scopes: Vec<Scope>,
    /// One end of every [`Pin`] handed out.
    pins: Vec<Rc<PinCore>>,
    /// Slots whose regions have closed and which a live pin still covers.
    retained: Vec<Retained>,
    next_region: u32,
    stats: Stats,
    /// Every slot a close has reclaimed, in the order it was reclaimed, and `None` when nothing
    /// asked for one.
    journal: Option<Vec<(Slot, V)>>,
    /// Slots whose contents a `cell_update` has taken out and not yet put back. A read or write
    /// of one in the meantime is refused rather than answered with the placeholder.
    taken: Vec<Slot>,
}

impl<V: Clone + Default> Default for Arena<V> {
    fn default() -> Arena<V> {
        Arena::new()
    }
}

impl<V: Clone + Default> Arena<V> {
    pub fn new() -> Arena<V> {
        Arena {
            chunks: Vec::new(),
            generations: Vec::new(),
            live: 0,
            scopes: Vec::new(),
            pins: Vec::new(),
            retained: Vec::new(),
            next_region: 0,
            stats: Stats {
                element: std::mem::size_of::<V>(),
                ..Stats::default()
            },
            journal: None,
            taken: Vec::new(),
        }
    }

    /// Starts recording what closes reclaim.
    pub fn journal(&mut self) {
        self.journal = Some(Vec::new());
    }

    /// What every close has reclaimed since [`Arena::journal`], in order.
    pub fn journalled(&self) -> &[(Slot, V)] {
        self.journal.as_deref().unwrap_or(&[])
    }

    pub fn journalling(&self) -> bool {
        self.journal.is_some()
    }

    /// Forgets what earlier entry points reclaimed, so a journal covers one run.
    pub fn clear_journal(&mut self) {
        if let Some(journal) = &mut self.journal {
            journal.clear();
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
    pub fn open(&mut self, kind: RegionKind, span: Span) -> RegionId {
        let id = RegionId(self.next_region);
        self.next_region = self.next_region.wrapping_add(1);
        self.scopes.push(Scope {
            id,
            kind,
            mark: self.live,
            ordinal: self.stats.regions_opened,
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

    /// Allocates in the innermost open region.
    pub fn alloc(&mut self, value: V) -> Option<Slot> {
        if self.scopes.is_empty() || self.live > u32::MAX as usize {
            return None;
        }
        let index = self.live;
        let c = chunk_of(index);
        if c == self.chunks.len() {
            self.chunks.push(Vec::with_capacity(CHUNK));
            self.generations.push(vec![0; CHUNK]);
            self.stats.chunks_allocated += 1;
        }
        // Guaranteed not to reallocate: the chunk was created with `CHUNK` capacity and holds
        // `offset_of(index) < CHUNK` values.
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

    pub fn get(&self, slot: Slot) -> Option<&V> {
        let index = self.resolve(slot)?;
        self.chunks[chunk_of(index)].get(offset_of(index))
    }

    /// `false` when the slot's region has closed, which the caller must report with [`stale_slot`]
    /// rather than ignore.
    pub fn set(&mut self, slot: Slot, value: V) -> bool {
        let Some(index) = self.resolve(slot) else {
            return false;
        };
        self.chunks[chunk_of(index)][offset_of(index)] = value;
        true
    }

    pub fn contains(&self, slot: Slot) -> bool {
        self.resolve(slot).is_some()
    }

    /// Whether a `cell_update` currently holds this slot's contents.
    pub fn is_taken(&self, slot: Slot) -> bool {
        self.taken.contains(&slot)
    }

    /// Moves the slot's contents out for a `cell_update`, leaving the slot marked as taken so
    /// that nothing reads the placeholder. `None` when the slot is stale or already taken.
    pub fn take(&mut self, slot: Slot) -> Option<V> {
        if self.is_taken(slot) {
            return None;
        }
        let index = self.resolve(slot)?;
        self.taken.push(slot);
        Some(std::mem::take(
            &mut self.chunks[chunk_of(index)][offset_of(index)],
        ))
    }

    /// Stores a `cell_update`'s answer and clears the mark. `false` when the slot's region has
    /// closed in the meantime; the mark is cleared either way.
    pub fn put_back(&mut self, slot: Slot, value: V) -> bool {
        self.taken.retain(|s| *s != slot);
        self.set(slot, value)
    }

    /// Closes `region` and every region nested inside it.
    pub fn close(&mut self, region: RegionId) -> Reclaim {
        self.close_at(region, false)
    }

    /// A close that frees whatever a pin says, for the end of an entry point.
    pub fn close_final(&mut self, region: RegionId) -> Reclaim {
        self.close_at(region, true)
    }

    fn close_at(&mut self, region: RegionId, force: bool) -> Reclaim {
        // Before deciding, so that a continuation which died between its capture and this close
        // does not defer anything.
        self.collect();
        let Some(at) = self.scope(region) else {
            return Reclaim::NotOpen;
        };
        let scope = self.scopes[at];
        debug_assert!(
            scope.mark <= self.live,
            "an open region's mark sits above the bump pointer"
        );
        let slots = self.live.saturating_sub(scope.mark);
        // A region holding nothing has nothing to hold on to, whoever is pinning it, and an empty
        // run would be state to carry for no memory.
        if slots > 0 && !force && self.pinned(scope.ordinal) {
            let closing: Vec<RegionId> = self.scopes[at..].iter().map(|s| s.id).collect();
            self.scopes.truncate(at);
            self.retain(scope, closing);
            return Reclaim::Retained(slots);
        }
        self.scopes.truncate(at);
        self.stats.closes_freed += 1;
        // Every run above this mark belongs to a region that nested inside the one closing, so its
        // ordinal is higher and no pin that spares this region could have covered it.
        debug_assert!(
            self.retained
                .iter()
                .all(|run| run.hi <= scope.mark || run.lo >= scope.mark),
            "a retained run straddles a region boundary, so regions did not nest"
        );
        self.retained.retain(|run| run.lo < scope.mark);
        self.truncate(scope.mark, true);
        // The truncation may have put an older retained run back at the top of the arena, where it
        // is a truncation of its own.
        self.release();
        Reclaim::Freed(slots)
    }

    /// Closes the innermost open region.
    pub fn close_current(&mut self) -> Option<RegionId> {
        let id = self.scopes.last()?.id;
        self.close(id);
        Some(id)
    }

    /// [`Arena::close_current`] under [`Arena::close_final`]'s rule.
    pub fn close_current_final(&mut self) -> Option<RegionId> {
        let id = self.scopes.last()?.id;
        self.close_final(id);
        Some(id)
    }

    /// Takes a continuation's claim on every region open at this point.
    pub fn pin(&mut self) -> Option<Pin> {
        if self.scopes.is_empty() {
            return None;
        }
        // A program that performs a million times inside one region takes a million pins before
        // anything closes, and a dead one is only pruned at a close.
        if self.pins.len() >= PIN_PRUNE_AT {
            self.pins.retain(|core| Rc::strong_count(core) > 1);
        }
        let core = Rc::new(PinCore {
            frontier: self.stats.regions_opened,
            top: self.live,
        });
        self.pins.push(Rc::clone(&core));
        self.stats.pins_taken += 1;
        Some(Pin(core))
    }

    /// The innermost open region the compiler called `unique`, if any.
    pub fn unique_open(&self) -> Option<RegionId> {
        self.scopes
            .iter()
            .rev()
            .find(|s| s.kind == RegionKind::Unique)
            .map(|s| s.id)
    }

    /// Forgets every claim a continuation made, and hands back what those claims were holding.
    pub fn abandon_pins(&mut self) {
        self.pins.clear();
        self.release();
    }

    /// Drops the pins whose continuations have died and hands back every run of slots that no live
    /// pin still covers.
    pub fn collect(&mut self) {
        // One owner is this arena's own end of the token; anything more is a continuation that can
        // still be resumed.
        self.pins.retain(|core| Rc::strong_count(core) > 1);
        self.release();
    }

    /// Slots held past their region's close for a continuation that can still reach them.
    pub fn retained_slots(&self) -> usize {
        self.retained.iter().map(|run| run.hi - run.lo).sum()
    }

    /// The regions whose slots are being held, ascending.
    pub fn retained_regions(&self) -> Vec<RegionId> {
        let mut out: Vec<RegionId> = self
            .retained
            .iter()
            .flat_map(|run| run.regions.iter().copied())
            .collect();
        out.sort_unstable();
        out
    }

    /// Continuations that can still reach a region open at their capture.
    pub fn live_pins(&self) -> usize {
        self.pins
            .iter()
            .filter(|core| Rc::strong_count(core) > 1)
            .count()
    }

    /// Whether a continuation captured while this region was open is still live.
    fn pinned(&self, ordinal: u64) -> bool {
        self.pins
            .iter()
            .any(|core| core.frontier > ordinal && Rc::strong_count(core) > 1)
    }

    /// Holds `scope`'s extent past its close, absorbing the runs nested inside it — their regions
    /// closed within this one, so this run's release covers them.
    fn retain(&mut self, scope: Scope, closing: Vec<RegionId>) {
        let mut regions = closing;
        while self.retained.last().is_some_and(|run| run.lo >= scope.mark) {
            let mut run = self.retained.pop().expect("just tested");
            regions.append(&mut run.regions);
        }
        regions.sort_unstable();
        self.retained.push(Retained {
            lo: scope.mark,
            hi: self.live,
            ordinal: scope.ordinal,
            regions,
        });
        self.stats.closes_deferred += 1;
    }

    /// Truncates away every retained run that has become both unpinned and the top of the arena.
    fn release(&mut self) {
        let floor = self.scopes.last().map_or(0, |s| s.mark);
        loop {
            let Some(run) = self.retained.last() else {
                return;
            };
            if run.hi != self.live || run.lo < floor || self.pinned(run.ordinal) {
                return;
            }
            let lo = run.lo;
            self.retained.pop();
            self.stats.slots_reclaimed_late += (self.live - lo) as u64;
            self.truncate(lo, true);
        }
    }

    /// The extent of `region` and of everything nested inside it, as it stands now.
    pub fn snapshot(&mut self, region: RegionId) -> Option<Snapshot<V>> {
        let at = self.scope(region)?;
        if self.scopes[at].kind == RegionKind::Unique {
            return None;
        }
        Some(self.snapshot_from(at))
    }

    /// Every region open at this point — the snapshot a continuation capture has to take.
    pub fn snapshot_open(&mut self) -> Result<Option<Snapshot<V>>, RegionId> {
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

    fn snapshot_from(&mut self, at: usize) -> Snapshot<V> {
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

    /// Re-installs a snapshot: the covered slots and the scopes that were open over them.
    pub fn restore(&mut self, snapshot: &Snapshot<V>) -> bool {
        // The region must still be open *at the depth it was taken from*.
        if self.scopes.get(snapshot.depth).map(|s| s.id) != Some(snapshot.region) {
            return false;
        }
        self.collect();
        if self.pins.iter().any(|core| core.top > snapshot.top)
            || self.retained.iter().any(|run| run.hi > snapshot.base)
        {
            return false;
        }
        // Everything allocated above the snapshot is freed with its generation bumped; the
        // snapshot's own slots are freed without one, because they are about to be written back
        // under the very identities a continuation holds.
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

    /// Ascending by index — the order a differential comparison and a rendered artifact both need.
    pub fn slots(&self) -> impl Iterator<Item = (Slot, &V)> {
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
    fn truncate(&mut self, mark: usize, invalidate: bool) {
        if mark >= self.live {
            return;
        }
        // Before the invalidation, so a journalled slot carries the generation the cell had rather
        // than the one its position went on to.
        if let Some(journal) = &mut self.journal {
            for index in mark..self.live {
                let (c, o) = (chunk_of(index), offset_of(index));
                let slot = Slot::new(index as u32, self.generations[c][o]);
                journal.push((slot, self.chunks[c][o].clone()));
            }
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

impl<V: Clone + Default> fmt::Debug for Arena<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Arena({} live, {} retained, {} regions open, {} chunks)",
            self.live,
            self.retained_slots(),
            self.scopes.len(),
            self.chunks.len()
        )
    }
}

/// A read or a write through a slot whose region has been reclaimed.
pub fn stale_slot(slot: Slot, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("slot {slot} belongs to a region whose memory has been reclaimed"),
    )
    .primary(span, "this read cannot be answered")
    .note(
        "a region's slots are freed at its close unless a continuation captured across it is \
         still live, so reaching one here means either a value outlived its region or a capture \
         was not counted",
    )
}

/// A continuation captured while a region the compiler called `unique` was open.
pub fn unique_capture(region: RegionId, region_span: Span, capture: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("a continuation was captured across `{region}`, which is `unique`"),
    )
    .primary(capture, "the continuation is captured here")
    .secondary(region_span, "this region is `unique`")
    .note(
        "`unique` is the claim that nothing can reach the region's slots after its close, so the \
         inference and the machine disagree about what this program does",
    )
}
