//! The value model compiled code runs on: one word per value, an `Int` immediate (low bit set)
//! or a pointer to a counted [`Obj`]; anything not laid out natively is bridged as a [`Value`].

use crate::blocks::{self, Blocks};
use crate::list;
use crate::map;
use ply_core::{Carry, Closure, ClosureKind, CtorCarries, Fields, Fixed, Symbol, Value};
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
/// A map's handle: one word, its tree's root or zero, and its tree's height in `layout`.
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
/// What [`kind`] answers for a `Char`'s word: no object has it.
pub const KIND_CHAR: u8 = 17;
/// A width of 64 or 128 bits: its bits in one payload word or two, `layout` the width's place in
/// [`ply_core::INT_TYPES`].
pub const KIND_FIXED: u8 = 18;
/// A `Decimal`: its 16 bytes, as `rust_decimal` serializes it, in two words.
pub const KIND_DECIMAL: u8 = 19;
/// A `Columns`: an array per field of its records (`crate::columns`).
pub const KIND_COLUMNS: u8 = 20;
/// A block emitted code made and lays out itself (`crate::emitted`): the runtime reads its count at
/// offset 0 and nothing else of it, and every walker of the runtime's own objects refuses it.
pub const KIND_REFUSED: u8 = 21;
pub const KIND_DEAD: u8 = 255;
/// How many kinds [`Heap::allocated_by_kind`] tells apart: every `KIND_*` an object is made with.
pub const KINDS: usize = 32;

/// What objects of `kind` are, as a census of them is told.
pub fn kind_name(kind: u8) -> String {
    match kind {
        KIND_UNIT => "units",
        KIND_BOOL => "Bools",
        KIND_INT => "boxed Ints",
        KIND_RECORD => "records",
        KIND_CTOR => "constructors",
        KIND_LIST => "lists",
        KIND_CLOSURE => "closures",
        KIND_BRIDGE => "bridges",
        KIND_MAP => "maps",
        KIND_STR => "strings",
        KIND_BYTES => "byte strings",
        KIND_LEAF => "list leaves",
        KIND_BRANCH => "list branches",
        KIND_MLEAF => "map leaves",
        KIND_MBRANCH => "map branches",
        KIND_ARRAY => "arrays",
        KIND_FLOAT => "Floats",
        KIND_FIXED => "fixed-width integers",
        KIND_DECIMAL => "Decimals",
        KIND_COLUMNS => "columns",
        KIND_REFUSED => "emitted blocks",
        _ => return format!("kind {kind}"),
    }
    .to_string()
}

/// The walker `walker`, handed a block emitted code laid out, which no walker reads.
#[cold]
#[inline(never)]
pub(crate) fn refused(walker: &str) -> ! {
    panic!("`{walker}` was handed a block emitted code laid out, which the runtime does not walk")
}

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
/// A record holding both words it counts and words it does not: a bitmap after its words says which.
pub const MIXED: u8 = 2;
/// A record holding a `Float`'s bits or a record in place: not every word of it is a value.
pub const RAW: u8 = 4;
/// A string or bytes value whose bytes are another's: its first word is that value, held, and its
/// second where its bytes begin there. It has no room to grow in place. The value it views counts
/// its views in `aux`, so it still grows in place where they are its only other holders.
pub const VIEW: u8 = 8;

/// The fewest bytes a slice holds before it is a view of its value rather than a copy: below it a
/// copy costs no more than the view's own object, and keeps nothing larger alive.
const VIEW_MIN: usize = 64;

/// `len`: payload words, byte count, list length or map entries; `layout`: shape, ctor index,
/// capacity, arity, dropped list prefix or map height; `flags`/`aux`: a `Bool`'s value, a list's
/// tail length and capacity.
#[repr(C, align(8))]
pub struct Obj {
    pub rc: u32,
    pub kind: u8,
    pub flags: u8,
    pub aux: u16,
    pub len: u32,
    pub layout: u32,
}

/// A field of an object's header that holds a count, named as [`Obj`] and the C prelude's `PlyObj`
/// name it: the field a kind keeps one of its counts in, which code the emitter writes reads by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Header {
    Flags,
    Aux,
    Layout,
}

impl Header {
    pub fn name(self) -> &'static str {
        match self {
            Header::Flags => "flags",
            Header::Aux => "aux",
            Header::Layout => "layout",
        }
    }

    /// The count `o` holds in this field.
    ///
    /// # Safety
    /// `o` points at a live object.
    pub unsafe fn of(self, o: *mut Obj) -> usize {
        unsafe {
            match self {
                Header::Flags => (*o).flags as usize,
                Header::Aux => (*o).aux as usize,
                Header::Layout => (*o).layout as usize,
            }
        }
    }

    /// `o` holding `v` in this field, cut to the field's width.
    ///
    /// # Safety
    /// `o` points at a live object.
    pub unsafe fn set(self, o: *mut Obj, v: usize) {
        unsafe {
            match self {
                Header::Flags => (*o).flags = v as u8,
                Header::Aux => (*o).aux = v as u16,
                Header::Layout => (*o).layout = v as u32,
            }
        }
    }
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

/// Whether `w` is held in the word itself: an `Int` (low bit set) or a `Char` (low bits `010`).
/// An object's address is a multiple of eight.
#[inline]
pub fn is_imm(w: Word) -> bool {
    w & 7 != 0
}

/// No object lies below this address, so a word under it is a value of its own: 0, or a one-word
/// sum's nullary value `8, 16, ..`, as emitted code lays them (`layout.pointer_floor`).
pub const POINTER_FLOOR: Word = 4096;

/// Whether `w` is an object's address: a multiple of eight at or above [`POINTER_FLOOR`].
#[inline]
pub fn is_object(w: Word) -> bool {
    !is_imm(w) && w >= POINTER_FLOOR
}

/// Whether `w` is an `Int` held in the word.
#[inline]
pub fn is_int(w: Word) -> bool {
    w & 1 == 1
}

/// Whether `w` is a `Char` held in the word: its code point above the tag `010`.
#[inline]
pub fn is_char(w: Word) -> bool {
    w & 7 == 2
}

/// A `Char`'s word.
#[inline]
pub fn char_word(c: char) -> Word {
    (c as i64) << 3 | 2
}

/// The `Char` a `Char`'s word holds.
#[inline]
pub fn char_of(w: Word) -> char {
    char::from_u32((w >> 3) as u32).unwrap_or('\u{fffd}')
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
    use super::{IMMORTAL, KIND_DEAD, Obj, Word, is_object};

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
        if !is_object(w) {
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

/// Whether word `i` of `o` is one it counts: every word of an object but a flat record's or
/// array's, and a mixed one's where its tail says so. Their other words are a `Float`'s bits, a
/// width's, immediates or the header of a record held in place.
#[inline]
pub unsafe fn counted(o: *mut Obj, i: usize) -> bool {
    unsafe {
        if (*o).kind != KIND_RECORD && (*o).kind != KIND_ARRAY && (*o).kind != KIND_CTOR {
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

/// How a record holds a field: a word, an `Int`'s 64 bits (a width below 64 bits is held as the
/// `Int` it stands for), a `Float`'s bits, a 64-bit width's bits (the width's place in
/// [`ply_core::INT_TYPES`]), a `Decimal`'s two words, or a record of the shape named in place, its
/// header and words inside the record's own.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Slot {
    Word,
    Int,
    Float,
    Wide(u8),
    Decimal,
    Record(u32),
    /// An `Option` as its payload's word, counted, and zero for `None`.
    Maybe,
}

impl Slot {
    /// The words a field held as its scalar's own takes: the payload of the object it would be.
    pub fn raw_words(self) -> usize {
        match self {
            Slot::Decimal => 2,
            _ => 1,
        }
    }

    /// Whether a field of the slot is a scalar held as its own words: an `Int`, a `Float`, a
    /// 64-bit width or a `Decimal`.
    pub fn is_raw(self) -> bool {
        matches!(
            self,
            Slot::Int | Slot::Float | Slot::Wide(_) | Slot::Decimal
        )
    }
}

/// What a field a record holds in place as a record is spelled with: after its name, and its shape's
/// fields after that, between parentheses and parted by commas.
pub const RECORD_SLOT: &str = "!r(";

/// A field held as a word, or as its scalar's own words, as a shape spells it: a scalar's spelling
/// after its name and `!`, below every byte a name holds, so a shape sorts as its names do.
pub fn spelled(name: &Symbol, slot: Slot) -> Symbol {
    let spelling = match slot {
        Slot::Int => "int",
        Slot::Float => "f",
        Slot::Wide(t) if t == ply_core::IntTy::I64 as u8 => "i64",
        Slot::Wide(_) => "u64",
        Slot::Decimal => "d",
        Slot::Maybe => "m",
        Slot::Word | Slot::Record(_) => return name.clone(),
    };
    Symbol::new(format!("{}!{spelling}", name.as_str()))
}

/// The scalar a field spelled `name!<spelling>` holds as its own words.
fn raw_of(spelling: &str) -> Option<Slot> {
    match spelling {
        "int" => Some(Slot::Int),
        "f" => Some(Slot::Float),
        "u64" => Some(Slot::Wide(ply_core::IntTy::U64 as u8)),
        "i64" => Some(Slot::Wide(ply_core::IntTy::I64 as u8)),
        "d" => Some(Slot::Decimal),
        "m" => Some(Slot::Maybe),
        _ => None,
    }
}

/// The `Decimal` two words of `o` from `at` hold.
pub unsafe fn decimal_at(o: *mut Obj, at: usize) -> ply_core::Decimal {
    let mut bytes = [0u8; 16];
    unsafe {
        bytes[..8].copy_from_slice(&word_at(o, at).to_le_bytes());
        bytes[8..].copy_from_slice(&word_at(o, at + 1).to_le_bytes());
    }
    ply_core::Decimal::deserialize(bytes)
}

/// The scalar field `at` of `o` holds as its own words, as a value.
unsafe fn raw_value(o: *mut Obj, at: usize, slot: Slot) -> Value {
    unsafe {
        match slot {
            Slot::Int => Value::Int(word_at(o, at) as i64),
            Slot::Float => Value::Float(f64::from_bits(word_at(o, at) as u64)),
            Slot::Wide(t) => Value::Fixed(Fixed::new(
                ply_core::INT_TYPES[t as usize],
                word_at(o, at) as u64 as u128,
            )),
            Slot::Decimal => Value::Decimal(decimal_at(o, at)),
            Slot::Word | Slot::Record(_) | Slot::Maybe => {
                unreachable!("a field held as a word, a record or an option")
            }
        }
    }
}

/// Copies scalar object `w`'s payload into the words of `o` from `at`, and lets `w` go: for an
/// `Int`, its 64 bits whether its word holds them or boxes them; for an `Option`, its payload's
/// word with a count of its own, or zero.
pub unsafe fn hold_raw(o: *mut Obj, at: usize, slot: Slot, w: Word) {
    unsafe {
        if slot == Slot::Int {
            set_word(o, at, as_int(w).unwrap_or(0) as Word);
        } else if slot == Slot::Maybe {
            let held = if (*obj(w)).len == 1 {
                let p = word_at(obj(w), 0);
                inc(p);
                p
            } else {
                0
            };
            set_word(o, at, held);
        } else {
            for i in 0..slot.raw_words() {
                set_word(o, at + i, word_at(obj(w), i));
            }
        }
    }
    dec(w);
}

/// The raw slot a record or an array holds a scalar object `w` as, where its unit's code reads one so.
pub fn raw_slot_of(layouts: &Layouts, w: Word) -> Option<Slot> {
    match kind(w) {
        KIND_INT if layouts.layout.raw_ints => Some(Slot::Int),
        KIND_FLOAT => Some(Slot::Float),
        KIND_FIXED if unsafe { (*obj(w)).len } == 1 => {
            Some(Slot::Wide(unsafe { (*obj(w)).layout } as u8))
        }
        KIND_DECIMAL if layouts.layout.raw_decimals => Some(Slot::Decimal),
        KIND_CTOR if layouts.layout.option_words && is_option(layouts, w) => Some(Slot::Maybe),
        _ => None,
    }
}

/// Whether `w` is the prelude's `Some` of one argument or its `None`.
pub fn is_option(layouts: &Layouts, w: Word) -> bool {
    let o = obj(w);
    let (layout, len) = unsafe { ((*o).layout, (*o).len) };
    (len == 1 && layouts.some == Some(layout)) || (len == 0 && layouts.none == Some(layout))
}

/// The `Option` object an `Option` held as its payload's word stands for, holding the payload once
/// more.
pub fn option_word(heap: &mut Heap, layouts: &Layouts, w: Word) -> Word {
    match (w, layouts.some, layouts.none) {
        // As the unit's own `None` is: no arguments and no flags.
        (0, _, Some(none)) => heap.alloc(KIND_CTOR, 0, 0, none) as Word,
        (_, Some(some), _) if w != 0 => {
            inc(w);
            heap.ctor(layouts, some, &[w])
        }
        _ => unreachable!("an `Option` held as its payload's word in a program with no `Option`"),
    }
}

/// The `Option` an `Option` held as its payload's word stands for, as a value.
fn option_value(layouts: &Layouts, payload: Option<Value>) -> Value {
    let name = |index: Option<u32>, bare: &str| match index {
        Some(i) => layouts.ctors[i as usize].0.clone(),
        None => Symbol::new(bare),
    };
    match payload {
        Some(v) => Value::ctor(name(layouts.some, "Some"), vec![v]),
        None => Value::ctor(name(layouts.none, "None"), vec![]),
    }
}

/// The width a 64- or 128-bit width's object holds, and its value.
pub unsafe fn fixed_of(o: *mut Obj) -> Fixed {
    unsafe {
        let ty = ply_core::INT_TYPES[(*o).layout as usize];
        let low = word_at(o, 0) as u64 as u128;
        let high = if (*o).len > 1 {
            (word_at(o, 1) as u64 as u128) << 64
        } else {
            0
        };
        Fixed::new(ty, high | low)
    }
}

/// A field held in place as a record of fields `nested` spells, as a shape spells it.
pub fn spelled_record(name: &Symbol, nested: &[Symbol]) -> Symbol {
    let inner: Vec<&str> = nested.iter().map(Symbol::as_str).collect();
    Symbol::new(format!(
        "{}{RECORD_SLOT}{})",
        name.as_str(),
        inner.join(",")
    ))
}

/// A nested shape's fields, parted at the commas outside parentheses.
fn split_fields(s: &str) -> Vec<Symbol> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0usize, 0usize);
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(Symbol::new(&s[start..i]));
                start = i + 1;
            }
            _ => {}
        }
    }
    if !s.is_empty() {
        out.push(Symbol::new(&s[start..]));
    }
    out
}

/// What a record's count holds where the record is held in place inside another: these two top
/// bits set, and below them how many words before it the header of the record holding it starts.
pub const EMBEDDED: u32 = 0xC000_0000;

/// The record holding `o` in place, where `o` is held in place.
#[inline]
pub unsafe fn holder(o: *mut Obj) -> Option<*mut Obj> {
    let rc = unsafe { (*o).rc };
    (rc & EMBEDDED == EMBEDDED && rc != IMMORTAL)
        .then(|| unsafe { (o as *mut Word).sub((rc & !EMBEDDED) as usize) as *mut Obj })
}

/// The object whose count stands for `o`'s: `o`, or the outermost record holding it in place.
#[inline]
pub unsafe fn counted_by(mut o: *mut Obj) -> *mut Obj {
    while let Some(h) = unsafe { holder(o) } {
        o = h;
    }
    o
}

/// A record's flags: flat when it counts none of its words, mixed when it counts some and holds a
/// tail saying which, raw when it holds words that are not values.
pub fn record_flags(width: usize, counted: &[usize], tail: usize, raw: bool) -> u8 {
    let raw = if raw { RAW } else { 0 };
    if width > 0 && counted.is_empty() {
        FLAT | raw
    } else if tail > 0 {
        MIXED | raw
    } else {
        raw
    }
}

/// Writes the bitmap of a mixed record's `tail` words after its `width`, a bit set for each
/// word it counts.
pub(crate) unsafe fn write_tail(o: *mut Obj, width: usize, tail: usize, counted: &[usize]) {
    for t in 0..tail {
        let bits = counted
            .iter()
            .filter(|i| **i / 64 == t)
            .fold(0u64, |acc, i| acc | 1 << (i % 64));
        unsafe { set_word(o, width + t, bits as i64) };
    }
}

/// Copies record `w`, of shape `shape`, into `o` at word `at`, header and all, holding what it
/// counts once more, and lets `w` go. The tail its shape reserves is written whether or not `w`
/// holds one: a flat record built where its fields' types were known has none.
pub(crate) unsafe fn embed(layouts: &Layouts, o: *mut Obj, at: usize, shape: u32, w: Word) {
    unsafe {
        let src = obj(w);
        let dst = words(o).add(at) as *mut Obj;
        let (flags, len) = ((*src).flags, (*src).len as usize);
        std::ptr::copy_nonoverlapping(words(src), words(dst), len);
        let (_, tail) = layouts.shape_words(shape);
        if tail > 0 {
            if flags & MIXED != 0 {
                std::ptr::copy_nonoverlapping(words(src).add(len), words(dst).add(len), tail);
            } else {
                write_tail(dst, len, tail, &layouts.shape_counted(shape));
            }
        }
        for i in 0..len {
            if counted(src, i) {
                inc(word_at(src, i));
            }
        }
        std::ptr::write(
            dst,
            Obj {
                rc: EMBEDDED | (HEADER / 8 + at) as u32,
                kind: KIND_RECORD,
                flags,
                aux: 0,
                len: len as u32,
                layout: shape,
            },
        );
        dec(w);
    }
}

/// Lets go of what the record held in place `at` words into `o` counts, before another is written
/// over it.
pub(crate) unsafe fn drop_embedded(o: *mut Obj, at: usize) {
    unsafe {
        let e = words(o).add(at) as *mut Obj;
        for i in 0..(*e).len as usize {
            if counted(e, i) {
                dec(word_at(e, i));
            }
        }
    }
}

/// The word a part `o` holds at `at` as `slot` is, as the object holds it: none for a scalar held
/// as its own words, and for a record held in place the address of its header there.
pub unsafe fn part_word(o: *mut Obj, at: usize, slot: Slot) -> Option<Word> {
    unsafe {
        match slot {
            Slot::Word => Some(word_at(o, at)),
            Slot::Record(_) => Some(words(o).add(at) as Word),
            Slot::Int | Slot::Float | Slot::Wide(_) | Slot::Decimal | Slot::Maybe => None,
        }
    }
}

/// The word a record holds field `at`, of `slot`, as, held once more: a `Float` boxed, an `Option`
/// made the object it stands for, and a record held in place as the address of its header there.
pub fn field_word(heap: &mut Heap, layouts: &Layouts, o: *mut Obj, at: usize, slot: Slot) -> Word {
    let w = unsafe { word_at(o, at) };
    match slot {
        Slot::Maybe => option_word(heap, layouts, w),
        Slot::Int => heap.boxed_int(w as i64),
        Slot::Float | Slot::Wide(_) | Slot::Decimal => {
            let (kind, layout) = match slot {
                Slot::Wide(t) => (KIND_FIXED, u32::from(t)),
                Slot::Decimal => (KIND_DECIMAL, 0),
                _ => (KIND_FLOAT, 0),
            };
            let b = heap.alloc(kind, 0, slot.raw_words() as u32, layout);
            for i in 0..slot.raw_words() {
                unsafe { set_word(b, i, word_at(o, at + i)) };
            }
            b as Word
        }
        Slot::Record(_) => {
            let inner = unsafe { words(o).add(at) } as Word;
            inc(inner);
            inner
        }
        Slot::Word => {
            inc(w);
            w
        }
    }
}

/// A closure's payload: its compiled code's address first, then the captured values.
pub const CLOSURE_CODE: usize = 0;
pub const CLOSURE_CAPTURES: usize = 1;
/// A closure flag: its first capture is the immediate address of the descriptor compiled code
/// reads to call it by its type.
pub const CLOSURE_DESCRIBED: u8 = 16;

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
    unsafe {
        if (*o).flags & VIEW != 0 {
            bytes_ptr(obj(word_at(o, 0))).add(word_at(o, 1) as usize)
        } else {
            (o as *mut u8).add(HEADER)
        }
    }
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

/// Record shapes: a shape is its sorted fields, each spelled with how the record holds it. A field
/// takes a word, or a record held in place takes its header's two words, its own and its tail's.
#[derive(Default)]
pub struct Shapes {
    ids: HashMap<Arc<[Symbol]>, u32>,
    spelled: Vec<Arc<[Symbol]>>,
    names: Vec<Arc<[Symbol]>>,
    slots: Vec<Arc<[Slot]>>,
    /// Where each field's words start.
    offsets: Vec<Arc<[usize]>>,
    /// The words a record of the shape holds, its tail apart.
    widths: Vec<usize>,
    /// The words after them a mixed record of the shape holds its bitmap in.
    tails: Vec<usize>,
    /// Which of its words a record of the shape counts.
    counted: Vec<Arc<[usize]>>,
}

impl Shapes {
    /// A shape held in place by another is interned first, so it has the lower id.
    fn intern(&mut self, mut fields: Vec<Symbol>) -> u32 {
        fields.sort();
        if let Some(id) = self.ids.get(fields.as_slice()) {
            return *id;
        }
        let mut names = Vec::with_capacity(fields.len());
        let mut slots = Vec::with_capacity(fields.len());
        for field in &fields {
            let s = field.as_str();
            match s.find(RECORD_SLOT).filter(|_| s.ends_with(')')) {
                Some(open) => {
                    let inner = &s[open + RECORD_SLOT.len()..s.len() - 1];
                    let nested = self.intern(split_fields(inner));
                    names.push(Symbol::new(&s[..open]));
                    slots.push(Slot::Record(nested));
                }
                None => match s.rsplit_once('!').and_then(|(n, r)| Some((n, raw_of(r)?))) {
                    Some((name, slot)) => {
                        names.push(Symbol::new(name));
                        slots.push(slot);
                    }
                    None => {
                        names.push(field.clone());
                        slots.push(Slot::Word);
                    }
                },
            }
        }
        let mut offsets = Vec::with_capacity(slots.len());
        let mut counted = Vec::new();
        let mut at = 0;
        for slot in &slots {
            offsets.push(at);
            match slot {
                Slot::Word => {
                    counted.push(at);
                    at += 1;
                }
                Slot::Int | Slot::Float | Slot::Wide(_) | Slot::Decimal => at += slot.raw_words(),
                Slot::Maybe => {
                    counted.push(at);
                    at += 1;
                }
                Slot::Record(n) => {
                    let n = *n as usize;
                    counted.extend(self.counted[n].iter().map(|c| at + 2 + c));
                    at += 2 + self.widths[n] + self.tails[n];
                }
            }
        }
        let mixed = !counted.is_empty() && counted.len() < at;
        let id = self.spelled.len() as u32;
        let rc: Arc<[Symbol]> = Arc::from(fields);
        self.ids.insert(rc.clone(), id);
        self.spelled.push(rc);
        self.names.push(Arc::from(names));
        self.slots.push(Arc::from(slots));
        self.offsets.push(Arc::from(offsets));
        self.widths.push(at);
        self.tails.push(if mixed { tail_words(at) } else { 0 });
        self.counted.push(Arc::from(counted));
        id
    }
}

/// Record shapes and constructors by index, shared by the compiler and the running entry, and by
/// the threads a `parallel` block's branches run on, any of which may intern a shape.
pub struct Layouts {
    shapes: std::sync::RwLock<Shapes>,
    pub ctors: Vec<(Symbol, usize)>,
    ctor_ids: HashMap<Symbol, u32>,
    /// Per constructor, the shape its arguments are laid out as, its fields `a000`, `a001`, ... in
    /// order; none where it holds every argument as its word.
    ctor_shapes: Box<[Option<u32>]>,
    /// The prelude constructors the runtime builds itself, resolved once.
    pub some: Option<u32>,
    pub none: Option<u32>,
    pub ok: Option<u32>,
    pub err: Option<u32>,
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
    /// Per constructor, the arithmetic its type's module states, by [`ply_core::instances`]'
    /// roles; empty for a unit that states none.
    numeric: Box<[[Stated; 5]]>,
    /// How a record the runtime makes is laid out: as the unit's code reads one.
    pub layout: RecordLayout,
}

/// How a unit's code reads what a layout change moved, which the runtime lays out every value it
/// makes as. A unit emitted by a `ply` before the change, as a bootstrap's first builder is, reads
/// it the older way.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RecordLayout {
    /// An `Int` held as its own 64 bits, rather than by its word.
    pub raw_ints: bool,
    /// A `Decimal` held as its own two words, rather than by its word.
    pub raw_decimals: bool,
    /// A long slice of a bytes value a view of its bytes, rather than a copy of them.
    pub bytes_views: bool,
    /// An `Option` held as its payload's word, rather than by its word.
    pub option_words: bool,
}

impl RecordLayout {
    /// What this runtime's own emitter writes.
    pub const CURRENT: RecordLayout = RecordLayout {
        raw_ints: true,
        raw_decimals: true,
        bytes_views: true,
        option_words: true,
    };
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
/// A field past the offsets a row holds, which is searched for.
const SEARCHED: u16 = u16::MAX - 1;

impl Layouts {
    pub fn new(ctors: Vec<(Symbol, usize)>) -> Layouts {
        Layouts::of(ctors, &[], RecordLayout::CURRENT)
    }

    /// The fields of a `{key, value}` entry.
    pub fn entry_fields() -> Vec<Symbol> {
        vec![Symbol::new("key"), Symbol::new("value")]
    }

    /// With `shapes` interned first, in order, so each one's id is its position.
    pub fn of(
        ctors: Vec<(Symbol, usize)>,
        shapes: &[Vec<Symbol>],
        layout: RecordLayout,
    ) -> Layouts {
        let ctor_ids: HashMap<Symbol, u32> = ctors
            .iter()
            .enumerate()
            .map(|(i, (n, _))| (n.clone(), i as u32))
            .collect();
        let by = |name: &str| ctor_ids.get(&Symbol::new(name)).copied();
        let (some, none, stop, go) = (by("Some"), by("None"), by("Stop"), by("Continue"));
        let (ok, err) = (by("Ok"), by("Err"));
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
            ok,
            err,
            stop,
            go,
            less,
            equal,
            greater,
            rows: Box::default(),
            width: 0,
            stated: Box::default(),
            numeric: Box::default(),
            ctor_shapes: Box::default(),
            layout,
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

    /// Where a record, a constructor or an array holds each of its parts, in order, and how; none
    /// for any other object.
    pub fn parts(&self, o: *mut Obj) -> Vec<(usize, Slot)> {
        unsafe {
            match (*o).kind {
                KIND_RECORD => {
                    let shape = (*o).layout;
                    let offsets = self.shape_offsets(shape);
                    let slots = self.shape_slots(shape);
                    offsets.iter().copied().zip(slots.iter().copied()).collect()
                }
                KIND_CTOR => (0..self.ctor_arity(o))
                    .map(|i| self.ctor_arg(o, i))
                    .collect(),
                KIND_ARRAY => {
                    let (slot, stride) = held_as(self, o);
                    (0..crate::array::len(o))
                        .map(|k| (k * stride, slot))
                        .collect()
                }
                KIND_REFUSED => refused("Layouts::parts"),
                _ => Vec::new(),
            }
        }
    }

    pub fn ctor_index(&self, name: &Symbol) -> Option<u32> {
        self.ctor_ids.get(name).copied()
    }

    /// Lays each constructor's arguments out as `args` spells them, one list per constructor in
    /// the table's order; an empty one holds every argument as its word.
    pub fn set_ctor_args(&mut self, args: &[Vec<Symbol>]) {
        self.ctor_shapes = args
            .iter()
            .map(|spelled| (!spelled.is_empty()).then(|| self.shape(spelled.clone())))
            .collect();
    }

    /// The shape constructor `index` lays its arguments out as, where it holds one by type.
    #[inline]
    pub fn ctor_shape(&self, index: u32) -> Option<u32> {
        self.ctor_shapes.get(index as usize).copied().flatten()
    }

    /// Where argument `i` of constructor object `o` starts among its words, and how it holds it.
    pub fn ctor_arg(&self, o: *mut Obj, i: usize) -> (usize, Slot) {
        match self.ctor_shape(unsafe { (*o).layout }) {
            Some(shape) => {
                let r = self.read();
                (r.offsets[shape as usize][i], r.slots[shape as usize][i])
            }
            None => (i, Slot::Word),
        }
    }

    /// How many arguments constructor object `o` holds.
    pub fn ctor_arity(&self, o: *mut Obj) -> usize {
        match self.ctor_shape(unsafe { (*o).layout }) {
            Some(_) => self.ctors[unsafe { (*o).layout } as usize].1,
            None => (unsafe { (*o).len }) as usize,
        }
    }

    pub fn index_fields(&mut self, names: &[Symbol]) {
        let mut indices: HashMap<&Symbol, Vec<usize>> = HashMap::with_capacity(names.len());
        for (index, name) in names.iter().enumerate() {
            indices.entry(name).or_default().push(index);
        }
        let interned = self.shapes.get_mut().unwrap_or_else(|e| e.into_inner());
        let width = names.len();
        let mut rows = vec![NO_FIELD; interned.names.len() * width].into_boxed_slice();
        for (row, shape) in interned.names.iter().enumerate() {
            for (at, field) in shape.iter().enumerate() {
                for &index in indices.get(field).map_or(&[][..], Vec::as_slice) {
                    rows[row * width + index] = u16::try_from(interned.offsets[row][at])
                        .ok()
                        .filter(|o| *o < SEARCHED)
                        .unwrap_or(SEARCHED);
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
            Some(&NO_FIELD) => None,
            Some(&at) if at != SEARCHED => Some(at as usize),
            _ => self.offset(shape, &names[index]),
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

    /// Whether a shape's records hold anything but words: a `Float`'s bits, or a record in place.
    pub fn holds_raw(&self, shape: u32) -> bool {
        self.read().slots[shape as usize]
            .iter()
            .any(|s| *s != Slot::Word)
    }

    /// How many fields a shape has.
    pub fn shape_fields(&self, shape: u32) -> usize {
        self.read().names[shape as usize].len()
    }

    /// Where each of a shape's fields starts among its records' words.
    pub fn shape_offsets(&self, shape: u32) -> Arc<[usize]> {
        self.read().offsets[shape as usize].clone()
    }

    /// The words a record of the shape holds, and the tail after them a mixed one's bitmap takes.
    pub fn shape_words(&self, shape: u32) -> (usize, usize) {
        let r = self.read();
        (r.widths[shape as usize], r.tails[shape as usize])
    }

    /// Which of its words a record of the shape counts.
    pub fn shape_counted(&self, shape: u32) -> Arc<[usize]> {
        self.read().counted[shape as usize].clone()
    }

    /// How a record of `shape` holds the field starting at word `at`.
    pub fn slot_at(&self, shape: u32, at: usize) -> Slot {
        let r = self.read();
        let i = r.offsets[shape as usize].partition_point(|o| *o < at);
        r.slots[shape as usize][i]
    }

    /// Where field `name` of `shape` starts among a record's words.
    pub fn offset(&self, shape: u32, name: &Symbol) -> Option<usize> {
        let r = self.read();
        let at = r.names[shape as usize].binary_search(name).ok()?;
        Some(r.offsets[shape as usize][at])
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
    by_kind: [usize; KINDS],
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

/// The live objects of `blocks` tallied under what `key` makes of each one's kind, `layout` and
/// payload bytes, with their count and payload bytes: for a block emitted code laid out, what its
/// block holds past the header.
pub(crate) fn live_in<K: Ord>(
    blocks: &Blocks,
    key: impl Fn(u8, u32, usize) -> K,
) -> std::collections::BTreeMap<K, (usize, usize)> {
    let mut tally: std::collections::BTreeMap<K, (usize, usize)> = Default::default();
    blocks.each(|block, bytes| {
        let o: *mut Obj = block.cast();
        let (kind, rc, layout) = unsafe { ((*o).kind, (*o).rc, (*o).layout) };
        if kind == KIND_DEAD || rc == 0 {
            return;
        }
        let size = match kind {
            KIND_REFUSED => bytes - HEADER,
            // Unsized: the header alone.
            _ => match unsafe { payload_bytes(o) } {
                usize::MAX => HEADER,
                size => size,
            },
        };
        let e = tally.entry(key(kind, layout, size)).or_insert((0, 0));
        e.0 += 1;
        e.1 += size;
    });
    tally
}

/// The live objects of `blocks` by what their headers say they are, as [`Heap::census`] reads them.
pub(crate) fn census_in(blocks: &Blocks) -> Vec<((u8, u32), usize, usize)> {
    let mut rows: Vec<((u8, u32), usize, usize)> =
        live_in(blocks, |kind, layout, size| match kind {
            KIND_RECORD | KIND_CTOR => (kind, layout),
            KIND_REFUSED => (kind, u32::try_from(size).unwrap_or(u32::MAX)),
            _ => (kind, 0),
        })
        .into_iter()
        .map(|(k, (n, b))| (k, n, b))
        .collect();
    rows.sort_by_key(|&(k, _, b)| (std::cmp::Reverse(b), k));
    rows
}

/// Two censuses as one, each sort's counts and bytes summed, the most bytes first.
pub(crate) fn census_merged(
    a: Vec<((u8, u32), usize, usize)>,
    b: Vec<((u8, u32), usize, usize)>,
) -> Vec<((u8, u32), usize, usize)> {
    let mut sums: std::collections::BTreeMap<(u8, u32), (usize, usize)> = Default::default();
    for (k, n, bytes) in a.into_iter().chain(b) {
        let e = sums.entry(k).or_insert((0, 0));
        e.0 += n;
        e.1 += bytes;
    }
    let mut rows: Vec<((u8, u32), usize, usize)> = sums
        .into_iter()
        .map(|(k, (n, bytes))| (k, n, bytes))
        .collect();
    rows.sort_by_key(|&(k, _, bytes)| (std::cmp::Reverse(bytes), k));
    rows
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

/// The payload bytes of an object [`Heap::alloc`] makes: its words, and after a mixed record's,
/// array's or constructor's words the bitmap saying which of them it counts.
fn words_bytes(kind: u8, flags: u8, len: u32) -> usize {
    let len = len as usize;
    let tailed = matches!(kind, KIND_RECORD | KIND_ARRAY | KIND_CTOR) && flags & MIXED != 0;
    8 * (len + if tailed { tail_words(len) } else { 0 })
}

/// The payload bytes an object was allocated with, from its header; `usize::MAX` if unsized.
unsafe fn payload_bytes(o: *mut Obj) -> usize {
    unsafe {
        match (*o).kind {
            KIND_RECORD | KIND_ARRAY | KIND_CTOR | KIND_CLOSURE | KIND_INT | KIND_FLOAT
            | KIND_FIXED | KIND_DECIMAL | KIND_COLUMNS => {
                words_bytes((*o).kind, (*o).flags, (*o).len)
            }
            KIND_LEAF | KIND_BRANCH => (*o).layout as usize * 8,
            KIND_LIST => (list::TAIL + list::cap(o)) * 8,
            KIND_MAP => 8,
            KIND_MLEAF => (*o).layout as usize * 16,
            KIND_MBRANCH => map::BRANCH_BYTES,
            KIND_STR | KIND_BYTES if (*o).flags & VIEW != 0 => 16,
            KIND_STR | KIND_BYTES => (*o).layout as usize,
            KIND_BRIDGE => BRIDGE_BYTES,
            KIND_REFUSED => refused("payload_bytes"),
            _ => usize::MAX,
        }
    }
}

/// A dead object's block, given back as [`give_back`] says.
unsafe fn recycle(o: *mut Obj, heap: *mut Heap) {
    unsafe {
        let size = payload_bytes(o);
        let bridge = (*o).kind == KIND_BRIDGE;
        (*o).kind = KIND_DEAD;
        if size == usize::MAX {
            return;
        }
        give_back(o, heap, Heap::object_size(size), bridge);
    }
}

/// Gives back a dead block of `object` bytes. Only the heap that owns the block may: its bridge
/// leaves the table first, so the bridge that takes the block next is not listed twice, and a
/// bridge another heap owns stays listed there, dead, until that heap's end.
unsafe fn give_back(o: *mut Obj, heap: *mut Heap, object: usize, bridge: bool) {
    unsafe {
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

/// Gives back a block whose last holder was emitted code, `payload` bytes past its header, without
/// a walk: emitted code let go of what it held. Freed so: a block of [`KIND_REFUSED`], a scalar's
/// box, a string or bytes value, and a list's or a map's own object or node, which the runtime and
/// emitted code both make.
pub(crate) unsafe fn free_sized(o: *mut Obj, heap: *mut Heap, payload: usize) {
    unsafe {
        let (kind, rc) = ((*o).kind, (*o).rc);
        debug_assert!(kind != KIND_DEAD, "{o:p} was freed after it died");
        debug_assert!(rc == 1 || rc == SHARED, "{o:p} was freed with holders left");
        debug_assert!(
            matches!(
                kind,
                KIND_REFUSED
                    | KIND_INT
                    | KIND_FLOAT
                    | KIND_FIXED
                    | KIND_DECIMAL
                    | KIND_STR
                    | KIND_BYTES
                    | KIND_LIST
                    | KIND_LEAF
                    | KIND_BRANCH
                    | KIND_MAP
                    | KIND_MLEAF
                    | KIND_MBRANCH
            ),
            "{o:p}, of kind {kind}, was freed by size, which lets go of nothing it holds"
        );
        let object = Heap::object_size(payload);
        #[cfg(debug_assertions)]
        sized_as(o, heap, object);
        (*o).rc = 0;
        (*o).kind = KIND_DEAD;
        give_back(o, heap, object, false);
    }
}

/// Panics where `o` was not allocated as `object` bytes, as far as its header or `heap`'s blocks
/// say: a block of [`KIND_REFUSED`] tells its size class alone, which is all a free relies on.
#[cfg(debug_assertions)]
unsafe fn sized_as(o: *mut Obj, heap: *mut Heap, object: usize) {
    unsafe {
        if (*o).kind != KIND_REFUSED {
            let had = Heap::object_size(payload_bytes(o));
            assert_eq!(
                had, object,
                "{o:p}, an object of {had} bytes, was freed as {object}"
            );
        }
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
/// it, and without a walk: a bridge's value is dropped, and a hold's or a continuation's words are
/// left unread, emitted code having let go of what a hold held.
pub(crate) unsafe fn free_handle(o: *mut Obj, heap: *mut Heap) {
    unsafe {
        debug_assert!(((*o).rc == SHARED || (*o).rc == 1) && (*o).kind != KIND_DEAD);
        match (*o).kind {
            KIND_BRIDGE => std::ptr::drop_in_place(bridge_slot(o)),
            KIND_CLOSURE
                if crate::rt::hold_id(o as Word).is_some()
                    || crate::detached::is_continuation(word_at(o, CLOSURE_CODE) as usize) => {}
            kind => {
                panic!("`rt_handle_free` was handed an object of kind {kind}, which is no handle")
            }
        }
        (*o).rc = 0;
        recycle(o, heap);
    }
}

/// The count of `o`, read as another thread may be changing it.
pub(crate) unsafe fn count_of(o: *mut Obj) -> u32 {
    count(o).load(Relaxed)
}

/// One holder fewer of a shared object whose count is its own: whether that was the last, its
/// memory then the caller's to release.
pub(crate) unsafe fn last_of_shared(o: *mut Obj) -> bool {
    let last = count(o).fetch_sub(1, Release) == SHARED | 1;
    if last {
        std::sync::atomic::fence(Acquire);
    }
    last
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
            by_kind: [0; KINDS],
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

    /// `blocks` in place of the blocks this heap takes from, which it answers.
    pub(crate) fn swap_blocks(&mut self, blocks: Blocks) -> Blocks {
        std::mem::replace(&mut self.blocks, blocks)
    }

    /// A heap whose entries never end: what outlives every entry lives here.
    pub fn persistent() -> Heap {
        let mut h = Heap::new();
        h.persistent = true;
        h
    }

    fn live_by<K: Ord>(
        &self,
        key: impl Fn(u8, u32, usize) -> K,
    ) -> std::collections::BTreeMap<K, (usize, usize)> {
        live_in(&self.blocks, key)
    }

    /// Live objects by kind, with their count and payload bytes.
    pub fn live_by_kind(&self) -> Vec<(u8, usize, usize)> {
        self.live_by(|kind, _, _| kind)
            .into_iter()
            .map(|(k, (n, b))| (k, n, b))
            .collect()
    }

    /// Live objects by what their headers say they are, the most bytes first, with their count and
    /// payload bytes: a record by its kind and shape, a constructor by its kind and index, a block
    /// emitted code laid out, whose header names no type, by its kind and payload bytes, and any
    /// other object by its kind and 0.
    pub fn census(&self) -> Vec<((u8, u32), usize, usize)> {
        census_in(&self.blocks)
    }

    /// How many objects are live.
    pub fn live(&self) -> usize {
        self.live_by_kind().iter().map(|(_, n, _)| n).sum()
    }

    /// The bytes of the blocks the entry's live objects take, read from the pages without a walk:
    /// a dead block waiting under `PLY_HEAP_DELAY` still counts.
    pub fn live_bytes(&self) -> usize {
        self.blocks.live()
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
    pub fn allocated_by_kind(&self) -> [usize; KINDS] {
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
        self.by_kind[kind as usize % KINDS] += 1;
        if self.census {
            let key = match kind {
                KIND_CTOR | KIND_RECORD => Some(layout),
                // Bytes arrive with `len` zero; the class is the capacity.
                KIND_BYTES | KIND_STR => Some((payload_bytes as u32).next_power_of_two()),
                KIND_REFUSED => Some(u32::try_from(payload_bytes).unwrap_or(u32::MAX)),
                _ => None,
            };
            if let Some(key) = key {
                *self.by_layout.entry((kind, key)).or_insert(0) += 1;
            }
        }
        p
    }

    /// A fresh object with `len` payload words, left as a recycled block's last tenant wrote them:
    /// fill every word before anything can release it, or [`dec`] follows garbage.
    pub fn alloc(&mut self, kind: u8, flags: u8, len: u32, layout: u32) -> *mut Obj {
        self.raw_alloc(kind, flags, len, layout, words_bytes(kind, flags, len))
    }

    /// A record of `fields`, which it takes, in the order of the names: laid out as a record of
    /// their type is wherever it is made, each scalar held as its bits and each record in place.
    pub fn record(&mut self, layouts: &Layouts, fields: &[(Symbol, Word)]) -> Word {
        let mut spelled_fields = Vec::with_capacity(fields.len());
        let mut words = Vec::with_capacity(fields.len());
        for (name, w) in fields {
            if let Some(slot) = raw_slot_of(layouts, *w) {
                spelled_fields.push(spelled(name, slot));
                words.push((*w, slot));
                continue;
            }
            match kind(*w) {
                KIND_RECORD => {
                    let shape = unsafe { (*obj(*w)).layout };
                    spelled_fields.push(spelled_record(name, &layouts.shape_spelled(shape)));
                    words.push((*w, Slot::Record(shape)));
                }
                _ => {
                    spelled_fields.push(name.clone());
                    words.push((*w, Slot::Word));
                }
            }
        }
        let shape = layouts.shape(spelled_fields);
        self.record_of(layouts, shape, &words)
    }

    /// A record of `shape` holding `words`, which it takes, one per field in the shape's order, each
    /// as the shape holds that field.
    pub fn record_shaped(&mut self, layouts: &Layouts, shape: u32, words: &[Word]) -> Word {
        let slots = layouts.shape_slots(shape);
        let held: Vec<(Word, Slot)> = words.iter().copied().zip(slots.iter().copied()).collect();
        self.record_of(layouts, shape, &held)
    }

    /// A record of `shape` holding `words`, which it takes, each as its slot says.
    fn record_of(&mut self, layouts: &Layouts, shape: u32, words: &[(Word, Slot)]) -> Word {
        self.laid_out(layouts, KIND_RECORD, shape, shape, words)
    }

    /// An object of `kind` whose header names `layout`, holding `words`, which it takes, as `shape`
    /// lays them out: a scalar a raw slot holds as its object's own words, and a record in place.
    fn laid_out(
        &mut self,
        layouts: &Layouts,
        kind: u8,
        layout: u32,
        shape: u32,
        words: &[(Word, Slot)],
    ) -> Word {
        let (width, tail) = layouts.shape_words(shape);
        let counted = layouts.shape_counted(shape);
        let o = self.alloc(
            kind,
            record_flags(
                width,
                &counted,
                tail,
                words.iter().any(|(_, s)| *s != Slot::Word),
            ),
            width as u32,
            layout,
        );
        let offsets = layouts.shape_offsets(shape);
        unsafe {
            for ((w, slot), at) in words.iter().zip(offsets.iter()) {
                match slot {
                    Slot::Record(inner) => embed(layouts, o, *at, *inner, *w),
                    Slot::Word => set_word(o, *at, *w),
                    raw => hold_raw(o, *at, *raw, *w),
                }
            }
            write_tail(o, width, tail, &counted);
        }
        o as Word
    }

    /// Constructor `index` applied to `args`, which it takes: its arguments laid out as its
    /// declaration types them, where its unit's code reads them so.
    pub fn ctor(&mut self, layouts: &Layouts, index: u32, args: &[Word]) -> Word {
        let Some(shape) = layouts.ctor_shape(index) else {
            let flat = if args.iter().all(|w| is_imm(*w)) {
                FLAT
            } else {
                0
            };
            let o = self.alloc(KIND_CTOR, flat, args.len() as u32, index);
            for (i, w) in args.iter().enumerate() {
                unsafe { set_word(o, i, *w) };
            }
            return o as Word;
        };
        let slots = layouts.shape_slots(shape);
        let words: Vec<(Word, Slot)> = args.iter().copied().zip(slots.iter().copied()).collect();
        self.laid_out(layouts, KIND_CTOR, index, shape, &words)
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

    /// Bytes `start..end` of string or bytes value `w`, which it takes: the value itself where that
    /// is all of it, a view of its bytes where the unit's code reads one and the slice is long, and
    /// a copy otherwise. A view of a view views the value under both.
    pub fn slice(&mut self, layouts: &Layouts, w: Word, start: usize, end: usize) -> Word {
        let o = obj(w);
        let (kind, len) = unsafe { ((*o).kind, (*o).len as usize) };
        if start == 0 && end == len {
            return w;
        }
        if !layouts.layout.bytes_views || end - start < VIEW_MIN {
            let out = self.joined(kind, unsafe { &bytes_of(o)[start..end] }, &[], 0) as Word;
            dec(w);
            return out;
        }
        let (of, from) = unsafe {
            if (*o).flags & VIEW != 0 {
                let under = word_at(o, 0);
                inc(under);
                let from = word_at(o, 1) as usize;
                dec(w);
                (under, from + start)
            } else {
                (w, start)
            }
        };
        let v = self.raw_alloc(kind, VIEW, (end - start) as u32, 0, 16);
        unsafe {
            set_word(v, 0, of);
            set_word(v, 1, from as i64);
            counted_view(obj(of), 1);
        }
        v as Word
    }

    /// Takes `a` and appends `b`: in place when `a` may grow in place and has room, else a doubled
    /// copy.
    pub fn append(&mut self, a: Word, b: &[u8]) -> Word {
        let o = obj(a);
        let (len, cap) = unsafe { ((*o).len as usize, (*o).layout as usize) };
        if grows_in_place(a) && len + b.len() <= cap {
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
        unsafe { set_word(o, 0, ply_core::float::bits(f) as i64) };
        o as Word
    }

    /// A `Decimal`'s object.
    pub fn boxed_decimal(&mut self, d: ply_core::Decimal) -> Word {
        let bytes = d.serialize();
        let half = |i: usize| i64::from_le_bytes(bytes[i..i + 8].try_into().expect("eight bytes"));
        let o = self.alloc(KIND_DECIMAL, 0, 2, 0);
        unsafe {
            set_word(o, 0, half(0));
            set_word(o, 1, half(8));
        }
        o as Word
    }

    /// A 64- or 128-bit width's object: its bits in one word or two.
    pub fn boxed_fixed(&mut self, f: Fixed) -> Word {
        let wide = f.ty.bits() > 64;
        let o = self.alloc(KIND_FIXED, 0, if wide { 2 } else { 1 }, f.ty as u32);
        unsafe {
            set_word(o, 0, f.bits() as u64 as i64);
            if wide {
                set_word(o, 1, (f.bits() >> 64) as u64 as i64);
            }
        }
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
        // Zero is an `Option` held as its payload's word: `None`.
        if !is_object(w) {
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
                KIND_FIXED => self.boxed_fixed(fixed_of(o)),
                KIND_DECIMAL => self.boxed_decimal(decimal_at(o, 0)),
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
                KIND_REFUSED => refused("Heap::copy"),
                kind => {
                    let (len, layout, flags) = ((*o).len, (*o).layout, (*o).flags);
                    let c = self.alloc(kind, flags, len, layout);
                    (*c).aux = (*o).aux;
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
                    if (kind == KIND_RECORD || kind == KIND_ARRAY || kind == KIND_CTOR)
                        && flags & MIXED != 0
                    {
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
        self.by_kind = [0; KINDS];
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
            Value::Fixed(f) => self.boxed_fixed(*f),
            Value::Decimal(d) => self.boxed_decimal(*d),
            Value::Bool(b) => bool(*b),
            Value::Float(f) => self.boxed_float(*f),
            Value::Char(c) => char_word(*c),
            Value::Unit => unit(),
            Value::Str(s) if u32::try_from(s.len()).is_ok() => self.str(s),
            Value::Bytes(b) if u32::try_from(b.len()).is_ok() => self.bytes(b),
            // Each scalar is held as its bits, as a record of its type is wherever it is made.
            // `Fields` iterates sorted, which is the layout's order.
            Value::Record(fields) => {
                let words: Vec<(Symbol, Word)> = fields
                    .iter()
                    .map(|(name, value)| (name.clone(), self.to_word(layouts, value)))
                    .collect();
                self.record(layouts, &words)
            }
            Value::Ctor { name, args } => match layouts.ctor_index(name) {
                Some(index) => {
                    let words: Vec<Word> = args.iter().map(|a| self.to_word(layouts, a)).collect();
                    self.ctor(layouts, index, &words)
                }
                None => self.bridge(v.clone()),
            },
            Value::List(items) => {
                let words: Vec<Word> = items.iter().map(|x| self.to_word(layouts, x)).collect();
                self.list_from(&words)
            }
            Value::Array(items) if u32::try_from(items.len()).is_ok() => {
                let words: Vec<Word> = items.iter().map(|x| self.to_word(layouts, x)).collect();
                self.array_of(layouts, &words)
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
                ClosureKind::Hold { held, id } => {
                    let held = held.as_ref().map(|x| self.to_word(layouts, x));
                    crate::rt::hold_word(self, held, *id)
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
        if is_char(w) {
            return Value::Char(char_of(w));
        }
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
                KIND_FIXED => Value::Fixed(fixed_of(o)),
                KIND_DECIMAL => Value::Decimal(decimal_at(o, 0)),
                KIND_STR => Value::Str(Arc::from(str_of(o))),
                KIND_BYTES => Value::Bytes(Arc::from(bytes_of(o))),
                KIND_RECORD => {
                    let names = layouts.shape_names((*o).layout);
                    let slots = layouts.shape_slots((*o).layout);
                    let offsets = layouts.shape_offsets((*o).layout);
                    let fields: Vec<(Symbol, Value)> = names
                        .iter()
                        .enumerate()
                        .map(|(i, name)| {
                            let field = carry.field(name);
                            let value =
                                read_held(layouts, o, offsets[i], slots[i], &field, ctors, walked);
                            (name.clone(), value)
                        })
                        .collect();
                    Value::Record(Arc::new(Fields::from_unsorted(fields)))
                }
                KIND_CTOR => {
                    let name = layouts.ctors[(*o).layout as usize].0.clone();
                    let args = (0..layouts.ctor_arity(o))
                        .map(|i| {
                            let field = carry.ctor_field(&name, i, ctors);
                            let (at, slot) = layouts.ctor_arg(o, i);
                            read_held(layouts, o, at, slot, &field, ctors, walked)
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
                    let (slot, stride) = held_as(layouts, o);
                    Value::array(
                        (0..crate::array::len(o))
                            .map(|k| read_held(layouts, o, k * stride, slot, &item, ctors, walked))
                            .collect(),
                    )
                }
                // The array of its records, which is the value it is.
                KIND_COLUMNS => {
                    let item = carry.item();
                    let names = layouts.shape_names((*o).layout);
                    let fields: Vec<Vec<Value>> = names
                        .iter()
                        .enumerate()
                        .map(|(k, name)| {
                            let column = crate::columns::column(o, k);
                            let field = item.field(name);
                            let (slot, stride) = held_as(layouts, column);
                            (0..crate::array::len(column))
                                .map(|i| {
                                    read_held(
                                        layouts,
                                        column,
                                        i * stride,
                                        slot,
                                        &field,
                                        ctors,
                                        walked,
                                    )
                                })
                                .collect()
                        })
                        .collect();
                    Value::array(
                        (0..crate::columns::len(o))
                            .map(|i| {
                                Value::Record(Arc::new(Fields::from_unsorted(
                                    names
                                        .iter()
                                        .zip(&fields)
                                        .map(|(name, column)| (name.clone(), column[i].clone()))
                                        .collect(),
                                )))
                            })
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
                    let kind = if let Some(id) = crate::rt::hold_id(w) {
                        let held = word_at(o, CLOSURE_CAPTURES);
                        ClosureKind::Hold {
                            held: (!crate::rt::hold_let_go(o))
                                .then(|| Heap::to_value_counted(layouts, held, walked)),
                            id,
                        }
                    } else {
                        let code = word_at(o, CLOSURE_CODE) as usize;
                        let arity = (*o).layout as usize;
                        let captured: Vec<Value> = (CLOSURE_CAPTURES..(*o).len as usize)
                            .map(|i| Heap::to_value_counted(layouts, word_at(o, i), walked))
                            .collect();
                        // Only the code tells: a continuation's captures are plain immediates.
                        if crate::detached::is_continuation(code) {
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
                KIND_REFUSED => refused("Heap::read"),
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
    if !is_object(w) {
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
    if !is_object(w) {
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

/// [`inc`] of an object [`share`] marked, or of a record held in place, which its holder counts.
pub fn inc_shared(w: Word) {
    let o = obj(w);
    match unsafe { holder(o) } {
        Some(h) => inc(h as Word),
        None => {
            count(o).fetch_add(1, Relaxed);
        }
    }
}

/// [`dec`] of an object [`share`] marked, the holder that takes the count to none dismantling it,
/// or of a record held in place, which its holder counts.
pub fn dec_shared(w: Word) {
    let o = obj(w);
    if let Some(h) = unsafe { holder(o) } {
        dec(h as Word);
        return;
    }
    if unsafe { last_of_shared(o) } {
        let heap = CURRENT.with(|c| c.get());
        unsafe { release(o, heap) }
    }
}

/// Marks everything under `words` shared, or nothing when something under them names what only
/// this thread's context holds: a task, a channel or a continuation.
pub fn share(words: &[Word]) -> bool {
    let mut marked: Vec<*mut Obj> = Vec::new();
    let mut pending: Vec<Word> = words.to_vec();
    while let Some(w) = pending.pop() {
        if !is_object(w) {
            continue;
        }
        let o = unsafe { counted_by(obj(w)) };
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
                KIND_REFUSED => refused("share"),
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

/// Whether a bridged value may be held by another thread: a task or a channel is a handle into
/// this thread's scheduler.
fn crosses_threads(v: &Value) -> bool {
    match v {
        Value::Task(_) | Value::Chan(_) => false,
        Value::Closure(c) => match &c.kind {
            ClosureKind::Native { captured, .. } => captured.iter().all(crosses_threads),
            ClosureKind::Hold { held, .. } => held.iter().all(crosses_threads),
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
    if !is_object(w) {
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

/// The objects `o` holds a count of: what releasing it lets go of, and what reaches anything
/// through it.
unsafe fn children(o: *mut Obj) -> Vec<Word> {
    unsafe {
        child_ranges(o)
            .iter()
            .flat_map(|(first, last)| *first..*last)
            .filter(|i| counted(o, *i))
            .map(|i| word_at(o, i))
            .filter(|c| !is_imm(*c) && *c != 0)
            .collect()
    }
}

/// The payload words of `o` that hold counted children.
unsafe fn child_ranges(o: *mut Obj) -> [(usize, usize); 2] {
    unsafe {
        match (*o).kind {
            KIND_RECORD | KIND_CTOR | KIND_ARRAY if (*o).flags & FLAT != 0 => [(0, 0), (0, 0)],
            KIND_RECORD | KIND_CTOR | KIND_LEAF | KIND_BRANCH | KIND_ARRAY | KIND_COLUMNS => {
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
            // The value whose bytes it views.
            KIND_STR | KIND_BYTES if (*o).flags & VIEW != 0 => [(0, 1), (0, 0)],
            KIND_REFUSED => refused("child_ranges"),
            _ => [(0, 0), (0, 0)],
        }
    }
}

/// `o`, held once, dies: its children are released and its header marked dead.
unsafe fn dismantle(o: *mut Obj, depth: usize, deferred: &mut Vec<*mut Obj>, heap: *mut Heap) {
    unsafe {
        if (*o).kind == KIND_REFUSED {
            refused("dismantle");
        }
        // A shared object reaches here with its count's own bits spent.
        debug_assert!(((*o).rc == SHARED || (*o).rc == 1) && (*o).kind != KIND_DEAD);
        (*o).rc = 0;
        if (*o).kind == KIND_BRIDGE {
            std::ptr::drop_in_place(bridge_slot(o));
        }
        if ((*o).kind == KIND_BYTES || (*o).kind == KIND_STR) && (*o).flags & VIEW != 0 {
            counted_view(obj(word_at(o, 0)), -1);
        }
        for i in child_ranges(o)
            .iter()
            .flat_map(|(first, last)| *first..*last)
        {
            if !counted(o, i) {
                continue;
            }
            let c = word_at(o, i);
            if !is_object(c) {
                continue;
            }
            let co = counted_by(obj(c));
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
        let o = unsafe { counted_by(obj(w)) };
        unsafe {
            if (*o).rc == IMMORTAL {
                continue;
            }
            if (*o).kind == KIND_REFUSED {
                refused("mark_immortal");
            }
            (*o).rc = IMMORTAL;
            pending.extend(children(o));
        }
    }
}

/// `ply_core::memo::world_independent` over a word: whether a memo may keep it.
pub fn world_independent(w: Word) -> bool {
    let mut pending = vec![w];
    while let Some(w) = pending.pop() {
        if is_imm(w) {
            continue;
        }
        let o = obj(w);
        unsafe {
            match (*o).kind {
                // A continuation's captures are immediates naming a body its entry alone holds, and
                // a hold is its entry's.
                KIND_CLOSURE
                    if crate::detached::is_continuation(word_at(o, CLOSURE_CODE) as usize)
                        || crate::rt::hold_id(w).is_some() =>
                {
                    return false;
                }
                KIND_BRIDGE => {
                    // A handle must not outlive its entry, and the memo would keep it.
                    if crate::rt::holds_a_handle(bridged(o)).is_some()
                        || !ply_core::memo::world_independent(bridged(o))
                    {
                        return false;
                    }
                }
                KIND_REFUSED => refused("world_independent"),
                _ => pending.extend(children(o)),
            }
        }
    }
    true
}

/// Adds `by` to the views of `o` it counts; a count past its field stays there, so `o` is held by
/// views it no longer counts and never grows in place again. A shared value counts none.
unsafe fn counted_view(o: *mut Obj, by: i32) {
    unsafe {
        let views = (*o).aux;
        if (*o).rc < SHARED && views < u16::MAX {
            (*o).aux = (i32::from(views) + by).clamp(0, i32::from(u16::MAX)) as u16;
        }
    }
}

/// Whether bytes or a string `w` may grow in place: no holder but one has it, besides the views of
/// it, which read only bytes it already holds.
pub fn grows_in_place(w: Word) -> bool {
    if !is_object(w) {
        return false;
    }
    let o = obj(w);
    unsafe {
        let (rc, views) = ((*o).rc, (*o).aux);
        (*o).flags & VIEW == 0 && rc < SHARED && views < u16::MAX && rc == 1 + u32::from(views)
    }
}

/// Whether one holder alone has `w`: what lets an update write in place.
#[inline]
pub fn is_unique(w: Word) -> bool {
    !is_imm(w) && count(obj(w)).load(Relaxed) == 1
}

#[inline]
pub fn kind(w: Word) -> u8 {
    if is_int(w) {
        KIND_INT
    } else if is_char(w) {
        KIND_CHAR
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
    if is_int(w) {
        return Some(imm_value(w));
    }
    if is_char(w) {
        return None;
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
    if !is_object(w) {
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
    if is_char(w) {
        return 16;
    }
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
            KIND_DECIMAL => 4,
            KIND_FIXED => 15,
            KIND_STR => 5,
            KIND_BYTES => 6,
            KIND_LIST => 7,
            KIND_MAP => 8,
            KIND_RECORD => 9,
            KIND_CTOR => 10,
            KIND_CLOSURE => 11,
            KIND_ARRAY | KIND_COLUMNS => 17,
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
                Value::Task(_) => 13,
                Value::Chan(_) => 18,
                Value::Secret(_) => 14,
                Value::Fixed(_) => 15,
                Value::Char(_) => 16,
                Value::Array(_) => 17,
            },
            KIND_REFUSED => refused("cmp_words"),
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
    // A `Char`'s word is its code point shifted above a tag, so words order as code points do.
    if ka == KIND_CHAR {
        return a.cmp(&b);
    }
    unsafe {
        match ka {
            KIND_UNIT => Ordering::Equal,
            KIND_BOOL => (*obj(a)).flags.cmp(&(*obj(b)).flags),
            // Byte order, which is the order `str` and `[u8]` have.
            KIND_STR | KIND_BYTES => bytes_of(obj(a)).cmp(bytes_of(obj(b))),
            KIND_FIXED => fixed_of(obj(a)).value_cmp(fixed_of(obj(b))),
            KIND_DECIMAL => decimal_at(obj(a), 0).cmp(&decimal_at(obj(b), 0)),
            KIND_FLOAT => ply_core::float::total_cmp(
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
                let ((sx, at_x), (sy, at_y)) = (held_as(layouts, x), held_as(layouts, y));
                let (n, m) = (crate::array::len(x), crate::array::len(y));
                for k in 0..n.min(m) {
                    let c = cmp_fields(layouts, (x, sx, k * at_x), (y, sy, k * at_y));
                    if c != Ordering::Equal {
                        return c;
                    }
                }
                n.cmp(&m)
            }
            KIND_COLUMNS => Heap::to_value(layouts, a).cmp(&Heap::to_value(layouts, b)),
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
            KIND_RECORD if ((*obj(a)).flags | (*obj(b)).flags) & RAW != 0 => {
                let (x, y) = (obj(a), obj(b));
                let (sx, sy) = ((*x).layout, (*y).layout);
                let (names_x, names_y) = (layouts.shape_names(sx), layouts.shape_names(sy));
                let (slots_x, slots_y) = (layouts.shape_slots(sx), layouts.shape_slots(sy));
                let (at_x, at_y) = (layouts.shape_offsets(sx), layouts.shape_offsets(sy));
                let (n, m) = (names_x.len(), names_y.len());
                for i in 0..n.min(m) {
                    let c = names_x[i].cmp(&names_y[i]).then_with(|| {
                        cmp_fields(layouts, (x, slots_x[i], at_x[i]), (y, slots_y[i], at_y[i]))
                    });
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
                let (n, m) = (layouts.ctor_arity(x), layouts.ctor_arity(y));
                for i in 0..n.min(m) {
                    let ((ax, sx), (ay, sy)) = (layouts.ctor_arg(x, i), layouts.ctor_arg(y, i));
                    let c = cmp_fields(layouts, (x, sx, ax), (y, sy, ay));
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

/// How array `o` holds each element, and how many words apart.
fn held_as(layouts: &Layouts, o: *mut Obj) -> (Slot, usize) {
    match unsafe { (*o).aux } {
        0 => (Slot::Word, 1),
        stride => (layouts.slot_at(unsafe { (*o).layout }, 0), stride as usize),
    }
}

/// The value `o` holds `at`, as `slot` holds it.
fn read_held(
    layouts: &Layouts,
    o: *mut Obj,
    at: usize,
    slot: Slot,
    carry: &Carry,
    ctors: &CtorCarries,
    walked: &mut Walked,
) -> Value {
    unsafe {
        match slot {
            // A width below 64 bits is held as an `Int`, which only its carry tells apart.
            Slot::Int => read_int(word_at(o, at) as i64, carry, walked),
            Slot::Float | Slot::Wide(_) | Slot::Decimal => raw_value(o, at, slot),
            Slot::Maybe => {
                let p = word_at(o, at);
                let payload = (p != 0).then(|| {
                    let name = layouts.some.map(|i| layouts.ctors[i as usize].0.clone());
                    let field = match name {
                        Some(n) => carry.ctor_field(&n, 0, ctors),
                        None => carry.item(),
                    };
                    Heap::read(layouts, p, &field, ctors, walked)
                });
                option_value(layouts, payload)
            }
            Slot::Record(_) => Heap::read(layouts, words(o).add(at) as Word, carry, ctors, walked),
            Slot::Word => Heap::read(layouts, word_at(o, at), carry, ctors, walked),
        }
    }
}

/// Two fields of records, each as its record holds it.
unsafe fn cmp_fields(
    layouts: &Layouts,
    (x, sx, ax): (*mut Obj, Slot, usize),
    (y, sy, ay): (*mut Obj, Slot, usize),
) -> Ordering {
    // `None` sorts before `Some`, as their names do, and two `Some`s as their payloads.
    if sx == Slot::Maybe && sy == Slot::Maybe {
        let (p, q) = unsafe { (word_at(x, ax), word_at(y, ay)) };
        return match (p, q) {
            (0, 0) => Ordering::Equal,
            (0, _) => Ordering::Less,
            (_, 0) => Ordering::Greater,
            _ => cmp_words(layouts, p, q),
        };
    }
    let word = |o: *mut Obj, slot: Slot, at: usize| unsafe {
        match slot {
            Slot::Word => Some(word_at(o, at)),
            Slot::Record(_) => Some(words(o).add(at) as Word),
            Slot::Int | Slot::Float | Slot::Wide(_) | Slot::Decimal | Slot::Maybe => None,
        }
    };
    let value = |o: *mut Obj, slot: Slot, at: usize| match word(o, slot, at) {
        Some(w) => Heap::to_value(layouts, w),
        None if slot == Slot::Maybe => {
            let p = unsafe { word_at(o, at) };
            option_value(layouts, (p != 0).then(|| Heap::to_value(layouts, p)))
        }
        None => unsafe { raw_value(o, at, slot) },
    };
    match (word(x, sx, ax), word(y, sy, ay)) {
        (Some(p), Some(q)) => cmp_words(layouts, p, q),
        _ => value(x, sx, ax).cmp(&value(y, sy, ay)),
    }
}

/// Whether a word can be a native map's key: one [`cmp_words`] orders in place.
pub fn native_key(layouts: &Layouts, w: Word) -> bool {
    if is_imm(w) {
        return true;
    }
    let o = obj(w);
    unsafe {
        match (*o).kind {
            KIND_UNIT | KIND_BOOL | KIND_INT | KIND_FIXED | KIND_DECIMAL | KIND_STR
            | KIND_BYTES => true,
            KIND_BRIDGE => matches!(
                bridged(o),
                Value::Str(_) | Value::Bytes(_) | Value::Fixed(_)
            ),
            KIND_RECORD | KIND_CTOR | KIND_ARRAY if (*o).flags & RAW != 0 => {
                layouts.parts(o).into_iter().all(|(at, slot)| match slot {
                    Slot::Decimal | Slot::Int | Slot::Float | Slot::Wide(_) => true,
                    Slot::Maybe => {
                        let p = word_at(o, at);
                        p == 0 || native_key(layouts, p)
                    }
                    _ => part_word(o, at, slot).is_some_and(|p| native_key(layouts, p)),
                })
            }
            KIND_RECORD | KIND_CTOR | KIND_LEAF | KIND_BRANCH | KIND_ARRAY | KIND_COLUMNS => {
                (0..(*o).len as usize).all(|i| !counted(o, i) || native_key(layouts, word_at(o, i)))
            }
            KIND_LIST => list::children(o).all(|w| native_key(layouts, w)),
            KIND_MAP => {
                let mut fine = true;
                map::for_each(o, |k, v| {
                    fine = fine && native_key(layouts, k) && native_key(layouts, v)
                });
                fine
            }
            KIND_REFUSED => refused("native_key"),
            _ => false,
        }
    }
}
