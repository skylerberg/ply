//! Where a heap's objects live. A block is one object's memory. Blocks of one size fill a page,
//! pages are cut from segments mapped from the operating system, and a block too big for a page is
//! the system allocator's. A dead block is listed through its own second word; a page left with no
//! live block serves any size next, and a segment left with no such page goes back.

use crate::vm;
use std::alloc::{Layout, alloc, dealloc};
use std::collections::{BTreeMap, BTreeSet};
use std::ptr::null_mut;
use std::sync::atomic::AtomicPtr;
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};

/// A mapping's bytes, which it is aligned to, so a block's address names its segment and page.
const SEGMENT: usize = 4 << 20;
const PAGE: usize = 64 << 10;
const PAGES: usize = SEGMENT / PAGE;
/// The largest block a page holds.
pub const SMALL: usize = 8 << 10;
const CLASSES: usize = 65;
/// The emptied pages a heap keeps as they are for the next page it needs.
const SPARES: usize = 32;
/// A segment's header, which its first page's blocks begin after.
const HEAD: usize = size_of::<Segment>().next_multiple_of(16);
/// The bytes before a larger block, the first word of them naming its owner.
const LEAD: usize = 16;

/// What other threads hand back to a heap: dead blocks, each naming the next in its second word.
#[derive(Default)]
struct Returned {
    small: AtomicPtr<u8>,
    large: AtomicPtr<u8>,
}

struct Page {
    /// Dead blocks, each naming the next in its second word.
    free: *mut u8,
    /// Where the blocks never handed out begin, and where the page's blocks end.
    fresh: *mut u8,
    end: *mut u8,
    /// The heap whose thread alone touches the rest of this.
    owner: AtomicPtr<Returned>,
    next: *mut Page,
    prev: *mut Page,
    /// A block's bytes; zero while the page holds none and waits for any class.
    block: u32,
    live: u32,
    class: u8,
    /// On its class's list, or on one of the lists of emptied pages.
    listed: bool,
    /// Emptied and kept as it is, not let go.
    spare: bool,
}

/// All zeros is a segment none of whose pages holds a block.
struct Segment {
    /// Pages holding blocks.
    held: usize,
    pages: [Page; PAGES],
}

/// The size class of a block of `bytes`: every word count up to sixteen, then eight to a doubling.
pub fn class_of(bytes: usize) -> usize {
    let words = bytes / 8;
    if words <= 16 {
        return words;
    }
    let n = words - 1;
    let top = (usize::BITS - 1 - n.leading_zeros()) as usize;
    8 * (top - 2) + ((n >> (top - 3)) & 7) + 1
}

/// The bytes of a class's blocks: the most a size of the class asks for.
pub fn block_of(class: usize) -> usize {
    if class <= 16 {
        return class * 8;
    }
    let k = class - 17;
    (9 + k % 8) << (k / 8 + 4)
}

/// The bytes of the block [`Blocks::take`] hands out for `bytes`.
pub fn taken_bytes(bytes: usize) -> usize {
    if bytes > SMALL {
        bytes
    } else {
        block_of(class_of(bytes))
    }
}

unsafe fn page_of(block: *mut u8) -> *mut Page {
    let segment = (block.addr() & !(SEGMENT - 1)) as *mut Segment;
    unsafe { &raw mut (*segment).pages[(block.addr() & (SEGMENT - 1)) / PAGE] }
}

fn segment_of(page: *mut Page) -> *mut Segment {
    (page.addr() & !(SEGMENT - 1)) as *mut Segment
}

/// Where a page's blocks may lie and how many bytes that is.
unsafe fn room(page: *mut Page) -> (*mut u8, usize) {
    let segment = segment_of(page);
    unsafe {
        let index = page.offset_from(&raw const (*segment).pages[0]) as usize;
        let lead = if index == 0 { HEAD } else { 0 };
        (segment.cast::<u8>().add(index * PAGE + lead), PAGE - lead)
    }
}

unsafe fn link(block: *mut u8) -> *mut *mut u8 {
    unsafe { block.add(8).cast() }
}

/// The heap a block, of `bytes` when it was taken, was last handed out by or adopted into.
unsafe fn owner(block: *mut u8, bytes: usize) -> &'static AtomicPtr<Returned> {
    unsafe {
        if bytes > SMALL {
            &*block.sub(LEAD).cast()
        } else {
            &(*page_of(block)).owner
        }
    }
}

fn large(bytes: usize) -> Layout {
    Layout::from_size_align(bytes + LEAD, 16).expect("a block's layout")
}

unsafe fn push(list: &mut *mut Page, page: *mut Page) {
    unsafe {
        (*page).prev = null_mut();
        (*page).next = *list;
        if !(*list).is_null() {
            (**list).prev = page;
        }
        *list = page;
        (*page).listed = true;
    }
}

/// Lists a page after the list's first, which stays the one being filled.
unsafe fn push_behind(list: &mut *mut Page, page: *mut Page) {
    let head = *list;
    if head.is_null() {
        return unsafe { push(list, page) };
    }
    unsafe {
        (*page).prev = head;
        (*page).next = (*head).next;
        if !(*head).next.is_null() {
            (*(*head).next).prev = page;
        }
        (*head).next = page;
        (*page).listed = true;
    }
}

unsafe fn unlink(list: &mut *mut Page, page: *mut Page) {
    unsafe {
        let (prev, next) = ((*page).prev, (*page).next);
        if prev.is_null() {
            *list = next;
        } else {
            (*prev).next = next;
        }
        if !next.is_null() {
            (*next).prev = prev;
        }
        (*page).prev = null_mut();
        (*page).next = null_mut();
        (*page).listed = false;
    }
}

/// A block the page has on hand, or null: a dead one first, then one never handed out.
#[inline]
unsafe fn pop(page: *mut Page, reused: &mut usize) -> *mut u8 {
    unsafe {
        let block = (*page).free;
        if !block.is_null() {
            (*page).free = *link(block);
            (*page).live += 1;
            *reused += 1;
            return block;
        }
        let block = (*page).fresh;
        if (*page).end.addr() - block.addr() < (*page).block as usize {
            return null_mut();
        }
        (*page).fresh = block.add((*page).block as usize);
        (*page).live += 1;
        block
    }
}

/// Hands a dead block, of `bytes` when it was taken, back to the heap that owns it, from any
/// thread: the heap takes it up when it next needs a page.
pub unsafe fn hand_back(block: *mut u8, bytes: usize) {
    unsafe {
        // A page of an ended heap has no owner, and its blocks are already given back.
        let Some(heap) = owner(block, bytes).load(Acquire).as_ref() else {
            return;
        };
        let list = if bytes > SMALL {
            &heap.large
        } else {
            &heap.small
        };
        let mut head = list.load(Relaxed);
        loop {
            *link(block) = head;
            match list.compare_exchange_weak(head, block, Release, Relaxed) {
                Ok(_) => return,
                Err(now) => head = now,
            }
        }
    }
}

/// One heap's blocks. Only the thread running the heap takes from it and gives to it; any thread
/// may [`hand_back`] a block of it.
pub struct Blocks {
    /// Boxed, so its address is the heap's name wherever the heap moves.
    returned: Box<Returned>,
    /// By class, the pages with a block on hand, the first the one being filled.
    filling: [*mut Page; CLASSES],
    spare: *mut Page,
    spares: usize,
    /// Emptied pages let go, and pages never used.
    clean: *mut Page,
    /// Pages on `spare` and `clean`.
    empty: usize,
    segments: BTreeSet<usize>,
    /// The bytes of each block the system allocator holds, by its address.
    large: BTreeMap<usize, usize>,
    reused: usize,
}

impl Default for Blocks {
    fn default() -> Blocks {
        Blocks {
            returned: Box::default(),
            filling: [null_mut(); CLASSES],
            spare: null_mut(),
            spares: 0,
            clean: null_mut(),
            empty: 0,
            segments: BTreeSet::new(),
            large: BTreeMap::new(),
            reused: 0,
        }
    }
}

impl Blocks {
    fn name(&self) -> *mut Returned {
        (&raw const *self.returned).cast_mut()
    }

    /// A block of at least `bytes`, a multiple of eight, holding whatever was last there.
    #[inline]
    pub fn take(&mut self, bytes: usize) -> *mut u8 {
        unsafe {
            if bytes > SMALL {
                if !self.returned.large.load(Relaxed).is_null() {
                    self.regain(self.name());
                }
                let base = alloc(large(bytes));
                assert!(!base.is_null(), "the heap is out of memory");
                base.cast::<AtomicPtr<Returned>>()
                    .write(AtomicPtr::new(self.name()));
                let block = base.add(LEAD);
                self.large.insert(block.addr(), bytes);
                return block;
            }
            let class = class_of(bytes);
            let page = self.filling[class];
            if !page.is_null() {
                let block = pop(page, &mut self.reused);
                if !block.is_null() {
                    return block;
                }
            }
            self.refill(class)
        }
    }

    /// A block of the class's next page with one on hand, which becomes the one being filled.
    #[cold]
    unsafe fn refill(&mut self, class: usize) -> *mut u8 {
        unsafe {
            loop {
                let page = self.filling[class];
                if page.is_null() {
                    if self.regain(self.name()) {
                        continue;
                    }
                    let page = self.page(class);
                    push(&mut self.filling[class], page);
                    continue;
                }
                let block = pop(page, &mut self.reused);
                if !block.is_null() {
                    return block;
                }
                unlink(&mut self.filling[class], page);
            }
        }
    }

    /// Takes back the blocks of this heap's that were handed back to `returned`; whether any were.
    unsafe fn regain(&mut self, returned: *const Returned) -> bool {
        unsafe {
            let mut small = (*returned).small.swap(null_mut(), Acquire);
            let mut large = (*returned).large.swap(null_mut(), Acquire);
            let any = !small.is_null() || !large.is_null();
            while !small.is_null() {
                let next = *link(small);
                self.give(small, SMALL);
                small = next;
            }
            while !large.is_null() {
                let next = *link(large);
                self.give(large, SMALL + 8);
                large = next;
            }
            any
        }
    }

    /// An emptied page, or one of a new segment, made `class`'s.
    unsafe fn page(&mut self, class: usize) -> *mut Page {
        unsafe {
            let page = if self.spare.is_null() {
                if self.clean.is_null() {
                    let segment = vm::map_aligned(SEGMENT);
                    assert!(!segment.is_null(), "the heap is out of memory");
                    self.segments.insert(segment.addr());
                    self.list(segment.cast());
                }
                let page = self.clean;
                unlink(&mut self.clean, page);
                page
            } else {
                let page = self.spare;
                unlink(&mut self.spare, page);
                self.spares -= 1;
                (*page).spare = false;
                page
            };
            self.empty -= 1;
            let (start, len) = room(page);
            let size = block_of(class);
            (*page).free = null_mut();
            (*page).fresh = start;
            (*page).end = start.add(len / size * size);
            (*page).owner.store(self.name(), Release);
            (*page).block = size as u32;
            (*page).live = 0;
            (*page).class = class as u8;
            (*segment_of(page)).held += 1;
            page
        }
    }

    /// Lists every page of a segment whose header is zeros as clean, the lowest to be taken first.
    unsafe fn list(&mut self, segment: *mut Segment) {
        for i in (0..PAGES).rev() {
            unsafe { push(&mut self.clean, &raw mut (*segment).pages[i]) };
        }
        self.empty += PAGES;
    }

    /// Whether `block`, of `bytes` when it was taken, is this heap's to take back.
    pub unsafe fn owns(&self, block: *mut u8, bytes: usize) -> bool {
        unsafe { owner(block, bytes).load(Relaxed) == self.name() }
    }

    /// Takes back a dead block this heap [owns](Blocks::owns). Its first word is left as it is.
    pub unsafe fn give(&mut self, block: *mut u8, bytes: usize) {
        unsafe {
            if bytes > SMALL {
                if let Some(bytes) = self.large.remove(&block.addr()) {
                    dealloc(block.sub(LEAD), large(bytes));
                }
                return;
            }
            let page = page_of(block);
            *link(block) = (*page).free;
            (*page).free = block;
            (*page).live -= 1;
            let class = (*page).class as usize;
            if (*page).live == 0 {
                if self.filling[class] != page {
                    self.retire(page);
                }
            } else if !(*page).listed {
                push_behind(&mut self.filling[class], page);
            }
        }
    }

    /// A page none of whose blocks is live leaves its class: kept as it is while few are, else let
    /// go. Its segment goes back with it when that leaves the heap a segment's worth of others.
    unsafe fn retire(&mut self, page: *mut Page) {
        unsafe {
            if (*page).listed {
                unlink(&mut self.filling[(*page).class as usize], page);
            }
            (*page).block = 0;
            if self.spares < SPARES {
                (*page).spare = true;
                self.spares += 1;
                push(&mut self.spare, page);
            } else {
                let (start, len) = room(page);
                // A first page begins after the header, where the system's own pages need not.
                if len == PAGE {
                    vm::let_go(start, len);
                }
                push(&mut self.clean, page);
            }
            self.empty += 1;
            let segment = segment_of(page);
            (*segment).held -= 1;
            if (*segment).held > 0 || self.empty < 2 * PAGES {
                return;
            }
            for i in 0..PAGES {
                let page = &raw mut (*segment).pages[i];
                if (*page).spare {
                    self.spares -= 1;
                    unlink(&mut self.spare, page);
                } else {
                    unlink(&mut self.clean, page);
                }
            }
            self.empty -= PAGES;
            self.segments.remove(&segment.addr());
            vm::unmap(segment.cast(), SEGMENT);
        }
    }

    /// Whether `address` is where a block this heap handed out begins.
    pub fn holds(&self, address: usize) -> bool {
        self.bytes_at(address).is_some()
    }

    /// The bytes of the block at `address`, where one this heap handed out begins there.
    pub fn bytes_at(&self, address: usize) -> Option<usize> {
        if !self.segments.contains(&(address & !(SEGMENT - 1))) {
            return self.large.get(&address).copied();
        }
        unsafe {
            let page = page_of(address as *mut u8);
            let block = (*page).block as usize;
            if block == 0 {
                return None;
            }
            let start = room(page).0.addr();
            (address >= start
                && address < (*page).fresh.addr()
                && (address - start).is_multiple_of(block))
            .then_some(block)
        }
    }

    /// Every block handed out and not since emptied with its page, dead ones among them, with its
    /// bytes.
    pub fn each(&self, mut f: impl FnMut(*mut u8, usize)) {
        unsafe {
            for &address in &self.segments {
                let segment = address as *mut Segment;
                for i in 0..PAGES {
                    let page = &raw mut (*segment).pages[i];
                    let bytes = (*page).block as usize;
                    if bytes == 0 {
                        continue;
                    }
                    let mut block = room(page).0;
                    while block < (*page).fresh {
                        f(block, bytes);
                        block = block.add(bytes);
                    }
                }
            }
        }
        for (&address, &bytes) in &self.large {
            f(address as *mut u8, bytes);
        }
    }

    /// The bytes of the pages holding blocks and of the larger blocks, whatever is live in them.
    pub fn held(&self) -> usize {
        let pages: usize = self
            .segments
            .iter()
            .map(|&address| unsafe { (*(address as *mut Segment)).held })
            .sum();
        pages * PAGE + self.large.values().sum::<usize>()
    }

    /// The bytes of the blocks handed out and not given back, each its size class's whole block: a
    /// block another thread let go of counts until this heap takes it back.
    pub fn live(&self) -> usize {
        let pages: usize = self
            .segments
            .iter()
            .map(|&address| unsafe {
                (*(address as *mut Segment))
                    .pages
                    .iter()
                    .map(|page| page.live as usize * page.block as usize)
                    .sum::<usize>()
            })
            .sum();
        pages + self.large.values().sum::<usize>()
    }

    /// The bytes mapped for pages, in use or not.
    pub fn mapped(&self) -> usize {
        self.segments.len() * SEGMENT
    }

    /// Blocks handed out that had held an object before.
    pub fn reused(&self) -> usize {
        self.reused
    }

    /// Takes over everything `other` holds: its blocks are this heap's from here on.
    pub fn adopt(&mut self, mut other: Blocks) {
        unsafe {
            for &address in &other.segments {
                let segment = address as *mut Segment;
                for i in 0..PAGES {
                    let page = &raw mut (*segment).pages[i];
                    if (*page).block == 0 {
                        if (*page).spare {
                            self.spares += 1;
                            push(&mut self.spare, page);
                        } else {
                            push(&mut self.clean, page);
                        }
                        self.empty += 1;
                        continue;
                    }
                    (*page).owner.store(self.name(), Release);
                    if (*page).listed {
                        push_behind(&mut self.filling[(*page).class as usize], page);
                    }
                }
            }
            for (&address, &bytes) in &other.large {
                owner(address as *mut u8, bytes).store(self.name(), Release);
            }
            self.segments.append(&mut other.segments);
            self.large.append(&mut other.large);
            self.reused += other.reused;
            self.regain(other.name());
        }
    }

    /// Gives back everything but one segment, which the next blocks come from.
    pub fn reset(&mut self) {
        unsafe {
            for (address, bytes) in std::mem::take(&mut self.large) {
                dealloc((address as *mut u8).sub(LEAD), large(bytes));
            }
            let kept = self.segments.first().copied();
            self.segments.retain(|&address| {
                if Some(address) != kept {
                    vm::unmap(address as *mut u8, SEGMENT);
                }
                Some(address) == kept
            });
            *self.returned = Returned::default();
            self.filling = [null_mut(); CLASSES];
            self.spare = null_mut();
            self.spares = 0;
            self.clean = null_mut();
            self.empty = 0;
            self.reused = 0;
            if let Some(address) = kept {
                let segment = address as *mut Segment;
                segment.write_bytes(0, 1);
                self.list(segment);
            }
        }
    }
}

impl Drop for Blocks {
    fn drop(&mut self) {
        unsafe {
            for (&address, &bytes) in &self.large {
                dealloc((address as *mut u8).sub(LEAD), large(bytes));
            }
            for &address in &self.segments {
                vm::unmap(address as *mut u8, SEGMENT);
            }
        }
    }
}
