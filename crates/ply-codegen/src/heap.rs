//! The value model compiled code runs on: one word per value, an `Int` immediate (low bit set)
//! or a pointer to a counted [`Obj`]; anything not laid out natively is bridged as a [`Value`].

use crate::blocks::{self, Blocks};
use crate::list;
use crate::map;
use ply_eval::{Carry, Closure, ClosureKind, CtorCarries, Fields, Fixed, Symbol, Value};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};

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
/// `len` elements, a word each.
pub const KIND_ARRAY: u8 = 15;
/// A `Float`: one payload word, its bits.
pub const KIND_FLOAT: u8 = 16;
pub const KIND_DEAD: u8 = 255;

/// A count no increment or decrement touches: the singletons, the constant pool, the memo.
pub const IMMORTAL: u32 = u32::MAX;

/// Set on the count of an object another thread may hold. The count changes atomically, and the
/// object is never unique, so nothing reuses it in place.
pub const SHARED: u32 = 1 << 31;

/// An object's count, read and changed as an atomic: another thread may hold a shared object.
#[inline]
fn count(o: *mut Obj) -> &'static std::sync::atomic::AtomicU32 {
    // SAFETY: `rc` is an aligned `u32` at the start of every object, which lives as long as its heap.
    unsafe { std::sync::atomic::AtomicU32::from_ptr(&raw mut (*o).rc) }
}

pub const HEADER: usize = 16;

/// A record or constructor none of whose fields holds a count: releasing it walks nothing.
pub const FLAT: u8 = 1;
/// A record holding both words it counts and a `Float`'s bits: a bitmap after its words says which.
pub const MIXED: u8 = 2;

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

/// Under `PLY_HEAP_CENSUS`, allocations are also tallied by layout, at a map insert each.
fn census_by_layout() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("PLY_HEAP_CENSUS").is_some())
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

    /// Points poison reports at `site` until the matching swap back, answering what it replaced.
    pub fn swap(site: *const i64) -> *const i64 {
        SITE.with(|s| s.replace(site))
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

/// The words after a mixed record's fields that say which fields are words it counts: one bit
/// each, set for a counted word.
#[inline]
pub fn tail_words(len: usize) -> usize {
    len.div_ceil(64)
}

/// Whether word `i` of `o` is one it counts: every word of an object but a flat record's, and a
/// mixed record's where its tail says so. A record's other words are a `Float`'s bits or immediates.
#[inline]
pub unsafe fn counted(o: *mut Obj, i: usize) -> bool {
    unsafe {
        if (*o).kind != KIND_RECORD {
            return true;
        }
        if (*o).flags & FLAT != 0 {
            return false;
        }
        if (*o).flags & MIXED == 0 {
            return true;
        }
        let len = (*o).len as usize;
        (word_at(o, len + i / 64) as u64 >> (i % 64)) & 1 == 1
    }
}

/// How a record holds a field: a word, or a `Float`'s bits.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Slot {
    Word,
    Float,
}

/// What a field a record holds as a `Float`'s bits is spelled with in a shape: after its name, and
/// below every byte a name holds, so a shape sorts as its names do.
pub const FLOAT_SLOT: &str = "!f";

/// A shape's field as its name and how it is held.
pub fn slot_of(field: &Symbol) -> (Symbol, Slot) {
    match field.as_str().strip_suffix(FLOAT_SLOT) {
        Some(name) => (Symbol::new(name), Slot::Float),
        None => (field.clone(), Slot::Word),
    }
}

/// A field as a shape spells it.
pub fn spelled(name: &Symbol, slot: Slot) -> Symbol {
    match slot {
        Slot::Word => name.clone(),
        Slot::Float => Symbol::new(format!("{}{FLOAT_SLOT}", name.as_str())),
    }
}

/// A closure's payload: its compiled code's address first, then the captured values.
pub const CLOSURE_CODE: usize = 0;
pub const CLOSURE_CAPTURES: usize = 1;

/// A bridge's payload: one owned `Value`.
const BRIDGE_BYTES: usize = std::mem::size_of::<Value>();

/// The bridged value lives in the payload as an owned `Value`.
unsafe fn bridge_slot(o: *mut Obj) -> *mut Value {
    unsafe { words(o) as *mut Value }
}

pub unsafe fn bridged<'a>(o: *mut Obj) -> &'a Value {
    unsafe { &*bridge_slot(o) }
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

/// Record shapes: a shape is its sorted fields, each spelled with how the record holds it, and a
/// field's offset is its position among them.
#[derive(Default)]
pub struct Shapes {
    ids: HashMap<Arc<[Symbol]>, u32>,
    spelled: Vec<Arc<[Symbol]>>,
    names: Vec<Arc<[Symbol]>>,
    slots: Vec<Arc<[Slot]>>,
}

impl Shapes {
    fn intern(&mut self, mut fields: Vec<Symbol>) -> u32 {
        fields.sort();
        if let Some(id) = self.ids.get(fields.as_slice()) {
            return *id;
        }
        let id = self.spelled.len() as u32;
        let (names, slots): (Vec<Symbol>, Vec<Slot>) = fields.iter().map(slot_of).unzip();
        let rc: Arc<[Symbol]> = Arc::from(fields);
        self.ids.insert(rc.clone(), id);
        self.spelled.push(rc);
        self.names.push(Arc::from(names));
        self.slots.push(Arc::from(slots));
        id
    }
}

/// Record shapes and constructors by index, shared by the compiler and the running entry, and by
/// the threads a `parallel` block's branches run on, any of which may intern a shape.
pub struct Layouts {
    shapes: std::sync::RwLock<Shapes>,
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
    /// Per constructor, what its type's module states its values are compared and shown through;
    /// empty for a unit that states none.
    stated: Box<[(Stated, Stated)]>,
    /// Per constructor, the arithmetic its type's module states, by [`ply_eval::instances`]'
    /// roles; empty for a unit that states none.
    numeric: Box<[[Stated; 5]]>,
}

/// A function a type's module states its values go through, as a unit holds it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stated {
    /// The type states none.
    No,
    /// It states one, and the unit did not take the function.
    Absent,
    /// The address of the function's `(ctx, args)` entry.
    By(usize),
}

/// No field at that index.
const NO_FIELD: u16 = u16::MAX;

impl Layouts {
    pub fn new(ctors: Vec<(Symbol, usize)>) -> Layouts {
        Layouts::of(ctors, &[])
    }

    /// The fields of a `{key, value}` entry.
    pub fn entry_fields() -> Vec<Symbol> {
        vec![Symbol::new("key"), Symbol::new("value")]
    }

    /// With `shapes` interned first, in order, so each one's id is its position.
    pub fn of(ctors: Vec<(Symbol, usize)>, shapes: &[Vec<Symbol>]) -> Layouts {
        let ctor_ids: HashMap<Symbol, u32> = ctors
            .iter()
            .enumerate()
            .map(|(i, (n, _))| (n.clone(), i as u32))
            .collect();
        let by = |name: &str| ctor_ids.get(&Symbol::new(name)).copied();
        let (some, none, stop, go) = (by("Some"), by("None"), by("Stop"), by("Continue"));
        let (less, equal, greater) = (by("Less"), by("Equal"), by("Greater"));
        let mut interned = Shapes::default();
        for names in shapes {
            interned.intern(names.clone());
        }
        interned.intern(Layouts::entry_fields());
        Layouts {
            shapes: std::sync::RwLock::new(interned),
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
            stated: Box::default(),
            numeric: Box::default(),
        }
    }

    /// Each constructor's `key` and `show`, by its index.
    pub fn state(&mut self, stated: Vec<(Stated, Stated)>) {
        let none = stated.iter().all(|s| *s == (Stated::No, Stated::No));
        self.stated = if none { Box::default() } else { stated.into() };
    }

    /// Each constructor's arithmetic, by its index.
    pub fn state_numeric(&mut self, numeric: Vec<[Stated; 5]>) {
        let none = numeric.iter().all(|s| *s == [Stated::No; 5]);
        self.numeric = if none { Box::default() } else { numeric.into() };
    }

    /// Whether any type of the unit states a `key`, a `show` or a `numeric`.
    pub fn states_any(&self) -> bool {
        !self.stated.is_empty() || !self.numeric.is_empty()
    }

    pub fn numeric(&self, ctor: u32, role: usize) -> Stated {
        self.numeric
            .get(ctor as usize)
            .map_or(Stated::No, |s| s[role])
    }

    pub fn key(&self, ctor: u32) -> Stated {
        self.stated.get(ctor as usize).map_or(Stated::No, |s| s.0)
    }

    pub fn show(&self, ctor: u32) -> Stated {
        self.stated.get(ctor as usize).map_or(Stated::No, |s| s.1)
    }

    pub fn ctor_index(&self, name: &Symbol) -> Option<u32> {
        self.ctor_ids.get(name).copied()
    }

    pub fn index_fields(&mut self, names: &[Symbol]) {
        let mut indices: HashMap<&Symbol, Vec<usize>> = HashMap::with_capacity(names.len());
        for (index, name) in names.iter().enumerate() {
            indices.entry(name).or_default().push(index);
        }
        let shapes = &self
            .shapes
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .names;
        let width = names.len();
        let mut rows = vec![NO_FIELD; shapes.len() * width].into_boxed_slice();
        for (row, shape) in shapes.iter().enumerate() {
            for (at, field) in shape.iter().enumerate() {
                for &index in indices.get(field).map_or(&[][..], Vec::as_slice) {
                    rows[row * width + index] = at as u16;
                }
            }
        }
        self.rows = rows;
        self.width = width;
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
    fn read(&self) -> std::sync::RwLockReadGuard<'_, Shapes> {
        self.shapes.read().unwrap_or_else(|e| e.into_inner())
    }

    pub fn shape(&self, mut fields: Vec<Symbol>) -> u32 {
        fields.sort();
        if let Some(id) = self.read().ids.get(fields.as_slice()) {
            return *id;
        }
        self.shapes
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .intern(fields)
    }

    pub fn shape_count(&self) -> usize {
        self.read().names.len()
    }

    /// A shape's fields' names, in its order.
    pub fn shape_names(&self, shape: u32) -> Arc<[Symbol]> {
        self.read().names[shape as usize].clone()
    }

    /// A shape's fields as a unit's table and a stored value spell them.
    pub fn shape_spelled(&self, shape: u32) -> Arc<[Symbol]> {
        self.read().spelled[shape as usize].clone()
    }

    /// How a shape's records hold each field, in its order.
    pub fn shape_slots(&self, shape: u32) -> Arc<[Slot]> {
        self.read().slots[shape as usize].clone()
    }

    /// Whether a shape's records hold a `Float` as its bits.
    pub fn holds_floats(&self, shape: u32) -> bool {
        self.read().slots[shape as usize].contains(&Slot::Float)
    }

    /// How many fields a shape has.
    pub fn shape_width(&self, shape: u32) -> usize {
        self.read().names[shape as usize].len()
    }

    pub fn offset(&self, shape: u32, name: &Symbol) -> Option<usize> {
        self.read().names[shape as usize].binary_search(name).ok()
    }
}

/// An entry's objects, whose memory goes back whole at its end; a persistent heap's never does.
pub struct Heap {
    blocks: Blocks,
    /// The bridges whose `Value` the end drops, each block's `layout` its index here.
    bridges: Vec<*mut Obj>,
    persistent: bool,
    /// Objects allocated since the last reset, the bytes they took, and the objects by kind.
    count: usize,
    bytes: usize,
    by_kind: [usize; 16],
    /// Under `PLY_HEAP_CENSUS`, allocations by kind and layout or power-of-two length.
    by_layout: HashMap<(u8, u32), usize>,
    /// Dead blocks not yet given back, oldest first, under `delay()`.
    delayed: std::collections::VecDeque<(*mut Obj, usize)>,
    poison: bool,
    delay: usize,
    census: bool,
}

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

/// The heap an entry running nested on this thread takes over until it swaps the old one back.
pub fn swap_current(heap: *mut Heap) -> *mut Heap {
    CURRENT.with(|c| c.replace(heap))
}

/// The payload bytes an object was allocated with, from its header; `usize::MAX` if unsized.
/// A record's payload: its words, then a mixed one's tail.
fn record_bytes(flags: u8, len: u32) -> usize {
    let len = len as usize;
    8 * (len
        + if flags & MIXED != 0 {
            tail_words(len)
        } else {
            0
        })
}

unsafe fn payload_bytes(o: *mut Obj) -> usize {
    unsafe {
        match (*o).kind {
            KIND_RECORD => record_bytes((*o).flags, (*o).len),
            KIND_CTOR | KIND_CLOSURE | KIND_INT | KIND_FLOAT | KIND_ARRAY => (*o).len as usize * 8,
            KIND_LEAF | KIND_BRANCH => (*o).layout as usize * 8,
            KIND_LIST => (list::TAIL + (*o).aux as usize) * 8,
            KIND_MAP => 8,
            KIND_MLEAF => (*o).layout as usize * 16,
            KIND_MBRANCH => map::BRANCH_BYTES,
            KIND_STR | KIND_BYTES => (*o).layout as usize,
            KIND_BRIDGE => BRIDGE_BYTES,
            _ => usize::MAX,
        }
    }
}

/// Gives a dead object's block back. Only the heap that owns the block may: its bridge leaves the
/// table first, so the bridge that takes the block next is not listed twice, and a bridge another
/// heap owns stays listed there, dead, until that heap's end.
unsafe fn recycle(o: *mut Obj, heap: *mut Heap) {
    unsafe {
        let size = payload_bytes(o);
        let bridge = (*o).kind == KIND_BRIDGE;
        (*o).kind = KIND_DEAD;
        if size == usize::MAX {
            return;
        }
        let object = Heap::object_size(size);
        if heap.is_null() || !(*heap).blocks.owns(o.cast(), object) {
            if !bridge {
                blocks::hand_back(o.cast(), object);
            }
            return;
        }
        if bridge {
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

impl Default for Heap {
    fn default() -> Heap {
        Heap::new()
    }
}

impl Heap {
    pub fn new() -> Heap {
        Heap {
            blocks: Blocks::default(),
            bridges: Vec::new(),
            persistent: false,
            count: 0,
            bytes: 0,
            by_kind: [0; 16],
            by_layout: HashMap::new(),
            delayed: std::collections::VecDeque::new(),
            poison: poisoning(),
            delay: delay(),
            census: census_by_layout(),
        }
    }

    /// Takes over a finished branch's memory: its objects are this entry's from here on.
    pub fn adopt_heap(&mut self, mut other: Heap) {
        self.blocks.adopt(std::mem::take(&mut other.blocks));
        for o in other.bridges.drain(..) {
            unsafe { (*o).layout = self.bridges.len() as u32 };
            self.bridges.push(o);
        }
        self.delayed.extend(other.delayed.drain(..));
        self.count += other.count;
        self.bytes += other.bytes;
        for (mine, theirs) in self.by_kind.iter_mut().zip(other.by_kind) {
            *mine += theirs;
        }
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
        self.blocks.each(|block| {
            let o: *mut Obj = block.cast();
            let (kind, rc) = unsafe { ((*o).kind, (*o).rc) };
            if kind == KIND_DEAD || rc == 0 {
                return;
            }
            let size = unsafe { payload_bytes(o) };
            let e = tally.entry(kind).or_insert((0, 0));
            e.0 += 1;
            // `usize::MAX` means unsized: count the header alone.
            e.1 += if size == usize::MAX { HEADER } else { size };
        });
        tally.into_iter().map(|(k, (n, b))| (k, n, b)).collect()
    }

    /// The bytes of the memory holding the entry's objects, whatever is live in it.
    pub fn held(&self) -> usize {
        self.blocks.held()
    }

    /// Objects allocated since the last reset.
    pub fn allocated(&self) -> usize {
        self.count
    }

    /// The bytes the objects allocated since the last reset took, headers included: what an
    /// update that copies spends and one in place does not.
    pub fn allocated_bytes(&self) -> usize {
        self.bytes
    }

    /// Allocations by kind, indexed by the `KIND_*` constants.
    pub fn allocated_by_kind(&self) -> [usize; 16] {
        self.by_kind
    }

    /// Under `PLY_HEAP_CENSUS`, allocations by kind and layout or length class, most first.
    pub fn allocated_by_layout(&self) -> Vec<((u8, u32), usize)> {
        let mut out: Vec<((u8, u32), usize)> =
            self.by_layout.iter().map(|(k, n)| (*k, *n)).collect();
        out.sort_by_key(|(k, n)| (std::cmp::Reverse(*n), *k));
        out
    }

    /// Allocations that took a dead object's block rather than fresh memory.
    pub fn recycled(&self) -> usize {
        self.blocks.reused()
    }

    /// How many bridges the table lists: what a leak of them grows.
    pub fn bridges(&self) -> usize {
        self.bridges.len()
    }

    /// Takes a dying bridge out of the table, the last entry moving into its place.
    unsafe fn unlist(&mut self, o: *mut Obj) {
        let at = unsafe { (*o).layout } as usize;
        debug_assert!(
            self.bridges.get(at) == Some(&o),
            "a bridge died into a heap whose table does not hold it"
        );
        self.bridges.swap_remove(at);
        if let Some(&moved) = self.bridges.get(at) {
            unsafe { (*moved).layout = at as u32 };
        }
    }

    /// Drops the `Value` of every bridge still listed; one released with no heap entered stays
    /// listed, dead, and is skipped.
    fn drop_bridges(&mut self) {
        for o in self.bridges.drain(..) {
            unsafe {
                if (*o).kind == KIND_BRIDGE {
                    std::ptr::drop_in_place(bridge_slot(o));
                }
            }
        }
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
        let p: *mut Obj = self.blocks.take(size).cast();
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
        self.bytes += size;
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
        if !self.blocks.holds(w as usize) {
            return false;
        }
        let o = w as *const Obj;
        unsafe { (*o).kind != KIND_DEAD && (*o).rc != 0 }
    }

    /// A fresh object with `len` payload words, left as a recycled block's last tenant wrote them:
    /// fill every word before anything can release it, or [`dec`] follows garbage.
    pub fn alloc(&mut self, kind: u8, flags: u8, len: u32, layout: u32) -> *mut Obj {
        let bytes = if kind == KIND_RECORD {
            record_bytes(flags, len)
        } else {
            len as usize * 8
        };
        self.raw_alloc(kind, flags, len, layout, bytes)
    }

    /// A record of `fields`, each named as its shape spells it and in its order: each `Float`
    /// held as its bits, and each other word as the record's.
    pub fn record(&mut self, layouts: &Layouts, fields: &[(Symbol, Word)]) -> Word {
        let mut spelled_fields = Vec::with_capacity(fields.len());
        let mut words = Vec::with_capacity(fields.len());
        for (name, w) in fields {
            if kind(*w) == KIND_FLOAT {
                spelled_fields.push(spelled(name, Slot::Float));
                words.push((unsafe { word_at(obj(*w), 0) }, Slot::Float));
                dec(*w);
            } else {
                spelled_fields.push(name.clone());
                words.push((*w, Slot::Word));
            }
        }
        let shape = layouts.shape(spelled_fields);
        self.record_of(shape, &words)
    }

    /// A record of `shape` holding `words`, each as its slot says, flagged for what it counts.
    fn record_of(&mut self, shape: u32, words: &[(Word, Slot)]) -> Word {
        let n = words.len();
        let floats = words.iter().filter(|(_, s)| *s == Slot::Float).count();
        let flags = if floats == 0 {
            0
        } else if floats == n {
            FLAT
        } else {
            MIXED
        };
        let o = self.alloc(KIND_RECORD, flags, n as u32, shape);
        unsafe {
            for (i, (w, _)) in words.iter().enumerate() {
                set_word(o, i, *w);
            }
            if flags == MIXED {
                for t in 0..tail_words(n) {
                    let bits = (t * 64..n.min(t * 64 + 64))
                        .filter(|i| words[*i].1 == Slot::Word)
                        .fold(0u64, |acc, i| acc | 1 << (i % 64));
                    set_word(o, n + t, bits as i64);
                }
            }
        }
        o as Word
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

    pub fn boxed_float(&mut self, f: f64) -> Word {
        let o = self.alloc(KIND_FLOAT, 0, 1, 0);
        unsafe { set_word(o, 0, ply_eval::float::bits(f) as i64) };
        o as Word
    }

    pub fn bridge(&mut self, v: Value) -> Word {
        let at = u32::try_from(self.bridges.len()).expect("a bridge table a header can index");
        let o = self.raw_alloc(KIND_BRIDGE, 0, 0, at, BRIDGE_BYTES);
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
                KIND_FLOAT => self.boxed_float(f64::from_bits(word_at(o, 0) as u64)),
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
                        let x = if counted(o, i) {
                            self.copy(word_at(o, i), copies)
                        } else {
                            word_at(o, i)
                        };
                        set_word(c, i, x);
                    }
                    if kind == KIND_RECORD && flags & MIXED != 0 {
                        let len = len as usize;
                        for t in len..len + tail_words(len) {
                            set_word(c, t, word_at(o, t));
                        }
                    }
                    c as Word
                }
            };
            copies.insert(w as usize, out);
            out
        }
    }

    /// Gives the entry's memory back; the answer is already copied out.
    pub fn end(&mut self) {
        if self.persistent {
            return;
        }
        self.drop_bridges();
        self.blocks.reset();
        self.count = 0;
        self.bytes = 0;
        self.by_kind = [0; 16];
        self.by_layout.clear();
        self.delayed.clear();
    }

    /// The compiled word for a `Value`: deep, and every object fresh in the entry.
    pub fn to_word(&mut self, layouts: &Layouts, v: &Value) -> Word {
        match v {
            Value::Int(n) => self.boxed_int(*n),
            // Below 64 bits a width is held as the `Int` it stands for, which its sign-extended
            // bits cut to 64 are; at 64 bits and past it is the runtime's own.
            Value::Fixed(f) if f.ty.bits() < 64 => imm(f.bits() as i64),
            Value::Bool(b) => bool(*b),
            Value::Float(f) => self.boxed_float(*f),
            Value::Unit => unit(),
            Value::Str(s) if u32::try_from(s.len()).is_ok() => self.str(s),
            Value::Bytes(b) if u32::try_from(b.len()).is_ok() => self.bytes(b),
            // Each `Float` is held as its bits, as a record of its type is wherever it is made.
            // `Fields` iterates sorted, which is the layout's order.
            Value::Record(fields) => {
                let mut spelled_fields = Vec::with_capacity(fields.len());
                let mut words = Vec::with_capacity(fields.len());
                for (name, value) in fields.iter() {
                    match value {
                        Value::Float(f) => {
                            spelled_fields.push(spelled(name, Slot::Float));
                            words.push((ply_eval::float::bits(*f) as i64, Slot::Float));
                        }
                        _ => {
                            spelled_fields.push(name.clone());
                            words.push((self.to_word(layouts, value), Slot::Word));
                        }
                    }
                }
                let shape = layouts.shape(spelled_fields);
                self.record_of(shape, &words)
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
            Value::Array(items) if u32::try_from(items.len()).is_ok() => {
                let o = self.alloc(KIND_ARRAY, 0, items.len() as u32, 0);
                for (i, x) in items.iter().enumerate() {
                    let w = self.to_word(layouts, x);
                    unsafe { set_word(o, i, w) };
                }
                o as Word
            }
            // A `Value::Map` iterates in key order, which is the tree's order too.
            Value::Map(entries) => {
                let words: Vec<(Word, Word)> = entries
                    .iter()
                    .map(|(k, v)| (self.to_word(layouts, k), self.to_word(layouts, v)))
                    .collect();
                if layouts.states_any() {
                    // A map made outside this unit's entry is in an order its keys do not decide.
                    let mut m = self.map_new();
                    for (k, v) in words {
                        m = self.map_insert(layouts, m, k, v);
                    }
                    m
                } else {
                    self.map_from_sorted(&words)
                }
            }
            Value::Closure(c) => match &c.kind {
                ClosureKind::Native {
                    code,
                    arity,
                    captured,
                }
                | ClosureKind::Continuation {
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
                ClosureKind::Ctor { .. } | ClosureKind::Builtin(_) | ClosureKind::Synth { .. } => {
                    self.bridge(v.clone())
                }
            },
            _ => self.bridge(v.clone()),
        }
    }

    /// The value a word denotes where no fixed width sits below it: deep, and a borrow — the word
    /// keeps its count.
    pub fn to_value(layouts: &Layouts, w: Word) -> Value {
        Heap::to_value_counted(layouts, w, &mut Walked::default())
    }

    /// [`Heap::to_value`], also counting objects read and noting any handle among them.
    pub fn to_value_counted(layouts: &Layouts, w: Word, walked: &mut Walked) -> Value {
        Heap::read(layouts, w, &Carry::Plain, &NO_CTORS, walked)
    }

    /// The value a word denotes, each word read as `carry` says it reads, the constructors' fields
    /// as `ctors` does; counted and noted as [`Heap::to_value_counted`] is.
    pub fn read(
        layouts: &Layouts,
        w: Word,
        carry: &Carry,
        ctors: &CtorCarries,
        walked: &mut Walked,
    ) -> Value {
        if is_imm(w) {
            return read_int(imm_value(w), carry, walked);
        }
        walked.read += 1;
        let o = obj(w);
        unsafe {
            match (*o).kind {
                KIND_UNIT => Value::Unit,
                KIND_BOOL => Value::Bool((*o).flags != 0),
                KIND_INT => read_int(word_at(o, 0), carry, walked),
                KIND_FLOAT => Value::Float(f64::from_bits(word_at(o, 0) as u64)),
                KIND_STR => Value::Str(Arc::from(str_of(o))),
                KIND_BYTES => Value::Bytes(Arc::from(bytes_of(o))),
                KIND_RECORD => {
                    let names = layouts.shape_names((*o).layout);
                    let slots = layouts.shape_slots((*o).layout);
                    let fields: Vec<(Symbol, Value)> = names
                        .iter()
                        .enumerate()
                        .map(|(i, name)| {
                            let field = carry.field(name);
                            let value = match slots[i] {
                                Slot::Float => Value::Float(f64::from_bits(word_at(o, i) as u64)),
                                Slot::Word => {
                                    Heap::read(layouts, word_at(o, i), &field, ctors, walked)
                                }
                            };
                            (name.clone(), value)
                        })
                        .collect();
                    Value::Record(Arc::new(Fields::from_unsorted(fields)))
                }
                KIND_CTOR => {
                    let name = layouts.ctors[(*o).layout as usize].0.clone();
                    let args = (0..(*o).len as usize)
                        .map(|i| {
                            let field = carry.ctor_field(&name, i, ctors);
                            Heap::read(layouts, word_at(o, i), &field, ctors, walked)
                        })
                        .collect();
                    Value::ctor(name, args)
                }
                KIND_LIST => {
                    let item = carry.item();
                    Value::list(
                        list::to_vec(o)
                            .into_iter()
                            .map(|x| Heap::read(layouts, x, &item, ctors, walked))
                            .collect(),
                    )
                }
                KIND_ARRAY => {
                    let item = carry.item();
                    Value::array(
                        (0..(*o).len as usize)
                            .map(|i| Heap::read(layouts, word_at(o, i), &item, ctors, walked))
                            .collect(),
                    )
                }
                KIND_MAP => {
                    let (key, value) = carry.entry();
                    Value::map(map::to_vec(o).into_iter().map(|(k, v)| {
                        (
                            Heap::read(layouts, k, &key, ctors, walked),
                            Heap::read(layouts, v, &value, ctors, walked),
                        )
                    }))
                }
                KIND_CLOSURE => {
                    walked.handle = true;
                    let code = word_at(o, CLOSURE_CODE) as usize;
                    let arity = (*o).layout as usize;
                    let captured: Vec<Value> = (CLOSURE_CAPTURES..(*o).len as usize)
                        .map(|i| Heap::to_value_counted(layouts, word_at(o, i), walked))
                        .collect();
                    // Only the code tells: a continuation's captures are plain immediates.
                    let kind = if crate::detached::is_continuation(code) {
                        ClosureKind::Continuation {
                            code,
                            arity,
                            captured,
                        }
                    } else {
                        ClosureKind::Native {
                            code,
                            arity,
                            captured,
                        }
                    };
                    Value::Closure(Arc::new(Closure { name: None, kind }))
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

/// What a conversion out of the heap read: how many objects, whether one was a handle, and whether
/// an `Int` word did not read, its carry leaving open whether it is an `Int` or a width, or it
/// holding no value of its width.
#[derive(Default)]
pub struct Walked {
    pub read: u64,
    pub handle: bool,
    pub unread: bool,
}

static NO_CTORS: std::sync::LazyLock<CtorCarries> = std::sync::LazyLock::new(CtorCarries::new);

fn read_int(n: i64, carry: &Carry, walked: &mut Walked) -> Value {
    match carry {
        Carry::Plain => Value::Int(n),
        Carry::Width(t) => Fixed::of(*t, i128::from(n)).map_or_else(
            || {
                walked.unread = true;
                Value::Int(n)
            },
            Value::Fixed,
        ),
        _ => {
            walked.unread = true;
            Value::Int(n)
        }
    }
}

impl Drop for Heap {
    fn drop(&mut self) {
        self.drop_bridges();
    }
}

/// One more holder of `w`. Zero is no object, as `ply_inc` and `ply_dec` read it.
#[inline]
pub fn inc(w: Word) {
    if is_imm(w) || w == 0 {
        return;
    }
    let o = obj(w);
    let rc = count(o).load(Relaxed);
    if rc < SHARED {
        debug_assert!(
            unsafe { (*o).kind } != KIND_DEAD,
            "a dead object was shared"
        );
        unsafe { (*o).rc = rc + 1 };
    } else if rc != IMMORTAL {
        inc_shared(w);
    }
}

/// One holder fewer; the last one dismantles the object, children and all.
#[inline]
pub fn dec(w: Word) {
    if is_imm(w) || w == 0 {
        return;
    }
    let o = obj(w);
    let rc = count(o).load(Relaxed);
    if rc == IMMORTAL {
        return;
    }
    if rc >= SHARED {
        dec_shared(w);
        return;
    }
    debug_assert!(
        unsafe { (*o).kind } != KIND_DEAD,
        "a dead object was released again"
    );
    if rc > 1 {
        unsafe { (*o).rc = rc - 1 };
        return;
    }
    let heap = CURRENT.with(|c| c.get());
    unsafe { release(o, heap) }
}

/// [`inc`] of an object [`share`] marked.
pub fn inc_shared(w: Word) {
    count(obj(w)).fetch_add(1, Relaxed);
}

/// [`dec`] of an object [`share`] marked: the holder that takes the count to none dismantles it.
pub fn dec_shared(w: Word) {
    let o = obj(w);
    if count(o).fetch_sub(1, Release) == SHARED | 1 {
        std::sync::atomic::fence(Acquire);
        let heap = CURRENT.with(|c| c.get());
        unsafe { release(o, heap) }
    }
}

/// Marks everything under `words` shared, or nothing when something under them names what only
/// this thread's context holds: a cell, a task or a continuation.
pub fn share(words: &[Word]) -> bool {
    let mut marked: Vec<*mut Obj> = Vec::new();
    let mut pending: Vec<Word> = words.to_vec();
    while let Some(w) = pending.pop() {
        if is_imm(w) || w == 0 {
            continue;
        }
        let o = obj(w);
        let rc = count(o).load(Relaxed);
        // Immortal, or shared already, and so judged when it was marked.
        if rc >= SHARED {
            continue;
        }
        let stays = unsafe {
            match (*o).kind {
                KIND_BRIDGE => crosses_threads(bridged(o)),
                KIND_CLOSURE => {
                    !crate::detached::is_continuation(word_at(o, CLOSURE_CODE) as usize)
                }
                _ => true,
            }
        };
        if !stays {
            for o in marked {
                unsafe { (*o).rc &= !SHARED };
            }
            return false;
        }
        unsafe { (*o).rc = rc | SHARED };
        marked.push(o);
        for (first, last) in unsafe { child_ranges(o) } {
            for i in first..last {
                if unsafe { counted(o, i) } {
                    pending.push(unsafe { word_at(o, i) });
                }
            }
        }
    }
    true
}

/// Whether a bridged value may be held by another thread: a cell is a slot of this entry's arena,
/// and a task or a channel a handle into this thread's scheduler.
fn crosses_threads(v: &Value) -> bool {
    match v {
        Value::Cell(_) | Value::Task(_) | Value::Chan(_) => false,
        Value::Closure(c) => match &c.kind {
            ClosureKind::Native { captured, .. } => captured.iter().all(crosses_threads),
            _ => true,
        },
        Value::List(items) => items.iter().all(crosses_threads),
        Value::Array(items) => items.iter().all(crosses_threads),
        Value::Map(entries) => entries
            .iter()
            .all(|(k, v)| crosses_threads(k) && crosses_threads(v)),
        Value::Record(fields) => fields.values().all(crosses_threads),
        Value::Ctor { args, .. } => args.iter().all(crosses_threads),
        Value::Int(_)
        | Value::Fixed(_)
        | Value::Char(_)
        | Value::Bool(_)
        | Value::Float(_)
        | Value::Decimal(_)
        | Value::Str(_)
        | Value::Bytes(_)
        | Value::Secret(_)
        | Value::Unit => true,
    }
}

/// `o`, held once, released with its children, their blocks back to `heap` where they are its.
unsafe fn release(o: *mut Obj, heap: *mut Heap) {
    let mut deferred = Vec::new();
    unsafe {
        dismantle(o, 0, &mut deferred, heap);
        while let Some(o) = deferred.pop() {
            dismantle(o, 0, &mut deferred, heap);
        }
    }
}

/// Perceus's `reset`: a unique record or constructor drops its fields and keeps its memory for the
/// next record or constructor of its width, which writes the whole header; answers `w`, or `0`
/// after releasing anything else. It keeps its width and is marked flat, so a token released unused
/// walks nothing and goes back to the free list of its own size.
pub fn reset(w: Word) -> Word {
    if is_imm(w) || w == 0 {
        return 0;
    }
    let o = obj(w);
    unsafe {
        if ((*o).kind != KIND_RECORD && (*o).kind != KIND_CTOR) || count(o).load(Relaxed) != 1 {
            dec(w);
            return 0;
        }
        if (*o).flags & FLAT == 0 {
            for i in 0..(*o).len as usize {
                let c = word_at(o, i);
                if counted(o, i) && !is_imm(c) && c != 0 {
                    dec(c);
                }
            }
            (*o).flags |= FLAT;
        }
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

/// The payload words of `o` that hold counted children.
unsafe fn child_ranges(o: *mut Obj) -> [(usize, usize); 2] {
    unsafe {
        match (*o).kind {
            KIND_RECORD | KIND_CTOR if (*o).flags & FLAT != 0 => [(0, 0), (0, 0)],
            KIND_RECORD | KIND_CTOR | KIND_LEAF | KIND_BRANCH | KIND_ARRAY => {
                [(0, (*o).len as usize), (0, 0)]
            }
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
            _ => [(0, 0), (0, 0)],
        }
    }
}

/// `o`, held once, dies: its children are released and its header marked dead.
unsafe fn dismantle(o: *mut Obj, depth: usize, deferred: &mut Vec<*mut Obj>, heap: *mut Heap) {
    unsafe {
        // A shared object reaches here with its count's own bits spent.
        debug_assert!(((*o).rc == SHARED || (*o).rc == 1) && (*o).kind != KIND_DEAD);
        (*o).rc = 0;
        if (*o).kind == KIND_BRIDGE {
            std::ptr::drop_in_place(bridge_slot(o));
        }
        for i in child_ranges(o)
            .iter()
            .flat_map(|(first, last)| *first..*last)
        {
            if !counted(o, i) {
                continue;
            }
            let c = word_at(o, i);
            if is_imm(c) || c == 0 {
                continue;
            }
            let co = obj(c);
            let rc = count(co).load(Relaxed);
            if rc == IMMORTAL {
                continue;
            }
            debug_assert!((*co).kind != KIND_DEAD, "a dead object was released again");
            let last = if rc >= SHARED {
                let was = count(co).fetch_sub(1, Release) == SHARED | 1;
                if was {
                    std::sync::atomic::fence(Acquire);
                }
                was
            } else if rc > 1 {
                (*co).rc = rc - 1;
                false
            } else {
                true
            };
            if !last {
                continue;
            }
            if depth < DISMANTLE_DEPTH {
                dismantle(co, depth + 1, deferred, heap);
            } else {
                deferred.push(co);
            }
        }
        recycle(o, heap);
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
                KIND_RECORD | KIND_CTOR | KIND_LEAF | KIND_BRANCH | KIND_ARRAY => {
                    for i in 0..(*o).len as usize {
                        if counted(o, i) {
                            pending.push(word_at(o, i));
                        }
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
                KIND_RECORD | KIND_CTOR | KIND_LEAF | KIND_BRANCH | KIND_ARRAY => {
                    for i in 0..(*o).len as usize {
                        if counted(o, i) {
                            pending.push(word_at(o, i));
                        }
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
                    // A continuation's captures are immediates naming a body its entry alone holds.
                    if crate::detached::is_continuation(word_at(o, CLOSURE_CODE) as usize) {
                        return false;
                    }
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

/// Whether `w` reaches cell `slot`, within the budget `rc::value_reaches_cell` spends.
pub fn reaches_cell(w: Word, slot: ply_eval::arena::Slot) -> bool {
    let mut budget = ply_eval::rc::CYCLE_WALK_BUDGET;
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
                KIND_RECORD | KIND_CTOR | KIND_LEAF | KIND_BRANCH | KIND_ARRAY => {
                    for i in 0..(*o).len as usize {
                        if counted(o, i) {
                            pending.push(word_at(o, i));
                        }
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
    !is_imm(w) && count(obj(w)).load(Relaxed) == 1
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

/// `Value`'s rank of a variant (its `discriminant`), which orders values of different kinds.
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
            KIND_FLOAT => 3,
            KIND_STR => 5,
            KIND_BYTES => 6,
            KIND_LIST => 7,
            KIND_MAP => 8,
            KIND_RECORD => 9,
            KIND_CTOR => 10,
            KIND_CLOSURE => 11,
            KIND_ARRAY => 17,
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
                Value::Chan(_) => 18,
                Value::Secret(_) => 14,
                Value::Fixed(_) => 15,
                Value::Char(_) => 16,
                Value::Array(_) => 17,
            },
            other => panic!("a word of kind {other} was ordered after its object died"),
        }
    }
}

/// `Value::cmp` over words, so a map crosses the seam in `Value`'s key order.
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
            KIND_FLOAT => ply_eval::float::total_cmp(
                f64::from_bits(word_at(obj(a), 0) as u64),
                f64::from_bits(word_at(obj(b), 0) as u64),
            ),
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
            KIND_ARRAY => {
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
                let (xs, ys) = (map::to_vec(obj(a)), map::to_vec(obj(b)));
                for ((xk, xv), (yk, yv)) in xs.iter().zip(&ys) {
                    let c = cmp_words(layouts, *xk, *yk).then_with(|| cmp_words(layouts, *xv, *yv));
                    if c != Ordering::Equal {
                        return c;
                    }
                }
                xs.len().cmp(&ys.len())
            }
            KIND_RECORD
                if layouts.holds_floats((*obj(a)).layout)
                    || layouts.holds_floats((*obj(b)).layout) =>
            {
                Heap::to_value(layouts, a).cmp(&Heap::to_value(layouts, b))
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
                if let Some((ka, kb)) = crate::rt::keys_of(layouts, a, b) {
                    let c = cmp_words(layouts, ka, kb);
                    dec(ka);
                    dec(kb);
                    return c;
                }
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
            KIND_BRIDGE => matches!(
                bridged(o),
                Value::Str(_) | Value::Bytes(_) | Value::Fixed(_)
            ),
            KIND_RECORD | KIND_CTOR | KIND_LEAF | KIND_BRANCH | KIND_ARRAY => {
                ((*o).kind != KIND_RECORD || (*o).flags & MIXED == 0)
                    && (0..(*o).len as usize).all(|i| native_key(word_at(o, i)))
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
