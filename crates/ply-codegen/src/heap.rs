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
}

impl Layouts {
    pub fn new(ctors: Vec<(Symbol, usize)>) -> Layouts {
        let ctor_ids = ctors
            .iter()
            .enumerate()
            .map(|(i, (n, _))| (n.clone(), i as u32))
            .collect();
        Layouts {
            shapes: RefCell::new(Shapes::default()),
            ctors,
            ctor_ids,
        }
    }

    pub fn ctor_index(&self, name: &Symbol) -> Option<u32> {
        self.ctor_ids.get(name).copied()
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

/// What an entry allocated, and what was made immortal through it.
#[derive(Default)]
pub struct Heap {
    log: Vec<*mut Obj>,
    immortal: Vec<*mut Obj>,
}

impl Heap {
    pub fn new() -> Heap {
        Heap {
            log: Vec::with_capacity(1024),
            immortal: Vec::new(),
        }
    }

    /// How many objects the entry has allocated so far.
    pub fn allocated(&self) -> usize {
        self.log.len()
    }

    fn raw_alloc(kind: u8, flags: u8, len: u32, layout: u32, payload_bytes: usize) -> *mut Obj {
        let l = Layout::from_size_align(HEADER + payload_bytes.max(8), 8).expect("a small layout");
        let p = unsafe { alloc(l) } as *mut Obj;
        assert!(!p.is_null(), "the heap is out of memory");
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
        p
    }

    /// A fresh object with `len` payload words, logged to the entry.
    pub fn alloc(&mut self, kind: u8, flags: u8, len: u32, layout: u32) -> *mut Obj {
        let p = Heap::raw_alloc(kind, flags, len, layout, len as usize * 8);
        self.log.push(p);
        p
    }

    /// A fresh list with room for `cap` words and none of them in use yet.
    pub fn alloc_list(&mut self, cap: u32) -> *mut Obj {
        let p = Heap::raw_alloc(KIND_LIST, 0, 0, cap, cap as usize * 8);
        self.log.push(p);
        p
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
        let o = Heap::raw_alloc(KIND_BRIDGE, 0, 0, 0, std::mem::size_of::<Value>());
        unsafe { bridge_slot(o).write(v) };
        self.log.push(o);
        o as Word
    }

    /// A word that lives as long as this heap does, past every entry: the constant pool and the
    /// memo. Every object under it is immortal too, so no count is touched through a constant.
    pub fn immortal(&mut self, layouts: &Layouts, v: &Value) -> Word {
        let w = self.to_word(layouts, v);
        self.freeze(w);
        w
    }

    fn freeze(&mut self, w: Word) {
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
                if let Some(i) = self.log.iter().rposition(|p| *p == o) {
                    self.log.swap_remove(i);
                }
                self.immortal.push(o);
                match (*o).kind {
                    KIND_RECORD | KIND_CTOR | KIND_LIST => {
                        for i in 0..(*o).len as usize {
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

    /// Releases everything the entry allocated. A live count is no obstacle: the entry's answer
    /// has already been copied out as a [`Value`], so nothing outside the entry can hold a word.
    pub fn end(&mut self) {
        for o in self.log.drain(..) {
            unsafe { release(o) };
        }
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
        self.end();
        for o in self.immortal.drain(..) {
            unsafe { release(o) };
        }
    }
}

/// Hands an object's memory back, dropping a still-live bridged value on the way.
unsafe fn release(o: *mut Obj) {
    unsafe {
        if (*o).kind == KIND_BRIDGE {
            std::ptr::drop_in_place(bridge_slot(o));
        }
        let bytes = payload_bytes(o);
        dealloc(
            o as *mut u8,
            Layout::from_size_align(HEADER + bytes.max(8), 8).expect("a small layout"),
        );
    }
}

/// The payload size an object was allocated with, read back off its header.
unsafe fn payload_bytes(o: *mut Obj) -> usize {
    unsafe {
        match (*o).kind {
            KIND_BRIDGE => std::mem::size_of::<Value>(),
            KIND_LIST => (*o).layout as usize * 8,
            KIND_DEAD => (*o).len as usize,
            _ => (*o).len as usize * 8,
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
                KIND_CLOSURE => {
                    for i in CLOSURE_CAPTURES..(*o).len as usize {
                        pending.push(word_at(o, i));
                    }
                }
                KIND_BRIDGE => std::ptr::drop_in_place(bridge_slot(o)),
                _ => {}
            }
            // Dead, with the byte size the memory was allocated with kept in `len`, so that
            // `Heap::end` frees exactly what was taken and drops no bridged value twice.
            let bytes = payload_bytes(o);
            (*o).kind = KIND_DEAD;
            (*o).len = bytes as u32;
        }
    }
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
    fn an_immortal_word_survives_the_end_of_every_entry_and_counts_nothing() {
        let mut h = Heap::new();
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
