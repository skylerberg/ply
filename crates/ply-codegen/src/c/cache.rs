//! Emitted bodies and units, kept between runs. A body names tables by its own positions
//! (`@@c3@@`), so a cached body is a function of the body alone.

use super::tables::Tables;
use ply_eval::Value;
use ply_span::Symbol;
use std::path::PathBuf;

fn dir() -> PathBuf {
    super::load::cache_dir().join("emit")
}

/// What a body's C is a function of: its root's key (`Source::keys`), the constructor table, the
/// helper table and the binary's stamp, so rebuilding `ply` invalidates the cache.
pub fn key(def_hash: &str, ctors: &str) -> String {
    let mut h = blake3::Hasher::new();
    for part in [
        "ply-c-emit-4",
        &exe_stamp(),
        &super::exports::helpers_digest(),
        ctors,
        def_hash,
    ] {
        h.update(part.as_bytes());
        h.update(&[0]);
    }
    h.finalize().to_hex().to_string()
}

/// The running binary's size and modification time: a cheap identity for this build.
fn exe_stamp() -> String {
    let Ok(exe) = std::env::current_exe() else {
        return String::new();
    };
    let Ok(m) = std::fs::metadata(&exe) else {
        return String::new();
    };
    let modified = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{}:{modified}", m.len())
}

/// The digest of the constructor table, whose positions a body writes as numbers.
pub fn ctors_digest(ctors: &[(Symbol, usize)]) -> String {
    let mut h = blake3::Hasher::new();
    for (name, arity) in ctors {
        h.update(name.as_str().as_bytes());
        h.update(&[0]);
        h.update(&arity.to_le_bytes());
    }
    h.finalize().to_hex()[..32].to_string()
}

/// A refusal's key folds in the offered set: a body is refused when a callee was not offered.
pub fn refusal_key(def_hash: &str, ctors: &str, fragment: &str) -> String {
    key(&format!("{def_hash}/{fragment}"), ctors)
}

/// The digest of the set of definitions this unit was offered.
pub fn fragment_digest(names: &[&str]) -> String {
    let mut sorted: Vec<&str> = names.to_vec();
    sorted.sort_unstable();
    let mut h = blake3::Hasher::new();
    for n in sorted {
        h.update(n.as_bytes());
        h.update(&[0]);
    }
    h.finalize().to_hex()[..32].to_string()
}

/// Why this definition was refused last time, if it was.
pub fn read_refusal(key: &str) -> Option<String> {
    std::fs::read_to_string(dir().join(format!("{key}.refused"))).ok()
}

pub fn write_refusal(key: &str, reason: &str) {
    let d = dir();
    if std::fs::create_dir_all(&d).is_err() {
        return;
    }
    let tmp = d.join(format!("{key}.{}.rtmp", std::process::id()));
    if std::fs::write(&tmp, reason).is_ok() {
        let _ = std::fs::rename(&tmp, d.join(format!("{key}.refused")));
    }
}

/// The body kept under `key`, if one is.
pub fn read(key: &str) -> Option<(String, Tables)> {
    decode(&std::fs::read_to_string(dir().join(format!("{key}.body"))).ok()?)
}

/// Keep this body; a failed write is ignored.
pub fn write(key: &str, text: &str, tables: &Tables) {
    let d = dir();
    if std::fs::create_dir_all(&d).is_err() {
        return;
    }
    let encoded = encode(text, tables);
    // Renamed into place, so a reader never sees half a body.
    let tmp = d.join(format!("{key}.{}.tmp", std::process::id()));
    if std::fs::write(&tmp, encoded).is_ok() {
        let _ = std::fs::rename(&tmp, d.join(format!("{key}.body")));
    }
}

pub fn encode(text: &str, t: &Tables) -> String {
    let mut out = encode_tables(&t.consts, &t.builtins, &t.fields, &t.shapes, &t.lambdas);
    out.push_str(&format!("calls {}\n", t.calls.len()));
    for c in &t.calls {
        out.push_str(&format!("{c}\n"));
    }
    out.push_str(&format!("performs {}\n", t.performs.len()));
    for e in &t.performs {
        out.push_str(&format!("{e}\n"));
    }
    out.push_str(&format!("handles {}\n", t.handles.len()));
    for e in &t.handles {
        out.push_str(&format!("{e}\n"));
    }
    out.push_str("text\n");
    out.push_str(text);
    out
}

/// The five tables a body and a whole unit both name, in one shared encoding.
pub(super) fn encode_tables(
    consts: &[Value],
    builtins: &[ply_eval::Builtin],
    fields: &[Symbol],
    shapes: &[Vec<Symbol>],
    lambdas: &[String],
) -> String {
    let mut out = format!("consts {}\n", consts.len());
    for v in consts {
        out.push_str(&encode_const(v));
        out.push('\n');
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
    out.push_str(&format!("lambdas {}\n", lambdas.len()));
    for l in lambdas {
        out.push_str(&format!("{l}\n"));
    }
    out
}

/// One pooled constant's line: its identity, so the pool sorts and deduplicates by it.
pub(super) fn encode_const(v: &Value) -> String {
    match v {
        Value::Unit => "u".to_string(),
        Value::Str(s) => format!("s {}", hex(s.as_bytes())),
        Value::Bytes(b) => format!("b {}", hex(b)),
        Value::Fixed(f) => format!("f {} {}", f.ty as u8, f.bits()),
        Value::Float(x) => format!("x {:016x}", x.to_bits()),
        Value::Decimal(d) => format!("d {} {}", d.mantissa(), d.scale()),
        other => unreachable!("a constant this tier does not pool: {other:?}"),
    }
}

/// Inverse of [`encode_tables`]; leaves the cursor after them.
pub(super) fn decode_tables(s: &str, at: &mut usize) -> Option<Tables> {
    let mut t = Tables::default();
    let n = count(line(s, at)?, "consts")?;
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
                let ty = ply_ty::INT_TYPES.iter().find(|t| **t as u8 == n)?;
                // Unsigned from `encode_tables`, signed from the Ply emitter: same bit pattern.
                let bits: u64 = match bits.parse::<u64>() {
                    Ok(b) => b,
                    Err(_) => bits.parse::<i64>().ok()? as u64,
                };
                Value::Fixed(ply_eval::Fixed::new(*ty, bits))
            }
            "x" => Value::Float(f64::from_bits(u64::from_str_radix(rest, 16).ok()?)),
            "d" => {
                let (mantissa, scale) = rest.split_once(' ')?;
                Value::Decimal(
                    ply_eval::Decimal::try_from_i128_with_scale(
                        mantissa.parse().ok()?,
                        scale.parse().ok()?,
                    )
                    .ok()?,
                )
            }
            // The Ply emitter keeps these literals as source text; parse as the lexer does.
            "X" => Value::Float(rest.replace('_', "").parse().ok()?),
            "D" => Value::Decimal(
                rest.replace('_', "")
                    .trim_end_matches('m')
                    .parse::<ply_eval::Decimal>()
                    .ok()?,
            ),
            _ => return None,
        });
    }
    let n = count(line(s, at)?, "builtins")?;
    for _ in 0..n {
        t.builtins
            .push(ply_eval::Builtin::from_name(&Symbol::new(line(s, at)?))?);
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
    let n = count(line(s, at)?, "lambdas")?;
    for _ in 0..n {
        t.lambdas.push(line(s, at)?.to_string());
    }
    Some(t)
}

/// Read one line and step the cursor past it; the text is found by offset, never by a marker.
pub(super) fn line<'a>(s: &'a str, at: &mut usize) -> Option<&'a str> {
    let rest = s.get(*at..)?;
    let end = rest.find('\n')?;
    *at += end + 1;
    Some(&rest[..end])
}

pub fn decode(s: &str) -> Option<(String, Tables)> {
    let mut at = 0usize;
    let mut t = decode_tables(s, &mut at)?;
    let n = count(line(s, &mut at)?, "calls")?;
    for _ in 0..n {
        t.calls.push(line(s, &mut at)?.to_string());
    }
    let n = count(line(s, &mut at)?, "performs")?;
    for _ in 0..n {
        t.performs.push(line(s, &mut at)?.to_string());
    }
    let n = count(line(s, &mut at)?, "handles")?;
    for _ in 0..n {
        t.handles.push(line(s, &mut at)?.to_string());
    }
    if line(s, &mut at)? != "text" {
        return None;
    }
    Some((s.get(at..)?.to_string(), t))
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

/// What a unit is a function of: every offered definition's hash, the constructors, the emitter.
pub fn unit_key(
    keys: &std::collections::HashMap<String, String>,
    offered: &[&str],
    ctors: &str,
    emitter: &str,
) -> Option<String> {
    let mut sorted: Vec<&str> = offered.to_vec();
    sorted.sort_unstable();
    let mut h = blake3::Hasher::new();
    h.update(b"ply-c-unit-3");
    h.update(emitter.as_bytes());
    for name in sorted {
        // No hash means an edit could go unnoticed, so no unit key.
        let hash = keys.get(name)?;
        h.update(name.as_bytes());
        h.update(&[0]);
        h.update(hash.as_bytes());
        h.update(&[0]);
    }
    Some(key(&h.finalize().to_hex()[..32], ctors))
}

/// How many times a unit has been rebuilt from the cache rather than emitted; for tests.
pub static UNITS_REUSED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The object a unit key was built as, so a worker skips assembling the C entirely.
pub fn read_unit(key: &str) -> Option<String> {
    let s = std::fs::read_to_string(dir().join(format!("{key}.unit"))).ok()?;
    let object = s.trim();
    (!object.is_empty() && object.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| object.to_string())
}

pub fn write_unit(key: &str, object: &str) {
    let d = dir();
    if std::fs::create_dir_all(&d).is_err() {
        return;
    }
    let tmp = d.join(format!("{key}.{}.utmp", std::process::id()));
    if std::fs::write(&tmp, format!("{object}\n")).is_ok() {
        let _ = std::fs::rename(&tmp, d.join(format!("{key}.unit")));
    }
}
