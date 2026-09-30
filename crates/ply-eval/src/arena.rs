//! The cell store: one slot per cell, freed when the region holding it closes, or, for a region a
//! [`Pin`] holds, when the last pin goes. Regions nest per control stack, an [`Owner`], so a close
//! never reaches another stack's regions, while a [`Slot`] names one cell for every stack.

use crate::value::Value;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::fmt;
use std::num::NonZeroU32;

pub const CHUNK: usize = 256;

/// The end of a region's slot list, of an owner's regions, or of the free scopes.
const NIL: u32 = u32::MAX;
/// The link of a slot no region holds.
const FREE: u32 = u32::MAX - 1;

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
}

impl fmt::Display for RegionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A region's scope and the scope's generation when it opened, so the id of a freed region never
/// names the region that reuses its scope.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RegionId {
    scope: u32,
    generation: u32,
}

impl RegionId {
    /// One integer, for compiled code that holds the id in a local.
    pub fn to_bits(self) -> u64 {
        (u64::from(self.generation) << 32) | u64::from(self.scope)
    }

    pub fn from_bits(bits: u64) -> RegionId {
        RegionId {
            scope: bits as u32,
            generation: (bits >> 32) as u32,
        }
    }
}

impl fmt::Display for RegionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "r{}.{}", self.scope, self.generation)
    }
}

/// A control stack, which regions nest on.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct Owner(pub usize);

impl Owner {
    /// The stack an entry point starts on, which holds the fixture's region and the entry's.
    pub const ENTRY: Owner = Owner(0);
}

/// A hold on a region by something that may run inside it again, such as a continuation. While
/// any is held, the region's close only takes it off its owner's nesting, keeping its slots and
/// its id, and the last [`Arena::unpin`] frees it. Only [`Arena::pin`] and [`Arena::repin`] make
/// one and only [`Arena::unpin`] takes one back, so no region is freed while a pin names it.
#[must_use = "a pin that is never given back keeps its region's slots for the arena's life"]
#[derive(Debug)]
pub struct Pin(RegionId);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Slot {
    index: u32,
    generation: u32,
}

impl Slot {
    pub fn new(index: u32, generation: u32) -> Slot {
        Slot { index, generation }
    }

    /// The lowest index that was free when the slot was allocated.
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
struct Meta {
    generation: u32,
    /// The slot its region allocated before it, [`NIL`] for the region's first, or [`FREE`].
    link: u32,
}

struct Scope {
    kind: RegionKind,
    /// Rises each time the scope is freed.
    generation: u32,
    state: State,
    /// The region's newest slot, from which [`Meta::link`] reaches the rest.
    newest: u32,
    cells: usize,
}

impl Scope {
    /// The region its owner opened before this one.
    fn below(&self) -> u32 {
        match self.state {
            State::Open { below, .. } => below,
            State::Parked { .. } | State::Free { .. } => {
                unreachable!("a nesting links open regions only")
            }
        }
    }
}

#[derive(Clone, Copy)]
enum State {
    /// On `owner`'s nesting, above `below`; `pins` holders keep it past its close.
    Open { owner: Owner, below: u32, pins: u32 },
    /// Closed while pinned: on no nesting, its slots and its id kept until the last pin goes.
    Parked { pins: NonZeroU32 },
    /// Holds no slot; `next` is the next free scope.
    Free { next: u32 },
}

#[derive(Clone, Copy)]
struct Nesting {
    innermost: u32,
    depth: usize,
}

impl Nesting {
    const EMPTY: Nesting = Nesting {
        innermost: NIL,
        depth: 0,
    };
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
    pub peak_live: usize,
    pub closes_freed: u64,
    /// The size of one stored value.
    pub element: usize,
}

impl Stats {}

/// Slots in chunks and regions nesting per owner, over interpreter values or the tier's heap words.
pub struct Arena<V = Value> {
    /// `chunks[c][o]` is the value at index `c * CHUNK + o`; a free slot holds `V::default()`.
    chunks: Vec<Vec<V>>,
    /// A position's generation only rises, so a stale slot never matches.
    meta: Vec<Vec<Meta>>,
    live: usize,
    /// Every index from here up is free.
    top: usize,
    /// The free indices below `top`, reused lowest first, as a bump pointer would.
    holes: BinaryHeap<Reverse<u32>>,
    scopes: Vec<Scope>,
    free_scopes: u32,
    /// Indexed by [`Owner`].
    owners: Vec<Nesting>,
    /// Regions open across every owner; a parked one counts on none.
    depth: usize,
    stats: Stats,
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
            meta: Vec::new(),
            live: 0,
            top: 0,
            holes: BinaryHeap::new(),
            scopes: Vec::new(),
            free_scopes: NIL,
            owners: Vec::new(),
            depth: 0,
            stats: Stats {
                element: std::mem::size_of::<V>(),
                ..Stats::default()
            },
            taken: Vec::new(),
        }
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    pub fn live(&self) -> usize {
        self.live
    }

    /// The regions `owner` holds open.
    pub fn depth(&self, owner: Owner) -> usize {
        self.owners.get(owner.0).map_or(0, |n| n.depth)
    }

    /// The regions every owner holds open.
    pub fn total_depth(&self) -> usize {
        self.depth
    }

    pub fn open(&mut self, owner: Owner, kind: RegionKind) -> RegionId {
        let at = if self.free_scopes == NIL {
            self.scopes.push(Scope {
                kind,
                generation: 0,
                state: State::Free { next: NIL },
                newest: NIL,
                cells: 0,
            });
            (self.scopes.len() - 1) as u32
        } else {
            let at = self.free_scopes;
            let State::Free { next } = self.scopes[at as usize].state else {
                unreachable!("the free list links free scopes only")
            };
            self.free_scopes = next;
            at
        };
        let scope = &mut self.scopes[at as usize];
        scope.kind = kind;
        scope.newest = NIL;
        scope.cells = 0;
        self.push(owner, at, 0);
        self.stats.regions_opened += 1;
        RegionId {
            scope: at,
            generation: self.scopes[at as usize].generation,
        }
    }

    /// `None` once the region is freed; a parked one still answers.
    pub fn kind(&self, region: RegionId) -> Option<RegionKind> {
        self.named(region).map(|at| self.scopes[at].kind)
    }

    /// Slots this region and the regions its owner opened inside it are holding.
    pub fn extent(&self, region: RegionId) -> Option<usize> {
        let (at, owner) = self.scope(region)?;
        let mut cells = 0;
        let mut s = self.owners[owner.0].innermost as usize;
        loop {
            cells += self.scopes[s].cells;
            if s == at {
                return Some(cells);
            }
            s = self.scopes[s].below() as usize;
        }
    }

    /// The regions `owner` holds open, innermost first.
    pub fn nesting(&self, owner: Owner) -> impl Iterator<Item = RegionId> + '_ {
        let mut at = self.owners.get(owner.0).map_or(NIL, |n| n.innermost);
        std::iter::from_fn(move || {
            if at == NIL {
                return None;
            }
            let scope = &self.scopes[at as usize];
            let id = RegionId {
                scope: at,
                generation: scope.generation,
            };
            at = scope.below();
            Some(id)
        })
    }

    /// Holds an open region past its close until the pin comes back; `None` unless it is open.
    pub fn pin(&mut self, region: RegionId) -> Option<Pin> {
        let (at, _) = self.scope(region)?;
        match &mut self.scopes[at].state {
            State::Open { pins, .. } => *pins += 1,
            State::Parked { .. } | State::Free { .. } => unreachable!("`scope` finds open regions"),
        }
        Some(Pin(region))
    }

    /// Another hold on the region `pin` holds, open or parked.
    pub fn repin(&mut self, pin: &Pin) -> Pin {
        let at = self.named(pin.0).expect("a pinned region is never freed");
        let scope = &mut self.scopes[at];
        scope.state = match scope.state {
            State::Open { owner, below, pins } => State::Open {
                owner,
                below,
                pins: pins + 1,
            },
            State::Parked { pins } => State::Parked {
                pins: pins.saturating_add(1),
            },
            State::Free { .. } => unreachable!("`named` finds open or parked regions"),
        };
        Pin(pin.0)
    }

    /// Gives a pin back, and answers how many slots that freed: the last pin of a parked region
    /// frees it, while an open one is left for its own close to free.
    pub fn unpin(&mut self, pin: Pin) -> usize {
        let at = self.named(pin.0).expect("a pinned region is never freed");
        match self.scopes[at].state {
            State::Open { owner, below, pins } => {
                self.scopes[at].state = State::Open {
                    owner,
                    below,
                    pins: pins - 1,
                };
                0
            }
            State::Parked { pins } => match NonZeroU32::new(pins.get() - 1) {
                Some(pins) => {
                    self.scopes[at].state = State::Parked { pins };
                    0
                }
                None => self.release(at as u32),
            },
            State::Free { .. } => unreachable!("`named` finds open or parked regions"),
        }
    }

    /// Puts the parked region `pin` holds back on `owner`'s nesting as its innermost, with the
    /// slots and the id it kept; `false` when the region is open.
    pub fn reopen(&mut self, pin: &Pin, owner: Owner) -> bool {
        let at = self.named(pin.0).expect("a pinned region is never freed");
        let State::Parked { pins } = self.scopes[at].state else {
            return false;
        };
        self.push(owner, at as u32, pins.get());
        true
    }

    /// A slot in `owner`'s innermost region; `None` when it holds none open.
    pub fn alloc(&mut self, owner: Owner, value: V) -> Option<Slot> {
        let region = self.owners.get(owner.0)?.innermost;
        if region == NIL {
            return None;
        }
        let index = match self.holes.pop() {
            Some(Reverse(index)) => index as usize,
            None if self.top < FREE as usize => {
                if chunk_of(self.top) == self.chunks.len() {
                    let mut values = Vec::with_capacity(CHUNK);
                    values.resize_with(CHUNK, V::default);
                    self.chunks.push(values);
                    self.meta.push(vec![
                        Meta {
                            generation: 0,
                            link: FREE,
                        };
                        CHUNK
                    ]);
                    self.stats.chunks_allocated += 1;
                }
                self.top += 1;
                self.top - 1
            }
            None => return None,
        };
        let (c, o) = (chunk_of(index), offset_of(index));
        let scope = &mut self.scopes[region as usize];
        let meta = &mut self.meta[c][o];
        meta.link = scope.newest;
        scope.newest = index as u32;
        scope.cells += 1;
        self.chunks[c][o] = value;
        self.live += 1;
        self.stats.allocations += 1;
        self.stats.peak_live = self.stats.peak_live.max(self.live);
        Some(Slot {
            index: index as u32,
            generation: meta.generation,
        })
    }

    pub fn get(&self, slot: Slot) -> Option<&V> {
        let index = self.resolve(slot)?;
        Some(&self.chunks[chunk_of(index)][offset_of(index)])
    }

    /// `false` when the slot's region has been freed.
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

    /// Stores a `cell_update`'s answer and clears the mark, even when the region has been freed.
    pub fn put_back(&mut self, slot: Slot, value: V) -> bool {
        self.taken.retain(|s| *s != slot);
        self.set(slot, value)
    }

    /// Closes `region` and every region its owner opened after it, and no other owner's: each is
    /// freed, or parked while a pin holds it.
    pub fn close(&mut self, region: RegionId) -> Reclaim {
        let Some((at, owner)) = self.scope(region) else {
            return Reclaim::NotOpen;
        };
        let mut freed = 0;
        loop {
            let innermost = self.owners[owner.0].innermost as usize;
            freed += self.pop(owner);
            if innermost == at {
                break;
            }
        }
        self.stats.closes_freed += 1;
        Reclaim::Freed(freed)
    }

    /// Closes the regions `owner` opened since it stood `depth` deep.
    pub fn close_above(&mut self, owner: Owner, depth: usize) {
        while self.depth(owner) > depth {
            self.pop(owner);
            self.stats.closes_freed += 1;
        }
    }

    /// Closes every region but the `depth` oldest that `keep` holds.
    pub fn close_all_but(&mut self, keep: Owner, depth: usize) {
        for owner in 0..self.owners.len() {
            let floor = if owner == keep.0 { depth } else { 0 };
            self.close_above(Owner(owner), floor);
        }
        while self.owners.last().is_some_and(|n| n.depth == 0) {
            self.owners.pop();
        }
    }

    /// Live slots ascending by index, for deterministic comparison and rendering.
    pub fn slots(&self) -> impl Iterator<Item = (Slot, &V)> {
        (0..self.top).filter_map(move |index| {
            let (c, o) = (chunk_of(index), offset_of(index));
            let meta = self.meta[c][o];
            (meta.link != FREE).then(|| {
                (
                    Slot {
                        index: index as u32,
                        generation: meta.generation,
                    },
                    &self.chunks[c][o],
                )
            })
        })
    }

    /// An open region's scope, and the owner whose nesting holds it.
    fn scope(&self, region: RegionId) -> Option<(usize, Owner)> {
        let at = self.named(region)?;
        match self.scopes[at].state {
            State::Open { owner, .. } => Some((at, owner)),
            State::Parked { .. } | State::Free { .. } => None,
        }
    }

    /// The scope `region` still names: open or parked, never freed.
    fn named(&self, region: RegionId) -> Option<usize> {
        let at = region.scope as usize;
        let scope = self.scopes.get(at)?;
        let freed = matches!(scope.state, State::Free { .. });
        (!freed && scope.generation == region.generation).then_some(at)
    }

    /// The live index a slot names, or `None` once its region has been freed.
    fn resolve(&self, slot: Slot) -> Option<usize> {
        let index = slot.index as usize;
        if index >= self.top {
            return None;
        }
        let meta = self.meta[chunk_of(index)][offset_of(index)];
        (meta.link != FREE && meta.generation == slot.generation).then_some(index)
    }

    /// Puts scope `at` on `owner`'s nesting as its innermost.
    fn push(&mut self, owner: Owner, at: u32, pins: u32) {
        if owner.0 >= self.owners.len() {
            self.owners.resize(owner.0 + 1, Nesting::EMPTY);
        }
        let nesting = &mut self.owners[owner.0];
        self.scopes[at as usize].state = State::Open {
            owner,
            below: nesting.innermost,
            pins,
        };
        nesting.innermost = at;
        nesting.depth += 1;
        self.depth += 1;
    }

    /// Takes `owner`'s innermost region off its nesting, parked while a pin holds it and freed
    /// otherwise, and answers how many slots that freed.
    fn pop(&mut self, owner: Owner) -> usize {
        let nesting = &mut self.owners[owner.0];
        let at = nesting.innermost;
        let State::Open { below, pins, .. } = self.scopes[at as usize].state else {
            unreachable!("a nesting links open regions only")
        };
        nesting.innermost = below;
        nesting.depth -= 1;
        self.depth -= 1;
        match NonZeroU32::new(pins) {
            Some(pins) => {
                self.scopes[at as usize].state = State::Parked { pins };
                0
            }
            None => self.release(at),
        }
    }

    /// Frees a region on no nesting: drops its slots, and moves its scope's generation on so no id
    /// of it matches again. Answers how many slots it held.
    fn release(&mut self, at: u32) -> usize {
        let scope = &mut self.scopes[at as usize];
        let (mut slot, cells) = (scope.newest, scope.cells);
        scope.generation = scope.generation.wrapping_add(1);
        scope.state = State::Free {
            next: self.free_scopes,
        };
        scope.newest = NIL;
        scope.cells = 0;
        self.free_scopes = at;
        // Newest first, so while one owner allocates each free lowers `top`.
        while slot != NIL {
            slot = self.free(slot as usize);
        }
        cells
    }

    /// Drops the value at `index` and answers the slot its region allocated before it.
    fn free(&mut self, index: usize) -> u32 {
        let (c, o) = (chunk_of(index), offset_of(index));
        let meta = &mut self.meta[c][o];
        let before = meta.link;
        meta.link = FREE;
        meta.generation = meta.generation.wrapping_add(1);
        drop(std::mem::take(&mut self.chunks[c][o]));
        self.live -= 1;
        if index + 1 == self.top {
            self.top = index;
        } else {
            self.holes.push(Reverse(index as u32));
        }
        before
    }
}

impl<V: Clone + Default> fmt::Debug for Arena<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Arena({} live, {} regions open, {} chunks)",
            self.live,
            self.depth,
            self.chunks.len()
        )
    }
}
