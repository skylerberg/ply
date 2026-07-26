//! Tagged, length-prefixed primitives for the front-end cache's entry payloads.
//!
//! Every composite writes a discriminant byte before its fields and a terminator
//! after them, and every variable-length field carries its own length. That is
//! the difference between this and a `serde`-derived binary encoding: there, the
//! shape is implicit in field declaration order, so swapping two fields of the
//! same type is a silent wire change that the decoder reads happily as the old
//! shape. Here it is a tag the decoder refuses.
//!
//! Refusing matters more than it sounds: a misparsed entry is a wrong
//! `Footprint`, and footprints decide which tests may run concurrently.

use crate::ContentHash;
use ply_hash::DefHash;
use ply_span::Symbol;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct DecodeError {
    pub(crate) what: &'static str,
    pub(crate) at: usize,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at byte {}", self.what, self.at)
    }
}

pub(crate) type Decoded<T> = Result<T, DecodeError>;

#[derive(Default)]
pub(crate) struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub(crate) fn new() -> Writer {
        Writer::default()
    }

    pub(crate) fn tag(&mut self, tag: u8) {
        self.buf.push(tag);
    }

    pub(crate) fn bool(&mut self, value: bool) {
        self.buf.push(u8::from(value));
    }

    pub(crate) fn u32(&mut self, value: u32) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    /// A collection's element count. Saturating rather than fallible: a cached
    /// entry with four billion members cannot exist, and a decoder that reads a
    /// count larger than the bytes that could hold it rejects the frame anyway.
    pub(crate) fn count(&mut self, count: usize) {
        self.u32(u32::try_from(count).unwrap_or(u32::MAX));
    }

    pub(crate) fn bytes(&mut self, bytes: &[u8]) {
        self.count(bytes.len());
        self.buf.extend_from_slice(bytes);
    }

    pub(crate) fn text(&mut self, text: &str) {
        self.bytes(text.as_bytes());
    }

    pub(crate) fn symbol(&mut self, symbol: &Symbol) {
        self.text(symbol.as_str());
    }

    pub(crate) fn def_hash(&mut self, hash: DefHash) {
        self.buf.extend_from_slice(&hash.0);
    }

    pub(crate) fn content_hash(&mut self, hash: ContentHash) {
        self.buf.extend_from_slice(&hash.0);
    }

    pub(crate) fn finish(self) -> Vec<u8> {
        self.buf
    }
}

pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, pos: 0 }
    }

    pub(crate) fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }

    fn take(&mut self, n: usize, what: &'static str) -> Decoded<&'a [u8]> {
        if self.remaining() < n {
            return Err(DecodeError { what, at: self.pos });
        }
        let out = &self.bytes[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    pub(crate) fn tag(&mut self, expected: u8, what: &'static str) -> Decoded<()> {
        let at = self.pos;
        let found = self.take(1, what)?[0];
        if found != expected {
            return Err(DecodeError { what, at });
        }
        Ok(())
    }

    pub(crate) fn byte(&mut self, what: &'static str) -> Decoded<u8> {
        Ok(self.take(1, what)?[0])
    }

    pub(crate) fn bool(&mut self, what: &'static str) -> Decoded<bool> {
        let at = self.pos;
        match self.byte(what)? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(DecodeError { what, at }),
        }
    }

    pub(crate) fn u32(&mut self, what: &'static str) -> Decoded<u32> {
        let bytes = self.take(4, what)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// Rejected when it exceeds the bytes left, since every element occupies at
    /// least one. Without that a corrupt length reserves gigabytes before the
    /// frame is found to be nonsense.
    pub(crate) fn count(&mut self, what: &'static str) -> Decoded<usize> {
        let at = self.pos;
        let count = self.u32(what)? as usize;
        if count > self.remaining() {
            return Err(DecodeError { what, at });
        }
        Ok(count)
    }

    pub(crate) fn bytes(&mut self, what: &'static str) -> Decoded<&'a [u8]> {
        let len = self.count(what)?;
        self.take(len, what)
    }

    pub(crate) fn text(&mut self, what: &'static str) -> Decoded<&'a str> {
        let at = self.pos;
        let bytes = self.bytes(what)?;
        std::str::from_utf8(bytes).map_err(|_| DecodeError { what, at })
    }

    pub(crate) fn symbol(&mut self, what: &'static str) -> Decoded<Symbol> {
        Ok(Symbol::new(self.text(what)?))
    }

    pub(crate) fn def_hash(&mut self, what: &'static str) -> Decoded<DefHash> {
        let mut out = [0u8; 32];
        out.copy_from_slice(self.take(32, what)?);
        Ok(DefHash(out))
    }

    pub(crate) fn content_hash(&mut self, what: &'static str) -> Decoded<ContentHash> {
        let mut out = [0u8; 32];
        out.copy_from_slice(self.take(32, what)?);
        Ok(ContentHash(out))
    }

    /// Trailing bytes mean the payload was written by something that does not
    /// agree with this decoder about the shape, which is exactly the case that
    /// must not be read as a value.
    pub(crate) fn end(self, what: &'static str) -> Decoded<()> {
        if self.remaining() != 0 {
            return Err(DecodeError { what, at: self.pos });
        }
        Ok(())
    }
}
