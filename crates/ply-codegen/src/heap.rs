//! The value model compiled code runs on (ADR 0035): one machine word per value, with an
//! immediate for an `Int` that fits and a pointer to a counted object for everything else.
//!
//! A word with its low bit set is an `Int` in its upper sixty-three bits; a word with it clear is
//! the address of an [`Obj`]. `Unit`, `true` and `false` are three immortal objects, so a `Bool`
//! is a pointer compare and never an allocation. A record, a constructor, a list and a native
//! closure are laid out as words after a sixteen-byte header; anything the model does not lay
//! out natively yet — strings, bytes, maps, floats, decimals, secrets, the interpreter's own
//! closures — is carried whole as a [`Value`] behind a `Bridge` object, cheap to hand back to a
//! builtin because the value's own buffer is shared rather than copied.
//!
//! Every object an entry allocates is logged, and [`Heap::end`] releases the log: a count that
//! reaches zero dismantles its object then and there — its children let go, a bridged value
//! dropped — but the memory itself waits for the end of the entry, so a stale reference reads a
//! `DEAD` header rather than someone else's object. Reuse of that memory within an entry is
//! ADR 0035's sequence step 4, not this file's.

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
pub const KIND_DEAD: u8 = 255;

/// A count no increment or decrement touches: the singletons, the constant pool, the memo.
pub const IMMORTAL: u32 = u32::MAX;

pub const HEADER: usize = 16;

/// The header every object starts with. `len` is the payload's word count for a record, a
/// constructor, a list or a closure; `layout` is a record's shape, a constructor's index, a
/// list's capacity or a closure's arity.
#[repr(C, align(8))]
pub struct Obj {
    pub rc: u32,
    pub kind: u8,
    pub flags: u8,
    _pad: u16,
    pub len: u32,
    pub layout: u32,
}

static UNIT_OBJ: Obj = Obj {
    rc: IMMORTAL,
    kind: KIND_UNIT,
    flags: 0,
    _pad: 0,
    len: 0,
    layout: 0,
};
static TRUE_OBJ: Obj = Obj {
    rc: IMMORTAL,
    kind: KIND_BOOL,
    flags: 1,
    _pad: 0,
    len: 0,
    layout: 0,
};
static FALSE_OBJ: Obj = Obj {
    rc: IMMORTAL,
    kind: KIND_BOOL,
    flags: 0,
    _pad: 0,
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
        }
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

    fn raw_alloc(
        &mut self,
        kind: u8,
        flags: u8,
        len: u32,
        layout: u32,
        payload_bytes: usize,
    ) -> *mut Obj {
        let size = Heap::object_size(payload_bytes);
        if (self.end as usize).wrapping_sub(self.cur as usize) < size || self.cur.is_null() {
            self.grow(size);
        }
        let p = self.cur as *mut Obj;
        self.cur = unsafe { self.cur.add(size) };
        unsafe {
            p.write(Obj {
                rc: 1,
                kind,
                flags,
                _pad: 0,
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

    /// A fresh list with room for `cap` words and none of them in use yet.
    pub fn alloc_list(&mut self, cap: u32) -> *mut Obj {
        self.raw_alloc(KIND_LIST, 0, 0, cap, cap as usize * 8)
    }

    /// A fresh map with room for `cap` entries and none in use yet.
    pub fn alloc_map(&mut self, cap: u32) -> *mut Obj {
        self.raw_alloc(KIND_MAP, 0, 0, cap, cap as usize * 16)
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
                KIND_BRIDGE => self.bridge(bridged(o).clone()),
                KIND_LIST => {
                    let n = (*o).len;
                    let c = self.alloc_list(n.max(1));
                    (*c).len = n;
                    for i in 0..n as usize {
                        let x = self.copy(word_at(o, i), copies);
                        set_word(c, i, x);
                    }
                    c as Word
                }
                KIND_MAP => {
                    let n = (*o).len;
                    let c = self.alloc_map(n.max(1));
                    (*c).len = n;
                    for i in 0..2 * n as usize {
                        let x = self.copy(word_at(o, i), copies);
                        set_word(c, i, x);
                    }
                    c as Word
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
    }

    // --- Conversions ------------------------------------------------------------------------

    /// The compiled word for an interpreter value: deep, and every object fresh in the entry.
    pub fn to_word(&mut self, layouts: &Layouts, v: &Value) -> Word {
        match v {
            Value::Int(n) => self.boxed_int(*n),
            Value::Bool(b) => bool(*b),
            Value::Unit => unit(),
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
                let n = items.len() as u32;
                let o = self.alloc_list(n.max(4));
                for (i, item) in items.iter().enumerate() {
                    let w = self.to_word(layouts, item);
                    unsafe { set_word(o, i, w) };
                }
                unsafe { (*o).len = n };
                o as Word
            }
            // The interpreter iterates in key order, which is the layout's order too.
            Value::Map(entries) => {
                let n = entries.size() as u32;
                let o = self.alloc_map(n.max(4));
                for (i, (k, v)) in entries.iter().enumerate() {
                    let kw = self.to_word(layouts, k);
                    let vw = self.to_word(layouts, v);
                    unsafe {
                        set_word(o, 2 * i, kw);
                        set_word(o, 2 * i + 1, vw);
                    }
                }
                unsafe { (*o).len = n };
                o as Word
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
        if is_imm(w) {
            return Value::Int(imm_value(w));
        }
        let o = obj(w);
        unsafe {
            match (*o).kind {
                KIND_UNIT => Value::Unit,
                KIND_BOOL => Value::Bool((*o).flags != 0),
                KIND_INT => Value::Int(word_at(o, 0)),
                KIND_RECORD => {
                    let names = layouts.shape_names((*o).layout);
                    let fields: Vec<(Symbol, Value)> = names
                        .iter()
                        .enumerate()
                        .map(|(i, name)| (name.clone(), Heap::to_value(layouts, word_at(o, i))))
                        .collect();
                    Value::Record(Arc::new(Fields::from_unsorted(fields)))
                }
                KIND_CTOR => {
                    let name = layouts.ctors[(*o).layout as usize].0.clone();
                    let args = (0..(*o).len as usize)
                        .map(|i| Heap::to_value(layouts, word_at(o, i)))
                        .collect();
                    Value::ctor(name, args)
                }
                KIND_LIST => Value::list(
                    (0..(*o).len as usize)
                        .map(|i| Heap::to_value(layouts, word_at(o, i)))
                        .collect(),
                ),
                KIND_MAP => Value::map((0..(*o).len as usize).map(|i| {
                    (
                        Heap::to_value(layouts, word_at(o, 2 * i)),
                        Heap::to_value(layouts, word_at(o, 2 * i + 1)),
                    )
                })),
                KIND_CLOSURE => {
                    let captured: Vec<Value> = (CLOSURE_CAPTURES..(*o).len as usize)
                        .map(|i| Heap::to_value(layouts, word_at(o, i)))
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
                KIND_BRIDGE => bridged(o).clone(),
                other => panic!("a word of kind {other} was read after its object died"),
            }
        }
    }
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

/// One holder fewer; the last one dismantles the object, children and all, without recursing.
pub fn dec(w: Word) {
    if is_imm(w) {
        return;
    }
    // The common case allocates nothing: a holder that is not the last just counts down.
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
        (*o).rc = 1;
    }
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
            debug_assert!((*o).kind != KIND_DEAD, "a dead object was released again");
            (*o).rc -= 1;
            if (*o).rc != 0 {
                continue;
            }
            match (*o).kind {
                KIND_RECORD | KIND_CTOR | KIND_LIST => {
                    for i in 0..(*o).len as usize {
                        pending.push(word_at(o, i));
                    }
                }
                KIND_MAP => {
                    for i in 0..2 * (*o).len as usize {
                        pending.push(word_at(o, i));
                    }
                }
                KIND_CLOSURE => {
                    for i in CLOSURE_CAPTURES..(*o).len as usize {
                        pending.push(word_at(o, i));
                    }
                }
                KIND_BRIDGE => std::ptr::drop_in_place(bridge_slot(o)),
                _ => {}
            }
            // Dead: read as such by anything still holding a stale word, dropped by nothing
            // twice, and its memory recycled with the entry.
            (*o).kind = KIND_DEAD;
        }
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
                KIND_RECORD | KIND_CTOR | KIND_LIST => {
                    for i in 0..(*o).len as usize {
                        pending.push(word_at(o, i));
                    }
                }
                KIND_MAP => {
                    for i in 0..2 * (*o).len as usize {
                        pending.push(word_at(o, i));
                    }
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
                KIND_RECORD | KIND_CTOR | KIND_LIST => {
                    for i in 0..(*o).len as usize {
                        pending.push(word_at(o, i));
                    }
                }
                KIND_MAP => {
                    for i in 0..2 * (*o).len as usize {
                        pending.push(word_at(o, i));
                    }
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

/// A list's items, borrowed.
pub unsafe fn list_items<'a>(o: *mut Obj) -> &'a [Word] {
    unsafe { std::slice::from_raw_parts(words(o), (*o).len as usize) }
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
            KIND_BRIDGE => bridged(obj(a)).cmp(bridged(obj(b))),
            KIND_LIST => {
                let (x, y) = (obj(a), obj(b));
                let (n, m) = ((*x).len as usize, (*y).len as usize);
                for i in 0..n.min(m) {
                    let c = cmp_words(layouts, word_at(x, i), word_at(y, i));
                    if c != Ordering::Equal {
                        return c;
                    }
                }
                n.cmp(&m)
            }
            KIND_MAP => {
                let (x, y) = (obj(a), obj(b));
                let (n, m) = ((*x).len as usize, (*y).len as usize);
                for i in 0..2 * n.min(m) {
                    let c = cmp_words(layouts, word_at(x, i), word_at(y, i));
                    if c != Ordering::Equal {
                        return c;
                    }
                }
                n.cmp(&m)
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
            KIND_UNIT | KIND_BOOL | KIND_INT => true,
            KIND_BRIDGE => matches!(bridged(o), Value::Str(_) | Value::Bytes(_)),
            KIND_RECORD | KIND_CTOR | KIND_LIST => {
                (0..(*o).len as usize).all(|i| native_key(word_at(o, i)))
            }
            KIND_MAP => (0..2 * (*o).len as usize).all(|i| native_key(word_at(o, i))),
            _ => false,
        }
    }
}

// --- Maps --------------------------------------------------------------------------------------

pub unsafe fn map_key(o: *mut Obj, i: usize) -> Word {
    unsafe { word_at(o, 2 * i) }
}

pub unsafe fn map_value(o: *mut Obj, i: usize) -> Word {
    unsafe { word_at(o, 2 * i + 1) }
}

/// Where `k` is in the map, or where it would go.
pub fn map_find(layouts: &Layouts, o: *mut Obj, k: Word) -> Result<usize, usize> {
    let n = unsafe { (*o).len } as usize;
    let (mut lo, mut hi) = (0usize, n);
    while lo < hi {
        let mid = (lo + hi) / 2;
        match cmp_words(layouts, unsafe { map_key(o, mid) }, k) {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => return Ok(mid),
        }
    }
    Err(lo)
}

impl Heap {
    /// A map with the same entries, each held once more, and room for `cap` of them.
    unsafe fn map_copy(&mut self, o: *mut Obj, cap: u32) -> *mut Obj {
        let n = unsafe { (*o).len };
        let out = self.alloc_map(cap.max(n).max(4));
        unsafe {
            for i in 0..2 * n as usize {
                let w = word_at(o, i);
                inc(w);
                set_word(out, i, w);
            }
            (*out).len = n;
        }
        out
    }

    /// `map_insert`: the entry replaced, key and value both, when the key is present, and put
    /// in order when it is not — in place when nothing else holds the map. Takes all three.
    pub fn map_insert(&mut self, layouts: &Layouts, m: Word, k: Word, v: Word) -> Word {
        let o = obj(m);
        let (len, cap) = unsafe { ((*o).len, (*o).layout) };
        match map_find(layouts, o, k) {
            Ok(i) => {
                let target = if is_unique(m) {
                    o
                } else {
                    let copy = unsafe { self.map_copy(o, cap) };
                    dec(m);
                    copy
                };
                unsafe {
                    dec(map_key(target, i));
                    dec(map_value(target, i));
                    set_word(target, 2 * i, k);
                    set_word(target, 2 * i + 1, v);
                }
                target as Word
            }
            Err(i) => {
                let target = if is_unique(m) && len < cap {
                    o
                } else {
                    // Room doubles only when there is none: a shared map copied on every insert
                    // must not grow on every copy.
                    let room = if len < cap { cap } else { (cap * 2).max(4) };
                    let copy = unsafe { self.map_copy(o, room) };
                    dec(m);
                    copy
                };
                unsafe {
                    let base = words(target);
                    std::ptr::copy(base.add(2 * i), base.add(2 * i + 2), 2 * (len as usize - i));
                    set_word(target, 2 * i, k);
                    set_word(target, 2 * i + 1, v);
                    (*target).len = len + 1;
                }
                target as Word
            }
        }
    }

    /// `map_remove`: the map without the key, and the map itself when the key was absent. Takes
    /// the map and reads the key.
    pub fn map_remove(&mut self, layouts: &Layouts, m: Word, k: Word) -> Word {
        let o = obj(m);
        let Ok(i) = map_find(layouts, o, k) else {
            return m;
        };
        let len = unsafe { (*o).len };
        let target = if is_unique(m) {
            o
        } else {
            let copy = unsafe { self.map_copy(o, (*o).layout) };
            dec(m);
            copy
        };
        unsafe {
            dec(map_key(target, i));
            dec(map_value(target, i));
            let base = words(target);
            std::ptr::copy(
                base.add(2 * i + 2),
                base.add(2 * i),
                2 * (len as usize - i - 1),
            );
            (*target).len = len - 1;
        }
        target as Word
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layouts() -> Layouts {
        Layouts::new(vec![(Symbol::new("Some"), 1), (Symbol::new("None"), 0)])
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
    fn a_shared_child_survives_its_parent() {
        let mut h = Heap::new();
        let l = layouts();
        let child = h.to_word(&l, &Value::str("kept"));
        inc(child);
        let parent = h.alloc_list(1);
        unsafe {
            set_word(parent, 0, child);
            (*parent).len = 1;
        }
        dec(parent as Word);
        assert_eq!(kind(child), KIND_BRIDGE);
        assert_eq!(Heap::to_value(&l, child), Value::str("kept"));
        dec(child);
        assert_eq!(kind(child), KIND_DEAD);
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
        let mut m = h.alloc_map(1) as Word;
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
        assert!(map_find(&l, obj(m), imm(3)).is_ok());
        assert!(map_find(&l, obj(m), imm(2)).is_err());
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
    fn a_shared_map_inserted_into_many_times_keeps_its_room_bounded() {
        let mut h = Heap::new();
        let l = layouts();
        let mut m = h.alloc_map(4) as Word;
        for i in 0..200 {
            // Held elsewhere too, so every insert copies.
            inc(m);
            m = h.map_insert(&l, m, imm(i), imm(i));
        }
        let (len, cap) = unsafe { ((*obj(m)).len, (*obj(m)).layout) };
        assert_eq!(len, 200);
        assert!(cap < 1024, "the room grew to {cap}");
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
        let list = entry.alloc_list(2);
        unsafe {
            set_word(list, 0, w);
            set_word(list, 1, shared);
            (*list).len = 2;
        }
        let copy = kept.adopt(list as Word);
        assert_ne!(copy, list as Word);
        assert_eq!(unsafe { word_at(obj(copy), 1) }, shared);
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
