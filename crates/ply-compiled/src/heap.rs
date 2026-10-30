//! The blocks compiled code and the runtime lay out: a one-word header, the count, then the payload,
//! which only the code that made the block reads. A word is a scalar's bits or a block's address;
//! what the runtime keeps of its own (a native object, a hold) it tells apart by its first
//! payload word.

use crate::blocks::{self, Blocks, Filling};
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};

pub type Word = i64;

/// A count no increment or decrement touches: the static data a unit lays out, the memo.
pub const IMMORTAL: u32 = u32::MAX;

/// The count of a block laid in a C frame, which it never outlives: no increment or decrement
/// touches it, as none touches an immortal one, but a keep or a copy copies the block, which is no
/// static data.
pub const FRAME: u32 = u32::MAX - 1;

/// Set on the count of a block another thread may hold. The count changes atomically, and the
/// block is never unique, so nothing reuses it in place.
pub const SHARED: u32 = 1 << 31;

/// A block's header: its count, in the low half of the word before its payload, and whether the
/// scheduler's own work made it, which emitted code clears on a block it takes itself.
#[repr(C, align(8))]
pub struct Obj {
    pub rc: u32,
    pub made_by_scheduler: bool,
}

pub const HEADER: usize = 8;

const _: () = assert!(HEADER == std::mem::size_of::<Obj>());

/// A block's count, read and changed as an atomic: another thread may hold a shared block.
#[inline]
fn count(o: *mut Obj) -> &'static std::sync::atomic::AtomicU32 {
    // SAFETY: `rc` is an aligned `u32` at the start of every block, which lives as long as its heap.
    unsafe { std::sync::atomic::AtomicU32::from_ptr(&raw mut (*o).rc) }
}

/// No block lies below this address, so a word under it is a value of its own: 0, or a one-word
/// sum's nullary value `8, 16, ..`, as emitted code lays them (`layout.pointer_floor`).
pub const POINTER_FLOOR: Word = 4096;

/// Whether `w` is a block's address: a multiple of eight at or above [`POINTER_FLOOR`].
#[inline]
pub fn is_object(w: Word) -> bool {
    w & 7 == 0 && w >= POINTER_FLOOR
}

/// Whether `v` fits the sixty-three bits a tagged `Int` carries.
#[inline]
pub fn fits_imm(v: i64) -> bool {
    (v << 1) >> 1 == v
}

/// `v` as a tagged `Int`: its bits shifted up past a low bit set, which no block's address has.
#[inline]
pub fn imm(v: i64) -> Word {
    debug_assert!(fits_imm(v));
    (v << 1) | 1
}

#[inline]
pub fn imm_value(w: Word) -> i64 {
    w >> 1
}

#[inline]
pub fn obj(w: Word) -> *mut Obj {
    debug_assert!(is_object(w));
    if poisoning() {
        poison::check(w);
    }
    w as *mut Obj
}

/// Under `PLY_HEAP_POISON`, dead payloads are poisoned so a stale read fails at the body's site.
#[inline]
pub fn poisoning() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("PLY_HEAP_POISON").is_some())
}

/// How many releases a dead block waits before reuse: `PLY_HEAP_DELAY`, zero by default.
pub fn delay() -> usize {
    static N: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("PLY_HEAP_DELAY")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(0)
    })
}

/// The poison diagnostic: the dead-payload word and the check every block read passes.
pub mod poison {
    use super::{IMMORTAL, Obj, Word, is_object};

    /// Immortal, so a read through it is caught and never counted.
    static POISONED: Obj = Obj {
        rc: IMMORTAL,
        made_by_scheduler: false,
    };

    pub fn word() -> Word {
        &raw const POISONED as Word
    }

    thread_local! {
        /// Three consecutive `i64`s in the running entry's `Ctx`: its site root, start and end.
        static SITE: std::cell::Cell<*const i64> = const { std::cell::Cell::new(std::ptr::null()) };
    }

    /// Points poison reports at `site` until the matching swap back, answering what it replaced.
    pub fn swap(site: *const i64) -> *const i64 {
        SITE.with(|s| s.replace(site))
    }

    pub fn leave() {
        SITE.with(|s| s.set(std::ptr::null()));
    }

    /// Panics, naming the body's site, when `w` is the poison word or a block given back, whose
    /// count is 0.
    pub fn check(w: Word) {
        if !is_object(w) {
            return;
        }
        let reason = if w == word() {
            "a payload word poisoned when its block died"
        } else if unsafe { (*(w as *const Obj)).rc } == 0 {
            "a block that has died"
        } else {
            return;
        };
        let site = SITE.with(|s| s.get());
        let at = if site.is_null() {
            "outside any entry".to_string()
        } else {
            let (root, start, end) = unsafe { (*site, *site.add(1), *site.add(2)) };
            format!("root {root} bytes {start}..{end} from its start")
        };
        panic!("a stale read: {reason}, at {at}");
    }
}

/// The payload words after a header.
#[inline]
pub unsafe fn words(o: *mut Obj) -> *mut Word {
    unsafe { (o as *mut u8).add(HEADER) as *mut Word }
}

#[inline]
pub unsafe fn word_at(o: *mut Obj, i: usize) -> Word {
    unsafe { *words(o).add(i) }
}

#[inline]
pub unsafe fn set_word(o: *mut Obj, i: usize, w: Word) {
    unsafe { *words(o).add(i) = w }
}

/// Where a function value's block, and a hold, which is laid out as one no call enters, keep the
/// word that says what they are, and their captures after it.
pub const CLOSURE_CODE: usize = 0;
pub const CLOSURE_CAPTURES: usize = 1;

/// The payload word a native object keeps its place in its heap's table in, and where what it
/// holds starts.
const LISTED_AT: usize = 1;
const NATIVE_HELD: usize = 2;

/// What a native object's first payload word holds: the address of this.
static NATIVE_MARK: u8 = 0;

/// A native object's payload: its mark, its place in its heap's table, then what a native made it
/// of, boxed.
const NATIVE_BYTES: usize = 8 * NATIVE_HELD + std::mem::size_of::<Box<dyn Native>>();

/// What a runtime object a native makes holds: a word in a scheduler's handle, a region's stacks.
/// Its Rust memory goes with the object; what it holds of emitted code's it lets go of as its last
/// holder lets it go, and at the entry's end the entry's memory goes whole.
pub trait Native: std::any::Any {
    fn release(&mut self, ctx: *mut crate::rt::Ctx);

    /// The handle of its run the object is, which no boundary lets out; none for any other.
    fn escapes(&self) -> Option<ply_core::escape::Handle> {
        None
    }
}

/// What the native object `w` is as [`Native::escapes`] says.
///
/// # Safety
/// `w` is a live native object.
pub unsafe fn native_escapes(w: Word) -> Option<ply_core::escape::Handle> {
    unsafe { (**native_slot(obj(w))).escapes() }
}

unsafe fn native_slot(o: *mut Obj) -> *mut Box<dyn Native> {
    unsafe { words(o).add(NATIVE_HELD) as *mut Box<dyn Native> }
}

/// Whether `w` is a native object's block.
pub fn is_native(w: Word) -> bool {
    is_object(w) && unsafe { word_at(obj(w), 0) } == &raw const NATIVE_MARK as Word
}

/// What the native object `w` holds, where it is one holding a `T`.
///
/// # Safety
/// `w` is live, and nothing else reads what it holds while the answer is held.
pub unsafe fn native_of<'a, T: Native>(w: Word) -> Option<&'a mut T> {
    if !is_native(w) {
        return None;
    }
    let held: &'a mut dyn Native = unsafe { &mut **native_slot(obj(w)) };
    (held as &mut dyn std::any::Any).downcast_mut::<T>()
}

/// An entry's blocks, whose memory goes back whole at its end; a persistent heap's never does.
pub struct Heap {
    blocks: Blocks,
    /// Boxed, so compiled code finds it wherever the heap moves.
    alloc: Box<Alloc>,
    /// The native objects whose contents the end drops, each block's [`LISTED_AT`] its index here.
    natives: Vec<*mut Obj>,
    persistent: bool,
    /// Whether the scheduler's own work runs: the blocks it makes are its own, and neither the
    /// program's counts nor its live blocks count them.
    scheduling: bool,
    /// Dead blocks not yet given back, oldest first, under `delay()`.
    delayed: std::collections::VecDeque<(*mut Obj, usize)>,
    poison: bool,
    delay: usize,
}

/// What compiled code reads and writes of a heap to take a block of at most
/// [`blocks::WORD_CLASSES`] words, and give one back, without calling the runtime, laid out as
/// `compiler.emit.abi` declares `PlyAlloc`. Any other block goes through the runtime.
#[repr(C)]
pub struct Alloc {
    /// The pages of the heap's blocks being filled, by class.
    pub(crate) blocks: *mut Filling,
    /// Blocks the program allocated since the last reset.
    pub(crate) count: usize,
    /// Nonzero while every block goes through the runtime: while the scheduler's own work runs, and
    /// under `PLY_HEAP_POISON` or `PLY_HEAP_DELAY`, which the runtime alone keeps.
    pub(crate) slow: usize,
}

thread_local! {
    /// The running entry's heap, which dying blocks return to; entries never nest on a thread.
    static CURRENT: std::cell::Cell<*mut Heap> = const { std::cell::Cell::new(std::ptr::null_mut()) };
}

/// The live blocks of `blocks` by the payload bytes their blocks hold, with their count and payload
/// bytes, the most bytes first: a block's count is 0 once it is given back.
pub(crate) fn census_in(blocks: &Blocks) -> Vec<(usize, usize, usize)> {
    let mut tally: std::collections::BTreeMap<usize, (usize, usize)> = Default::default();
    blocks.each(|block, bytes| {
        if unsafe { (*block.cast::<Obj>()).rc } == 0 {
            return;
        }
        let e = tally.entry(bytes - HEADER).or_insert((0, 0));
        e.0 += 1;
        e.1 += bytes - HEADER;
    });
    sorted(tally)
}

/// How many live blocks of `blocks` the program made, the scheduler's own not among them.
pub(crate) fn program_live_in(blocks: &Blocks) -> usize {
    let mut n = 0;
    blocks.each(|block, _| {
        let o = block.cast::<Obj>();
        if unsafe { (*o).rc != 0 && !(*o).made_by_scheduler } {
            n += 1;
        }
    });
    n
}

/// Two censuses as one, each size's counts and bytes summed, the most bytes first.
pub(crate) fn census_merged(
    a: Vec<(usize, usize, usize)>,
    b: Vec<(usize, usize, usize)>,
) -> Vec<(usize, usize, usize)> {
    let mut sums: std::collections::BTreeMap<usize, (usize, usize)> = Default::default();
    for (size, n, bytes) in a.into_iter().chain(b) {
        let e = sums.entry(size).or_insert((0, 0));
        e.0 += n;
        e.1 += bytes;
    }
    sorted(sums)
}

fn sorted(tally: std::collections::BTreeMap<usize, (usize, usize)>) -> Vec<(usize, usize, usize)> {
    let mut rows: Vec<(usize, usize, usize)> =
        tally.into_iter().map(|(k, (n, b))| (k, n, b)).collect();
    rows.sort_by_key(|&(k, _, b)| (std::cmp::Reverse(b), k));
    rows
}

/// The entry beginning on this thread allocates from `heap`; its dead blocks go back to it.
pub fn enter(heap: *mut Heap) {
    CURRENT.with(|c| c.set(heap));
}

pub fn leave() {
    CURRENT.with(|c| c.set(std::ptr::null_mut()));
}

/// The heap an entry running nested on this thread takes over until it swaps the old one back.
pub fn swap_current(heap: *mut Heap) -> *mut Heap {
    CURRENT.with(|c| c.replace(heap))
}

/// Gives back a dead block of `object` bytes. Only the heap that owns the block may: a native
/// object leaves the table first, so the one that takes the block next is not listed twice, and
/// one another heap owns stays listed there, dead, until that heap's end.
unsafe fn give_back(o: *mut Obj, heap: *mut Heap, object: usize, listed: bool) {
    unsafe {
        if heap.is_null() || !(*heap).blocks.owns(o.cast(), object) {
            if !listed {
                blocks::hand_back(o.cast(), object);
            }
            return;
        }
        if listed {
            (*heap).unlist(o);
        }
        // Reused even when poisoned: holding dead blocks back exhausts memory.
        if (*heap).poison {
            let words = (object - HEADER) / 8;
            for i in 0..words {
                set_word(o, i, poison::word());
            }
        }
        let delay = (*heap).delay;
        if delay == 0 {
            (*heap).blocks.give(o.cast(), object);
            return;
        }
        (*heap).delayed.push_back((o, object));
        if (*heap).delayed.len() > delay
            && let Some((o, object)) = (*heap).delayed.pop_front()
        {
            (*heap).blocks.give(o.cast(), object);
        }
    }
}

/// Gives back a block whose last holder was emitted code, `payload` bytes past its header, without
/// a walk: emitted code let go of what it held.
pub(crate) unsafe fn free_sized(o: *mut Obj, heap: *mut Heap, payload: usize) {
    unsafe {
        let rc = (*o).rc;
        debug_assert!(rc == 1 || rc == SHARED, "{o:p} was freed with holders left");
        let object = Heap::object_size(payload);
        #[cfg(debug_assertions)]
        sized_as(o, heap, object);
        (*o).rc = 0;
        give_back(o, heap, object, false);
    }
}

/// Panics where `heap`'s blocks say `o` was not allocated as `object` bytes.
#[cfg(debug_assertions)]
unsafe fn sized_as(o: *mut Obj, heap: *mut Heap, object: usize) {
    unsafe {
        if let Some(had) = heap.as_ref().and_then(|h| h.blocks.bytes_at(o.addr())) {
            assert_eq!(
                had,
                blocks::taken_bytes(object),
                "{o:p} was freed as {object} bytes, and its block was taken as {had}"
            );
        }
    }
}

/// Gives back a runtime handle whose last holder was emitted code, with what the runtime keeps for
/// it, and without a walk: a native object lets go of what it holds and is dropped, and a hold's
/// words are left unread, emitted code having let go of what it held.
/// `ctx`, which a native object lets go of what it holds through, is null only for another.
pub(crate) unsafe fn free_handle(o: *mut Obj, heap: *mut Heap, ctx: *mut crate::rt::Ctx) {
    unsafe {
        debug_assert!((*o).rc == SHARED || (*o).rc == 1);
        let w = o as Word;
        let (payload, listed) = if is_native(w) {
            assert!(
                !ctx.is_null(),
                "a native object was let go of outside any context"
            );
            let mut held = native_slot(o).read();
            held.release(ctx);
            drop(held);
            (NATIVE_BYTES, true)
        } else if crate::rt::scoped_id(w).is_some() {
            (crate::rt::SCOPED_BYTES, false)
        } else {
            panic!("`rt_handle_free` was handed a block that is no handle")
        };
        (*o).rc = 0;
        give_back(o, heap, Heap::object_size(payload), listed);
    }
}

/// The count of `o`, read as another thread may be changing it.
pub(crate) unsafe fn count_of(o: *mut Obj) -> u32 {
    count(o).load(Relaxed)
}

/// One holder fewer of a shared block whose count is its own: whether that was the last, its
/// memory then the caller's to release.
pub(crate) unsafe fn last_of_shared(o: *mut Obj) -> bool {
    let last = count(o).fetch_sub(1, Release) == SHARED | 1;
    if last {
        std::sync::atomic::fence(Acquire);
    }
    last
}

/// One holder fewer of `w`, a native object or a hold; the last gives it back to the running entry's heap.
pub fn release_handle(w: Word) {
    unsafe { released(w, CURRENT.with(|c| c.get()), std::ptr::null_mut()) }
}

/// One holder fewer of `w`, a runtime handle of `ctx`'s entry; the last gives it back.
pub fn release_object(ctx: *mut crate::rt::Ctx, w: Word) {
    unsafe { released(w, &raw mut (*ctx).heap, ctx) }
}

unsafe fn released(w: Word, heap: *mut Heap, ctx: *mut crate::rt::Ctx) {
    let o = obj(w);
    let rc = count(o).load(Relaxed);
    let last = if rc == IMMORTAL {
        false
    } else if rc >= SHARED {
        unsafe { last_of_shared(o) }
    } else if rc > 1 {
        unsafe { (*o).rc = rc - 1 };
        false
    } else {
        true
    };
    if last {
        unsafe { free_handle(o, heap, ctx) }
    }
}

/// One holder more of `w`, a block another thread may hold.
pub fn inc_shared(w: Word) {
    count(obj(w)).fetch_add(1, Relaxed);
}

impl Default for Heap {
    fn default() -> Heap {
        Heap::new()
    }
}

impl Heap {
    pub fn new() -> Heap {
        let mut blocks = Blocks::default();
        let (poison, delay) = (poisoning(), delay());
        let alloc = Box::new(Alloc {
            blocks: blocks.filling(),
            count: 0,
            slow: usize::from(poison || delay > 0),
        });
        Heap {
            blocks,
            alloc,
            natives: Vec::new(),
            persistent: false,
            scheduling: false,
            delayed: std::collections::VecDeque::new(),
            poison,
            delay,
        }
    }

    /// What compiled code takes blocks from and gives them back to, wherever this heap moves.
    pub(crate) fn alloc(&mut self) -> *mut Alloc {
        &raw mut *self.alloc
    }

    /// The scheduler's own work begins, where `on`, or the program's runs again: whether the
    /// scheduler's ran before.
    pub(crate) fn set_scheduling(&mut self, on: bool) -> bool {
        self.alloc.slow = usize::from(on || self.poison || self.delay > 0);
        std::mem::replace(&mut self.scheduling, on)
    }

    /// Takes over a finished branch's memory: its blocks are this entry's from here on.
    pub fn adopt_heap(&mut self, mut other: Heap) {
        self.blocks.adopt(std::mem::take(&mut other.blocks));
        for o in other.natives.drain(..) {
            unsafe { set_word(o, LISTED_AT, self.natives.len() as Word) };
            self.natives.push(o);
        }
        self.delayed.extend(other.delayed.drain(..));
        self.alloc.count += other.alloc.count;
    }

    /// `blocks` in place of the blocks this heap takes from, which it answers.
    pub(crate) fn swap_blocks(&mut self, blocks: Blocks) -> Blocks {
        let was = std::mem::replace(&mut self.blocks, blocks);
        self.alloc.blocks = self.blocks.filling();
        was
    }

    /// A heap whose entries never end: what outlives every entry lives here.
    pub fn persistent() -> Heap {
        let mut h = Heap::new();
        h.persistent = true;
        h
    }

    /// Live blocks by the payload bytes their blocks hold, the most bytes first, with their count
    /// and payload bytes.
    pub fn census(&self) -> Vec<(usize, usize, usize)> {
        census_in(&self.blocks)
    }

    /// How many blocks are live.
    pub fn live(&self) -> usize {
        self.census().iter().map(|(_, n, _)| n).sum()
    }

    /// The bytes of the blocks the entry's live values take, read from the pages without a walk: a
    /// dead block waiting under `PLY_HEAP_DELAY` still counts.
    pub fn live_bytes(&self) -> usize {
        self.blocks.live()
    }

    /// The bytes of the memory holding the entry's blocks, whatever is live in it.
    pub fn held(&self) -> usize {
        self.blocks.held()
    }

    /// Blocks allocated since the last reset.
    pub fn allocated(&self) -> usize {
        self.alloc.count
    }

    /// How many live blocks the program made, the scheduler's own not among them.
    pub(crate) fn program_live(&self) -> usize {
        program_live_in(&self.blocks)
    }

    /// Allocations that took a dead block rather than fresh memory.
    pub fn recycled(&self) -> usize {
        self.blocks.reused()
    }

    /// How many native objects the table lists: what a leak of them grows.
    pub fn natives(&self) -> usize {
        self.natives.len()
    }

    /// Takes a dying native object out of the table, the last entry moving into its place.
    unsafe fn unlist(&mut self, o: *mut Obj) {
        let at = unsafe { word_at(o, LISTED_AT) } as usize;
        debug_assert!(
            self.natives.get(at) == Some(&o),
            "a native object died into a heap whose table does not hold it"
        );
        self.natives.swap_remove(at);
        if let Some(&moved) = self.natives.get(at) {
            unsafe { set_word(moved, LISTED_AT, at as Word) };
        }
    }

    /// Drops what every native object holds, of those still listed; one released with no heap
    /// entered stays listed, dead, and is skipped.
    fn drop_natives(&mut self) {
        for o in self.natives.drain(..) {
            unsafe {
                if (*o).rc != 0 {
                    std::ptr::drop_in_place(native_slot(o));
                }
            }
        }
    }

    /// The bytes a block with this payload takes, header included and rounded to a word.
    pub fn object_size(payload_bytes: usize) -> usize {
        (HEADER + payload_bytes.max(8) + 7) & !7
    }

    /// A fresh block of `payload_bytes` past its header, counted once where the program made it,
    /// its payload left as a recycled block's last tenant wrote it.
    pub(crate) fn raw_alloc(&mut self, payload_bytes: usize) -> *mut Obj {
        let size = Heap::object_size(payload_bytes);
        let p: *mut Obj = self.blocks.take(size).cast();
        unsafe {
            p.write(Obj {
                rc: 1,
                made_by_scheduler: self.scheduling,
            })
        };
        if !self.scheduling {
            self.alloc.count += 1;
        }
        p
    }

    /// A native object holding `held`, which it takes, listed in the table.
    pub fn native(&mut self, held: Box<dyn Native>) -> Word {
        let o = self.raw_alloc(NATIVE_BYTES);
        unsafe {
            set_word(o, 0, &raw const NATIVE_MARK as Word);
            set_word(o, LISTED_AT, self.natives.len() as Word);
            native_slot(o).write(held);
        }
        self.natives.push(o);
        o as Word
    }

    /// Gives the entry's memory back; the answer is already copied out.
    pub fn end(&mut self) {
        if self.persistent {
            return;
        }
        self.drop_natives();
        self.blocks.reset();
        self.alloc.count = 0;
        self.delayed.clear();
    }
}

impl Drop for Heap {
    fn drop(&mut self) {
        self.drop_natives();
    }
}
