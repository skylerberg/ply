//! A value's content digest: BLAKE3 over an encoding in which values `==` calls equal are one run
//! of bytes. A constructor is written under the name its module declares, so moving a module moves
//! no digest.

use crate::limit::grow;
use crate::{Diagnostic, Span, Value, codes};

/// The digest, or the refusal of a value `derivable(hash, ·)` admits no type of.
pub fn digest(v: &Value, span: Span) -> Result<[u8; 32], Diagnostic> {
    let mut h = blake3::Hasher::new();
    write(&mut h, v, span)?;
    Ok(*h.finalize().as_bytes())
}

fn write(h: &mut blake3::Hasher, v: &Value, span: Span) -> Result<(), Diagnostic> {
    match v {
        Value::Unit => tag(h, 0),
        Value::Bool(b) => tag(h, if *b { 2 } else { 1 }),
        Value::Int(n) => int(h, *n),
        // Below 64 bits a width is the `Int` it reads as, which is how compiled code holds it.
        Value::Fixed(f) if f.ty.bits() < 64 => int(h, f.bits() as i64),
        Value::Fixed(f) => {
            tag(h, 4);
            h.update(&[f.ty as u8]);
            h.update(&f.raw().to_le_bytes());
        }
        Value::Char(c) => {
            tag(h, 5);
            h.update(&u32::from(*c).to_le_bytes());
        }
        Value::Decimal(d) => {
            tag(h, 6);
            h.update(&d.normalize().serialize());
        }
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
            count(h, 11, m.size());
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
            let simple = name.as_str().rsplit('.').next().unwrap_or(name.as_str());
            blob(h, 14, simple.as_bytes());
            count(h, 15, args.len());
            for x in args.iter() {
                grow(|| write(h, x, span))?;
            }
        }
        Value::Float(_)
        | Value::Closure(_)
        | Value::Cell(_)
        | Value::Task(_)
        | Value::Secret(_) => {
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

fn tag(h: &mut blake3::Hasher, t: u8) {
    h.update(&[t]);
}

fn int(h: &mut blake3::Hasher, n: i64) {
    tag(h, 3);
    h.update(&n.to_le_bytes());
}

fn count(h: &mut blake3::Hasher, t: u8, n: usize) {
    tag(h, t);
    h.update(&(n as u64).to_le_bytes());
}

fn blob(h: &mut blake3::Hasher, t: u8, b: &[u8]) {
    count(h, t, b.len());
    h.update(b);
}
