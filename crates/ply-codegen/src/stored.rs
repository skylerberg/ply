//! A value as text a unit's C holds, and back: what a build keeps of a `const` definition. The
//! value is written as the heap lays it out, so a word reads back as the word it was: a record
//! names its shape's fields and a constructor its name, which any unit holding the value's types
//! places; a map's entries are in its tree's order, so reading one compares nothing; and an object
//! met twice is written once. A closure, and a bridged cell, task, channel or secret, has no text.

use crate::heap::{
    self, Heap, KIND_ARRAY, KIND_BOOL, KIND_BRIDGE, KIND_BYTES, KIND_CLOSURE, KIND_CTOR, KIND_INT,
    KIND_LIST, KIND_MAP, KIND_RECORD, KIND_STR, KIND_UNIT, Layouts, Word, bridged, bytes_of,
    is_imm, obj, set_word, word_at,
};
use crate::{array, list, map};
use ply_eval::Symbol;
use std::collections::HashMap;
use std::io::{Read, Write};

const FORMAT: &[u8; 4] = b"PLW1";

const IMM: u8 = 0;
const INT: u8 = 1;
const UNIT: u8 = 2;
const FALSE: u8 = 3;
const TRUE: u8 = 4;
const STR: u8 = 5;
const BYTES: u8 = 6;
const RECORD: u8 = 7;
const CTOR: u8 = 8;
const LIST: u8 = 9;
const ARRAY: u8 = 10;
const MAP: u8 = 11;
const BRIDGE: u8 = 12;
/// An object written earlier, by its place among the objects completed so far.
const AGAIN: u8 = 13;
/// A list or an array holding immediates alone: its words as they are, eight bytes each.
const LIST_WORDS: u8 = 14;
const ARRAY_WORDS: u8 = 15;

/// The text of `w`, which it reads: lowercase hex, so a C string literal holds it as it is.
pub fn text(layouts: &Layouts, w: Word) -> Result<Vec<u8>, String> {
    let raw = Writer::new(layouts).written(w)?;
    let unpacked = |e: std::io::Error| format!("a value whose stored form does not pack ({e})");
    let mut packer = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    packer.write_all(&raw).map_err(unpacked)?;
    let packed = packer.finish().map_err(unpacked)?;
    let mut out = Vec::with_capacity(packed.len() * 2);
    for b in packed {
        out.push(HEX[usize::from(b >> 4)]);
        out.push(HEX[usize::from(b & 15)]);
    }
    Ok(out)
}

/// The value `text` holds, immortal in `heap`, which never resets: a unit reads it into its own, so
/// reading allocates nothing an entry counts or releases. `nullaries` is the unit's one object of
/// each constructor that takes nothing, by the constructor's index, or zero.
pub fn read(
    layouts: &Layouts,
    nullaries: &[Word],
    heap: &mut Heap,
    text: &[u8],
) -> Result<Word, String> {
    let packed = unhex(text)?;
    let mut raw = Vec::new();
    flate2::read::ZlibDecoder::new(packed.as_slice())
        .read_to_end(&mut raw)
        .map_err(|e| format!("it does not unpack: {e}"))?;
    let body = raw
        .strip_prefix(FORMAT.as_slice())
        .ok_or("it is not a stored value")?;
    let w = Reader {
        bytes: body,
        at: 0,
        heap,
        layouts,
        nullaries,
        shapes: Vec::new(),
        ctors: Vec::new(),
        seen: Vec::new(),
    }
    .value()?;
    heap::mark_immortal(w);
    Ok(w)
}

const HEX: &[u8; 16] = b"0123456789abcdef";

fn unhex(text: &[u8]) -> Result<Vec<u8>, String> {
    let digit = |c: u8| match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        _ => Err("it holds a byte that is no hex digit".to_string()),
    };
    if !text.len().is_multiple_of(2) {
        return Err("it holds half a byte".to_string());
    }
    text.chunks_exact(2)
        .map(|pair| Ok(digit(pair[0])? << 4 | digit(pair[1])?))
        .collect()
}

/// What is left to write: a word, or the end of an object whose parts are all written.
enum Pending {
    Word(Word),
    Done(usize),
}

struct Writer<'l> {
    layouts: &'l Layouts,
    out: Vec<u8>,
    shapes: HashMap<u32, u64>,
    ctors: HashMap<u32, u64>,
    /// Each object written whole, by its address, and its place among them.
    done: HashMap<usize, u64>,
}

impl<'l> Writer<'l> {
    fn new(layouts: &'l Layouts) -> Writer<'l> {
        Writer {
            layouts,
            out: FORMAT.to_vec(),
            shapes: HashMap::new(),
            ctors: HashMap::new(),
            done: HashMap::new(),
        }
    }

    fn written(mut self, w: Word) -> Result<Vec<u8>, String> {
        let mut pending = vec![Pending::Word(w)];
        while let Some(next) = pending.pop() {
            match next {
                Pending::Done(address) => {
                    let place = self.done.len() as u64;
                    self.done.insert(address, place);
                }
                Pending::Word(w) => self.word(w, &mut pending)?,
            }
        }
        Ok(self.out)
    }

    fn word(&mut self, w: Word, pending: &mut Vec<Pending>) -> Result<(), String> {
        if is_imm(w) {
            self.out.push(IMM);
            let n = heap::imm_value(w);
            self.varint(((n << 1) ^ (n >> 63)) as u64);
            return Ok(());
        }
        if let Some(&place) = self.done.get(&(w as usize)) {
            self.out.push(AGAIN);
            self.varint(place);
            return Ok(());
        }
        let o = obj(w);
        let (kind, flags, len, layout) = unsafe { ((*o).kind, (*o).flags, (*o).len, (*o).layout) };
        let parts: Vec<Word> = match kind {
            KIND_UNIT => {
                self.out.push(UNIT);
                return Ok(());
            }
            KIND_BOOL => {
                self.out.push(if flags != 0 { TRUE } else { FALSE });
                return Ok(());
            }
            KIND_INT => {
                self.out.push(INT);
                self.out
                    .extend_from_slice(&unsafe { word_at(o, 0) }.to_le_bytes());
                Vec::new()
            }
            KIND_STR | KIND_BYTES => {
                self.out.push(if kind == KIND_STR { STR } else { BYTES });
                self.blob(unsafe { bytes_of(o) });
                Vec::new()
            }
            KIND_RECORD => {
                self.out.push(RECORD);
                self.shape(layout);
                self.out.push(flags);
                (0..len as usize)
                    .map(|i| unsafe { word_at(o, i) })
                    .collect()
            }
            KIND_CTOR => {
                // A nullary constructor is its unit's one object of it, which a place would not name.
                if len == 0 {
                    self.out.push(CTOR);
                    self.ctor(layout);
                    self.out.push(flags);
                    self.varint(0);
                    return Ok(());
                }
                self.out.push(CTOR);
                self.ctor(layout);
                self.out.push(flags);
                self.varint(u64::from(len));
                (0..len as usize)
                    .map(|i| unsafe { word_at(o, i) })
                    .collect()
            }
            KIND_LIST => self.sequence(LIST, LIST_WORDS, list::to_vec(o)),
            KIND_ARRAY => self.sequence(ARRAY, ARRAY_WORDS, array::items(o).to_vec()),
            KIND_MAP => {
                let entries = map::to_vec(o);
                self.out.push(MAP);
                self.varint(entries.len() as u64);
                entries.into_iter().flat_map(|(k, v)| [k, v]).collect()
            }
            KIND_BRIDGE => {
                let value = unsafe { bridged(o) };
                let plain = ply_eval::codec::encode(value)
                    .map_err(|_| format!("a {}", value.type_name()))?;
                self.out.push(BRIDGE);
                self.blob(&plain);
                Vec::new()
            }
            KIND_CLOSURE => return Err("a function".to_string()),
            other => return Err(format!("an object of kind {other}")),
        };
        pending.push(Pending::Done(w as usize));
        pending.extend(parts.into_iter().rev().map(Pending::Word));
        Ok(())
    }

    /// A list's or an array's header, and the elements still to write: none where every one is an
    /// immediate, which are written here as the words they are.
    fn sequence(&mut self, tag: u8, words: u8, items: Vec<Word>) -> Vec<Word> {
        if !items.is_empty() && items.iter().all(|w| is_imm(*w)) {
            self.out.push(words);
            self.varint(items.len() as u64);
            self.out.reserve(items.len() * 8);
            for w in &items {
                self.out.extend_from_slice(&w.to_le_bytes());
            }
            return Vec::new();
        }
        self.out.push(tag);
        self.varint(items.len() as u64);
        items
    }

    /// A shape by its place among those written, or its fields the first time.
    fn shape(&mut self, shape: u32) {
        if let Some(&place) = self.shapes.get(&shape) {
            return self.varint(place + 1);
        }
        self.shapes.insert(shape, self.shapes.len() as u64);
        let names = self.layouts.shape_names(shape);
        self.varint(0);
        self.varint(names.len() as u64);
        for name in names.iter() {
            self.blob(name.as_str().as_bytes());
        }
    }

    fn ctor(&mut self, index: u32) {
        if let Some(&place) = self.ctors.get(&index) {
            return self.varint(place + 1);
        }
        self.ctors.insert(index, self.ctors.len() as u64);
        self.varint(0);
        let name = self.layouts.ctors[index as usize].0.clone();
        self.blob(name.as_str().as_bytes());
    }

    fn blob(&mut self, b: &[u8]) {
        self.varint(b.len() as u64);
        self.out.extend_from_slice(b);
    }

    fn varint(&mut self, mut n: u64) {
        while n >= 0x80 {
            self.out.push((n as u8) | 0x80);
            n >>= 7;
        }
        self.out.push(n as u8);
    }
}

/// An object whose parts are still being read.
struct Open {
    make: Make,
    want: usize,
    parts: Vec<Word>,
}

enum Make {
    Record { shape: u32, flags: u8 },
    Ctor { index: u32, flags: u8 },
    List,
    Array,
    Map,
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    heap: &'a mut Heap,
    layouts: &'a Layouts,
    nullaries: &'a [Word],
    shapes: Vec<u32>,
    ctors: Vec<u32>,
    /// Each object read whole, in the order the writer finished them.
    seen: Vec<Word>,
}

impl<'a> Reader<'a> {
    fn value(mut self) -> Result<Word, String> {
        let mut open: Vec<Open> = Vec::new();
        loop {
            let mut read = self.next(&mut open)?;
            // A word read is a part of the object open around it, which it may complete in turn.
            while let Some(w) = read {
                let Some(top) = open.last_mut() else {
                    if self.at != self.bytes.len() {
                        return Err(format!("{} bytes follow it", self.bytes.len() - self.at));
                    }
                    return Ok(w);
                };
                top.parts.push(w);
                read = if top.parts.len() == top.want {
                    let done = open.pop().expect("the object just filled");
                    Some(self.made(done))
                } else {
                    None
                };
            }
        }
    }

    /// The next word where it is whole as read, or `None` once an object is opened for its parts.
    fn next(&mut self, open: &mut Vec<Open>) -> Result<Option<Word>, String> {
        let tag = self.byte()?;
        let (make, want) = match tag {
            IMM => {
                let z = self.varint()?;
                let n = ((z >> 1) as i64) ^ -((z & 1) as i64);
                if !heap::fits_imm(n) {
                    return Err("it holds an immediate past sixty-three bits".to_string());
                }
                return Ok(Some(heap::imm(n)));
            }
            UNIT => return Ok(Some(heap::unit())),
            FALSE => return Ok(Some(heap::bool(false))),
            TRUE => return Ok(Some(heap::bool(true))),
            AGAIN => {
                let place = self.varint()? as usize;
                let w = *self
                    .seen
                    .get(place)
                    .ok_or("it names an object before writing it")?;
                return Ok(Some(w));
            }
            INT => {
                let n = i64::from_le_bytes(self.array()?);
                let o = self.heap.alloc(KIND_INT, 0, 1, 0);
                unsafe { set_word(o, 0, n) };
                return Ok(Some(self.kept(o as Word)));
            }
            STR => {
                let b = self.blob()?;
                let s =
                    std::str::from_utf8(b).map_err(|_| "it holds a string that is not UTF-8")?;
                let w = self.heap.str(s);
                return Ok(Some(self.kept(w)));
            }
            BYTES => {
                let b = self.blob()?;
                let w = self.heap.bytes(b);
                return Ok(Some(self.kept(w)));
            }
            BRIDGE => {
                let value = ply_eval::codec::decode(self.blob()?)?;
                let w = self.heap.bridge(value);
                return Ok(Some(self.kept(w)));
            }
            LIST_WORDS | ARRAY_WORDS => {
                let n = self.varint()?;
                if n > ((self.bytes.len() - self.at) / 8) as u64 {
                    return Err(format!("it counts {n} words with fewer bytes left"));
                }
                let words: Vec<Word> = (0..n)
                    .map(|_| self.array().map(i64::from_le_bytes))
                    .collect::<Result<_, _>>()?;
                if words.iter().any(|w| !is_imm(*w)) {
                    return Err("it holds a word that is no immediate".to_string());
                }
                let w = if tag == LIST_WORDS {
                    self.heap.list_from(&words)
                } else {
                    self.heap.array_from(&words)
                };
                return Ok(Some(self.kept(w)));
            }
            RECORD => {
                let shape = self.shape()?;
                let flags = self.byte()?;
                let want = self.layouts.shape_width(shape);
                (Make::Record { shape, flags }, want)
            }
            CTOR => {
                let index = self.ctor()?;
                let flags = self.byte()?;
                let want = self.count()?;
                if want != self.layouts.ctors[index as usize].1 {
                    return Err(format!(
                        "it holds a `{}` of {want} fields",
                        self.layouts.ctors[index as usize].0
                    ));
                }
                if want == 0 {
                    let one = self.nullaries.get(index as usize).copied().unwrap_or(0);
                    return Ok(Some(if one != 0 {
                        one
                    } else {
                        self.heap.alloc(KIND_CTOR, flags, 0, index) as Word
                    }));
                }
                (Make::Ctor { index, flags }, want)
            }
            LIST => (Make::List, self.count()?),
            ARRAY => (Make::Array, self.count()?),
            MAP => {
                let entries = self.count()?;
                (Make::Map, entries * 2)
            }
            other => return Err(format!("it holds a tag {other} this runtime does not read")),
        };
        let opened = Open {
            make,
            want,
            parts: Vec::with_capacity(want),
        };
        if want == 0 {
            return Ok(Some(self.made(opened)));
        }
        open.push(opened);
        Ok(None)
    }

    fn made(&mut self, done: Open) -> Word {
        let filled = |o: *mut heap::Obj, parts: &[Word]| {
            for (i, w) in parts.iter().enumerate() {
                unsafe { set_word(o, i, *w) };
            }
            o as Word
        };
        let w = match done.make {
            Make::Record { shape, flags } => {
                let o = self
                    .heap
                    .alloc(KIND_RECORD, flags, done.parts.len() as u32, shape);
                filled(o, &done.parts)
            }
            Make::Ctor { index, flags } => {
                let o = self
                    .heap
                    .alloc(KIND_CTOR, flags, done.parts.len() as u32, index);
                filled(o, &done.parts)
            }
            Make::List => self.heap.list_from(&done.parts),
            Make::Array => self.heap.array_from(&done.parts),
            Make::Map => {
                let entries: Vec<(Word, Word)> = done
                    .parts
                    .chunks_exact(2)
                    .map(|pair| (pair[0], pair[1]))
                    .collect();
                self.heap.map_from_sorted(&entries)
            }
        };
        self.kept(w)
    }

    /// `w`, placed among the objects read whole.
    fn kept(&mut self, w: Word) -> Word {
        self.seen.push(w);
        w
    }

    fn shape(&mut self) -> Result<u32, String> {
        match self.varint()? {
            0 => {
                let n = self.count()?;
                let mut names = Vec::with_capacity(n);
                for _ in 0..n {
                    names.push(self.name()?);
                }
                if names.windows(2).any(|pair| pair[0] >= pair[1]) {
                    return Err("it holds a shape whose fields are out of order".to_string());
                }
                let shape = self.layouts.shape(names);
                self.shapes.push(shape);
                Ok(shape)
            }
            n => self
                .shapes
                .get(n as usize - 1)
                .copied()
                .ok_or_else(|| format!("it names shape {n} before writing it")),
        }
    }

    fn ctor(&mut self) -> Result<u32, String> {
        match self.varint()? {
            0 => {
                let name = self.name()?;
                let index = self.layouts.ctor_index(&name).ok_or_else(|| {
                    format!("it holds a `{name}`, a constructor this unit does not have")
                })?;
                self.ctors.push(index);
                Ok(index)
            }
            n => self
                .ctors
                .get(n as usize - 1)
                .copied()
                .ok_or_else(|| format!("it names constructor {n} before writing it")),
        }
    }

    fn name(&mut self) -> Result<Symbol, String> {
        let b = self.blob()?;
        Ok(Symbol::new(
            std::str::from_utf8(b).map_err(|_| "it holds a name that is not UTF-8")?,
        ))
    }

    fn byte(&mut self) -> Result<u8, String> {
        let b = *self.bytes.get(self.at).ok_or("it ends early")?;
        self.at += 1;
        Ok(b)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let b = self
            .bytes
            .get(self.at..self.at + N)
            .ok_or("it ends early")?;
        self.at += N;
        Ok(b.try_into().expect("a slice of N bytes"))
    }

    fn blob(&mut self) -> Result<&'a [u8], String> {
        let n = self.count()?;
        let bytes: &'a [u8] = self.bytes;
        let b = &bytes[self.at..self.at + n];
        self.at += n;
        Ok(b)
    }

    /// A length no larger than what is left, so a corrupt count cannot ask for a huge allocation.
    fn count(&mut self) -> Result<usize, String> {
        let n = self.varint()?;
        if n > (self.bytes.len() - self.at) as u64 {
            return Err(format!("it counts {n} with fewer bytes left"));
        }
        Ok(n as usize)
    }

    fn varint(&mut self) -> Result<u64, String> {
        let mut n = 0u64;
        for shift in (0..64).step_by(7) {
            let b = self.byte()?;
            n |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(n);
            }
        }
        Err("it holds a number longer than 64 bits".to_string())
    }
}
