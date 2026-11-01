use crate::{Fixed, INT_TYPES, IntegerType, Symbol};
use std::collections::HashMap;

pub const FORMAT: &[u8; 4] = b"PLV1";

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
    pub const HANDLE: u8 = 15;
    pub const FUN: u8 = 16;
    pub const BACK_REFERENCE: u8 = 17;
}

pub mod fun {
    pub const NAMED: u8 = 0;
    pub const ANON: u8 = 1;
    pub const CONST: u8 = 2;
    pub const PROJECT: u8 = 3;
    pub const TABLE: u8 = 4;
    pub const RESUMPTION: u8 = 5;
}

pub mod shown {
    pub const SEALED: u64 = 0;
    pub const SCOPED: u64 = 1;
}

pub fn shareable(t: u8) -> bool {
    matches!(t, tag::STR | tag::BYTES | tag::LIST | tag::ARRAY | tag::MAP)
}

pub struct Writer {
    out: Vec<u8>,
    symbols: HashMap<Box<[u8]>, u64>,
}

impl Default for Writer {
    fn default() -> Writer {
        Writer::new()
    }
}

impl Writer {
    pub fn new() -> Writer {
        Writer {
            out: FORMAT.to_vec(),
            symbols: HashMap::new(),
        }
    }

    pub fn tag(&mut self, t: u8) {
        self.out.push(t);
    }

    pub fn int(&mut self, n: i64) {
        self.out.push(tag::INT);
        self.varint(((n << 1) ^ (n >> 63)) as u64);
    }

    pub fn float(&mut self, bits: u64) {
        self.out.push(tag::FLOAT);
        self.out.extend_from_slice(&bits.to_le_bytes());
    }

    pub fn decimal(&mut self, d: [u8; 16]) {
        self.out.push(tag::DECIMAL);
        self.out.extend_from_slice(&d);
    }

    pub fn fixed(&mut self, f: Fixed) {
        self.out.push(tag::FIXED);
        self.out.push(width_index(f.ty));
        self.varint(f.bits() as u64);
        if f.ty.bits() == 128 {
            self.varint((f.bits() >> 64) as u64);
        }
    }

    pub fn char(&mut self, c: u32) {
        self.out.push(tag::CHAR);
        self.varint(u64::from(c));
    }

    pub fn str(&mut self, s: &[u8]) {
        self.out.push(tag::STR);
        self.blob(s);
    }

    pub fn bytes(&mut self, b: &[u8]) {
        self.out.push(tag::BYTES);
        self.blob(b);
    }

    pub fn count(&mut self, t: u8, n: u64) {
        self.out.push(t);
        self.varint(n);
    }

    pub fn ctor(&mut self, name: &[u8], n: u64) {
        self.out.push(tag::CTOR);
        self.name(name);
        self.varint(n);
    }

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

    pub fn handle(&mut self, at: u64) {
        self.out.push(tag::HANDLE);
        self.varint(at);
    }

    pub fn again(&mut self, n: u64) {
        self.out.push(tag::BACK_REFERENCE);
        self.varint(n);
    }

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

pub struct Reader<B> {
    bytes: B,
    at: usize,
    symbols: Vec<Symbol>,
}

impl<B: AsRef<[u8]>> Reader<B> {
    pub fn new(bytes: B) -> Result<Reader<B>, String> {
        if !bytes.as_ref().starts_with(FORMAT) {
            return Err("not an encoded value".into());
        }
        Ok(Reader {
            bytes,
            at: FORMAT.len(),
            symbols: Vec::new(),
        })
    }

    pub fn at(&self) -> usize {
        self.at
    }

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

    pub fn char(&mut self) -> Result<char, String> {
        u32::try_from(self.varint()?)
            .ok()
            .and_then(char::from_u32)
            .ok_or_else(|| "a character that is not a Unicode scalar value".into())
    }

    pub fn blob(&mut self) -> Result<&[u8], String> {
        let n = self.count()?;
        let at = self.at;
        self.at += n;
        Ok(&self.bytes.as_ref()[at..at + n])
    }

    pub fn str(&mut self) -> Result<&str, String> {
        std::str::from_utf8(self.blob()?).map_err(|_| "a string that is not UTF-8".into())
    }

    pub fn count(&mut self) -> Result<usize, String> {
        let n = self.varint()?;
        if n > (self.bytes.as_ref().len() - self.at) as u64 {
            return Err(format!("a count of {n} with fewer bytes left"));
        }
        Ok(n as usize)
    }

    pub fn symbol(&mut self) -> Result<Symbol, String> {
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
}

fn width_index(ty: IntegerType) -> u8 {
    INT_TYPES
        .iter()
        .position(|t| *t == ty)
        .expect("every width is in INT_TYPES") as u8
}
