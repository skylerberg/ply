//! The encoding a value's content digest hashes, of a scalar and of a string: values `==` calls
//! equal are one run of bytes in it.

use crate::{Decimal, IntTy};

/// `b3:` and a digest's first twelve hex characters: every digest Ply prints for a person to
/// compare is written this way.
pub fn short(digest: &[u8; 32]) -> String {
    let mut out = String::from("b3:");
    for byte in &digest[..6] {
        out.push_str(&format!("{byte:02x}"));
    }
    out
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

/// `s`'s encoding.
pub fn scalar(s: Scalar) -> Vec<u8> {
    let mut out = Vec::new();
    match s {
        Scalar::Unit => out.push(0),
        Scalar::Bool(b) => out.push(if b { 2 } else { 1 }),
        Scalar::Int(n) => int(&mut out, n),
        Scalar::Width(ty, bits) => {
            let bits = ty.normalize(bits);
            // Below 64 bits a width is the `Int` it reads as, which is how compiled code holds it.
            if ty.bits() < 64 {
                int(&mut out, bits as i64);
            } else {
                let raw = if ty.bits() == 128 {
                    bits
                } else {
                    bits & (u128::MAX >> (128 - ty.bits()))
                };
                out.extend_from_slice(&[4, ty as u8]);
                out.extend_from_slice(&raw.to_le_bytes());
            }
        }
        Scalar::Char(c) => {
            out.push(5);
            out.extend_from_slice(&c.to_le_bytes());
        }
        Scalar::Decimal(d) => {
            out.push(6);
            out.extend_from_slice(&d.normalize().serialize());
        }
    }
    out
}

/// A string's encoding where `text`, else a byte string's.
pub fn text(text: bool, b: &[u8]) -> Vec<u8> {
    let mut out = vec![if text { 7 } else { 8 }];
    out.extend_from_slice(&(b.len() as u64).to_le_bytes());
    out.extend_from_slice(b);
    out
}

fn int(out: &mut Vec<u8>, n: i64) {
    out.push(3);
    out.extend_from_slice(&n.to_le_bytes());
}
