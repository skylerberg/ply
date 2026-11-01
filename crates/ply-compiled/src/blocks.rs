use crate::vm;
use std::alloc::{Layout, alloc, dealloc};
use std::collections::{BTreeMap, BTreeSet};
use std::ptr::null_mut;
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release, SeqCst};
use std::sync::atomic::{AtomicPtr, AtomicU32};
use std::sync::{Mutex, PoisonError};

pub(crate) const SEGMENT: usize = 4 << 20;
pub(crate) const PAGE: usize = 64 << 10;
const PAGES: usize = SEGMENT / PAGE;
pub(crate) const WORD_STEP_SIZE_CLASSES: usize = 16;
pub(crate) const LINK_OFFSET: usize = 8;
pub const SMALL: usize = 8 << 10;
const CLASSES: usize = 65;
const MAX_SPARE_PAGES: usize = 32;
const HEAD: usize = size_of::<Segment>().next_multiple_of(16);
const LARGE_HEADER_BYTES: usize = 16;
const MAX_POOLED_SEGMENTS: usize = 64;

static EMPTIED_SEGMENTS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

fn allocate_segment() -> *mut Segment {
    let emptied = EMPTIED_SEGMENTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .pop();
    match emptied {
        Some(address) => address as *mut Segment,
        None => unsafe { vm::map_aligned(SEGMENT) }.cast(),
    }
}

unsafe fn release_segment(segment: *mut Segment) {
    unsafe { segment.write_bytes(0, 1) };
    let mut emptied = EMPTIED_SEGMENTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if emptied.len() < MAX_POOLED_SEGMENTS {
        emptied.push(segment.addr());
        return;
    }
    drop(emptied);
    unsafe { vm::unmap(segment.cast(), SEGMENT) };
}

#[derive(Default)]
pub(crate) struct Returned {
    small: AtomicPtr<u8>,
    large: AtomicPtr<u8>,
}

#[repr(C)]
pub(crate) struct Page {
    pub(crate) free: *mut u8,
    pub(crate) fresh: *mut u8,
    pub(crate) end: *mut u8,
    pub(crate) owner: AtomicPtr<Returned>,
    pub(crate) remote_free_state: AtomicU32,
    pub(crate) next: *mut Page,
    pub(crate) previous: *mut Page,
    pub(crate) block: u32,
    pub(crate) live: u32,
    pub(crate) class: u8,
    pub(crate) listed: bool,
    pub(crate) spare: bool,
}

#[repr(C)]
pub(crate) struct Segment {
    retained_bytes: usize,
    pub(crate) pages: [Page; PAGES],
}

pub fn class_of(bytes: usize) -> usize {
    let words = bytes / 8;
    if words <= WORD_STEP_SIZE_CLASSES {
        return words;
    }
    let n = words - 1;
    let top = (usize::BITS - 1 - n.leading_zeros()) as usize;
    8 * (top - 2) + ((n >> (top - 3)) & 7) + 1
}

pub fn class_block_bytes(class: usize) -> usize {
    if class <= WORD_STEP_SIZE_CLASSES {
        return class * 8;
    }
    let k = class - WORD_STEP_SIZE_CLASSES - 1;
    (9 + k % 8) << (k / 8 + 4)
}

pub fn reserved_bytes(bytes: usize) -> usize {
    if bytes > SMALL {
        bytes
    } else {
        class_block_bytes(class_of(bytes))
    }
}

unsafe fn page_of(block: *mut u8) -> *mut Page {
    let segment = (block.addr() & !(SEGMENT - 1)) as *mut Segment;
    unsafe { &raw mut (*segment).pages[(block.addr() & (SEGMENT - 1)) / PAGE] }
}

fn segment_of(page: *mut Page) -> *mut Segment {
    (page.addr() & !(SEGMENT - 1)) as *mut Segment
}

unsafe fn room(page: *mut Page) -> (*mut u8, usize) {
    let segment = segment_of(page);
    unsafe {
        let index = page.offset_from(&raw const (*segment).pages[0]) as usize;
        let lead = if index == 0 { HEAD } else { 0 };
        (segment.cast::<u8>().add(index * PAGE + lead), PAGE - lead)
    }
}

unsafe fn link(block: *mut u8) -> *mut *mut u8 {
    unsafe { block.add(LINK_OFFSET).cast() }
}

unsafe fn owner(block: *mut u8, bytes: usize) -> &'static AtomicPtr<Returned> {
    unsafe {
        if bytes > SMALL {
            &*block.sub(LARGE_HEADER_BYTES).cast()
        } else {
            &(*page_of(block)).owner
        }
    }
}

unsafe fn remote_free_state_of(block: *mut u8, bytes: usize) -> &'static AtomicU32 {
    unsafe {
        if bytes > SMALL {
            &*block.sub(LARGE_HEADER_BYTES - 8).cast()
        } else {
            &(*page_of(block)).remote_free_state
        }
    }
}

fn waited(handing: &AtomicU32) {
    while handing.load(SeqCst) != 0 {
        std::hint::spin_loop();
    }
}

fn large(bytes: usize) -> Layout {
    Layout::from_size_align(bytes + LARGE_HEADER_BYTES, 16).expect("a block's layout")
}

unsafe fn push(list: &mut *mut Page, page: *mut Page) {
    unsafe {
        (*page).previous = null_mut();
        (*page).next = *list;
        if !(*list).is_null() {
            (**list).previous = page;
        }
        *list = page;
        (*page).listed = true;
    }
}

unsafe fn push_after_head(list: &mut *mut Page, page: *mut Page) {
    let head = *list;
    if head.is_null() {
        return unsafe { push(list, page) };
    }
    unsafe {
        (*page).previous = head;
        (*page).next = (*head).next;
        if !(*head).next.is_null() {
            (*(*head).next).previous = page;
        }
        (*head).next = page;
        (*page).listed = true;
    }
}

unsafe fn unlink(list: &mut *mut Page, page: *mut Page) {
    unsafe {
        let (previous, next) = ((*page).previous, (*page).next);
        if previous.is_null() {
            *list = next;
        } else {
            (*previous).next = next;
        }
        if !next.is_null() {
            (*next).previous = previous;
        }
        (*page).previous = null_mut();
        (*page).next = null_mut();
        (*page).listed = false;
    }
}

#[inline]
unsafe fn pop(page: *mut Page) -> *mut u8 {
    unsafe {
        let block = (*page).free;
        if !block.is_null() {
            (*page).free = *link(block);
            (*page).live += 1;
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

pub unsafe fn free_block(block: *mut u8, bytes: usize) {
    unsafe { RemoteFree::begin(block, bytes).finish() }
}

pub struct RemoteFree {
    block: *mut u8,
    bytes: usize,
    heap: *const Returned,
}

impl RemoteFree {
    pub unsafe fn begin(block: *mut u8, bytes: usize) -> RemoteFree {
        unsafe {
            remote_free_state_of(block, bytes).fetch_add(1, SeqCst);
            RemoteFree {
                block,
                bytes,
                heap: owner(block, bytes).load(SeqCst),
            }
        }
    }

    pub unsafe fn finish(self) {
        unsafe {
            let Some(heap) = self.heap.as_ref() else {
                return;
            };
            let list = if self.bytes > SMALL {
                &heap.large
            } else {
                &heap.small
            };
            let mut head = list.load(Relaxed);
            loop {
                *link(self.block) = head;
                match list.compare_exchange_weak(head, self.block, Release, Relaxed) {
                    Ok(_) => return,
                    Err(now) => head = now,
                }
            }
        }
    }
}

impl Drop for RemoteFree {
    fn drop(&mut self) {
        unsafe { remote_free_state_of(self.block, self.bytes).fetch_sub(1, Release) };
    }
}

#[repr(C)]
pub(crate) struct ActivePages {
    pub(crate) pages: [*mut Page; CLASSES],
    pub(crate) name: *const Returned,
    pub(crate) reused: usize,
}

pub struct Blocks {
    returned: Box<Returned>,
    filling: Box<ActivePages>,
    spare: *mut Page,
    spare_page_count: usize,
    clean: *mut Page,
    empty: usize,
    segments: BTreeSet<usize>,
    large: BTreeMap<usize, usize>,
}

impl Default for Blocks {
    fn default() -> Blocks {
        let returned = Box::<Returned>::default();
        let filling = Box::new(ActivePages {
            pages: [null_mut(); CLASSES],
            name: &raw const *returned,
            reused: 0,
        });
        Blocks {
            returned,
            filling,
            spare: null_mut(),
            spare_page_count: 0,
            clean: null_mut(),
            empty: 0,
            segments: BTreeSet::new(),
            large: BTreeMap::new(),
        }
    }
}

impl Blocks {
    fn name(&self) -> *mut Returned {
        (&raw const *self.returned).cast_mut()
    }

    pub(crate) fn filling(&mut self) -> *mut ActivePages {
        &raw mut *self.filling
    }

    #[inline]
    pub fn take(&mut self, bytes: usize) -> *mut u8 {
        unsafe {
            if bytes > SMALL {
                if !self.returned.large.load(Relaxed).is_null() {
                    self.reclaim_remote_frees(self.name());
                }
                let base = alloc(large(bytes));
                assert!(!base.is_null(), "the heap is out of memory");
                base.cast::<AtomicPtr<Returned>>()
                    .write(AtomicPtr::new(self.name()));
                base.add(8).cast::<AtomicU32>().write(AtomicU32::new(0));
                let block = base.add(LARGE_HEADER_BYTES);
                self.large.insert(block.addr(), bytes);
                return block;
            }
            let class = class_of(bytes);
            let page = self.filling.pages[class];
            if !page.is_null() {
                let block = pop(page);
                if !block.is_null() {
                    return block;
                }
            }
            self.refill(class)
        }
    }

    #[cold]
    unsafe fn refill(&mut self, class: usize) -> *mut u8 {
        unsafe {
            loop {
                let page = self.filling.pages[class];
                if page.is_null() {
                    if self.reclaim_remote_frees(self.name()) {
                        continue;
                    }
                    let page = self.page(class);
                    push(&mut self.filling.pages[class], page);
                    continue;
                }
                let block = pop(page);
                if !block.is_null() {
                    return block;
                }
                unlink(&mut self.filling.pages[class], page);
            }
        }
    }

    unsafe fn reclaim_remote_frees(&mut self, returned: *const Returned) -> bool {
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

    unsafe fn page(&mut self, class: usize) -> *mut Page {
        unsafe {
            let page = if self.spare.is_null() {
                if self.clean.is_null() {
                    let segment = allocate_segment();
                    assert!(!segment.is_null(), "the heap is out of memory");
                    self.segments.insert(segment.addr());
                    self.list(segment);
                }
                let page = self.clean;
                unlink(&mut self.clean, page);
                page
            } else {
                let page = self.spare;
                unlink(&mut self.spare, page);
                self.spare_page_count -= 1;
                (*page).spare = false;
                page
            };
            self.empty -= 1;
            let (start, len) = room(page);
            let size = class_block_bytes(class);
            (*page).free = null_mut();
            (*page).fresh = start;
            (*page).end = start.add(len / size * size);
            (*page).owner.store(self.name(), Release);
            (*page).block = size as u32;
            (*page).live = 0;
            (*page).class = class as u8;
            (*segment_of(page)).retained_bytes += 1;
            page
        }
    }

    unsafe fn list(&mut self, segment: *mut Segment) {
        for i in (0..PAGES).rev() {
            unsafe { push(&mut self.clean, &raw mut (*segment).pages[i]) };
        }
        self.empty += PAGES;
    }

    pub unsafe fn owns(&self, block: *mut u8, bytes: usize) -> bool {
        unsafe { owner(block, bytes).load(Relaxed) == self.name() }
    }

    pub unsafe fn give(&mut self, block: *mut u8, bytes: usize) {
        unsafe {
            if bytes > SMALL {
                if let Some(bytes) = self.large.remove(&block.addr()) {
                    dealloc(block.sub(LARGE_HEADER_BYTES), large(bytes));
                }
                return;
            }
            let page = page_of(block);
            *link(block) = (*page).free;
            (*page).free = block;
            (*page).live -= 1;
            let class = (*page).class as usize;
            if (*page).live == 0 {
                if self.filling.pages[class] != page {
                    self.retire(page);
                }
            } else if !(*page).listed {
                push_after_head(&mut self.filling.pages[class], page);
            }
        }
    }

    unsafe fn retire(&mut self, page: *mut Page) {
        unsafe {
            if (*page).listed {
                unlink(&mut self.filling.pages[(*page).class as usize], page);
            }
            self.recycle_empty_page(page);
            let segment = segment_of(page);
            (*segment).retained_bytes -= 1;
            if (*segment).retained_bytes > 0 || self.empty < 2 * PAGES {
                return;
            }
            for i in 0..PAGES {
                let page = &raw mut (*segment).pages[i];
                if (*page).spare {
                    self.spare_page_count -= 1;
                    unlink(&mut self.spare, page);
                } else {
                    unlink(&mut self.clean, page);
                }
            }
            self.empty -= PAGES;
            self.segments.remove(&segment.addr());
            release_segment(segment);
        }
    }

    unsafe fn recycle_empty_page(&mut self, page: *mut Page) {
        unsafe {
            (*page).block = 0;
            (*page).spare = self.spare_page_count < MAX_SPARE_PAGES;
            if (*page).spare {
                self.spare_page_count += 1;
                push(&mut self.spare, page);
            } else {
                let (start, len) = room(page);
                if len == PAGE {
                    vm::let_go(start, len);
                }
                push(&mut self.clean, page);
            }
            self.empty += 1;
        }
    }

    pub fn contains(&self, address: usize) -> bool {
        self.bytes_at(address).is_some()
    }

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

    pub fn retained_bytes(&self) -> usize {
        let pages: usize = self
            .segments
            .iter()
            .map(|&address| unsafe { (*(address as *mut Segment)).retained_bytes })
            .sum();
        pages * PAGE + self.large.values().sum::<usize>()
    }

    pub fn spare_bytes(&self) -> usize {
        self.spare_page_count * PAGE
    }

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

    pub fn mapped(&self) -> usize {
        self.segments.len() * SEGMENT
    }

    pub fn adopt(&mut self, mut other: Blocks) {
        unsafe {
            other.segments.retain(|&address| {
                let segment = address as *mut Segment;
                if (*segment).pages.iter().all(|page| page.live == 0) && self.empty >= PAGES {
                    release_segment(segment);
                    return false;
                }
                for i in 0..PAGES {
                    let page = &raw mut (*segment).pages[i];
                    if (*page).live > 0 {
                        (*page).owner.store(self.name(), SeqCst);
                        if (*page).listed {
                            push_after_head(&mut self.filling.pages[(*page).class as usize], page);
                        }
                    } else if (*page).block > 0 || (*page).spare {
                        if (*page).block > 0 {
                            (*segment).retained_bytes -= 1;
                        }
                        self.recycle_empty_page(page);
                    } else {
                        push(&mut self.clean, page);
                        self.empty += 1;
                    }
                }
                true
            });
            for (&address, &bytes) in &other.large {
                owner(address as *mut u8, bytes).store(self.name(), SeqCst);
            }
            other.quiet();
            self.segments.append(&mut other.segments);
            self.large.append(&mut other.large);
            self.reclaim_remote_frees(other.name());
        }
    }

    fn quiet(&self) {
        unsafe {
            for &address in &self.segments {
                let segment = address as *mut Segment;
                for i in 0..PAGES {
                    waited(&(*segment).pages[i].remote_free_state);
                }
            }
            for (&address, &bytes) in &self.large {
                waited(remote_free_state_of(address as *mut u8, bytes));
            }
        }
    }

    pub fn reset(&mut self) {
        self.quiet();
        unsafe {
            for (address, bytes) in std::mem::take(&mut self.large) {
                dealloc((address as *mut u8).sub(LARGE_HEADER_BYTES), large(bytes));
            }
            let kept = self.segments.first().copied();
            self.segments.retain(|&address| {
                if Some(address) != kept {
                    release_segment(address as *mut Segment);
                }
                Some(address) == kept
            });
            *self.returned = Returned::default();
            self.filling.pages = [null_mut(); CLASSES];
            self.spare = null_mut();
            self.spare_page_count = 0;
            self.clean = null_mut();
            self.empty = 0;
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
        self.quiet();
        unsafe {
            for (&address, &bytes) in &self.large {
                dealloc((address as *mut u8).sub(LARGE_HEADER_BYTES), large(bytes));
            }
            for &address in &self.segments {
                release_segment(address as *mut Segment);
            }
        }
    }
}
