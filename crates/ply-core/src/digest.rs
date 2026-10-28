//! A value's content digest: BLAKE3 over an encoding in which values `==` calls equal are one run
//! of bytes. A constructor is written under the name its module declares, so moving a module moves
//! no digest.

use crate::limit::grow;
use crate::{Decimal, Diagnostic, IntTy, Span, Value, codes};

/// `b3:` and a digest's first twelve hex characters: every digest Ply prints for a person to
/// compare is written this way.
pub fn short(digest: &[u8; 32]) -> String {
    let mut out = String::from("b3:");
    for byte in &digest[..6] {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// The digest, or the refusal of a value `derivable(hash, ·)` admits no type of.
pub fn digest(v: &Value, span: Span) -> Result<[u8; 32], Diagnostic> {
    let mut h = blake3::Hasher::new();
    write(&mut h, v, span)?;
    Ok(*h.finalize().as_bytes())
}

/// The encoding [`digest`] hashes, or the same refusal.
pub fn encoded(v: &Value, span: Span) -> Result<Vec<u8>, Diagnostic> {
    let mut out = Vec::new();
    write(&mut out, v, span)?;
    Ok(out)
}

/// A scalar as compiled code holds it, which the encoding writes alone: a width by its type and bits.
pub enum Scalar {
    Unit,
    Bool(bool),
    Int(i64),
    Width(IntTy, u128),
    Char(u32),
    Decimal(Decimal),
}

/// `s`'s encoding: what [`digest`] hashes of it.
pub fn scalar(s: Scalar) -> Vec<u8> {
    let mut out = Vec::new();
    write_scalar(&mut out, s);
    out
}

/// A string's encoding where `text`, else a byte string's.
pub fn text(text: bool, b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    blob(&mut out, if text { 7 } else { 8 }, b);
    out
}

/// Where an encoding is written: a hasher, or the bytes themselves.
trait Sink {
    fn update(&mut self, b: &[u8]);
}

impl Sink for blake3::Hasher {
    fn update(&mut self, b: &[u8]) {
        blake3::Hasher::update(self, b);
    }
}

impl Sink for Vec<u8> {
    fn update(&mut self, b: &[u8]) {
        self.extend_from_slice(b);
    }
}

fn write(h: &mut impl Sink, v: &Value, span: Span) -> Result<(), Diagnostic> {
    match v {
        Value::Unit => write_scalar(h, Scalar::Unit),
        Value::Bool(b) => write_scalar(h, Scalar::Bool(*b)),
        Value::Int(n) => write_scalar(h, Scalar::Int(*n)),
        Value::Fixed(f) => write_scalar(h, Scalar::Width(f.ty, f.bits())),
        Value::Char(c) => write_scalar(h, Scalar::Char(u32::from(*c))),
        Value::Decimal(d) => write_scalar(h, Scalar::Decimal(*d)),
        Value::Str(s) => blob(h, 7, s.as_bytes()),
        Value::Bytes(b) => blob(h, 8, b),
        Value::List(items) => {
            count(h, 9, items.len());
            for x in items.iter() {
                grow(|| write(h, x, span))?;
            }
        }
        Value::Array(items) => {
            count(h, 10, items.len());
            for x in items.iter() {
                grow(|| write(h, x, span))?;
            }
        }
        Value::Map(m) => {
            count(h, 11, m.len());
            for (k, x) in m.iter() {
                grow(|| write(h, k, span))?;
                grow(|| write(h, x, span))?;
            }
        }
        Value::Record(fields) => {
            count(h, 12, fields.len());
            for (name, x) in fields.iter() {
                blob(h, 13, name.as_str().as_bytes());
                grow(|| write(h, x, span))?;
            }
        }
        Value::Ctor { name, args } => {
            // Values of a keyed type are equal as their keys are, so the key is what is written.
            if let Some(key) = crate::instances::key(v) {
                tag(h, 16);
                return grow(|| write(h, &key, span));
            }
            let simple = name.as_str().rsplit('.').next().unwrap_or(name.as_str());
            blob(h, 14, simple.as_bytes());
            count(h, 15, args.len());
            for x in args.iter() {
                grow(|| write(h, x, span))?;
            }
        }
        Value::Float(_) | Value::Closure(_) | Value::Secret(_) => {
            return Err(Diagnostic::error(
                codes::RUNTIME_ERROR,
                format!("`digest` cannot hash a {}", v.type_name()),
            )
            .primary(span, "no hash agrees with `==` here")
            .note(
                "reaching this is a defect in Ply: `derivable(hash, a)` refuses this type at \
                 compile time",
            ));
        }
    }
    Ok(())
}

fn write_scalar(h: &mut impl Sink, s: Scalar) {
    match s {
        Scalar::Unit => tag(h, 0),
        Scalar::Bool(b) => tag(h, if b { 2 } else { 1 }),
        Scalar::Int(n) => int(h, n),
        Scalar::Width(ty, bits) => {
            let bits = ty.normalize(bits);
            // Below 64 bits a width is the `Int` it reads as, which is how compiled code holds it.
            if ty.bits() < 64 {
                int(h, bits as i64);
            } else {
                let raw = if ty.bits() == 128 {
                    bits
                } else {
                    bits & (u128::MAX >> (128 - ty.bits()))
                };
                tag(h, 4);
                h.update(&[ty as u8]);
                h.update(&raw.to_le_bytes());
            }
        }
        Scalar::Char(c) => {
            tag(h, 5);
            h.update(&c.to_le_bytes());
        }
        Scalar::Decimal(d) => {
            tag(h, 6);
            h.update(&d.normalize().serialize());
        }
    }
}

fn tag(h: &mut impl Sink, t: u8) {
    h.update(&[t]);
}

fn int(h: &mut impl Sink, n: i64) {
    tag(h, 3);
    h.update(&n.to_le_bytes());
}

fn count(h: &mut impl Sink, t: u8, n: usize) {
    tag(h, t);
    h.update(&(n as u64).to_le_bytes());
}

fn blob(h: &mut impl Sink, t: u8, b: &[u8]) {
    count(h, t, b.len());
    h.update(b);
}
