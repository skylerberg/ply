//! Plain values as bytes: how a cache keeps an answer it does not interpret. A closure, cell, task
//! or secret has no bytes.

use crate::value::{Decimal, Fields, Fixed, Value};
use crate::{INT_TYPES, IntTy, List, Symbol};
use std::collections::HashMap;
use std::sync::Arc;

const FORMAT: &[u8; 4] = b"PLV1";

const UNIT: u8 = 0;
const FALSE: u8 = 1;
const TRUE: u8 = 2;
const INT: u8 = 3;
const FLOAT: u8 = 4;
const DECIMAL: u8 = 5;
const FIXED: u8 = 6;
const STR: u8 = 7;
const BYTES: u8 = 8;
const LIST: u8 = 9;
const MAP: u8 = 10;
const RECORD: u8 = 11;
const CTOR: u8 = 12;
const CHAR: u8 = 13;
const ARRAY: u8 = 14;

pub fn encode(v: &Value) -> Result<Vec<u8>, String> {
    let mut e = Encoder {
        out: FORMAT.to_vec(),
        symbols: HashMap::new(),
    };
    e.value(v)?;
    Ok(e.out)
}

pub fn decode(bytes: &[u8]) -> Result<Value, String> {
    let body = bytes
        .strip_prefix(FORMAT.as_slice())
        .ok_or("not an encoded value")?;
    let mut d = Decoder {
        bytes: body,
        at: 0,
        symbols: Vec::new(),
    };
    let v = d.value()?;
    if d.at != body.len() {
        return Err(format!("{} bytes follow the value", body.len() - d.at));
    }
    Ok(v)
}

struct Encoder<'v> {
    out: Vec<u8>,
    // Names repeat in every row of a table, so each is written once and then by number.
    symbols: HashMap<&'v str, u64>,
}

impl<'v> Encoder<'v> {
    fn value(&mut self, v: &'v Value) -> Result<(), String> {
        match v {
            Value::Unit => self.out.push(UNIT),
            Value::Bool(false) => self.out.push(FALSE),
            Value::Bool(true) => self.out.push(TRUE),
            &Value::Int(n) => {
                self.out.push(INT);
                self.varint(((n << 1) ^ (n >> 63)) as u64);
            }
            Value::Float(x) => {
                self.out.push(FLOAT);
                self.out.extend_from_slice(&x.to_bits().to_le_bytes());
            }
            Value::Decimal(d) => {
                self.out.push(DECIMAL);
                self.out.extend_from_slice(&d.serialize());
            }
            Value::Fixed(f) => {
                self.out.push(FIXED);
                self.out.push(width_index(f.ty));
                // The low word is all a narrower width has; decoding sign-extends it again.
                self.varint(f.bits() as u64);
                if f.ty.bits() == 128 {
                    self.varint((f.bits() >> 64) as u64);
                }
            }
            &Value::Char(c) => {
                self.out.push(CHAR);
                self.varint(u64::from(u32::from(c)));
            }
            Value::Str(s) => {
                self.out.push(STR);
                self.blob(s.as_bytes());
            }
            Value::Bytes(b) => {
                self.out.push(BYTES);
                self.blob(b);
            }
            Value::List(items) => {
                self.out.push(LIST);
                self.varint(items.len() as u64);
                for item in items.iter() {
                    self.value(item)?;
                }
            }
            Value::Array(items) => {
                self.out.push(ARRAY);
                self.varint(items.len() as u64);
                for item in items.iter() {
                    self.value(item)?;
                }
            }
            Value::Map(m) => {
                self.out.push(MAP);
                self.varint(m.size() as u64);
                for (k, v) in m.iter() {
                    self.value(k)?;
                    self.value(v)?;
                }
            }
            Value::Record(fields) => {
                self.out.push(RECORD);
                self.varint(fields.len() as u64);
                for (name, v) in fields.iter() {
                    self.symbol(name);
                    self.value(v)?;
                }
            }
            Value::Ctor { name, args } => {
                self.out.push(CTOR);
                self.symbol(name);
                self.varint(args.len() as u64);
                for arg in args.iter() {
                    self.value(arg)?;
                }
            }
            Value::Closure(_)
            | Value::Cell(_)
            | Value::Task(_)
            | Value::Chan(_)
            | Value::Secret(_) => {
                return Err(format!("a {} is not plain data", v.type_name()));
            }
        }
        Ok(())
    }

    fn symbol(&mut self, name: &'v Symbol) {
        let next = self.symbols.len() as u64 + 1;
        match self.symbols.get(name.as_str()) {
            Some(&n) => self.varint(n),
            None => {
                self.symbols.insert(name.as_str(), next);
                self.varint(0);
                self.blob(name.as_str().as_bytes());
            }
        }
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

struct Decoder<'b> {
    bytes: &'b [u8],
    at: usize,
    symbols: Vec<Symbol>,
}

impl Decoder<'_> {
    fn value(&mut self) -> Result<Value, String> {
        let tag = self.byte()?;
        Ok(match tag {
            UNIT => Value::Unit,
            FALSE => Value::Bool(false),
            TRUE => Value::Bool(true),
            INT => {
                let z = self.varint()?;
                Value::Int(((z >> 1) as i64) ^ -((z & 1) as i64))
            }
            FLOAT => Value::Float(f64::from_bits(u64::from_le_bytes(self.array()?))),
            DECIMAL => Value::Decimal(Decimal::deserialize(self.array()?)),
            FIXED => {
                let ty = *INT_TYPES
                    .get(self.byte()? as usize)
                    .ok_or("a fixed width this format does not have")?;
                let low = u128::from(self.varint()?);
                let high = if ty.bits() == 128 {
                    u128::from(self.varint()?) << 64
                } else {
                    0
                };
                Value::Fixed(Fixed::new(ty, high | low))
            }
            CHAR => Value::Char(
                u32::try_from(self.varint()?)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or("a character that is not a Unicode scalar value")?,
            ),
            STR => {
                let b = self.blob()?;
                Value::str(std::str::from_utf8(b).map_err(|_| "a string that is not UTF-8")?)
            }
            BYTES => Value::bytes(self.blob()?),
            LIST => {
                let n = self.count()?;
                let mut items = Vec::with_capacity(n);
                for _ in 0..n {
                    items.push(self.value()?);
                }
                Value::List(List::from(items))
            }
            ARRAY => {
                let n = self.count()?;
                let mut items = Vec::with_capacity(n);
                for _ in 0..n {
                    items.push(self.value()?);
                }
                Value::array(items)
            }
            MAP => {
                let n = self.count()?;
                let mut entries = Vec::with_capacity(n);
                for _ in 0..n {
                    let k = self.value()?;
                    entries.push((k, self.value()?));
                }
                Value::map(entries)
            }
            RECORD => {
                let n = self.count()?;
                let mut fields = Vec::with_capacity(n);
                for _ in 0..n {
                    let name = self.symbol()?;
                    fields.push((name, self.value()?));
                }
                Value::Record(Arc::new(Fields::from_unsorted(fields)))
            }
            CTOR => {
                let name = self.symbol()?;
                let n = self.count()?;
                let mut args = Vec::with_capacity(n);
                for _ in 0..n {
                    args.push(self.value()?);
                }
                Value::ctor(name, args)
            }
            other => {
                return Err(format!(
                    "a value tagged {other}, which this format does not have"
                ));
            }
        })
    }

    fn symbol(&mut self) -> Result<Symbol, String> {
        match self.varint()? {
            0 => {
                let b = self.blob()?;
                let name =
                    Symbol::new(std::str::from_utf8(b).map_err(|_| "a name that is not UTF-8")?);
                self.symbols.push(name.clone());
                Ok(name)
            }
            n => self
                .symbols
                .get(n as usize - 1)
                .cloned()
                .ok_or_else(|| format!("name {n} before it was written")),
        }
    }

    fn byte(&mut self) -> Result<u8, String> {
        let b = *self.bytes.get(self.at).ok_or("the value ends early")?;
        self.at += 1;
        Ok(b)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let b = self
            .bytes
            .get(self.at..self.at + N)
            .ok_or("the value ends early")?;
        self.at += N;
        Ok(b.try_into().expect("a slice of N bytes"))
    }

    fn blob(&mut self) -> Result<&[u8], String> {
        let n = self.count()?;
        let b = self
            .bytes
            .get(self.at..self.at + n)
            .ok_or("the value ends early")?;
        self.at += n;
        Ok(b)
    }

    /// A length no larger than what is left, so a corrupt count cannot ask for a huge allocation.
    fn count(&mut self) -> Result<usize, String> {
        let n = self.varint()?;
        if n > (self.bytes.len() - self.at) as u64 {
            return Err(format!("a count of {n} with fewer bytes left"));
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
        Err("a number longer than 64 bits".into())
    }
}

fn width_index(ty: IntTy) -> u8 {
    INT_TYPES
        .iter()
        .position(|t| *t == ty)
        .expect("every width is in INT_TYPES") as u8
}
