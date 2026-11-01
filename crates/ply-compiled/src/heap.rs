use crate::blocks::{self, ActivePages, Blocks};
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};

pub type Word = i64;

pub const IMMORTAL: u32 = u32::MAX;

pub const FRAME: u32 = u32::MAX - 1;

pub const SHARED: u32 = 1 << 31;

#[repr(C, align(8))]
pub struct Object {
    pub rc: u32,
    pub made_by_scheduler: bool,
}

pub const HEADER: usize = 8;

const _: () = assert!(HEADER == std::mem::size_of::<Object>());

#[inline]
fn count(o: *mut Object) -> &'static std::sync::atomic::AtomicU32 {
    unsafe { std::sync::atomic::AtomicU32::from_ptr(&raw mut (*o).rc) }
}

pub const POINTER_FLOOR: Word = 4096;

#[inline]
pub fn is_object(w: Word) -> bool {
    w & 7 == 0 && w >= POINTER_FLOOR
}

#[inline]
pub fn fits_imm(v: i64) -> bool {
    (v << 1) >> 1 == v
}

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
pub fn object(w: Word) -> *mut Object {
    debug_assert!(is_object(w));
    if poisoning_enabled() {
        poison_freed::check(w);
    }
    w as *mut Object
}

#[inline]
pub fn poisoning_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("PLY_HEAP_POISON").is_some())
}

pub fn delay() -> usize {
    static N: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("PLY_HEAP_DELAY")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(0)
    })
}

pub mod poison_freed {
    use super::{IMMORTAL, Object, Word, is_object};

    static POISONED: Object = Object {
        rc: IMMORTAL,
        made_by_scheduler: false,
    };

    pub fn word() -> Word {
        &raw const POISONED as Word
    }

    thread_local! {
        static CURRENT_BODY_RANGE: std::cell::Cell<*const i64> = const { std::cell::Cell::new(std::ptr::null()) };
    }

    pub fn swap(site: *const i64) -> *const i64 {
        CURRENT_BODY_RANGE.with(|s| s.replace(site))
    }

    pub fn leave() {
        CURRENT_BODY_RANGE.with(|s| s.set(std::ptr::null()));
    }

    pub fn check(w: Word) {
        if !is_object(w) {
            return;
        }
        let reason = if w == word() {
            "a payload word poisoned when its block died"
        } else if unsafe { (*(w as *const Object)).rc } == 0 {
            "a block that has died"
        } else {
            return;
        };
        let site = CURRENT_BODY_RANGE.with(|s| s.get());
        let at = if site.is_null() {
            "outside any entry".to_string()
        } else {
            let (root, start, end) = unsafe { (*site, *site.add(1), *site.add(2)) };
            format!("root {root} bytes {start}..{end} from its start")
        };
        panic!("a stale read: {reason}, at {at}");
    }
}

#[inline]
pub unsafe fn words(o: *mut Object) -> *mut Word {
    unsafe { (o as *mut u8).add(HEADER) as *mut Word }
}

#[inline]
pub unsafe fn word_at(o: *mut Object, i: usize) -> Word {
    unsafe { *words(o).add(i) }
}

#[inline]
pub unsafe fn set_word(o: *mut Object, i: usize, w: Word) {
    unsafe { *words(o).add(i) = w }
}

pub const CLOSURE_CODE: usize = 0;
pub const CLOSURE_CAPTURES: usize = 1;

const NATIVES_INDEX_WORD: usize = 1;
const NATIVE_BOX_WORD: usize = 2;

static NATIVE_MARK: u8 = 0;

const NATIVE_BYTES: usize = 8 * NATIVE_BOX_WORD + std::mem::size_of::<Box<dyn Native>>();

pub trait Native: std::any::Any {
    fn release(&mut self, context: *mut crate::rt::Context);

    fn escapes(&self) -> Option<ply_core::escape::Handle> {
        None
    }
}

pub unsafe fn native_escapes(w: Word) -> Option<ply_core::escape::Handle> {
    unsafe { (**native_slot(object(w))).escapes() }
}

unsafe fn native_slot(o: *mut Object) -> *mut Box<dyn Native> {
    unsafe { words(o).add(NATIVE_BOX_WORD) as *mut Box<dyn Native> }
}

pub fn is_native(w: Word) -> bool {
    is_object(w) && unsafe { word_at(object(w), 0) } == &raw const NATIVE_MARK as Word
}

pub unsafe fn native_of<'a, T: Native>(w: Word) -> Option<&'a mut T> {
    if !is_native(w) {
        return None;
    }
    let held: &'a mut dyn Native = unsafe { &mut **native_slot(object(w)) };
    (held as &mut dyn std::any::Any).downcast_mut::<T>()
}

pub struct Heap {
    blocks: Blocks,
    alloc: Box<Alloc>,
    natives: Vec<*mut Object>,
    persistent: bool,
    runtime_blocks: usize,
    scheduling: bool,
    deferred_frees: std::collections::VecDeque<(*mut Object, usize)>,
    poison_freed: bool,
    delay: usize,
}

#[repr(C)]
pub struct Alloc {
    pub(crate) blocks: *mut ActivePages,
    pub(crate) count: usize,
    pub(crate) slow: usize,
}

thread_local! {
    static CURRENT: std::cell::Cell<*mut Heap> = const { std::cell::Cell::new(std::ptr::null_mut()) };
}

pub(crate) fn size_histogram_in(blocks: &Blocks) -> Vec<(usize, usize, usize)> {
    let mut tally: std::collections::BTreeMap<usize, (usize, usize)> = Default::default();
    blocks.each(|block, bytes| {
        if unsafe { (*block.cast::<Object>()).rc } == 0 {
            return;
        }
        let e = tally.entry(bytes - HEADER).or_insert((0, 0));
        e.0 += 1;
        e.1 += bytes - HEADER;
    });
    sorted(tally)
}

pub(crate) fn program_live_block_count_in(blocks: &Blocks) -> usize {
    let mut n = 0;
    blocks.each(|block, _| {
        let o = block.cast::<Object>();
        if unsafe { (*o).rc != 0 && !(*o).made_by_scheduler } {
            n += 1;
        }
    });
    n
}

pub(crate) fn merge_size_histograms(
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

pub fn enter(heap: *mut Heap) {
    CURRENT.with(|c| c.set(heap));
}

pub fn leave() {
    CURRENT.with(|c| c.set(std::ptr::null_mut()));
}

pub fn swap_current(heap: *mut Heap) -> *mut Heap {
    CURRENT.with(|c| c.replace(heap))
}

unsafe fn give_back(o: *mut Object, heap: *mut Heap, object: usize, listed: bool) {
    unsafe {
        if heap.is_null() || !(*heap).blocks.owns(o.cast(), object) {
            if !listed {
                blocks::free_block(o.cast(), object);
            }
            return;
        }
        if listed {
            (*heap).unlist(o);
        }
        if (*heap).poison_freed {
            let words = (object - HEADER) / 8;
            for i in 0..words {
                set_word(o, i, poison_freed::word());
            }
        }
        let delay = (*heap).delay;
        if delay == 0 {
            (*heap).blocks.give(o.cast(), object);
            return;
        }
        (*heap).deferred_frees.push_back((o, object));
        if (*heap).deferred_frees.len() > delay
            && let Some((o, object)) = (*heap).deferred_frees.pop_front()
        {
            (*heap).blocks.give(o.cast(), object);
        }
    }
}

pub(crate) unsafe fn free_sized(o: *mut Object, heap: *mut Heap, payload: usize) {
    unsafe {
        let rc = (*o).rc;
        debug_assert!(rc == 1 || rc == SHARED, "{o:p} was freed with holders left");
        let object = Heap::object_size(payload);
        #[cfg(debug_assertions)]
        assert_block_size(o, heap, object);
        (*o).rc = 0;
        give_back(o, heap, object, false);
    }
}

#[cfg(debug_assertions)]
unsafe fn assert_block_size(o: *mut Object, heap: *mut Heap, object: usize) {
    unsafe {
        if let Some(had) = heap.as_ref().and_then(|h| h.blocks.bytes_at(o.addr())) {
            assert_eq!(
                had,
                blocks::reserved_bytes(object),
                "{o:p} was freed as {object} bytes, and its block was taken as {had}"
            );
        }
    }
}

pub(crate) unsafe fn free_handle(
    o: *mut Object,
    heap: *mut Heap,
    context: *mut crate::rt::Context,
) {
    unsafe {
        debug_assert!((*o).rc == SHARED || (*o).rc == 1);
        let w = o as Word;
        let (payload, listed) = if is_native(w) {
            assert!(
                !context.is_null(),
                "a native object was let go of outside any context"
            );
            let mut held = native_slot(o).read();
            held.release(context);
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

pub(crate) unsafe fn count_of(o: *mut Object) -> u32 {
    count(o).load(Relaxed)
}

pub(crate) unsafe fn last_of_shared(o: *mut Object) -> bool {
    let last = count(o).fetch_sub(1, Release) == SHARED | 1;
    if last {
        std::sync::atomic::fence(Acquire);
    }
    last
}

pub fn release_handle(w: Word) {
    unsafe { released(w, CURRENT.with(|c| c.get()), std::ptr::null_mut()) }
}

pub fn release_object(context: *mut crate::rt::Context, w: Word) {
    unsafe { released(w, &raw mut (*context).heap, context) }
}

unsafe fn released(w: Word, heap: *mut Heap, context: *mut crate::rt::Context) {
    let o = object(w);
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
        unsafe { free_handle(o, heap, context) }
    }
}

pub fn inc_shared(w: Word) {
    count(object(w)).fetch_add(1, Relaxed);
}

impl Default for Heap {
    fn default() -> Heap {
        Heap::new()
    }
}

impl Heap {
    pub fn new() -> Heap {
        let mut blocks = Blocks::default();
        let (poison_freed, delay) = (poisoning_enabled(), delay());
        let alloc = Box::new(Alloc {
            blocks: blocks.filling(),
            count: 0,
            slow: usize::from(poison_freed || delay > 0),
        });
        Heap {
            blocks,
            alloc,
            natives: Vec::new(),
            persistent: false,
            runtime_blocks: 0,
            scheduling: false,
            deferred_frees: std::collections::VecDeque::new(),
            poison_freed,
            delay,
        }
    }

    pub(crate) fn alloc(&mut self) -> *mut Alloc {
        &raw mut *self.alloc
    }

    pub(crate) fn replace_scheduler_active(&mut self, on: bool) -> bool {
        self.alloc.slow = usize::from(on || self.poison_freed || self.delay > 0);
        std::mem::replace(&mut self.scheduling, on)
    }

    pub fn adopt_heap(&mut self, mut other: Heap) {
        self.blocks.adopt(std::mem::take(&mut other.blocks));
        for o in other.natives.drain(..) {
            unsafe { set_word(o, NATIVES_INDEX_WORD, self.natives.len() as Word) };
            self.natives.push(o);
        }
        self.deferred_frees.extend(other.deferred_frees.drain(..));
        self.runtime_blocks += other.runtime_blocks;
    }

    pub(crate) fn swap_blocks(&mut self, blocks: Blocks) -> Blocks {
        let was = std::mem::replace(&mut self.blocks, blocks);
        self.alloc.blocks = self.blocks.filling();
        was
    }

    pub fn persistent() -> Heap {
        let mut h = Heap::new();
        h.persistent = true;
        h
    }

    pub fn census(&self) -> Vec<(usize, usize, usize)> {
        size_histogram_in(&self.blocks)
    }

    pub fn live(&self) -> usize {
        self.census().iter().map(|(_, n, _)| n).sum()
    }

    pub fn live_bytes(&self) -> usize {
        self.blocks.live()
    }

    pub fn retained_bytes(&self) -> usize {
        self.blocks.retained_bytes()
    }

    pub fn runtime_blocks(&self) -> usize {
        self.runtime_blocks
    }

    pub(crate) fn program_live_block_count(&self) -> usize {
        program_live_block_count_in(&self.blocks)
    }

    pub fn natives(&self) -> usize {
        self.natives.len()
    }

    unsafe fn unlist(&mut self, o: *mut Object) {
        let at = unsafe { word_at(o, NATIVES_INDEX_WORD) } as usize;
        debug_assert!(
            self.natives.get(at) == Some(&o),
            "a native object died into a heap whose table does not hold it"
        );
        self.natives.swap_remove(at);
        if let Some(&moved) = self.natives.get(at) {
            unsafe { set_word(moved, NATIVES_INDEX_WORD, at as Word) };
        }
    }

    fn drop_natives(&mut self) {
        for o in self.natives.drain(..) {
            unsafe {
                if (*o).rc != 0 {
                    std::ptr::drop_in_place(native_slot(o));
                }
            }
        }
    }

    pub fn object_size(payload_bytes: usize) -> usize {
        (HEADER + payload_bytes.max(8) + 7) & !7
    }

    pub(crate) fn raw_alloc(&mut self, payload_bytes: usize) -> *mut Object {
        let size = Heap::object_size(payload_bytes);
        let p: *mut Object = self.blocks.take(size).cast();
        unsafe {
            p.write(Object {
                rc: 1,
                made_by_scheduler: self.scheduling,
            })
        };
        if !self.scheduling {
            self.runtime_blocks += 1;
        }
        p
    }

    pub fn native(&mut self, held: Box<dyn Native>) -> Word {
        let o = self.raw_alloc(NATIVE_BYTES);
        unsafe {
            set_word(o, 0, &raw const NATIVE_MARK as Word);
            set_word(o, NATIVES_INDEX_WORD, self.natives.len() as Word);
            native_slot(o).write(held);
        }
        self.natives.push(o);
        o as Word
    }

    pub fn end(&mut self) {
        if self.persistent {
            return;
        }
        self.drop_natives();
        self.blocks.reset();
        self.runtime_blocks = 0;
        self.deferred_frees.clear();
    }
}

impl Drop for Heap {
    fn drop(&mut self) {
        self.drop_natives();
    }
}
