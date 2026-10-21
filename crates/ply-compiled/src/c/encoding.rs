//! The pools a unit names, written into its C and read back from it: one encoding, so what a unit
//! says about itself is what loading it reads.

use ply_core::{Builtin, Symbol, Value};

/// The pools a unit's positions index, each in the unit's order.
#[derive(Default)]
pub(super) struct Pools {
    pub consts: Vec<Value>,
    pub builtins: Vec<Builtin>,
    pub fields: Vec<Symbol>,
    pub shapes: Vec<Vec<Symbol>>,
}

pub(super) fn encode_pools(
    consts: &[Value],
    builtins: &[Builtin],
    fields: &[Symbol],
    shapes: &[Vec<Symbol>],
) -> String {
    // A unit whose constants are its own static data names none, and says nothing of them.
    let mut out = String::new();
    if !consts.is_empty() {
        out.push_str(&format!("consts {}\n", consts.len()));
        for v in consts {
            out.push_str(&encode_const(v));
            out.push('\n');
        }
    }
    out.push_str(&format!("builtins {}\n", builtins.len()));
    for b in builtins {
        out.push_str(&format!("{}\n", b.name()));
    }
    out.push_str(&format!("fields {}\n", fields.len()));
    for f in fields {
        out.push_str(&format!("{f}\n"));
    }
    out.push_str(&format!("shapes {}\n", shapes.len()));
    for names in shapes {
        out.push_str(&format!(
            "{}\n",
            names
                .iter()
                .map(|n| n.as_str().to_string())
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    out
}

/// One pooled constant's line: its identity, so the pool sorts and deduplicates by it.
fn encode_const(v: &Value) -> String {
    match v {
        Value::Unit => "u".to_string(),
        Value::Str(s) => format!("s {}", hex(s.as_bytes())),
        Value::Bytes(b) => format!("b {}", hex(b)),
        Value::Fixed(f) => format!("f {} {}", f.ty as u8, f.bits()),
        Value::Char(c) => format!("c {}", u32::from(*c)),
        Value::Float(x) => format!("x {:016x}", ply_core::float::bits(*x)),
        Value::Decimal(d) => format!("d {} {}", d.mantissa(), d.scale()),
        other => unreachable!("a constant this backend does not pool: {other:?}"),
    }
}

/// Inverse of [`encode_pools`]; leaves the cursor after them. A text with no `consts` table pools
/// no constants.
pub(super) fn decode_pools(s: &str, at: &mut usize) -> Option<Pools> {
    let mut t = Pools::default();
    let n = section(s, at, "consts")?;
    for _ in 0..n {
        let l = line(s, at)?;
        let (tag, rest) = l.split_at(1);
        let rest = rest.strip_prefix(' ').unwrap_or(rest);
        t.consts.push(match tag {
            "u" => Value::Unit,
            "s" => Value::str(String::from_utf8(unhex(rest)?).ok()?),
            "b" => Value::bytes(unhex(rest)?),
            "f" => {
                let (ty, bits) = rest.split_once(' ')?;
                let n: u8 = ty.parse().ok()?;
                let ty = ply_core::INT_TYPES.iter().find(|t| **t as u8 == n)?;
                Value::Fixed(ply_core::Fixed::new(*ty, bits.parse().ok()?))
            }
            "c" => Value::Char(char::from_u32(rest.parse().ok()?)?),
            "x" => Value::Float(f64::from_bits(u64::from_str_radix(rest, 16).ok()?)),
            "d" => {
                let (mantissa, scale) = rest.split_once(' ')?;
                Value::Decimal(
                    ply_core::Decimal::try_from_i128_with_scale(
                        mantissa.parse().ok()?,
                        scale.parse().ok()?,
                    )
                    .ok()?,
                )
            }
            _ => return None,
        });
    }
    let n = count(line(s, at)?, "builtins")?;
    for _ in 0..n {
        t.builtins
            .push(Builtin::from_name(&Symbol::new(line(s, at)?))?);
    }
    let n = count(line(s, at)?, "fields")?;
    for _ in 0..n {
        t.fields.push(Symbol::new(line(s, at)?));
    }
    let n = count(line(s, at)?, "shapes")?;
    for _ in 0..n {
        t.shapes
            .push(line(s, at)?.split_whitespace().map(Symbol::new).collect());
    }
    Some(t)
}

/// How many rows a table `label` holds where the text opens one here, its line stepped past, and
/// none where the text holds no such table here.
pub(super) fn section(s: &str, at: &mut usize, label: &str) -> Option<usize> {
    let opens = s
        .get(*at..)?
        .strip_prefix(label)
        .is_some_and(|rest| rest.starts_with(' '));
    if opens {
        count(line(s, at)?, label)
    } else {
        Some(0)
    }
}

/// Read one line and step the cursor past it; the text is found by offset, never by a marker.
pub(super) fn line<'a>(s: &'a str, at: &mut usize) -> Option<&'a str> {
    let rest = s.get(*at..)?;
    let end = rest.find('\n')?;
    *at += end + 1;
    Some(&rest[..end])
}

pub(super) fn count(line: &str, label: &str) -> Option<usize> {
    line.strip_prefix(label)?.trim().parse().ok()
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap_or('0'));
        s.push(char::from_digit((b & 0xf) as u32, 16).unwrap_or('0'));
    }
    s
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    let bytes = s.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks(2) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out.push((hi * 16 + lo) as u8);
    }
    Some(out)
}
