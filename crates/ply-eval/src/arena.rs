//! The region allocator: a bump arena whose scopes are the program's regions.

use crate::value::Value;
use ply_span::Span;
use std::fmt;

pub const CHUNK: usize = 256;

const fn chunk_of(index: usize) -> usize {
    index / CHUNK
}

const fn offset_of(index: usize) -> usize {
    index % CHUNK
}

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

    pub fn snapshots(self) -> bool {
        matches!(self, RegionKind::Shared)
    }
}

impl fmt::Display for RegionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RegionId(pub u32);

impl fmt::Display for RegionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "r{}", self.0)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Slot {
    index: u32,
    generation: u32,
}

impl Slot {
    pub fn new(index: u32, generation: u32) -> Slot {
        Slot { index, generation }
    }

    /// Ascending in allocation order within one arena.
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
    span: Span,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reclaim {
    Freed(usize),
    /// The region was not open, as when a teardown runs twice.
    NotOpen,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Stats {
    /// Chunks taken from the global allocator over the arena's whole life.
    pub chunks_allocated: usize,
    pub allocations: u64,
    pub regions_opened: u64,
    pub snapshots: u64,
    pub slots_copied: u64,
    pub restores: u64,
    pub peak_live: usize,
    pub closes_freed: u64,
    /// The size of one stored value.
    pub element: usize,
}

impl Stats {
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
    /// The outermost region the snapshot covers; its close would discard the snapshot.
    pub fn region(&self) -> RegionId {
        self.region
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

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

/// A bump arena whose scopes are regions, over interpreter values or the tier's heap words.
pub struct Arena<V = Value> {
    /// `chunks[c][o]` is the value at index `c * CHUNK + o`.
    chunks: Vec<Vec<V>>,
    /// Never truncated: a position's generation only rises, so a stale slot never matches.
    generations: Vec<Vec<u32>>,
    live: usize,
    scopes: Vec<Scope>,
    next_region: u32,
    stats: Stats,
    /// Every slot a close reclaimed, in order; `None` unless journalling.
    journal: Option<Vec<(Slot, V)>>,
    /// Slots a `cell_update` has taken out; touching one meanwhile is refused.
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
            next_region: 0,
            stats: Stats {
                element: std::mem::size_of::<V>(),
                ..Stats::default()
            },
            journal: None,
            taken: Vec::new(),
        }
    }

    pub fn journal(&mut self) {
        self.journal = Some(Vec::new());
    }

    pub fn journalled(&self) -> &[(Slot, V)] {
        self.journal.as_deref().unwrap_or(&[])
    }

    pub fn journalling(&self) -> bool {
        self.journal.is_some()
    }

    pub fn clear_journal(&mut self) {
        if let Some(journal) = &mut self.journal {
            journal.clear();
        }
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    pub fn live(&self) -> usize {
        self.live
    }

    pub fn depth(&self) -> usize {
        self.scopes.len()
    }

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
        // Never reallocates: the chunk was created with `CHUNK` capacity.
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

    /// `false` when the slot's region has closed.
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

    pub fn is_taken(&self, slot: Slot) -> bool {
        self.taken.contains(&slot)
    }

    /// Moves the contents out for a `cell_update` and marks the slot taken; `None` if unavailable.
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

    /// Stores a `cell_update`'s answer and clears the mark, even when the region has closed.
    pub fn put_back(&mut self, slot: Slot, value: V) -> bool {
        self.taken.retain(|s| *s != slot);
        self.set(slot, value)
    }

    /// Closes `region` and every region nested inside it.
    pub fn close(&mut self, region: RegionId) -> Reclaim {
        let Some(at) = self.scope(region) else {
            return Reclaim::NotOpen;
        };
        let scope = self.scopes[at];
        debug_assert!(
            scope.mark <= self.live,
            "an open region's mark sits above the bump pointer"
        );
        let slots = self.live.saturating_sub(scope.mark);
        self.scopes.truncate(at);
        self.stats.closes_freed += 1;
        self.truncate(scope.mark, true);
        Reclaim::Freed(slots)
    }

    pub fn close_current(&mut self) -> Option<RegionId> {
        let id = self.scopes.last()?.id;
        self.close(id);
        Some(id)
    }

    pub fn unique_open(&self) -> Option<RegionId> {
        self.scopes
            .iter()
            .rev()
            .find(|s| s.kind == RegionKind::Unique)
            .map(|s| s.id)
    }

    pub fn snapshot(&mut self, region: RegionId) -> Option<Snapshot<V>> {
        let at = self.scope(region)?;
        if self.scopes[at].kind == RegionKind::Unique {
            return None;
        }
        Some(self.snapshot_from(at))
    }

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

    pub fn restore(&mut self, snapshot: &Snapshot<V>) -> bool {
        // The region must still be open at the depth it was taken from.
        if self.scopes.get(snapshot.depth).map(|s| s.id) != Some(snapshot.region) {
            return false;
        }
        // The snapshot's own slots keep their generations, so cells made before it still resolve.
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

    /// Ascending by index, for deterministic comparison and rendering.
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
        // Before invalidating, so the journal records the generation the cell had.
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
            "Arena({} live, {} regions open, {} chunks)",
            self.live,
            self.scopes.len(),
            self.chunks.len()
        )
    }
}
