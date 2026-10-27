//! The flat format the runtime and `std.machine` exchange machine data in: `std.bin`'s encoding of
//! a sequence of primitives, which the module reads into its own types and the runtime writes and
//! reads by position, naming nothing. A count or a tag is a LEB128 varint, an integer its zigzag as
//! one, a `Bool` one byte, a `Float` its bits as eight bytes little-endian, bytes and text a count
//! and the bytes, a list a count and its items, an option a tag (0 none, 1 some) and its value.

use ply_core::{Fun, Plain};

/// Bytes being written.
#[derive(Default)]
pub struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    pub fn new() -> Writer {
        Writer::default()
    }

    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }

    pub fn varint(&mut self, n: u64) -> &mut Writer {
        self.wide(u128::from(n))
    }

    /// A varint of up to 128 bits, as `std.bin` writes a `U128`.
    fn wide(&mut self, mut n: u128) -> &mut Writer {
        while n >= 0x80 {
            self.bytes.push((n as u8 & 0x7f) | 0x80);
            n >>= 7;
        }
        self.bytes.push(n as u8);
        self
    }

    pub fn int(&mut self, n: i64) -> &mut Writer {
        self.varint(((n << 1) ^ (n >> 63)) as u64)
    }

    /// A count or a figure as an `Int`, which none here has past `i64::MAX`.
    pub fn count(&mut self, n: u64) -> &mut Writer {
        self.int(i64::try_from(n).unwrap_or(i64::MAX))
    }

    pub fn tag(&mut self, t: u64) -> &mut Writer {
        self.varint(t)
    }

    pub fn bool(&mut self, b: bool) -> &mut Writer {
        self.bytes.push(u8::from(b));
        self
    }

    pub fn byte(&mut self, b: u8) -> &mut Writer {
        self.bytes.push(b);
        self
    }

    pub fn bytes(&mut self, b: &[u8]) -> &mut Writer {
        self.varint(b.len() as u64);
        self.bytes.extend_from_slice(b);
        self
    }

    pub fn str(&mut self, s: &str) -> &mut Writer {
        self.bytes(s.as_bytes())
    }

    /// What another writer of this format wrote, as part of what this one writes.
    pub fn written(&mut self, b: &[u8]) -> &mut Writer {
        self.bytes.extend_from_slice(b);
        self
    }

    /// An option: none, or `write` of its value.
    pub fn option<T>(&mut self, v: Option<T>, write: impl FnOnce(&mut Writer, T)) -> &mut Writer {
        match v {
            None => {
                self.tag(0);
            }
            Some(v) => {
                self.tag(1);
                write(self, v);
            }
        }
        self
    }

    /// A list: its count, and `write` of each item.
    pub fn list<T>(
        &mut self,
        items: impl ExactSizeIterator<Item = T>,
        mut write: impl FnMut(&mut Writer, T),
    ) -> &mut Writer {
        self.varint(items.len() as u64);
        for item in items {
            write(self, item);
        }
        self
    }

    /// A value a diagnostic shows, as `std.value.Value`: its case's tag, then what the case holds.
    // Tags 15 and 16 stay unused, so a toolchain of another build reads the others alike.
    pub fn plain(&mut self, p: &Plain) -> &mut Writer {
        match p {
            Plain::Unit => self.tag(0),
            Plain::Bool(b) => self.tag(1).bool(*b),
            Plain::Int(n) => self.tag(2).int(*n),
            Plain::Float(f) => {
                self.tag(3);
                self.bytes
                    .extend_from_slice(&ply_core::float::bits(*f).to_le_bytes());
                self
            }
            Plain::Decimal(d) => self.tag(4).str(&d.to_string()),
            Plain::Fixed { ty, bits } => self.tag(5).str(ty.name()).wide(*bits),
            Plain::Char(c) => self.tag(6).varint(u64::from(*c)),
            Plain::Str(s) => self.tag(7).str(s),
            Plain::Bytes(b) => self.tag(8).bytes(b),
            Plain::List(items) => self.tag(9).list(items.iter(), |w, p| {
                w.plain(p);
            }),
            Plain::Record(fields) => self.tag(10).list(fields.iter(), |w, (name, p)| {
                w.str(name).plain(p);
            }),
            Plain::Ctor(name, args) => self.tag(11).str(name).list(args.iter(), |w, p| {
                w.plain(p);
            }),
            Plain::Map(entries) => self.tag(12).list(entries.iter(), |w, (k, v)| {
                w.plain(k).plain(v);
            }),
            Plain::Fn(f) => self.tag(13).fun(f),
            Plain::Hold => self.tag(14),
            Plain::Secret => self.tag(17),
            Plain::Elided(n) => self.tag(18).count(*n),
            Plain::Array(items) => self.tag(19).list(items.iter(), |w, p| {
                w.plain(p);
            }),
        }
    }

    fn fun(&mut self, f: &Fun) -> &mut Writer {
        match f {
            Fun::Named(name) => self.tag(0).str(name),
            Fun::Anonymous => self.tag(1),
            Fun::Const { arity, value } => self.tag(2).byte(*arity).plain(value),
            Fun::Project { arity, index } => self.tag(3).byte(*arity).byte(*index),
            Fun::Table {
                arity,
                entries,
                default,
            } => self
                .tag(4)
                .byte(*arity)
                .list(entries.iter(), |w, (k, v)| {
                    w.plain(k).plain(v);
                })
                .plain(default),
        }
    }
}

/// Bytes being read, each read refusing what no writer of the format writes.
pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, at: 0 }
    }

    /// Refuses bytes left over.
    pub fn end(&self) -> Result<(), String> {
        if self.at == self.bytes.len() {
            Ok(())
        } else {
            Err(format!("bytes follow the value at {}", self.at))
        }
    }

    pub fn varint(&mut self) -> Result<u64, String> {
        let mut n = 0u64;
        for shift in (0..64).step_by(7) {
            let b = *self
                .bytes
                .get(self.at)
                .ok_or_else(|| format!("the bytes end inside a varint at {}", self.at))?;
            self.at += 1;
            n |= u64::from(b & 0x7f) << shift;
            if b < 0x80 {
                return Ok(n);
            }
        }
        Err(format!("a varint wider than 64 bits at {}", self.at))
    }

    pub fn int(&mut self) -> Result<i64, String> {
        let z = self.varint()?;
        Ok(((z >> 1) as i64) ^ -((z & 1) as i64))
    }

    /// An `Int` that is a count or a figure: never below 0.
    pub fn count(&mut self) -> Result<u64, String> {
        let at = self.at;
        u64::try_from(self.int()?).map_err(|_| format!("a negative count at {at}"))
    }

    /// A tag below `count`.
    pub fn tag(&mut self, count: u64) -> Result<u64, String> {
        let at = self.at;
        let t = self.varint()?;
        if t < count {
            Ok(t)
        } else {
            Err(format!("tag {t} of {count} at {at}"))
        }
    }

    pub fn bool(&mut self) -> Result<bool, String> {
        match self.bytes.get(self.at) {
            Some(b @ (0 | 1)) => {
                self.at += 1;
                Ok(*b == 1)
            }
            _ => Err(format!("no boolean at {}", self.at)),
        }
    }

    /// The length of what follows, which the bytes left hold.
    fn length(&mut self) -> Result<usize, String> {
        let at = self.at;
        let n = self.varint()?;
        usize::try_from(n)
            .ok()
            .filter(|n| *n <= self.bytes.len() - self.at)
            .ok_or_else(|| format!("a length of {n} past the bytes left at {at}"))
    }

    pub fn bytes(&mut self) -> Result<&'a [u8], String> {
        let n = self.length()?;
        let read = &self.bytes[self.at..self.at + n];
        self.at += n;
        Ok(read)
    }

    pub fn str(&mut self) -> Result<String, String> {
        let at = self.at;
        String::from_utf8(self.bytes()?.to_vec())
            .map_err(|_| format!("text that is not UTF-8 at {at}"))
    }

    pub fn option<T>(
        &mut self,
        read: impl FnOnce(&mut Reader<'a>) -> Result<T, String>,
    ) -> Result<Option<T>, String> {
        match self.tag(2)? {
            0 => Ok(None),
            _ => read(self).map(Some),
        }
    }

    pub fn list<T>(
        &mut self,
        mut read: impl FnMut(&mut Reader<'a>) -> Result<T, String>,
    ) -> Result<Vec<T>, String> {
        let n = self.length()?;
        (0..n).map(|_| read(self)).collect()
    }
}
