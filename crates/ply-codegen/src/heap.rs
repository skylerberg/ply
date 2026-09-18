//! The value model compiled code runs on: one word per value, an `Int` immediate (low bit set)
//! or a pointer to a counted [`Obj`]; anything not laid out natively is bridged as a [`Value`].

use crate::list;
use crate::map;
use ply_eval::{Closure, ClosureKind, Fields, Value};
use ply_span::Symbol;
use std::alloc::{Layout, alloc, dealloc};
use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

pub type Word = i64;

pub const KIND_UNIT: u8 = 0;
pub const KIND_BOOL: u8 = 1;
pub const KIND_INT: u8 = 2;
pub const KIND_RECORD: u8 = 3;
pub const KIND_CTOR: u8 = 4;
pub const KIND_LIST: u8 = 5;
pub const KIND_CLOSURE: u8 = 6;
pub const KIND_BRIDGE: u8 = 7;
/// A map's handle: one word, its tree's root or zero.
pub const KIND_MAP: u8 = 8;
/// `len` bytes of UTF-8 with room for `layout`.
pub const KIND_STR: u8 = 9;
/// `len` bytes with room for `layout`.
pub const KIND_BYTES: u8 = 10;
/// A list's trie nodes: `len` elements or children of `layout` slots.
pub const KIND_LEAF: u8 = 11;
pub const KIND_BRANCH: u8 = 12;
/// A map's tree nodes: a leaf of `len` sorted pairs, a branch of `len` children and max keys.
pub const KIND_MLEAF: u8 = 13;
pub const KIND_MBRANCH: u8 = 14;
pub const KIND_DEAD: u8 = 255;

/// A count no increment or decrement touches: the singletons, the constant pool, the memo.
pub const IMMORTAL: u32 = u32::MAX;

pub const HEADER: usize = 16;

/// A record or constructor none of whose fields holds a count: releasing it walks nothing.
pub const FLAT: u8 = 1;

/// `len`: payload words, byte count or list length; `layout`: shape, ctor index, capacity, arity
/// or dropped list prefix; `flags`/`aux`: a `Bool`'s value, a list's tail length and capacity.
#[repr(C, align(8))]
pub struct Obj {
    pub rc: u32,
    pub kind: u8,
    pub flags: u8,
    pub aux: u16,
    pub len: u32,
    pub layout: u32,
}

static UNIT_OBJ: Obj = Obj {
    rc: IMMORTAL,
    kind: KIND_UNIT,
    flags: 0,
    aux: 0,
    len: 0,
    layout: 0,
};
static TRUE_OBJ: Obj = Obj {
    rc: IMMORTAL,
    kind: KIND_BOOL,
    flags: 1,
    aux: 0,
    len: 0,
    layout: 0,
};
static FALSE_OBJ: Obj = Obj {
    rc: IMMORTAL,
    kind: KIND_BOOL,
    flags: 0,
    aux: 0,
    len: 0,
    layout: 0,
};

#[inline]
pub fn unit() -> Word {
    &raw const UNIT_OBJ as Word
}

#[inline]
pub fn bool(b: bool) -> Word {
    if b {
        &raw const TRUE_OBJ as Word
    } else {
        &raw const FALSE_OBJ as Word
    }
}

#[inline]
pub fn is_imm(w: Word) -> bool {
    w & 1 == 1
}

/// Whether `v` fits the sixty-three bits an immediate carries.
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
pub fn obj(w: Word) -> *mut Obj {
    debug_assert!(!is_imm(w) && w != 0);
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

/// Under `PLY_C_PHASES`, allocations are also tallied by layout, at a map insert each.
fn census_by_layout() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("PLY_C_PHASES").is_some())
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

/// A bytes value of one byte or none, laid out as the heap lays one out.
#[repr(C)]
struct SmallBytes {
    obj: Obj,
    payload: [u8; 8],
}

const fn small_bytes_object(len: u32) -> Obj {
    Obj {
        rc: IMMORTAL,
        kind: KIND_BYTES,
        flags: 0,
        aux: 0,
        len,
        layout: len,
    }
}

static EMPTY_BYTES: SmallBytes = SmallBytes {
    obj: small_bytes_object(0),
    payload: [0; 8],
};

static ONE_BYTE: [SmallBytes; 256] = {
    let mut out = [const {
        SmallBytes {
            obj: small_bytes_object(1),
            payload: [0; 8],
        }
    }; 256];
    let mut i = 0;
    while i < 256 {
        out[i].payload[0] = i as u8;
        i += 1;
    }
    out
};

/// The immortal bytes value holding `b` when it is one byte or none.
pub fn small_bytes(b: &[u8]) -> Option<Word> {
    match b {
        [] => Some(&raw const EMPTY_BYTES.obj as Word),
        [one] => Some(&raw const ONE_BYTE[*one as usize].obj as Word),
        _ => None,
    }
}

/// The poison diagnostic: the dead-payload word and the check every object read passes.
pub mod poison {
    use super::{IMMORTAL, KIND_DEAD, Obj, Word, is_imm};

    /// Dead by its header and immortal by its count: a read through it is caught, never counted.
    static POISONED: Obj = Obj {
        rc: IMMORTAL,
        kind: KIND_DEAD,
        flags: 0,
        aux: 0,
        len: 0,
        layout: 0,
    };

    pub fn word() -> Word {
        &raw const POISONED as Word
    }

    thread_local! {
        /// Three consecutive `i64`s in the running entry's `Ctx`: its site root, start and end.
        static SITE: std::cell::Cell<*const i64> = const { std::cell::Cell::new(std::ptr::null()) };
    }

    pub fn enter(site: *const i64) {
        SITE.with(|s| s.set(site));
    }

    pub fn leave() {
        SITE.with(|s| s.set(std::ptr::null()));
    }

    /// Panics, naming the body's site, when `w` is the poison word or points at a dead header.
    pub fn check(w: Word) {
        if is_imm(w) || w == 0 {
            return;
        }
        let why = if w == word() {
            "a payload word poisoned when its object died"
        } else if unsafe { (*(w as *const Obj)).kind } == KIND_DEAD {
            "an object that has died"
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
        panic!("a stale read: {why}, at {at}");
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

/// A closure's payload: its compiled code's address first, then the captured values.
pub const CLOSURE_CODE: usize = 0;
pub const CLOSURE_CAPTURES: usize = 1;

/// The bridged value lives in the payload as an owned `Value`.
unsafe fn bridge_slot(o: *mut Obj) -> *mut Value {
    unsafe { words(o) as *mut Value }
}

pub unsafe fn bridged<'a>(o: *mut Obj) -> &'a Value {
    unsafe { &*bridge_slot(o) }
}

pub unsafe fn bridged_mut<'a>(o: *mut Obj) -> &'a mut Value {
    unsafe { &mut *bridge_slot(o) }
}

/// Where a string's or a bytes value's payload starts.
#[inline]
pub unsafe fn bytes_ptr(o: *mut Obj) -> *mut u8 {
    unsafe { (o as *mut u8).add(HEADER) }
}

/// The bytes a string or a bytes value holds, borrowed.
#[inline]
pub unsafe fn bytes_of<'a>(o: *mut Obj) -> &'a [u8] {
    unsafe { std::slice::from_raw_parts(bytes_ptr(o), (*o).len as usize) }
}

/// A string's payload as the text it is: a native string never holds anything but UTF-8.
pub unsafe fn str_of<'a>(o: *mut Obj) -> &'a str {
    unsafe { std::str::from_utf8_unchecked(bytes_of(o)) }
}

/// Record shapes: a shape is its sorted field names, and a field's offset its position in them.
#[derive(Default)]
pub struct Shapes {
    ids: HashMap<Rc<[Symbol]>, u32>,
    names: Vec<Rc<[Symbol]>>,
}

impl Shapes {
    fn intern(&mut self, mut fields: Vec<Symbol>) -> u32 {
        fields.sort();
        if let Some(id) = self.ids.get(fields.as_slice()) {
            return *id;
        }
        let id = self.names.len() as u32;
        let rc: Rc<[Symbol]> = Rc::from(fields);
        self.ids.insert(rc.clone(), id);
        self.names.push(rc);
        id
    }
}

/// Record shapes and constructors by index, shared by the compiler and the running entry.
pub struct Layouts {
    shapes: RefCell<Shapes>,
    pub ctors: Vec<(Symbol, usize)>,
    ctor_ids: HashMap<Symbol, u32>,
    /// The prelude constructors the runtime builds itself, resolved once.
    pub some: Option<u32>,
    pub none: Option<u32>,
    pub stop: Option<u32>,
    pub go: Option<u32>,
    pub less: Option<u32>,
    pub equal: Option<u32>,
    pub greater: Option<u32>,
    /// Field offsets by shape and index, `width` to a row, for shapes interned before indexing.
    rows: Box<[u16]>,
    width: usize,
    /// The shape of a `{key, value}` entry.
    entry_shape: u32,
}

/// No field at that index.
const NO_FIELD: u16 = u16::MAX;

impl Layouts {
    pub fn new(ctors: Vec<(Symbol, usize)>) -> Layouts {
        let ctor_ids: HashMap<Symbol, u32> = ctors
            .iter()
            .enumerate()
            .map(|(i, (n, _))| (n.clone(), i as u32))
            .collect();
        let by = |name: &str| ctor_ids.get(&Symbol::new(name)).copied();
        let (some, none, stop, go) = (by("Some"), by("None"), by("Stop"), by("Continue"));
        let (less, equal, greater) = (by("Less"), by("Equal"), by("Greater"));
        let mut shapes = Shapes::default();
        let entry_shape = shapes.intern(vec![Symbol::new("key"), Symbol::new("value")]);
        Layouts {
            shapes: RefCell::new(shapes),
            ctors,
            ctor_ids,
            some,
            none,
            stop,
            go,
            less,
            equal,
            greater,
            rows: Box::default(),
            width: 0,
            entry_shape,
        }
    }

    pub fn ctor_index(&self, name: &Symbol) -> Option<u32> {
        self.ctor_ids.get(name).copied()
    }

    pub fn entry_shape(&self) -> u32 {
        self.entry_shape
    }

    pub fn index_fields(&mut self, names: &[Symbol]) {
        let rows: Box<[u16]> = self
            .shapes
            .get_mut()
            .names
            .iter()
            .flat_map(|shape| {
                names
                    .iter()
                    .map(move |name| shape.binary_search(name).map_or(NO_FIELD, |at| at as u16))
            })
            .collect();
        self.rows = rows;
        self.width = names.len();
    }

    /// The offset of `names[index]` in `shape`: a load for an indexed shape, else a search.
    #[inline]
    pub fn offset_by_index(&self, shape: u32, index: usize, names: &[Symbol]) -> Option<usize> {
        match self.rows.get(shape as usize * self.width + index) {
            Some(&at) => (at != NO_FIELD).then_some(at as usize),
            None => self.offset(shape, &names[index]),
        }
    }

    /// The id of the shape with exactly these fields, in any order.
    pub fn shape(&self, fields: Vec<Symbol>) -> u32 {
        self.shapes.borrow_mut().intern(fields)
    }

    /// Every shape's fields in id order, so a cached unit re-interns them to the ids its C bakes.
    pub fn all_shape_names(&self) -> Vec<Vec<Symbol>> {
        self.shapes
            .borrow()
            .names
            .iter()
            .map(|n| n.to_vec())
            .collect()
    }

    pub fn shape_names(&self, shape: u32) -> Rc<[Symbol]> {
        self.shapes.borrow().names[shape as usize].clone()
    }

    /// How many fields a shape has.
    pub fn shape_width(&self, shape: u32) -> usize {
        self.shapes.borrow().names[shape as usize].len()
    }

    pub fn offset(&self, shape: u32, name: &Symbol) -> Option<usize> {
        self.shapes.borrow().names[shape as usize]
            .binary_search(name)
            .ok()
    }
}

/// An entry's bump allocator over chunks recycled at its end; a persistent heap never recycles.
#[repr(C)]
pub struct Heap {
    /// First so compiled code can bump them at fixed offsets from the context.
    cur: *mut u8,
    end: *mut u8,
    chunks: Vec<(*mut u8, usize)>,
    /// One bit per word of each chunk, set at each object start, for [`Heap::is_object`].
    starts: Vec<Vec<u64>>,
    /// Which chunk `cur` is in.
    chunk: usize,
    /// Bridged values allocated since the last reset, to drop at the end.
    bridges: Vec<*mut Obj>,
    persistent: bool,
    /// Objects allocated since the last reset, and the same by kind.
    count: usize,
    by_kind: [usize; 16],
    /// Under `PLY_C_PHASES`, allocations by kind and layout or power-of-two length.
    by_layout: HashMap<(u8, u32), usize>,
    recycled: usize,
    /// Dead objects by size class in words, taken before the bump pointer moves.
    free: Vec<Vec<*mut Obj>>,
    /// Dead blocks past the small classes, by the power of two their size was rounded up to.
    large: Vec<Vec<*mut Obj>>,
    /// Dead blocks not yet on a free list, oldest first, under `delay()`.
    delayed: std::collections::VecDeque<(*mut Obj, usize)>,
    poison: bool,
    delay: usize,
    census: bool,
}

/// Small size classes, in words; larger blocks are reused by power of two.
const REUSE_CLASSES: usize = 64;

thread_local! {
    /// The running entry's heap, which dying objects return to; entries never nest on a thread.
    static CURRENT: std::cell::Cell<*mut Heap> = const { std::cell::Cell::new(std::ptr::null_mut()) };
}

/// The entry beginning on this thread allocates from `heap`; its dead objects go back to it.
pub fn enter(heap: *mut Heap) {
    CURRENT.with(|c| c.set(heap));
}

pub fn leave() {
    CURRENT.with(|c| c.set(std::ptr::null_mut()));
}

/// The payload bytes an object was allocated with, from its header; `usize::MAX` if unsized.
unsafe fn payload_bytes(o: *mut Obj) -> usize {
    unsafe {
        match (*o).kind {
            KIND_RECORD | KIND_CTOR | KIND_CLOSURE | KIND_INT => (*o).len as usize * 8,
            KIND_LEAF | KIND_BRANCH => (*o).layout as usize * 8,
            KIND_LIST => (list::TAIL + (*o).aux as usize) * 8,
            KIND_MAP => 8,
            KIND_MLEAF => (*o).layout as usize * 16,
            KIND_MBRANCH => 2 * map::KEYS * 8,
            KIND_STR | KIND_BYTES => (*o).layout as usize,
            _ => usize::MAX,
        }
    }
}

/// Returns a dead object to its free list; never a bridge, which the drop log would drop twice.
unsafe fn recycle(o: *mut Obj, heap: *mut Heap) {
    if heap.is_null() {
        return;
    }
    unsafe {
        if (*o).kind == KIND_BRIDGE {
            return;
        }
        let size = payload_bytes(o);
        if size == usize::MAX {
            return;
        }
        let object = Heap::object_size(size);
        // Reused even when poisoned: holding dead blocks back exhausts memory.
        if (*heap).poison {
            let words = (object - HEADER) / 8;
            for i in 0..words {
                set_word(o, i, poison::word());
            }
        }
        let delay = (*heap).delay;
        if delay == 0 {
            (*heap).free_list(object).push(o);
            return;
        }
        (*heap).delayed.push_back((o, object));
        if (*heap).delayed.len() > delay
            && let Some((o, object)) = (*heap).delayed.pop_front()
        {
            (*heap).free_list(object).push(o);
        }
    }
}

impl Default for Heap {
    fn default() -> Heap {
        Heap::new()
    }
}

const FIRST_CHUNK: usize = 1 << 20;
const LARGEST_CHUNK: usize = 64 << 20;

/// The byte offset of the bump pointer and of the chunk's end within a [`Heap`].
pub const HEAP_CUR: usize = 0;
pub const HEAP_END: usize = 8;

impl Heap {
    pub fn new() -> Heap {
        Heap {
            cur: std::ptr::null_mut(),
            end: std::ptr::null_mut(),
            chunks: Vec::new(),
            starts: Vec::new(),
            chunk: 0,
            bridges: Vec::new(),
            persistent: false,
            count: 0,
            by_kind: [0; 16],
            by_layout: HashMap::new(),
            recycled: 0,
            free: Vec::new(),
            large: Vec::new(),
            delayed: std::collections::VecDeque::new(),
            poison: poisoning(),
            delay: delay(),
            census: census_by_layout(),
        }
    }

    /// The free list for a dead block of `object` bytes: its size class or its power of two.
    fn free_list(&mut self, object: usize) -> &mut Vec<*mut Obj> {
        let (lists, class) = match Heap::large_class(object) {
            Some(class) => (&mut self.large, class),
            None => (&mut self.free, object / 8),
        };
        if lists.len() <= class {
            lists.resize_with(class + 1, Vec::new);
        }
        &mut lists[class]
    }

    /// A heap whose entries never end: what outlives every entry lives here.
    pub fn persistent() -> Heap {
        let mut h = Heap::new();
        h.persistent = true;
        h
    }

    /// Live objects by kind, with their count and payload bytes.
    pub fn live_by_kind(&self) -> Vec<(u8, usize, usize)> {
        let mut tally: std::collections::BTreeMap<u8, (usize, usize)> = Default::default();
        for (i, (base, cap)) in self.chunks.iter().enumerate() {
            let limit = if i == self.chunk {
                self.cur as usize
            } else {
                *base as usize + *cap
            };
            let bits = &self.starts[i];
            for (word, bitmap) in bits.iter().enumerate() {
                if *bitmap == 0 {
                    continue;
                }
                for b in 0..64 {
                    if bitmap & (1u64 << b) == 0 {
                        continue;
                    }
                    let address = *base as usize + (word * 64 + b) * 8;
                    if address >= limit {
                        break;
                    }
                    let o = address as *mut Obj;
                    let (kind, rc, size) = unsafe { ((*o).kind, (*o).rc, payload_bytes(o)) };
                    if kind == KIND_DEAD || rc == 0 {
                        continue;
                    }
                    let e = tally.entry(kind).or_insert((0, 0));
                    e.0 += 1;
                    // `usize::MAX` means unsized (a bridge, a singleton): count the header alone.
                    e.1 += if size == usize::MAX { HEADER } else { size };
                }
            }
        }
        tally.into_iter().map(|(k, (n, b))| (k, n, b)).collect()
    }

    /// The bytes the entry's chunks reserve, whatever is live in them.
    pub fn chunk_bytes(&self) -> usize {
        self.chunks.iter().map(|(_, cap)| *cap).sum()
    }

    /// Objects allocated since the last reset.
    pub fn allocated(&self) -> usize {
        self.count
    }

    /// Allocations by kind, indexed by the `KIND_*` constants.
    pub fn allocated_by_kind(&self) -> [usize; 16] {
        self.by_kind
    }

    /// Under `PLY_C_PHASES`, allocations by kind and layout or length class, most first.
    pub fn allocated_by_layout(&self) -> Vec<((u8, u32), usize)> {
        let mut out: Vec<((u8, u32), usize)> =
            self.by_layout.iter().map(|(k, n)| (*k, *n)).collect();
        out.sort_by_key(|(k, n)| (std::cmp::Reverse(*n), *k));
        out
    }

    /// Allocations served from a free list rather than fresh memory.
    pub fn recycled(&self) -> usize {
        self.recycled
    }

    /// Moves to a chunk with `need` bytes free: the next fitting one on hand, or a new, larger one.
    fn grow(&mut self, need: usize) {
        while self.chunk + 1 < self.chunks.len() {
            self.chunk += 1;
            let (p, cap) = self.chunks[self.chunk];
            if cap >= need {
                self.cur = p;
                self.end = unsafe { p.add(cap) };
                return;
            }
        }
        let last = self.chunks.last().map_or(0, |c| c.1);
        let cap = need.max(FIRST_CHUNK).max((last * 2).min(LARGEST_CHUNK));
        let p = unsafe { alloc(Layout::from_size_align(cap, 16).expect("a chunk layout")) };
        assert!(!p.is_null(), "the heap is out of memory");
        self.chunks.push((p, cap));
        self.starts.push(vec![0; cap / 512 + 1]);
        self.chunk = self.chunks.len() - 1;
        self.cur = p;
        self.end = unsafe { p.add(cap) };
    }

    /// The bytes an object with this payload takes, header included and rounded to a word.
    pub fn object_size(payload_bytes: usize) -> usize {
        (HEADER + payload_bytes.max(8) + 7) & !7
    }

    /// A block's size: the object size in the small classes, else the next power of two.
    fn block_size(payload_bytes: usize) -> usize {
        let size = Heap::object_size(payload_bytes);
        match Heap::large_class(size) {
            Some(class) => 1 << class,
            None => size,
        }
    }

    /// The power-of-two class of a block past the small classes.
    fn large_class(size: usize) -> Option<usize> {
        (size / 8 >= REUSE_CLASSES)
            .then(|| usize::BITS as usize - (size - 1).leading_zeros() as usize)
    }

    pub(crate) fn raw_alloc(
        &mut self,
        kind: u8,
        flags: u8,
        len: u32,
        layout: u32,
        payload_bytes: usize,
    ) -> *mut Obj {
        let size = Heap::block_size(payload_bytes);
        let recycled = match Heap::large_class(size) {
            Some(class) => self.large.get_mut(class).and_then(Vec::pop),
            None => self.free.get_mut(size / 8).and_then(Vec::pop),
        };
        let p = match recycled {
            // Its start bit is still set: only `end` clears bits, and it empties every free list.
            Some(p) => {
                self.recycled += 1;
                p
            }
            None => {
                if (self.end as usize).wrapping_sub(self.cur as usize) < size || self.cur.is_null()
                {
                    self.grow(size);
                }
                let p = self.cur as *mut Obj;
                let bit = (p as usize - self.chunks[self.chunk].0 as usize) / 8;
                self.starts[self.chunk][bit / 64] |= 1 << (bit % 64);
                self.cur = unsafe { self.cur.add(size) };
                p
            }
        };
        unsafe {
            p.write(Obj {
                rc: 1,
                kind,
                flags,
                aux: 0,
                len,
                layout,
            });
        }
        self.count += 1;
        self.by_kind[kind as usize & 15] += 1;
        if self.census {
            let key = match kind {
                KIND_CTOR | KIND_RECORD => Some(layout),
                // Bytes arrive with `len` zero; the class is the capacity.
                KIND_BYTES | KIND_STR => Some((payload_bytes as u32).next_power_of_two()),
                _ => None,
            };
            if let Some(key) = key {
                *self.by_layout.entry((kind, key)).or_insert(0) += 1;
            }
        }
        p
    }

    /// Whether `w` is a live object this heap allocated in the running entry.
    pub fn is_object(&self, w: Word) -> bool {
        if is_imm(w) || w == 0 || !(w as usize).is_multiple_of(8) {
            return false;
        }
        let address = w as usize;
        for (i, (base, cap)) in self.chunks.iter().enumerate() {
            let base = *base as usize;
            if address >= base && address < base + *cap {
                let bit = (address - base) / 8;
                if self.starts[i][bit / 64] & (1 << (bit % 64)) == 0 {
                    return false;
                }
                let o = address as *const Obj;
                return unsafe { (*o).kind != KIND_DEAD && (*o).rc != 0 };
            }
        }
        false
    }

    /// A fresh object with `len` payload words, left as a recycled block's last tenant wrote them:
    /// fill every word before anything can release it, or [`dec`] follows garbage.
    pub fn alloc(&mut self, kind: u8, flags: u8, len: u32, layout: u32) -> *mut Obj {
        self.raw_alloc(kind, flags, len, layout, len as usize * 8)
    }

    /// [`dec`] of a mortal word held once, into this heap without the thread-local lookup.
    pub fn release_last(&mut self, w: Word) {
        debug_assert!(!is_imm(w) && w != 0);
        let o = obj(w);
        unsafe {
            debug_assert!((*o).rc == 1 && (*o).kind != KIND_DEAD);
            release(o, self);
        }
    }

    /// A fresh string or bytes value with room for `cap` bytes and none in use yet.
    pub fn alloc_bytes(&mut self, kind: u8, cap: u32) -> *mut Obj {
        self.raw_alloc(kind, 0, 0, cap, cap as usize)
    }

    /// A string or bytes value of `kind` holding `a` then `b`, with at least `room` capacity.
    fn joined(&mut self, kind: u8, a: &[u8], b: &[u8], room: usize) -> *mut Obj {
        let len = a.len() + b.len();
        if kind == KIND_BYTES
            && len <= 1
            && let Some(w) = small_bytes(if a.is_empty() { b } else { a })
        {
            return obj(w);
        }
        let o = self.alloc_bytes(kind, room.max(len) as u32);
        unsafe {
            std::ptr::copy_nonoverlapping(a.as_ptr(), bytes_ptr(o), a.len());
            std::ptr::copy_nonoverlapping(b.as_ptr(), bytes_ptr(o).add(a.len()), b.len());
            (*o).len = len as u32;
        }
        o
    }

    pub fn str(&mut self, s: &str) -> Word {
        self.joined(KIND_STR, s.as_bytes(), &[], 0) as Word
    }

    pub fn bytes(&mut self, b: &[u8]) -> Word {
        self.joined(KIND_BYTES, b, &[], 0) as Word
    }

    /// Takes `a` and appends `b`: in place when `a` is unique and has room, else a doubled copy.
    pub fn append(&mut self, a: Word, b: &[u8]) -> Word {
        let o = obj(a);
        let (len, cap) = unsafe { ((*o).len as usize, (*o).layout as usize) };
        if is_unique(a) && len + b.len() <= cap {
            unsafe {
                std::ptr::copy_nonoverlapping(b.as_ptr(), bytes_ptr(o).add(len), b.len());
                (*o).len = (len + b.len()) as u32;
            }
            return a;
        }
        let kind = unsafe { (*o).kind };
        let out = self.joined(kind, unsafe { bytes_of(o) }, b, (len + b.len()) * 2);
        dec(a);
        out as Word
    }

    pub fn boxed_int(&mut self, v: i64) -> Word {
        if fits_imm(v) {
            return imm(v);
        }
        let o = self.alloc(KIND_INT, 0, 1, 0);
        unsafe { set_word(o, 0, v) };
        o as Word
    }

    pub fn bridge(&mut self, v: Value) -> Word {
        let o = self.raw_alloc(KIND_BRIDGE, 0, 0, 0, std::mem::size_of::<Value>());
        unsafe { bridge_slot(o).write(v) };
        self.bridges.push(o);
        o as Word
    }

    /// A constant-pool word: immortal, as is everything under it.
    pub fn immortal(&mut self, layouts: &Layouts, v: &Value) -> Word {
        debug_assert!(
            self.persistent,
            "an immortal word needs a heap that never resets"
        );
        let w = self.to_word(layouts, v);
        mark_immortal(w);
        w
    }

    /// An immortal deep copy of `w` into this persistent heap, sharing what is already immortal.
    pub fn adopt(&mut self, w: Word) -> Word {
        debug_assert!(
            self.persistent,
            "an adopted word needs a heap that never resets"
        );
        let mut copies: HashMap<usize, Word> = HashMap::new();
        let out = self.copy(w, &mut copies);
        mark_immortal(out);
        out
    }

    fn copy(&mut self, w: Word, copies: &mut HashMap<usize, Word>) -> Word {
        if is_imm(w) {
            return w;
        }
        let o = obj(w);
        unsafe {
            if (*o).rc == IMMORTAL {
                return w;
            }
            if let Some(c) = copies.get(&(w as usize)) {
                return *c;
            }
            let out = match (*o).kind {
                KIND_UNIT | KIND_BOOL => return w,
                KIND_INT => self.boxed_int(word_at(o, 0)),
                KIND_STR | KIND_BYTES => self.joined((*o).kind, bytes_of(o), &[], 0) as Word,
                KIND_BRIDGE => self.bridge(bridged(o).clone()),
                KIND_LIST => {
                    let items: Vec<Word> = list::to_vec(o)
                        .into_iter()
                        .map(|x| self.copy(x, copies))
                        .collect();
                    self.list_from(&items)
                }
                KIND_MAP => {
                    let entries: Vec<(Word, Word)> = map::to_vec(o)
                        .into_iter()
                        .map(|(k, v)| (self.copy(k, copies), self.copy(v, copies)))
                        .collect();
                    self.map_from_sorted(&entries)
                }
                kind => {
                    let (len, layout, flags) = ((*o).len, (*o).layout, (*o).flags);
                    let c = self.alloc(kind, flags, len, layout);
                    let first = if kind == KIND_CLOSURE {
                        set_word(c, CLOSURE_CODE, word_at(o, CLOSURE_CODE));
                        CLOSURE_CAPTURES
                    } else {
                        0
                    };
                    for i in first..len as usize {
                        let x = self.copy(word_at(o, i), copies);
                        set_word(c, i, x);
                    }
                    c as Word
                }
            };
            copies.insert(w as usize, out);
            out
        }
    }

    /// Resets the entry's memory, keeping its chunks; the answer is already copied out.
    pub fn end(&mut self) {
        if self.persistent {
            return;
        }
        for o in self.bridges.drain(..) {
            unsafe {
                if (*o).kind == KIND_BRIDGE {
                    std::ptr::drop_in_place(bridge_slot(o));
                }
            }
        }
        if let Some((p, cap)) = self.chunks.first() {
            self.cur = *p;
            self.end = unsafe { p.add(*cap) };
        }
        self.chunk = 0;
        self.count = 0;
        self.by_kind = [0; 16];
        self.by_layout.clear();
        self.recycled = 0;
        for class in &mut self.free {
            class.clear();
        }
        for class in &mut self.large {
            class.clear();
        }
        self.delayed.clear();
        for bits in &mut self.starts {
            bits.fill(0);
        }
    }

    /// The compiled word for an interpreter value: deep, and every object fresh in the entry.
    pub fn to_word(&mut self, layouts: &Layouts, v: &Value) -> Word {
        match v {
            Value::Int(n) => self.boxed_int(*n),
            Value::Bool(b) => bool(*b),
            Value::Unit => unit(),
            Value::Str(s) if u32::try_from(s.len()).is_ok() => self.str(s),
            Value::Bytes(b) if u32::try_from(b.len()).is_ok() => self.bytes(b),
            Value::Record(fields) => {
                let shape = layouts.shape(fields.keys().cloned().collect::<Vec<Symbol>>());
                let n = fields.len() as u32;
                let o = self.alloc(KIND_RECORD, 0, n, shape);
                // `Fields` iterates sorted, which is the layout's order.
                for (i, (_, value)) in fields.iter().enumerate() {
                    let w = self.to_word(layouts, value);
                    unsafe { set_word(o, i, w) };
                }
                o as Word
            }
            Value::Ctor { name, args } => match layouts.ctor_index(name) {
                Some(index) => {
                    let o = self.alloc(KIND_CTOR, 0, args.len() as u32, index);
                    for (i, a) in args.iter().enumerate() {
                        let w = self.to_word(layouts, a);
                        unsafe { set_word(o, i, w) };
                    }
                    o as Word
                }
                None => self.bridge(v.clone()),
            },
            Value::List(items) => {
                let words: Vec<Word> = items.iter().map(|x| self.to_word(layouts, x)).collect();
                self.list_from(&words)
            }
            // The interpreter iterates in key order, which is the tree's order too.
            Value::Map(entries) => {
                let words: Vec<(Word, Word)> = entries
                    .iter()
                    .map(|(k, v)| (self.to_word(layouts, k), self.to_word(layouts, v)))
                    .collect();
                self.map_from_sorted(&words)
            }
            Value::Closure(c) => match &c.kind {
                ClosureKind::Native {
                    code,
                    arity,
                    captured,
                } => {
                    let o = self.alloc(
                        KIND_CLOSURE,
                        0,
                        (captured.len() + CLOSURE_CAPTURES) as u32,
                        *arity as u32,
                    );
                    unsafe { set_word(o, CLOSURE_CODE, *code as Word) };
                    for (i, cap) in captured.iter().enumerate() {
                        let w = self.to_word(layouts, cap);
                        unsafe { set_word(o, CLOSURE_CAPTURES + i, w) };
                    }
                    o as Word
                }
                _ => self.bridge(v.clone()),
            },
            _ => self.bridge(v.clone()),
        }
    }

    /// The interpreter value a word denotes: deep, and a borrow — the word keeps its count.
    pub fn to_value(layouts: &Layouts, w: Word) -> Value {
        Heap::to_value_counted(layouts, w, &mut Walked::default())
    }

    /// [`Heap::to_value`], also counting objects read and noting any handle among them.
    pub fn to_value_counted(layouts: &Layouts, w: Word, walked: &mut Walked) -> Value {
        if is_imm(w) {
            return Value::Int(imm_value(w));
        }
        walked.read += 1;
        let o = obj(w);
        unsafe {
            match (*o).kind {
                KIND_UNIT => Value::Unit,
                KIND_BOOL => Value::Bool((*o).flags != 0),
                KIND_INT => Value::Int(word_at(o, 0)),
                KIND_STR => Value::Str(Arc::from(str_of(o))),
                KIND_BYTES => Value::Bytes(Arc::from(bytes_of(o))),
                KIND_RECORD => {
                    let names = layouts.shape_names((*o).layout);
                    let fields: Vec<(Symbol, Value)> = names
                        .iter()
                        .enumerate()
                        .map(|(i, name)| {
                            (
                                name.clone(),
                                Heap::to_value_counted(layouts, word_at(o, i), walked),
                            )
                        })
                        .collect();
                    Value::Record(Arc::new(Fields::from_unsorted(fields)))
                }
                KIND_CTOR => {
                    let name = layouts.ctors[(*o).layout as usize].0.clone();
                    let args = (0..(*o).len as usize)
                        .map(|i| Heap::to_value_counted(layouts, word_at(o, i), walked))
                        .collect();
                    Value::ctor(name, args)
                }
                KIND_LIST => Value::list(
                    list::to_vec(o)
                        .into_iter()
                        .map(|x| Heap::to_value_counted(layouts, x, walked))
                        .collect(),
                ),
                KIND_MAP => Value::map(map::to_vec(o).into_iter().map(|(k, v)| {
                    (
                        Heap::to_value_counted(layouts, k, walked),
                        Heap::to_value_counted(layouts, v, walked),
                    )
                })),
                KIND_CLOSURE => {
                    walked.handle = true;
                    let captured: Vec<Value> = (CLOSURE_CAPTURES..(*o).len as usize)
                        .map(|i| Heap::to_value_counted(layouts, word_at(o, i), walked))
                        .collect();
                    Value::Closure(Arc::new(Closure {
                        name: None,
                        kind: ClosureKind::Native {
                            code: word_at(o, CLOSURE_CODE) as usize,
                            arity: (*o).layout as usize,
                            captured,
                        },
                    }))
                }
                KIND_BRIDGE => {
                    let v = bridged(o).clone();
                    if crate::rt::holds_a_handle(&v).is_some() {
                        walked.handle = true;
                    }
                    v
                }
                other => panic!("a word of kind {other} was read after its object died"),
            }
        }
    }
}

/// What a conversion out of the heap read: how many objects, and whether one was a handle.
#[derive(Default)]
pub struct Walked {
    pub read: u64,
    pub handle: bool,
}

impl Drop for Heap {
    fn drop(&mut self) {
        for o in self.bridges.drain(..) {
            unsafe {
                if (*o).kind == KIND_BRIDGE {
                    std::ptr::drop_in_place(bridge_slot(o));
                }
            }
        }
        for (p, cap) in self.chunks.drain(..) {
            unsafe { dealloc(p, Layout::from_size_align(cap, 16).expect("a chunk layout")) };
        }
    }
}

/// One more holder of `w`. Zero is no object, as `ply_inc` and `ply_dec` read it.
#[inline]
pub fn inc(w: Word) {
    if is_imm(w) || w == 0 {
        return;
    }
    let o = obj(w);
    unsafe {
        if (*o).rc != IMMORTAL {
            debug_assert!((*o).kind != KIND_DEAD, "a dead object was shared");
            (*o).rc += 1;
        }
    }
}

/// One holder fewer; the last one dismantles the object, children and all.
#[inline]
pub fn dec(w: Word) {
    if is_imm(w) || w == 0 {
        return;
    }
    let o = obj(w);
    unsafe {
        if (*o).rc == IMMORTAL {
            return;
        }
        debug_assert!((*o).kind != KIND_DEAD, "a dead object was released again");
        if (*o).rc > 1 {
            (*o).rc -= 1;
            return;
        }
    }
    let heap = CURRENT.with(|c| c.get());
    unsafe { release(o, heap) }
}

/// `o`, held once, released with its children, into `heap`'s free lists when it has them.
unsafe fn release(o: *mut Obj, heap: *mut Heap) {
    let mut deferred = Vec::new();
    unsafe {
        dismantle(o, 0, &mut deferred, heap);
        while let Some(o) = deferred.pop() {
            dismantle(o, 0, &mut deferred, heap);
        }
    }
}

/// Perceus's `reset`: a unique record drops its fields and keeps its memory with `len` zeroed;
/// answers `w`, or `0` after releasing anything else.
pub fn reset(w: Word) -> Word {
    if is_imm(w) || w == 0 {
        return 0;
    }
    let o = obj(w);
    unsafe {
        if (*o).kind != KIND_RECORD || (*o).rc != 1 {
            dec(w);
            return 0;
        }
        if (*o).flags & FLAT == 0 {
            for i in 0..(*o).len as usize {
                let c = word_at(o, i);
                if !is_imm(c) && c != 0 {
                    dec(c);
                }
            }
        }
        (*o).len = 0;
    }
    w
}

/// Byte order, by hand for short keys and by `memcmp` past sixteen bytes.
#[inline]
fn cmp_bytes(x: &[u8], y: &[u8]) -> Ordering {
    let n = x.len().min(y.len());
    if n > 16 {
        return x.cmp(y);
    }
    for i in 0..n {
        if x[i] != y[i] {
            return x[i].cmp(&y[i]);
        }
    }
    x.len().cmp(&y.len())
}

/// How deep dismantling recurses on the stack before deferring to a heap list.
const DISMANTLE_DEPTH: usize = 32;

/// `o`, held once, dies: its children are released and its header marked dead.
unsafe fn dismantle(o: *mut Obj, depth: usize, deferred: &mut Vec<*mut Obj>, heap: *mut Heap) {
    unsafe {
        debug_assert!((*o).rc == 1 && (*o).kind != KIND_DEAD);
        (*o).rc = 0;
        let ranges: [(usize, usize); 2] = match (*o).kind {
            KIND_RECORD | KIND_CTOR if (*o).flags & FLAT != 0 => [(0, 0), (0, 0)],
            KIND_RECORD | KIND_CTOR | KIND_LEAF | KIND_BRANCH => [(0, (*o).len as usize), (0, 0)],
            KIND_MLEAF => [(0, 2 * (*o).len as usize), (0, 0)],
            // The root when there is one, then the tail.
            KIND_LIST => [(0, list::TAIL + list::tail_len(o)), (0, 0)],
            // The root when there is one.
            KIND_MAP => [(0, 1), (0, 0)],
            // The children, then their greatest keys.
            KIND_MBRANCH => [
                (0, (*o).len as usize),
                (map::KEYS, map::KEYS + (*o).len as usize),
            ],
            KIND_CLOSURE => [(CLOSURE_CAPTURES, (*o).len as usize), (0, 0)],
            KIND_BRIDGE => {
                std::ptr::drop_in_place(bridge_slot(o));
                [(0, 0), (0, 0)]
            }
            _ => [(0, 0), (0, 0)],
        };
        for i in ranges.iter().flat_map(|(first, last)| *first..*last) {
            let c = word_at(o, i);
            if is_imm(c) || c == 0 {
                continue;
            }
            let co = obj(c);
            if (*co).rc == IMMORTAL {
                continue;
            }
            debug_assert!((*co).kind != KIND_DEAD, "a dead object was released again");
            if (*co).rc > 1 {
                (*co).rc -= 1;
            } else if depth < DISMANTLE_DEPTH {
                dismantle(co, depth + 1, deferred, heap);
            } else {
                deferred.push(co);
            }
        }
        // The class is read off the header before it is marked dead.
        recycle(o, heap);
        (*o).kind = KIND_DEAD;
    }
}

/// Marks everything under `w` immortal in place, so no count is touched through it again.
pub fn mark_immortal(w: Word) {
    let mut pending = vec![w];
    while let Some(w) = pending.pop() {
        if is_imm(w) {
            continue;
        }
        let o = obj(w);
        unsafe {
            if (*o).rc == IMMORTAL {
                continue;
            }
            (*o).rc = IMMORTAL;
            match (*o).kind {
                KIND_RECORD | KIND_CTOR | KIND_LEAF | KIND_BRANCH => {
                    for i in 0..(*o).len as usize {
                        pending.push(word_at(o, i));
                    }
                }
                KIND_LIST => pending.extend(list::children(o)),
                KIND_MAP => {
                    let r = word_at(o, 0);
                    if r != 0 {
                        pending.push(r);
                    }
                }
                KIND_MLEAF | KIND_MBRANCH => {
                    pending.extend(map::child_words(o).map(|i| word_at(o, i)));
                }
                KIND_CLOSURE => {
                    for i in CLOSURE_CAPTURES..(*o).len as usize {
                        pending.push(word_at(o, i));
                    }
                }
                _ => {}
            }
        }
    }
}

/// `ply_eval::memo::world_independent` over a word: whether a memo may keep it.
pub fn world_independent(w: Word) -> bool {
    let mut pending = vec![w];
    while let Some(w) = pending.pop() {
        if is_imm(w) {
            continue;
        }
        let o = obj(w);
        unsafe {
            match (*o).kind {
                KIND_RECORD | KIND_CTOR | KIND_LEAF | KIND_BRANCH => {
                    for i in 0..(*o).len as usize {
                        pending.push(word_at(o, i));
                    }
                }
                KIND_LIST => pending.extend(list::children(o)),
                KIND_MAP => {
                    let r = word_at(o, 0);
                    if r != 0 {
                        pending.push(r);
                    }
                }
                KIND_MLEAF | KIND_MBRANCH => {
                    pending.extend(map::child_words(o).map(|i| word_at(o, i)));
                }
                KIND_CLOSURE => {
                    for i in CLOSURE_CAPTURES..(*o).len as usize {
                        pending.push(word_at(o, i));
                    }
                }
                KIND_BRIDGE => {
                    // A handle must not outlive its entry, and the memo would keep it.
                    if crate::rt::holds_a_handle(bridged(o)).is_some()
                        || !ply_eval::memo::world_independent(bridged(o))
                    {
                        return false;
                    }
                }
                _ => {}
            }
        }
    }
    true
}

/// Whether `w` reaches cell `slot`, within the interpreter's walk budget.
pub fn reaches_cell(w: Word, slot: ply_eval::arena::Slot) -> bool {
    let mut budget = 256usize;
    let mut pending = vec![w];
    while let Some(w) = pending.pop() {
        if is_imm(w) {
            continue;
        }
        if budget == 0 {
            return false;
        }
        budget -= 1;
        let o = obj(w);
        unsafe {
            match (*o).kind {
                KIND_RECORD | KIND_CTOR | KIND_LEAF | KIND_BRANCH => {
                    for i in 0..(*o).len as usize {
                        pending.push(word_at(o, i));
                    }
                }
                KIND_LIST => pending.extend(list::children(o)),
                KIND_MAP => {
                    let r = word_at(o, 0);
                    if r != 0 {
                        pending.push(r);
                    }
                }
                KIND_MLEAF | KIND_MBRANCH => {
                    pending.extend(map::child_words(o).map(|i| word_at(o, i)));
                }
                KIND_CLOSURE => {
                    for i in CLOSURE_CAPTURES..(*o).len as usize {
                        pending.push(word_at(o, i));
                    }
                }
                KIND_BRIDGE => match bridged(o) {
                    Value::Cell(s) => {
                        if *s == slot {
                            return true;
                        }
                    }
                    v => {
                        if ply_eval::rc::value_reaches_cell(v, slot) {
                            return true;
                        }
                    }
                },
                _ => {}
            }
        }
    }
    false
}

/// Whether one holder alone has `w`: what lets an update write in place.
#[inline]
pub fn is_unique(w: Word) -> bool {
    !is_imm(w) && unsafe { (*obj(w)).rc == 1 }
}

#[inline]
pub fn kind(w: Word) -> u8 {
    if is_imm(w) {
        KIND_INT
    } else if w == 0 {
        // Zero is "no word here"; reading its header would fault at address 4.
        KIND_DEAD
    } else {
        unsafe { (*obj(w)).kind }
    }
}

/// The `Int` a word carries, if it is one.
#[inline]
pub fn as_int(w: Word) -> Option<i64> {
    if is_imm(w) {
        return Some(imm_value(w));
    }
    if w == 0 {
        return None;
    }
    let o = obj(w);
    unsafe {
        if (*o).kind == KIND_INT {
            Some(word_at(o, 0))
        } else {
            None
        }
    }
}

#[inline]
pub fn as_bool(w: Word) -> Option<bool> {
    if is_imm(w) || w == 0 {
        return None;
    }
    let o = obj(w);
    unsafe {
        if (*o).kind == KIND_BOOL {
            Some((*o).flags != 0)
        } else {
            None
        }
    }
}

/// The interpreter's rank of a value's variant, which orders values of different kinds.
fn rank(w: Word) -> u8 {
    if is_imm(w) {
        return 2;
    }
    let o = obj(w);
    unsafe {
        match (*o).kind {
            KIND_UNIT => 0,
            KIND_BOOL => 1,
            KIND_INT => 2,
            KIND_STR => 5,
            KIND_BYTES => 6,
            KIND_LIST => 7,
            KIND_MAP => 8,
            KIND_RECORD => 9,
            KIND_CTOR => 10,
            KIND_CLOSURE => 11,
            KIND_BRIDGE => match bridged(o) {
                Value::Unit => 0,
                Value::Bool(_) => 1,
                Value::Int(_) => 2,
                Value::Float(_) => 3,
                Value::Decimal(_) => 4,
                Value::Str(_) => 5,
                Value::Bytes(_) => 6,
                Value::List(_) => 7,
                Value::Map(_) => 8,
                Value::Record(_) => 9,
                Value::Ctor { .. } => 10,
                Value::Closure(_) => 11,
                Value::Cell(_) => 12,
                Value::Task(_) => 13,
                Value::Continuation(_) => 14,
                Value::Secret(_) => 15,
                Value::Fixed(_) => 16,
            },
            other => panic!("a word of kind {other} was ordered after its object died"),
        }
    }
}

/// `Value::cmp` over words, so a map crosses the seam in the interpreter's key order.
pub fn cmp_words(layouts: &Layouts, a: Word, b: Word) -> Ordering {
    if a == b {
        return Ordering::Equal;
    }
    // Fast path for a map's usual keys: two strings or two byte strings.
    if !is_imm(a) && !is_imm(b) && a != 0 && b != 0 {
        let (oa, ob) = (obj(a), obj(b));
        let (ka, kb) = unsafe { ((*oa).kind, (*ob).kind) };
        if ka == kb && (ka == KIND_STR || ka == KIND_BYTES) {
            return unsafe { cmp_bytes(bytes_of(oa), bytes_of(ob)) };
        }
    }
    let (ra, rb) = (rank(a), rank(b));
    if ra != rb {
        return ra.cmp(&rb);
    }
    if let (Some(x), Some(y)) = (as_int(a), as_int(b)) {
        return x.cmp(&y);
    }
    let (ka, kb) = (kind(a), kind(b));
    if ka != kb {
        return Heap::to_value(layouts, a).cmp(&Heap::to_value(layouts, b));
    }
    unsafe {
        match ka {
            KIND_UNIT => Ordering::Equal,
            KIND_BOOL => (*obj(a)).flags.cmp(&(*obj(b)).flags),
            // Byte order, which is the order `str` and `[u8]` have.
            KIND_STR | KIND_BYTES => bytes_of(obj(a)).cmp(bytes_of(obj(b))),
            KIND_BRIDGE => bridged(obj(a)).cmp(bridged(obj(b))),
            KIND_LIST => {
                let (xs, ys) = (list::to_vec(obj(a)), list::to_vec(obj(b)));
                for (x, y) in xs.iter().zip(&ys) {
                    let c = cmp_words(layouts, *x, *y);
                    if c != Ordering::Equal {
                        return c;
                    }
                }
                xs.len().cmp(&ys.len())
            }
            KIND_MAP => {
                let (xs, ys) = (map::to_vec(obj(a)), map::to_vec(obj(b)));
                for ((xk, xv), (yk, yv)) in xs.iter().zip(&ys) {
                    let c = cmp_words(layouts, *xk, *yk).then_with(|| cmp_words(layouts, *xv, *yv));
                    if c != Ordering::Equal {
                        return c;
                    }
                }
                xs.len().cmp(&ys.len())
            }
            KIND_RECORD => {
                let (x, y) = (obj(a), obj(b));
                let (names_a, names_b) = (
                    layouts.shape_names((*x).layout),
                    layouts.shape_names((*y).layout),
                );
                let (n, m) = (names_a.len(), names_b.len());
                for i in 0..n.min(m) {
                    let c = names_a[i]
                        .cmp(&names_b[i])
                        .then_with(|| cmp_words(layouts, word_at(x, i), word_at(y, i)));
                    if c != Ordering::Equal {
                        return c;
                    }
                }
                n.cmp(&m)
            }
            KIND_CTOR => {
                let (x, y) = (obj(a), obj(b));
                let by_name = layouts.ctors[(*x).layout as usize]
                    .0
                    .cmp(&layouts.ctors[(*y).layout as usize].0);
                if by_name != Ordering::Equal {
                    return by_name;
                }
                let (n, m) = ((*x).len as usize, (*y).len as usize);
                for i in 0..n.min(m) {
                    let c = cmp_words(layouts, word_at(x, i), word_at(y, i));
                    if c != Ordering::Equal {
                        return c;
                    }
                }
                n.cmp(&m)
            }
            KIND_CLOSURE => Ordering::Equal,
            _ => Heap::to_value(layouts, a).cmp(&Heap::to_value(layouts, b)),
        }
    }
}

/// Whether a word can be a native map's key: ordered in place and canonical.
pub fn native_key(w: Word) -> bool {
    if is_imm(w) {
        return true;
    }
    let o = obj(w);
    unsafe {
        match (*o).kind {
            KIND_UNIT | KIND_BOOL | KIND_INT | KIND_STR | KIND_BYTES => true,
            KIND_BRIDGE => matches!(bridged(o), Value::Str(_) | Value::Bytes(_)),
            KIND_RECORD | KIND_CTOR | KIND_LEAF | KIND_BRANCH => {
                (0..(*o).len as usize).all(|i| native_key(word_at(o, i)))
            }
            KIND_LIST => list::children(o).all(native_key),
            KIND_MAP => {
                let mut fine = true;
                map::for_each(o, |k, v| fine = fine && native_key(k) && native_key(v));
                fine
            }
            _ => false,
        }
    }
}
