//! PLV1: plain values as bytes. How a cache keeps an answer it does not interpret, and the one
//! format a compiled value crosses into the runtime and back in. A value the bytes cannot hold (a
//! task, a channel, a credential, a hold, a function the program wrote) crosses as a
//! [`tag::HANDLE`], a position in the handles kept beside the bytes; [`encode`] and [`decode`] keep
//! none, and refuse one.

use crate::value::{Closure, ClosureKind, Decimal, Fields, Fixed, Synth, Value};
use crate::{INT_TYPES, IntTy, List, Symbol};
use std::collections::HashMap;
use std::sync::Arc;

/// What every encoding opens with.
pub const FORMAT: &[u8; 4] = b"PLV1";

/// The byte each value opens with.
pub mod tag {
    pub const UNIT: u8 = 0;
    pub const FALSE: u8 = 1;
    pub const TRUE: u8 = 2;
    pub const INT: u8 = 3;
    pub const FLOAT: u8 = 4;
    pub const DECIMAL: u8 = 5;
    pub const FIXED: u8 = 6;
    pub const STR: u8 = 7;
    pub const BYTES: u8 = 8;
    pub const LIST: u8 = 9;
    pub const MAP: u8 = 10;
    pub const RECORD: u8 = 11;
    pub const CTOR: u8 = 12;
    pub const CHAR: u8 = 13;
    pub const ARRAY: u8 = 14;
    /// Position `n` of the handles kept beside the bytes.
    pub const HANDLE: u8 = 15;
    /// A function as `std.value.Fun` describes one: one of [`super::fun`]'s cases follows.
    pub const FUN: u8 = 16;
    /// The `n`th string, bytes, list, array or map these bytes hold, numbered from 0 in the order
    /// each opens, again: written where a value is shared.
    pub const AGAIN: u8 = 17;
}

/// What follows [`tag::FUN`], `std.value.Fun`'s cases in its order. `NAMED` is followed by the
/// name; `CONST` by its arity and value; `PROJECT` by its arity and the index of the argument it
/// answers; `TABLE` by its arity, how many entries, each entry's key and value, and its default.
pub mod fun {
    pub const NAMED: u8 = 0;
    pub const ANON: u8 = 1;
    pub const CONST: u8 = 2;
    pub const PROJECT: u8 = 3;
    pub const TABLE: u8 = 4;
}

/// Whether a value of `tag` is numbered for [`tag::AGAIN`].
pub fn shareable(t: u8) -> bool {
    matches!(t, tag::STR | tag::BYTES | tag::LIST | tag::ARRAY | tag::MAP)
}

pub fn encode(v: &Value) -> Result<Vec<u8>, String> {
    encode_with(v, &mut |v| {
        Err(format!("a {} is not plain data", v.type_name()))
    })
}

/// `v`'s bytes, each value they cannot hold handed to `handle`, which answers the position it keeps
/// that value at. A function the prover generated is data, and is written whole.
pub fn encode_with(
    v: &Value,
    handle: &mut dyn FnMut(&Value) -> Result<u64, String>,
) -> Result<Vec<u8>, String> {
    let mut w = Writer::new();
    write_value(&mut w, v, handle)?;
    Ok(w.finish())
}

/// Writes `v` whole, each value the bytes cannot hold handed to `handle` as [`encode_with`] does.
pub fn write_value(
    w: &mut Writer,
    v: &Value,
    handle: &mut dyn FnMut(&Value) -> Result<u64, String>,
) -> Result<(), String> {
    match v {
        Value::Unit => w.tag(tag::UNIT),
        &Value::Bool(b) => w.tag(if b { tag::TRUE } else { tag::FALSE }),
        &Value::Int(n) => w.int(n),
        Value::Float(x) => w.float(crate::float::bits(*x)),
        Value::Decimal(d) => w.decimal(d.serialize()),
        Value::Fixed(f) => w.fixed(*f),
        &Value::Char(c) => w.char(u32::from(c)),
        Value::Str(s) => w.str(s.as_bytes()),
        Value::Bytes(b) => w.bytes(b),
        Value::List(items) => {
            w.count(tag::LIST, items.len() as u64);
            for item in items.iter() {
                write_value(w, item, handle)?;
            }
        }
        Value::Array(items) => {
            w.count(tag::ARRAY, items.len() as u64);
            for item in items.iter() {
                write_value(w, item, handle)?;
            }
        }
        Value::Map(m) => {
            w.count(tag::MAP, m.len() as u64);
            for (k, v) in m.iter() {
                write_value(w, k, handle)?;
                write_value(w, v, handle)?;
            }
        }
        Value::Record(fields) => {
            w.count(tag::RECORD, fields.len() as u64);
            for (name, v) in fields.iter() {
                w.name(name.as_bytes());
                write_value(w, v, handle)?;
            }
        }
        Value::Ctor { name, args } => {
            w.ctor(name.as_bytes(), args.len() as u64);
            for arg in args.iter() {
                write_value(w, arg, handle)?;
            }
        }
        Value::Closure(c) => match &c.kind {
            ClosureKind::Synth { arity, rule } => {
                let arity = u64::from(*arity);
                match rule {
                    Synth::Const(x) => {
                        w.fun(fun::CONST);
                        w.varint(arity);
                        write_value(w, x, handle)?;
                    }
                    Synth::Project(index) => {
                        w.fun(fun::PROJECT);
                        w.varint(arity);
                        w.varint(u64::from(*index));
                    }
                    Synth::Table { entries, default } => {
                        w.fun(fun::TABLE);
                        w.varint(arity);
                        w.varint(entries.len() as u64);
                        for (k, x) in entries {
                            write_value(w, k, handle)?;
                            write_value(w, x, handle)?;
                        }
                        write_value(w, default, handle)?;
                    }
                }
            }
            _ => w.handle(handle(v)?),
        },
        Value::Task(_) | Value::Chan(_) | Value::Secret(_) => w.handle(handle(v)?),
    }
    Ok(())
}

pub fn decode(bytes: &[u8]) -> Result<Value, String> {
    decode_with(bytes, &mut |at| {
        Err(format!(
            "handle {at}, and no handles were kept beside the bytes"
        ))
    })
}

/// The value `bytes` hold, each [`tag::HANDLE`] the value `handle` answers for its position, and
/// each [`tag::AGAIN`] a copy of the value it names. A function the program wrote is refused: only
/// its own run can hold one.
pub fn decode_with(
    bytes: &[u8],
    handle: &mut dyn FnMut(u64) -> Result<Value, String>,
) -> Result<Value, String> {
    let mut d = Decoder {
        r: Reader::new(bytes)?,
        handle,
        opened: Vec::new(),
    };
    let v = d.value()?;
    d.r.end()?;
    Ok(v)
}

/// PLV1 written a part at a time: what [`encode`] writes a `Value` with, and what compiled code
/// writes one with through the runtime. Each method writes one whole part; which parts make a value
/// is the caller's to keep.
pub struct Writer {
    out: Vec<u8>,
    // Names repeat in every row of a table, so each is written once and then by number.
    symbols: HashMap<Box<[u8]>, u64>,
}

impl Default for Writer {
    fn default() -> Writer {
        Writer::new()
    }
}

impl Writer {
    /// Bytes holding nothing yet past [`FORMAT`].
    pub fn new() -> Writer {
        Writer {
            out: FORMAT.to_vec(),
            symbols: HashMap::new(),
        }
    }

    /// A value that is its tag alone: [`tag::UNIT`], [`tag::FALSE`] or [`tag::TRUE`].
    pub fn tag(&mut self, t: u8) {
        self.out.push(t);
    }

    pub fn int(&mut self, n: i64) {
        self.out.push(tag::INT);
        self.varint(((n << 1) ^ (n >> 63)) as u64);
    }

    /// A `Float` by its bits, as `float::bits` gives them.
    pub fn float(&mut self, bits: u64) {
        self.out.push(tag::FLOAT);
        self.out.extend_from_slice(&bits.to_le_bytes());
    }

    /// A `Decimal` as `rust_decimal` serializes it.
    pub fn decimal(&mut self, d: [u8; 16]) {
        self.out.push(tag::DECIMAL);
        self.out.extend_from_slice(&d);
    }

    pub fn fixed(&mut self, f: Fixed) {
        self.out.push(tag::FIXED);
        self.out.push(width_index(f.ty));
        // The low word is all a narrower width has; decoding sign-extends it again.
        self.varint(f.bits() as u64);
        if f.ty.bits() == 128 {
            self.varint((f.bits() >> 64) as u64);
        }
    }

    /// A `Char` by its scalar value.
    pub fn char(&mut self, c: u32) {
        self.out.push(tag::CHAR);
        self.varint(u64::from(c));
    }

    /// A `String` by its UTF-8, which a reader checks.
    pub fn str(&mut self, s: &[u8]) {
        self.out.push(tag::STR);
        self.blob(s);
    }

    pub fn bytes(&mut self, b: &[u8]) {
        self.out.push(tag::BYTES);
        self.blob(b);
    }

    /// The head of a list, array or map of `n` items or of a record of `n` fields: `t`, which is
    /// one of those four, and `n`. The items follow, a map's as each key then its value and a
    /// record's as each field's [`Writer::name`] then its value, in name order.
    pub fn count(&mut self, t: u8, n: u64) {
        self.out.push(t);
        self.varint(n);
    }

    /// The head of the constructor `name` given `n` arguments, which follow.
    pub fn ctor(&mut self, name: &[u8], n: u64) {
        self.out.push(tag::CTOR);
        self.name(name);
        self.varint(n);
    }

    /// A record field's name, or a named function's.
    pub fn name(&mut self, name: &[u8]) {
        let next = self.symbols.len() as u64 + 1;
        match self.symbols.get(name) {
            Some(&n) => self.varint(n),
            None => {
                self.symbols.insert(name.into(), next);
                self.varint(0);
                self.blob(name);
            }
        }
    }

    /// Position `at` of the handles kept beside the bytes.
    pub fn handle(&mut self, at: u64) {
        self.out.push(tag::HANDLE);
        self.varint(at);
    }

    /// The `n`th shareable value written before, again.
    pub fn again(&mut self, n: u64) {
        self.out.push(tag::AGAIN);
        self.varint(n);
    }

    /// The head of a function: [`tag::FUN`] and which of [`fun`]'s cases, whose parts follow.
    pub fn fun(&mut self, case: u8) {
        self.out.push(tag::FUN);
        self.out.push(case);
    }

    pub fn varint(&mut self, mut n: u64) {
        while n >= 0x80 {
            self.out.push((n as u8) | 0x80);
            n >>= 7;
        }
        self.out.push(n as u8);
    }

    fn blob(&mut self, b: &[u8]) {
        self.varint(b.len() as u64);
        self.out.extend_from_slice(b);
    }

    pub fn finish(self) -> Vec<u8> {
        self.out
    }
}

/// PLV1 read a part at a time, over bytes it borrows or owns: what [`decode`] reads a `Value` with,
/// and what compiled code reads one with through the runtime. Every read answers an error where
/// the bytes end early or do not hold what it reads.
pub struct Reader<B> {
    bytes: B,
    at: usize,
    symbols: Vec<Symbol>,
    // Set while a value is read a second time, whose names were numbered the first.
    again: bool,
}

impl<B: AsRef<[u8]>> Reader<B> {
    /// A reader at the first value of `bytes`, which open with [`FORMAT`].
    pub fn new(bytes: B) -> Result<Reader<B>, String> {
        if !bytes.as_ref().starts_with(FORMAT) {
            return Err("not an encoded value".into());
        }
        Ok(Reader {
            bytes,
            at: FORMAT.len(),
            symbols: Vec::new(),
            again: false,
        })
    }

    /// Where the next byte is read from.
    pub fn at(&self) -> usize {
        self.at
    }

    /// An error unless every byte has been read.
    pub fn end(&self) -> Result<(), String> {
        let left = self.bytes.as_ref().len() - self.at;
        if left != 0 {
            return Err(format!("{left} bytes follow the value"));
        }
        Ok(())
    }

    pub fn byte(&mut self) -> Result<u8, String> {
        let b = *self
            .bytes
            .as_ref()
            .get(self.at)
            .ok_or("the value ends early")?;
        self.at += 1;
        Ok(b)
    }

    /// Reads a tag, which must be `t`.
    pub fn expect(&mut self, t: u8) -> Result<(), String> {
        match self.byte()? {
            found if found == t => Ok(()),
            found => Err(format!(
                "a value tagged {found} where one tagged {t} was read"
            )),
        }
    }

    pub fn varint(&mut self) -> Result<u64, String> {
        let mut n = 0u64;
        for shift in (0..64).step_by(7) {
            let b = self.byte()?;
            n |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(n);
            }
        }
        Err("a number longer than 64 bits".into())
    }

    /// An `Int`'s body, after its tag.
    pub fn int(&mut self) -> Result<i64, String> {
        let z = self.varint()?;
        Ok(((z >> 1) as i64) ^ -((z & 1) as i64))
    }

    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let b = self
            .bytes
            .as_ref()
            .get(self.at..self.at + N)
            .ok_or("the value ends early")?;
        self.at += N;
        Ok(b.try_into().expect("a slice of N bytes"))
    }

    /// A fixed width's body, after its tag.
    pub fn fixed(&mut self) -> Result<Fixed, String> {
        let ty = *INT_TYPES
            .get(self.byte()? as usize)
            .ok_or("a fixed width this format does not have")?;
        let low = u128::from(self.varint()?);
        let high = if ty.bits() == 128 {
            u128::from(self.varint()?) << 64
        } else {
            0
        };
        Ok(Fixed::new(ty, high | low))
    }

    /// A `Char`'s body, after its tag.
    pub fn char(&mut self) -> Result<char, String> {
        u32::try_from(self.varint()?)
            .ok()
            .and_then(char::from_u32)
            .ok_or_else(|| "a character that is not a Unicode scalar value".into())
    }

    /// A string's or bytes value's body, after its tag.
    pub fn blob(&mut self) -> Result<&[u8], String> {
        let n = self.count()?;
        let at = self.at;
        self.at += n;
        Ok(&self.bytes.as_ref()[at..at + n])
    }

    /// A string's body, after its tag.
    pub fn str(&mut self) -> Result<&str, String> {
        std::str::from_utf8(self.blob()?).map_err(|_| "a string that is not UTF-8".into())
    }

    /// A length no larger than what is left, so a corrupt count cannot ask for a huge allocation.
    pub fn count(&mut self) -> Result<usize, String> {
        let n = self.varint()?;
        if n > (self.bytes.as_ref().len() - self.at) as u64 {
            return Err(format!("a count of {n} with fewer bytes left"));
        }
        Ok(n as usize)
    }

    /// A record field's name, a constructor's or a named function's.
    pub fn symbol(&mut self) -> Result<Symbol, String> {
        match self.varint()? {
            0 => {
                let b = self.blob()?;
                let name =
                    Symbol::new(std::str::from_utf8(b).map_err(|_| "a name that is not UTF-8")?);
                if !self.again {
                    self.symbols.push(name.clone());
                }
                Ok(name)
            }
            n => self
                .symbols
                .get(n as usize - 1)
                .cloned()
                .ok_or_else(|| format!("name {n} before it was written")),
        }
    }
}

struct Decoder<'b, 'h> {
    r: Reader<&'b [u8]>,
    handle: &'h mut dyn FnMut(u64) -> Result<Value, String>,
    /// Where each shareable value opens, and whether it has closed, in the order they open.
    opened: Vec<(usize, bool)>,
}

impl Decoder<'_, '_> {
    fn value(&mut self) -> Result<Value, String> {
        let start = self.r.at();
        let t = self.r.byte()?;
        if !shareable(t) || self.r.again {
            return self.body(t);
        }
        let n = self.opened.len();
        self.opened.push((start, false));
        let v = self.body(t)?;
        self.opened[n].1 = true;
        Ok(v)
    }

    fn values(&mut self, n: usize) -> Result<Vec<Value>, String> {
        let mut items = Vec::with_capacity(n);
        for _ in 0..n {
            items.push(self.value()?);
        }
        Ok(items)
    }

    fn body(&mut self, t: u8) -> Result<Value, String> {
        let r = &mut self.r;
        Ok(match t {
            tag::UNIT => Value::Unit,
            tag::FALSE => Value::Bool(false),
            tag::TRUE => Value::Bool(true),
            tag::INT => Value::Int(r.int()?),
            tag::FLOAT => Value::Float(f64::from_bits(u64::from_le_bytes(r.array()?))),
            tag::DECIMAL => Value::Decimal(Decimal::deserialize(r.array()?)),
            tag::FIXED => Value::Fixed(r.fixed()?),
            tag::CHAR => Value::Char(r.char()?),
            tag::STR => Value::str(r.str()?),
            tag::BYTES => Value::bytes(r.blob()?),
            tag::LIST => {
                let n = r.count()?;
                Value::List(List::from(self.values(n)?))
            }
            tag::ARRAY => {
                let n = r.count()?;
                Value::array(self.values(n)?)
            }
            tag::MAP => {
                let n = r.count()?;
                let mut entries = Vec::with_capacity(n);
                for _ in 0..n {
                    let k = self.value()?;
                    entries.push((k, self.value()?));
                }
                Value::map(entries)
            }
            tag::RECORD => {
                let n = r.count()?;
                let mut fields = Vec::with_capacity(n);
                for _ in 0..n {
                    let name = self.r.symbol()?;
                    fields.push((name, self.value()?));
                }
                Value::Record(Arc::new(Fields::from_unsorted(fields)))
            }
            tag::CTOR => {
                let name = r.symbol()?;
                let n = r.count()?;
                Value::ctor(name, self.values(n)?)
            }
            tag::HANDLE => {
                let at = r.varint()?;
                (self.handle)(at)?
            }
            tag::FUN => self.function()?,
            tag::AGAIN => {
                let n = r.varint()?;
                let &(at, closed) = usize::try_from(n)
                    .ok()
                    .and_then(|n| self.opened.get(n))
                    .ok_or_else(|| format!("value {n} again before it was written"))?;
                if !closed {
                    return Err(format!("value {n} again inside itself"));
                }
                // Read again from where it opened: its names were numbered the first time.
                let (was_at, was_again) = (self.r.at, self.r.again);
                (self.r.at, self.r.again) = (at, true);
                let v = self.value();
                (self.r.at, self.r.again) = (was_at, was_again);
                v?
            }
            other => {
                return Err(format!(
                    "a value tagged {other}, which this format does not have"
                ));
            }
        })
    }

    fn function(&mut self) -> Result<Value, String> {
        let case = self.r.byte()?;
        let synth = |arity: u64, rule: Synth| -> Result<Value, String> {
            let arity = u8::try_from(arity).map_err(|_| "a function of more than 255 arguments")?;
            Ok(Value::Closure(Arc::new(Closure {
                name: None,
                kind: ClosureKind::Synth { arity, rule },
            })))
        };
        match case {
            fun::NAMED | fun::ANON => {
                if case == fun::NAMED {
                    self.r.symbol()?;
                }
                Err("a function the program wrote, which only its own run can hold".into())
            }
            fun::CONST => {
                let arity = self.r.varint()?;
                synth(arity, Synth::Const(self.value()?))
            }
            fun::PROJECT => {
                let arity = self.r.varint()?;
                let index = self.r.varint()?;
                if index >= arity {
                    return Err("a generated function whose argument is not one of its own".into());
                }
                synth(arity, Synth::Project(index as u8))
            }
            fun::TABLE => {
                let arity = self.r.varint()?;
                if arity == 0 {
                    return Err("a table of no arguments".into());
                }
                let n = self.r.count()?;
                let mut entries = Vec::with_capacity(n);
                for _ in 0..n {
                    let k = self.value()?;
                    entries.push((k, self.value()?));
                }
                let default = self.value()?;
                synth(arity, Synth::Table { entries, default })
            }
            other => Err(format!(
                "a function of case {other}, which this format does not have"
            )),
        }
    }
}

fn width_index(ty: IntTy) -> u8 {
    INT_TYPES
        .iter()
        .position(|t| *t == ty)
        .expect("every width is in INT_TYPES") as u8
}
