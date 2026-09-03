//! The value model compiled code runs on (ADR 0035): one machine word per value, with an
//! immediate for an `Int` that fits and a pointer to a counted object for everything else.
//!
//! A word with its low bit set is an `Int` in its upper sixty-three bits; a word with it clear is
//! the address of an [`Obj`]. `Unit`, `true` and `false` are three immortal objects, so a `Bool`
//! is a pointer compare and never an allocation. A record, a constructor, a list, a map and a
//! native closure are laid out as words after a sixteen-byte header, and a string or a bytes
//! value as its bytes after the same header, with room to grow so that appending to one nobody
//! else holds is a copy of the appended piece alone; anything the model does not lay out
//! natively yet — floats, decimals, secrets, the interpreter's own closures — is carried whole as
//! a [`Value`] behind a `Bridge` object.
//!
//! Every object an entry allocates is logged, and [`Heap::end`] releases the log: a count that
//! reaches zero dismantles its object then and there — its children let go, a bridged value
//! dropped — and in a release build its memory goes back to the entry's free list for its size
//! class, so an entry's memory is bounded by what it holds; in a debug build the memory waits
//! for the end of the entry instead, so a stale reference reads a `DEAD` header rather than
//! someone else's object, which is the net the suites run under.

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
/// A sorted array of key and value words, `len` entries of two words with room for `layout`.
pub const KIND_MAP: u8 = 8;
/// `len` bytes of UTF-8 with room for `layout`.
pub const KIND_STR: u8 = 9;
/// `len` bytes with room for `layout`.
pub const KIND_BYTES: u8 = 10;
/// A list's trie nodes (`list.rs`): `len` elements or children of `layout` slots.
pub const KIND_LEAF: u8 = 11;
pub const KIND_BRANCH: u8 = 12;
/// A map's tree nodes (`map.rs`): a leaf of `len` sorted pairs, a branch of `len` children
/// with their greatest keys beside them.
pub const KIND_MLEAF: u8 = 13;
pub const KIND_MBRANCH: u8 = 14;
pub const KIND_DEAD: u8 = 255;

/// A count no increment or decrement touches: the singletons, the constant pool, the memo.
pub const IMMORTAL: u32 = u32::MAX;

pub const HEADER: usize = 16;

/// A record or constructor none of whose fields holds a count — each an immediate or an
/// immortal — so releasing it walks nothing. Set where one is built from such fields, kept where
/// an update in place writes only such fields, and never set otherwise.
pub const FLAT: u8 = 1;

/// The header every object starts with. `len` is the payload's word count for a record, a
/// constructor, a trie node or a closure, its byte count for a string or a bytes value, and a
/// list's length; `layout` is a record's shape, a constructor's index, a node's or a string's
/// capacity, a closure's arity or the prefix a list dropped; `flags` and `aux` are a `Bool`'s
/// value and a list's tail length and tail capacity (`list.rs`).
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

pub fn unit() -> Word {
    &raw const UNIT_OBJ as Word
}

pub fn bool(b: bool) -> Word {
    if b {
        &raw const TRUE_OBJ as Word
    } else {
        &raw const FALSE_OBJ as Word
    }
}

pub fn is_imm(w: Word) -> bool {
    w & 1 == 1
}

/// Whether `v` fits the sixty-three bits an immediate carries.
pub fn fits_imm(v: i64) -> bool {
    (v << 1) >> 1 == v
}

pub fn imm(v: i64) -> Word {
    debug_assert!(fits_imm(v));
    (v << 1) | 1
}

pub fn imm_value(w: Word) -> i64 {
    w >> 1
}

pub fn obj(w: Word) -> *mut Obj {
    debug_assert!(!is_imm(w) && w != 0);
    w as *mut Obj
}

/// The payload words after a header.
pub unsafe fn words(o: *mut Obj) -> *mut Word {
    unsafe { (o as *mut u8).add(HEADER) as *mut Word }
}

pub unsafe fn word_at(o: *mut Obj, i: usize) -> Word {
    unsafe { *words(o).add(i) }
}

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
pub unsafe fn bytes_ptr(o: *mut Obj) -> *mut u8 {
    unsafe { (o as *mut u8).add(HEADER) }
}

/// The bytes a string or a bytes value holds, borrowed.
pub unsafe fn bytes_of<'a>(o: *mut Obj) -> &'a [u8] {
    unsafe { std::slice::from_raw_parts(bytes_ptr(o), (*o).len as usize) }
}

/// A string's payload as the text it is: a native string never holds anything but UTF-8.
pub unsafe fn str_of<'a>(o: *mut Obj) -> &'a str {
    unsafe { std::str::from_utf8_unchecked(bytes_of(o)) }
}

/// The shapes records are laid out by: a shape is its sorted field names, and a field's offset
/// is its position in them.
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

/// What a word is read against: the shapes, shared between the compiler that interns a
/// literal's and the entry that interns an arriving record's, and the constructors by index.
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
    /// Per shape, the offset of each field name the compiled unit reads by index, filled on the
    /// shape's first such read so a lookup is a load rather than a search over symbols.
    offsets: RefCell<Vec<Option<Box<[u16]>>>>,
    /// The shape of a `{key, value}` entry, interned once.
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
            offsets: RefCell::new(Vec::new()),
            entry_shape,
        }
    }

    pub fn ctor_index(&self, name: &Symbol) -> Option<u32> {
        self.ctor_ids.get(name).copied()
    }

    pub fn entry_shape(&self) -> u32 {
        self.entry_shape
    }

    /// The offset of the field named at `index` in `names`, in `shape`: cached per shape over
    /// every name the unit reads, so that after the first read it is one load.
    pub fn offset_by_index(&self, shape: u32, index: usize, names: &[Symbol]) -> Option<usize> {
        {
            let offsets = self.offsets.borrow();
            if let Some(Some(row)) = offsets.get(shape as usize)
                && let Some(at) = row.get(index)
            {
                return (*at != NO_FIELD).then_some(*at as usize);
            }
        }
        let row: Box<[u16]> = names
            .iter()
            .map(|name| self.offset(shape, name).map_or(NO_FIELD, |at| at as u16))
            .collect();
        let at = row.get(index).copied();
        let mut offsets = self.offsets.borrow_mut();
        if offsets.len() <= shape as usize {
            offsets.resize(shape as usize + 1, None);
        }
        offsets[shape as usize] = Some(row);
        at.and_then(|at| (at != NO_FIELD).then_some(at as usize))
    }

    /// The id of the shape with exactly these fields, in any order.
    pub fn shape(&self, fields: Vec<Symbol>) -> u32 {
        self.shapes.borrow_mut().intern(fields)
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

/// What an entry allocates from: a bump pointer over chunks that are recycled at the entry's
/// end, since the entry's answer is copied out before then and nothing outside it can hold a
/// word. A persistent heap — the constant pool, the memo — never recycles.
#[repr(C)]
pub struct Heap {
    /// The next free byte and the end of the current chunk, first so that compiled code can
    /// bump them at fixed offsets from the context.
    cur: *mut u8,
    end: *mut u8,
    chunks: Vec<(*mut u8, usize)>,
    /// Which chunk `cur` is in.
    chunk: usize,
    /// Bridged values allocated since the last reset, whose interpreter value must be dropped.
    bridges: Vec<*mut Obj>,
    persistent: bool,
    /// Objects allocated since the last reset.
    count: usize,
    /// Dead objects by size class, for an allocation of that class to take before the bump
    /// pointer moves: what keeps an entry's memory bounded by what it holds rather than by what
    /// it ever held. Filled only while `reuse` is set, which a release build does and a debug
    /// build does not, so the tests keep reading a stale word as a dead header.
    free: Vec<Vec<*mut Obj>>,
    reuse: bool,
}

/// The size classes a dead object is kept in, in words; anything larger goes back only at the
/// entry's end.
const REUSE_CLASSES: usize = 64;

thread_local! {
    /// The heap of the entry running on this thread, which a dying object goes back to. Entries
    /// never nest on a thread (a re-entry is declined), so one is enough.
    static CURRENT: std::cell::Cell<*mut Heap> = const { std::cell::Cell::new(std::ptr::null_mut()) };
}

/// The entry beginning on this thread allocates from `heap`; its dead objects go back to it.
pub fn enter(heap: *mut Heap) {
    CURRENT.with(|c| c.set(heap));
}

pub fn leave() {
    CURRENT.with(|c| c.set(std::ptr::null_mut()));
}

/// The payload an object was allocated with, from its header: what its allocation site asked
/// for, kind by kind, so a dead object goes back to the class it came from.
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

/// A dead object goes back to the current heap's free list for its class. A bridged one does
/// not: its slot is on the heap's drop log, and a second bridged value in the same slot would
/// be dropped twice at the entry's end.
unsafe fn recycle(o: *mut Obj, heap: *mut Heap) {
    if heap.is_null() {
        return;
    }
    unsafe {
        if !(*heap).reuse || (*o).kind == KIND_BRIDGE {
            return;
        }
        let size = payload_bytes(o);
        if size == usize::MAX {
            return;
        }
        let class = Heap::object_size(size) / 8;
        if class >= REUSE_CLASSES {
            return;
        }
        let free = &mut (*heap).free;
        if free.len() <= class {
            free.resize_with(class + 1, Vec::new);
        }
        free[class].push(o);
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
            chunk: 0,
            bridges: Vec::new(),
            persistent: false,
            count: 0,
            free: Vec::new(),
            reuse: !cfg!(debug_assertions),
        }
    }

    /// Whether dead objects are reused within an entry: on in a release build, off in a debug
    /// one, and set here by a test that exercises the reuse itself.
    pub fn set_reuse(&mut self, reuse: bool) {
        self.reuse = reuse;
    }

    /// A heap whose entries never end: what outlives every entry lives here.
    pub fn persistent() -> Heap {
        let mut h = Heap::new();
        h.persistent = true;
        h
    }

    /// How many objects have been allocated since the last reset.
    pub fn allocated(&self) -> usize {
        self.count
    }

    /// Moves to a chunk with `need` bytes free: the next one already on hand that fits, or a
    /// new one, each larger than the last up to a bound.
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
        self.chunk = self.chunks.len() - 1;
        self.cur = p;
        self.end = unsafe { p.add(cap) };
    }

    /// The bytes an object with this payload takes, header included and rounded to a word.
    pub fn object_size(payload_bytes: usize) -> usize {
        (HEADER + payload_bytes.max(8) + 7) & !7
    }

    pub(crate) fn raw_alloc(
        &mut self,
        kind: u8,
        flags: u8,
        len: u32,
        layout: u32,
        payload_bytes: usize,
    ) -> *mut Obj {
        let size = Heap::object_size(payload_bytes);
        // A dead object of this class, if the entry has one, before the bump pointer moves.
        let recycled = self.free.get_mut(size / 8).and_then(Vec::pop);
        let p = match recycled {
            Some(p) => p,
            None => {
                if (self.end as usize).wrapping_sub(self.cur as usize) < size || self.cur.is_null()
                {
                    self.grow(size);
                }
                let p = self.cur as *mut Obj;
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
        p
    }

    /// A fresh object with `len` payload words.
    pub fn alloc(&mut self, kind: u8, flags: u8, len: u32, layout: u32) -> *mut Obj {
        self.raw_alloc(kind, flags, len, layout, len as usize * 8)
    }

    /// [`dec`] for a word compiled code has already found held once and mortal: released into
    /// this heap's free lists without asking a thread-local which heap that is.
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

    /// A string or bytes value of `kind` holding `a` then `b`, with at least `room` bytes of
    /// capacity.
    fn joined(&mut self, kind: u8, a: &[u8], b: &[u8], room: usize) -> *mut Obj {
        let len = a.len() + b.len();
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

    /// `a` with `b` appended, of `a`'s kind. Takes `a`: when nobody else holds it and it has the
    /// room, the bytes are written after its own and it is answered; otherwise a fresh value with
    /// room to grow again is answered and `a` released. A value built by appending to it in a
    /// loop therefore copies each piece once.
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

    /// A word that lives as long as this persistent heap does: the constant pool. Every object
    /// under it is immortal too, so no count is touched through a constant.
    pub fn immortal(&mut self, layouts: &Layouts, v: &Value) -> Word {
        debug_assert!(
            self.persistent,
            "an immortal word needs a heap that never resets"
        );
        let w = self.to_word(layouts, v);
        mark_immortal(w);
        w
    }

    /// A copy of everything under `w` into this persistent heap, immortal: what the memo keeps
    /// of an entry's word, since the entry's own memory is recycled. An object already immortal
    /// is shared rather than copied.
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

    /// Resets the entry's memory: every bridged value still alive is dropped, and the chunks
    /// are kept for the next entry. A live count is no obstacle: the entry's answer has already
    /// been copied out as a [`Value`], so nothing outside the entry can hold a word.
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
        for class in &mut self.free {
            class.clear();
        }
    }

    // --- Conversions ------------------------------------------------------------------------

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

    /// [`Heap::to_value`], noting what it read on the way: the objects, for the seam's census,
    /// and whether any was a handle — a closure, or a bridged value holding one — which the
    /// seam refuses to carry out, so it need not walk the answer a second time to ask.
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

// --- Counts ------------------------------------------------------------------------------------

/// One more holder of `w`.
pub fn inc(w: Word) {
    if is_imm(w) {
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
pub fn dec(w: Word) {
    if is_imm(w) {
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

/// Perceus's `reset`: `w`, a record held once, lets its fields go and keeps its memory for the
/// next record of the same width — its length zeroed, so a release before that walks nothing.
/// Answers the word kept, or `0` — the object released — for anything that is not such a record.
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

/// Byte order over two slices, by hand for the short keys a map is probed with and by `memcmp`
/// past that.
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

/// How deep a dying object's dying children are dismantled on the stack before the rest are
/// deferred to a heap list: a record of scalars, the common case, allocates nothing.
const DISMANTLE_DEPTH: usize = 32;

/// `o`, held once, dies: each child is let go, a child it was the last holder of dismantled in
/// turn, and its header marked dead — read as such by anything still holding a stale word,
/// dropped by nothing twice, and its memory recycled with the entry.
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

/// Marks everything under `w` immortal where it lies: no count is touched through it again, and
/// the entry's end hands it to whoever keeps the memo rather than releasing it.
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

/// `ply_eval::memo::world_independent` over a word: whether what it denotes means the same thing
/// in a world it was not produced in, which is what a remembered constant must.
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
                    if !ply_eval::memo::world_independent(bridged(o)) {
                        return false;
                    }
                }
                _ => {}
            }
        }
    }
    true
}

/// Whether one holder alone has `w`: what lets an update write in place.
pub fn is_unique(w: Word) -> bool {
    !is_imm(w) && unsafe { (*obj(w)).rc == 1 }
}

pub fn kind(w: Word) -> u8 {
    if is_imm(w) {
        KIND_INT
    } else {
        unsafe { (*obj(w)).kind }
    }
}

/// The `Int` a word carries, if it is one.
pub fn as_int(w: Word) -> Option<i64> {
    if is_imm(w) {
        return Some(imm_value(w));
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

pub fn as_bool(w: Word) -> Option<bool> {
    if is_imm(w) {
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

// --- Order -------------------------------------------------------------------------------------

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

/// `Value::cmp` over words: structural, total and deterministic, the order a map's keys are held
/// in — and the same order, so a map crosses the seam with its entries where the interpreter
/// would put them. Two words of one native kind compare in place; anything else compares as the
/// values it denotes.
pub fn cmp_words(layouts: &Layouts, a: Word, b: Word) -> Ordering {
    if a == b {
        return Ordering::Equal;
    }
    // Two strings, or two byte strings — a map's keys, most often — are compared as bytes
    // before any of the ranking below is asked of them.
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

/// Whether a word can be a native map's key: ordered in place, and already canonical — a
/// `Decimal` is neither, a `Secret` has no order, and both are the interpreter's to refuse.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn layouts() -> Layouts {
        Layouts::new(vec![(Symbol::new("Some"), 1), (Symbol::new("None"), 0)])
    }

    /// Two byte strings compare as bytes on the fast path, and as the interpreter orders them.
    #[test]
    fn two_byte_strings_compare_in_byte_order_at_every_length() {
        let mut h = Heap::new();
        let l = layouts();
        let long: Vec<u8> = (0..40u8).collect();
        let mut longer = long.clone();
        longer.push(0);
        let cases: [(&[u8], &[u8]); 6] = [
            (b"kA", b"kB"),
            (b"kB", b"kA"),
            (b"k", b"kA"),
            (b"kA", b"kA"),
            (&long, &longer),
            (&longer, &long),
        ];
        for (x, y) in cases {
            let (a, b) = (h.bytes(x), h.bytes(y));
            assert_eq!(cmp_words(&l, a, b), x.cmp(y), "{x:?} against {y:?}");
            assert_eq!(
                cmp_words(&l, a, b),
                Heap::to_value(&l, a).cmp(&Heap::to_value(&l, b))
            );
        }
        let (s, b) = (h.str("kA"), h.bytes(b"kA"));
        assert_eq!(
            cmp_words(&l, s, b),
            Heap::to_value(&l, s).cmp(&Heap::to_value(&l, b)),
            "a string against bytes takes the general path"
        );
    }

    /// A flat record's release walks nothing and still recycles it; a field written in place
    /// that holds a count takes the flag with it.
    #[test]
    fn a_flat_record_is_released_without_a_walk_and_loses_the_flag_when_it_gains_a_count() {
        let mut h = Heap::new();
        h.set_reuse(true);
        let o = h.alloc(KIND_RECORD, FLAT, 2, 0);
        unsafe {
            set_word(o, 0, imm(1));
            set_word(o, 1, imm(2));
        }
        crate::heap::enter(&mut h);
        dec(o as Word);
        crate::heap::leave();
        unsafe { assert_eq!((*o).kind, KIND_DEAD) };
        let again = h.alloc(KIND_RECORD, 0, 2, 0);
        assert_eq!(again, o, "a flat record's memory was not recycled");
        let child = h.alloc(KIND_RECORD, 0, 1, 0);
        unsafe {
            set_word(child, 0, imm(1));
            set_word(again, 0, child as Word);
            set_word(again, 1, imm(2));
            (*again).flags = 0;
        }
        crate::heap::enter(&mut h);
        dec(again as Word);
        crate::heap::leave();
        unsafe { assert_eq!((*child).kind, KIND_DEAD, "a counted field was not let go") };
    }

    /// Perceus's reset keeps a record held once with its fields let go, and releases anything
    /// else.
    #[test]
    fn a_reset_record_keeps_its_memory_and_lets_its_fields_go() {
        let mut h = Heap::new();
        let child = h.alloc(KIND_RECORD, 0, 1, 0);
        unsafe { set_word(child, 0, imm(1)) };
        let o = h.alloc(KIND_RECORD, 0, 2, 0);
        unsafe {
            set_word(o, 0, child as Word);
            set_word(o, 1, imm(7));
        }
        inc(child as Word);
        assert_eq!(reset(o as Word), o as Word);
        unsafe {
            assert_eq!((*o).len, 0);
            assert_eq!((*o).rc, 1);
            assert_eq!((*o).kind, KIND_RECORD);
            assert_eq!((*child).rc, 1, "the field was let go once");
        }
        dec(o as Word);
        unsafe { assert_eq!((*o).kind, KIND_DEAD) };

        let shared = h.alloc(KIND_RECORD, 0, 1, 0);
        unsafe { set_word(shared, 0, imm(1)) };
        inc(shared as Word);
        assert_eq!(
            reset(shared as Word),
            0,
            "a record held twice is released, not kept"
        );
        unsafe { assert_eq!((*shared).rc, 1) };
        assert_eq!(reset(imm(3)), 0);
        dec(child as Word);
        dec(shared as Word);
    }

    #[test]
    fn an_int_that_fits_is_an_immediate_and_one_that_does_not_is_boxed() {
        let mut h = Heap::new();
        let l = layouts();
        for n in [0i64, 1, -1, i64::MAX >> 1, i64::MIN >> 1] {
            let w = h.boxed_int(n);
            assert!(is_imm(w));
            assert_eq!(as_int(w), Some(n));
        }
        for n in [i64::MAX, i64::MIN, (i64::MAX >> 1) + 1] {
            let w = h.boxed_int(n);
            assert!(!is_imm(w));
            assert_eq!(as_int(w), Some(n));
            assert_eq!(Heap::to_value(&l, w), Value::Int(n));
        }
    }

    #[test]
    fn every_value_kind_round_trips() {
        let mut h = Heap::new();
        let l = layouts();
        let record = Value::Record(Arc::new(Fields::from_unsorted(vec![
            (Symbol::new("b"), Value::Int(2)),
            (
                Symbol::new("a"),
                Value::list(vec![Value::Bool(true), Value::Unit]),
            ),
        ])));
        let values = vec![
            Value::Int(7),
            Value::Bool(false),
            Value::Unit,
            Value::str("hi"),
            Value::bytes(b"raw"),
            record.clone(),
            Value::ctor("Some", vec![record.clone()]),
            Value::ctor("None", vec![]),
            Value::list(vec![Value::Int(1), Value::str("x"), record]),
            Value::map(vec![(Value::Int(1), Value::Int(2))]),
        ];
        for v in values {
            let w = h.to_word(&l, &v);
            assert_eq!(Heap::to_value(&l, w), v, "{v:?}");
        }
        h.end();
    }

    #[test]
    fn a_record_is_laid_out_in_sorted_field_order_whatever_order_it_was_written_in() {
        let mut h = Heap::new();
        let l = layouts();
        let v = Value::Record(Arc::new(Fields::from_unsorted(vec![
            (Symbol::new("z"), Value::Int(26)),
            (Symbol::new("a"), Value::Int(1)),
        ])));
        let w = h.to_word(&l, &v);
        let o = obj(w);
        let shape = unsafe { (*o).layout };
        assert_eq!(l.offset(shape, &Symbol::new("a")), Some(0));
        assert_eq!(l.offset(shape, &Symbol::new("z")), Some(1));
        assert_eq!(as_int(unsafe { word_at(o, 0) }), Some(1));
        assert_eq!(as_int(unsafe { word_at(o, 1) }), Some(26));
    }

    #[test]
    fn the_last_holder_dismantles_and_the_memory_waits_for_the_end() {
        let mut h = Heap::new();
        let l = layouts();
        let inner = Value::list(vec![Value::Int(1)]);
        let w = h.to_word(&l, &Value::ctor("Some", vec![inner]));
        let child = unsafe { word_at(obj(w), 0) };
        inc(w);
        dec(w);
        assert_eq!(kind(w), KIND_CTOR);
        dec(w);
        assert_eq!(kind(w), KIND_DEAD);
        assert_eq!(kind(child), KIND_DEAD);
        h.end();
    }

    #[test]
    fn a_dead_object_is_reused_by_the_next_of_its_class_within_an_entry() {
        let mut h = Heap::new();
        h.set_reuse(true);
        enter(&mut h);
        let l = layouts();
        let first = h.to_word(&l, &Value::list(vec![Value::Int(1), Value::Int(2)]));
        let record = h.to_word(
            &l,
            &Value::Record(Arc::new(Fields::from_unsorted(vec![
                (Symbol::new("a"), Value::Int(1)),
                (Symbol::new("b"), Value::Int(2)),
            ]))),
        );
        dec(first);
        let again = h.to_word(&l, &Value::list(vec![Value::Int(3), Value::Int(4)]));
        assert_eq!(
            again, first,
            "a list of the same class took the dead list's slot"
        );
        assert_eq!(
            Heap::to_value(&l, again),
            Value::list(vec![Value::Int(3), Value::Int(4)])
        );
        dec(record);
        let other = h.to_word(&l, &Value::str("ab"));
        assert_ne!(other, record, "a string is not a record's class");
        let bridged = h.bridge(Value::Float(1.5));
        dec(bridged);
        let bridged_again = h.bridge(Value::Float(2.5));
        assert_ne!(bridged_again, bridged, "a bridged slot is never reused");
        leave();
        h.end();
    }

    #[test]
    fn a_shared_child_survives_its_parent() {
        let mut h = Heap::new();
        let l = layouts();
        let child = h.to_word(&l, &Value::str("kept"));
        inc(child);
        let parent = h.list_from(&[child]);
        dec(parent);
        assert_eq!(kind(child), KIND_STR);
        assert_eq!(Heap::to_value(&l, child), Value::str("kept"));
        dec(child);
        assert_eq!(kind(child), KIND_DEAD);
        h.end();
    }

    #[test]
    fn appending_to_a_string_nobody_else_holds_writes_in_place_and_to_a_shared_one_copies() {
        let mut h = Heap::new();
        let l = layouts();
        let mut s = h.str("ab");
        let first = s;
        s = h.append(s, b"cd");
        assert_ne!(
            s, first,
            "an exact-sized value has no room, so the first append copies"
        );
        assert_eq!(kind(first), KIND_DEAD);
        let grown = s;
        s = h.append(s, b"e");
        assert_eq!(
            s, grown,
            "the copy left room, so the next append is in place"
        );
        assert_eq!(Heap::to_value(&l, s), Value::str("abcde"));
        inc(s);
        let other = h.append(s, b"f");
        assert_ne!(other, s, "a value held twice is not written into");
        assert_eq!(Heap::to_value(&l, s), Value::str("abcde"));
        assert_eq!(Heap::to_value(&l, other), Value::str("abcdef"));
        let raw = h.bytes(b"\xff\x00");
        assert_eq!(Heap::to_value(&l, raw), Value::bytes(b"\xff\x00"));
        assert_eq!(kind(raw), KIND_BYTES);
        h.end();
    }

    #[test]
    fn words_order_exactly_as_the_values_they_denote() {
        let mut h = Heap::new();
        let l = layouts();
        let record = |a: i64, b: &str| {
            Value::Record(Arc::new(Fields::from_unsorted(vec![
                (Symbol::new("n"), Value::Int(a)),
                (Symbol::new("s"), Value::str(b)),
            ])))
        };
        let values = vec![
            Value::Unit,
            Value::Bool(false),
            Value::Bool(true),
            Value::Int(-3),
            Value::Int(7),
            Value::Int(i64::MAX),
            Value::str("a"),
            Value::str("b"),
            Value::bytes(b"a"),
            Value::list(vec![]),
            Value::list(vec![Value::Int(1)]),
            Value::list(vec![Value::Int(1), Value::Int(0)]),
            Value::map(vec![(Value::Int(1), Value::str("x"))]),
            record(1, "z"),
            record(2, "a"),
            Value::ctor("None", vec![]),
            Value::ctor("Some", vec![Value::Int(1)]),
            Value::ctor("Some", vec![Value::Int(2)]),
        ];
        let words: Vec<Word> = values.iter().map(|v| h.to_word(&l, v)).collect();
        for (i, a) in values.iter().enumerate() {
            for (j, b) in values.iter().enumerate() {
                assert_eq!(
                    cmp_words(&l, words[i], words[j]),
                    a.cmp(b),
                    "{a:?} vs {b:?}"
                );
            }
        }
        h.end();
    }

    #[test]
    fn a_native_map_holds_its_entries_in_the_interpreters_order_and_round_trips() {
        let mut h = Heap::new();
        let l = layouts();
        let mut m = h.map_new();
        for (k, v) in [
            (5, "five"),
            (1, "one"),
            (3, "three"),
            (1, "uno"),
            (4, "four"),
        ] {
            let kw = h.to_word(&l, &Value::Int(k));
            let vw = h.to_word(&l, &Value::str(v));
            m = h.map_insert(&l, m, kw, vw);
        }
        assert_eq!(
            Heap::to_value(&l, m),
            Value::map(vec![
                (Value::Int(1), Value::str("uno")),
                (Value::Int(3), Value::str("three")),
                (Value::Int(4), Value::str("four")),
                (Value::Int(5), Value::str("five")),
            ])
        );
        assert!(map::get(&l, obj(m), imm(3)).is_some());
        assert!(map::get(&l, obj(m), imm(2)).is_none());
        let m = h.map_remove(&l, m, imm(3));
        let m = h.map_remove(&l, m, imm(99));
        assert_eq!(unsafe { (*obj(m)).len }, 3);
        // A shared map is copied by an insert and the original keeps its entries.
        inc(m);
        let kw = h.to_word(&l, &Value::Int(2));
        let m2 = h.map_insert(&l, m, kw, imm(0));
        assert_ne!(m, m2);
        assert_eq!(unsafe { (*obj(m)).len }, 3);
        assert_eq!(unsafe { (*obj(m2)).len }, 4);
        h.end();
    }

    #[test]
    fn an_adopted_word_is_a_copy_that_outlives_the_entry_and_shares_what_was_already_immortal() {
        let l = layouts();
        let mut entry = Heap::new();
        let mut kept = Heap::persistent();
        let shared = kept.immortal(&l, &Value::str("shared"));
        let record = Value::Record(Arc::new(Fields::from_unsorted(vec![
            (Symbol::new("n"), Value::Int(1)),
            (Symbol::new("s"), Value::str("own")),
        ])));
        let w = entry.to_word(&l, &record);
        let list = entry.list_from(&[w, shared]);
        let copy = kept.adopt(list);
        assert_ne!(copy, list);
        assert_eq!(list::get(obj(copy), 1), shared);
        entry.end();
        assert_eq!(
            Heap::to_value(&l, copy),
            Value::list(vec![record, Value::str("shared")])
        );
    }

    #[test]
    fn an_immortal_word_survives_the_end_of_every_entry_and_counts_nothing() {
        let mut h = Heap::persistent();
        let l = layouts();
        let w = h.immortal(&l, &Value::list(vec![Value::str("a"), Value::Int(1)]));
        h.end();
        inc(w);
        dec(w);
        dec(w);
        assert_eq!(
            Heap::to_value(&l, w),
            Value::list(vec![Value::str("a"), Value::Int(1)])
        );
    }
}
