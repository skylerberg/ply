//! What one emitted body is, kept between runs.
//!
//! The emitted tier's time is not the C compiler. Measured on the self-hosted front end's twelve
//! modules: **optimise and lower 1.7s, generate the C 0.085s, `cc` 1.6s** per unit. The compiler is
//! the smaller half and it already has a cache of its own; the larger half is the inliner, and the
//! only artefact that holds its work is the C it produced.
//!
//! So this caches the C, one body at a time, keyed on what the body is a function of. Per body
//! rather than per unit because that is what makes an edit cost the edit: one definition moving
//! re-emits one definition, not a project.
//!
//! A body's text names the unit's tables by its own positions -- `@@c3@@`, resolved when the body
//! goes into a unit -- so what is kept here is a function of the body alone and can be read back
//! into a unit that looks nothing like the one it came from.

use super::emit::Tables;
use ply_eval::Value;
use ply_span::Symbol;
use std::path::PathBuf;

/// Where emitted bodies are kept. Beside the objects, under `PLY_C_CACHE`.
fn dir() -> PathBuf {
    super::load::cache_dir().join("emit")
}

/// What a body's C is a function of, as one name.
///
/// The definition's hash covers its own text *and* every definition it references, which is what
/// an inlining emitter needs: a body's C changes when anything it inlines changes, and
/// `HashOutput::defs` moves for exactly that reason.
///
/// Beside it: the constructor table, whose positions the text writes as numbers; how hard the
/// inliner was told to work; and the compiler binary's own stamp, so that rebuilding `ply` throws
/// the cache away rather than asking anyone to remember to.
pub fn key(def_hash: &str, ctors: &str, inlining: (usize, usize)) -> String {
    let mut h = blake3::Hasher::new();
    // The runtime's helper table is part of the key: a body's C calls the helpers by shape, and
    // a shape that moved would otherwise be read back from a body emitted against the old one.
    for part in [
        "ply-c-emit-3",
        &exe_stamp(),
        &super::bundle::runtime_digest(),
        &format!("{}:{}", inlining.0, inlining.1),
        ctors,
        def_hash,
    ] {
        h.update(part.as_bytes());
        h.update(&[0]);
    }
    h.finalize().to_hex().to_string()
}

/// The running binary's size and modification time, which is a cheap identity for "the emitter as
/// it is today".
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

/// A refusal is worth keeping too, and it costs more than a body: the refusal happens *during* the
/// emit, so a definition this tier will not take pays the inliner in full, every run. On the
/// spike's twelve modules that is a thousand of them.
///
/// Keyed with the fragment folded in, because a refusal is not a property of the definition alone:
/// a body is refused when something it calls was not offered, and a different command offers a
/// different set. A success needs no such thing -- its calls are recorded and checked.
pub fn refusal_key(
    def_hash: &str,
    ctors: &str,
    inlining: (usize, usize),
    fragment: &str,
) -> String {
    key(&format!("{def_hash}/{fragment}"), ctors, inlining)
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

/// Keep a refusal.
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

/// Keep this body. A failure to write is a cache that did not help, never a run that fails.
pub fn write(key: &str, text: &str, tables: &Tables) {
    let d = dir();
    if std::fs::create_dir_all(&d).is_err() {
        return;
    }
    let encoded = encode(text, tables);
    // Written beside and renamed, so a reader never sees half a body.
    let tmp = d.join(format!("{key}.{}.tmp", std::process::id()));
    if std::fs::write(&tmp, encoded).is_ok() {
        let _ = std::fs::rename(&tmp, d.join(format!("{key}.body")));
    }
}

/// One body as lines: the tables it names, then its text.
pub(super) fn encode(text: &str, t: &Tables) -> String {
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

/// The five tables a body and a whole unit both name, in one encoding, so the two cannot drift.
fn encode_tables(
    consts: &[Value],
    builtins: &[ply_eval::Builtin],
    fields: &[Symbol],
    shapes: &[Vec<Symbol>],
    lambdas: &[String],
) -> String {
    let mut out = format!("consts {}\n", consts.len());
    for v in consts {
        out.push_str(&match v {
            Value::Unit => "u\n".to_string(),
            Value::Str(s) => format!("s {}\n", hex(s.as_bytes())),
            Value::Bytes(b) => format!("b {}\n", hex(b)),
            Value::Fixed(f) => format!("f {} {}\n", f.ty as u8, f.bits()),
            // A `Float` by its bits and a `Decimal` by its mantissa and scale: exact both ways.
            Value::Float(x) => format!("x {:016x}\n", x.to_bits()),
            Value::Decimal(d) => format!("d {} {}\n", d.mantissa(), d.scale()),
            other => unreachable!("a constant this tier does not pool: {other:?}"),
        });
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

/// The same five, read back. Leaves the cursor after them.
fn decode_tables(s: &str, at: &mut usize) -> Option<Tables> {
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
                let ty = ply_core::ty::INT_TYPES.iter().find(|t| **t as u8 == n)?;
                // Written unsigned by the reference and as the wrapped `Int` by the emitter in
                // Ply, whose integers are signed: one bit pattern either way.
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
            // The emitter written in Ply keeps a literal as its source text, and converts it
            // here by the rule the lexer converts it with: the same parse, underscores dropped,
            // the `m` suffix off a `Decimal`.
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

/// Read one line and step the cursor past it, so that the text's start is a byte offset rather
/// than a search for a marker: a field, a call or a shape can be spelled anything at all, `text`
/// included, and a marker they can spell is a marker that splits the file in the wrong place.
fn line<'a>(s: &'a str, at: &mut usize) -> Option<&'a str> {
    let rest = s.get(*at..)?;
    let end = rest.find('\n')?;
    *at += end + 1;
    Some(&rest[..end])
}

pub(super) fn decode(s: &str) -> Option<(String, Tables)> {
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

fn count(line: &str, label: &str) -> Option<usize> {
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

/// Everything a worker needs to put a unit back together without emitting it.
///
/// A body cache saves the *emitting*, which is most of one worker's time and none of the other
/// ten's: each still walks fourteen hundred cached bodies, substitutes their placeholders and
/// assembles twenty-nine megabytes of C, only to hand it to an object cache that already had the
/// answer. Sharing the built unit in process is not available -- `ply_eval::Value` holds `Rc`,
/// so nothing containing one crosses a rayon worker -- so what is shared is this, through the
/// same file system the objects already live on.
pub struct UnitCache {
    /// The object the assembled source hashes to, recorded so no source is needed to find it.
    pub object: String,
    pub taken: Vec<String>,
    /// What the fixpoint dropped and why, so a unit read back reports the same refusals as the
    /// build that wrote it.
    pub refusals: Vec<(String, String)>,
    pub consts: Vec<Value>,
    pub fields: Vec<Symbol>,
    pub builtins: Vec<ply_eval::Builtin>,
    pub shapes: Vec<Vec<Symbol>>,
    pub lambdas: Vec<String>,
}

/// What a unit is a function of: every offered definition and its hash, the constructor table,
/// the inlining, and the binary. The *names* alone are not enough -- an edit leaves the offered
/// set identical and changes what the unit contains.
pub fn unit_key(
    keys: &std::collections::HashMap<String, String>,
    offered: &[&str],
    ctors: &str,
    inlining: (usize, usize),
    who: &str,
) -> Option<String> {
    let mut sorted: Vec<&str> = offered.to_vec();
    sorted.sort_unstable();
    let mut h = blake3::Hasher::new();
    h.update(b"ply-c-unit-2");
    // Which emitter filled the unit, and in which mode: a unit the reference emitted must not be
    // served to a run that asked the Ply emitter to, or that run measures nothing, and a unit
    // the Ply emitter filled behind the reference's acceptance is not the one it fills alone.
    h.update(who.as_bytes());
    for name in sorted {
        // Without a hash for every offered definition there is nothing to notice an edit by, and
        // a unit cache that cannot notice one is a wrong answer rather than a slow one.
        let hash = keys.get(name)?;
        h.update(name.as_bytes());
        h.update(&[0]);
        h.update(hash.as_bytes());
        h.update(&[0]);
    }
    Some(key(&h.finalize().to_hex()[..32], ctors, inlining))
}

/// How many times a unit has been rebuilt from the cache rather than emitted.
///
/// The saving here is invisible from the outside: a unit put back together answers exactly what
/// the one that built it answered, which is the whole point and also means a test cannot tell the
/// two apart by asking. This is what it asks instead.
pub static UNITS_REUSED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn read_unit(key: &str) -> Option<UnitCache> {
    decode_unit(&std::fs::read_to_string(dir().join(format!("{key}.unit"))).ok()?)
}

pub fn write_unit(key: &str, u: &UnitCache) {
    let d = dir();
    if std::fs::create_dir_all(&d).is_err() {
        return;
    }
    let tmp = d.join(format!("{key}.{}.utmp", std::process::id()));
    if std::fs::write(&tmp, encode_unit(u)).is_ok() {
        let _ = std::fs::rename(&tmp, d.join(format!("{key}.unit")));
    }
}

pub fn encode_unit(u: &UnitCache) -> String {
    let mut out = format!("object {}\n", u.object);
    out.push_str(&format!("taken {}\n", u.taken.len()));
    for t in &u.taken {
        out.push_str(&format!("{t}\n"));
    }
    out.push_str(&format!("refused {}\n", u.refusals.len()));
    for (function, construct) in &u.refusals {
        out.push_str(&format!("{function}\n{construct}\n"));
    }
    out.push_str(&encode_tables(
        &u.consts,
        &u.builtins,
        &u.fields,
        &u.shapes,
        &u.lambdas,
    ));
    out
}

pub fn decode_unit(s: &str) -> Option<UnitCache> {
    let mut at = 0usize;
    let object = line(s, &mut at)?.strip_prefix("object ")?.to_string();
    let n = count(line(s, &mut at)?, "taken")?;
    let mut taken = Vec::with_capacity(n);
    for _ in 0..n {
        taken.push(line(s, &mut at)?.to_string());
    }
    let n = count(line(s, &mut at)?, "refused")?;
    let mut refusals = Vec::with_capacity(n);
    for _ in 0..n {
        let function = line(s, &mut at)?.to_string();
        let construct = line(s, &mut at)?.to_string();
        refusals.push((function, construct));
    }
    let t = decode_tables(s, &mut at)?;
    Some(UnitCache {
        object,
        taken,
        refusals,
        consts: t.consts,
        fields: t.fields,
        builtins: t.builtins,
        shapes: t.shapes,
        lambdas: t.lambdas,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A body goes to disk and comes back the same, tables and all.
    ///
    /// The tables hold names a program chose, so they can be spelled anything -- `text` included,
    /// which is what a marker-terminated format gets wrong and why the text's start is an offset.
    #[test]
    fn a_body_round_trips_through_the_encoding() {
        let mut t = Tables::default();
        t.consts.push(Value::Unit);
        t.consts.push(Value::str("hello\nworld"));
        t.consts.push(Value::bytes([0u8, 255, 10]));
        t.builtins.push(ply_eval::Builtin::BytesLen);
        t.fields.push(Symbol::new("text"));
        t.shapes.push(vec![Symbol::new("text"), Symbol::new("b")]);
        t.calls.push("text".to_string());
        t.lambdas.push("ply_m_f_lambda0".to_string());
        let text = "Word f(void) {\n  return @@c1@@;\n}\ntext\n";

        let (back, out) = decode(&encode(text, &t)).expect("the encoding round trips");
        assert_eq!(back, text, "the text came back changed");
        assert_eq!(out.fields, t.fields);
        assert_eq!(out.shapes, t.shapes);
        assert_eq!(out.calls, t.calls);
        assert_eq!(out.lambdas, t.lambdas);
        assert_eq!(out.builtins, t.builtins);
        assert_eq!(out.consts.len(), 3);
        assert!(matches!(out.consts[0], Value::Unit));
        assert_eq!(
            format!("{:?}", out.consts[1]),
            format!("{:?}", Value::str("hello\nworld"))
        );
    }
}
