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
    for part in [
        "ply-c-emit-1",
        &exe_stamp(),
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
fn encode(text: &str, t: &Tables) -> String {
    let mut out = String::new();
    out.push_str(&format!("consts {}\n", t.consts.len()));
    for v in &t.consts {
        out.push_str(&match v {
            Value::Unit => "u\n".to_string(),
            Value::Str(s) => format!("s {}\n", hex(s.as_bytes())),
            Value::Bytes(b) => format!("b {}\n", hex(b)),
            Value::Fixed(f) => format!("f {} {}\n", f.ty as u8, f.bits()),
            // Nothing else reaches the pool: `literal` puts only these four there.
            other => unreachable!("a constant this tier does not pool: {other:?}"),
        });
    }
    out.push_str(&format!("builtins {}\n", t.builtins.len()));
    for b in &t.builtins {
        out.push_str(&format!("{}\n", b.name()));
    }
    out.push_str(&format!("fields {}\n", t.fields.len()));
    for f in &t.fields {
        out.push_str(&format!("{f}\n"));
    }
    out.push_str(&format!("shapes {}\n", t.shapes.len()));
    for names in &t.shapes {
        out.push_str(&format!(
            "{}\n",
            names
                .iter()
                .map(|n| n.as_str().to_string())
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    out.push_str(&format!("calls {}\n", t.calls.len()));
    for c in &t.calls {
        out.push_str(&format!("{c}\n"));
    }
    out.push_str("text\n");
    out.push_str(text);
    out
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

fn decode(s: &str) -> Option<(String, Tables)> {
    let mut at = 0usize;
    let mut lines = std::iter::from_fn(|| line(s, &mut at));
    let mut t = Tables::default();
    let n = count(lines.next()?, "consts")?;
    for _ in 0..n {
        let line = lines.next()?;
        let (tag, rest) = line.split_at(1);
        let rest = rest.strip_prefix(' ').unwrap_or(rest);
        t.consts.push(match tag {
            "u" => Value::Unit,
            "s" => Value::str(String::from_utf8(unhex(rest)?).ok()?),
            "b" => Value::bytes(unhex(rest)?),
            "f" => {
                let (ty, bits) = rest.split_once(' ')?;
                let n: u8 = ty.parse().ok()?;
                let ty = ply_core::ty::INT_TYPES.iter().find(|t| **t as u8 == n)?;
                Value::Fixed(ply_eval::Fixed::new(*ty, bits.parse().ok()?))
            }
            _ => return None,
        });
    }
    let n = count(lines.next()?, "builtins")?;
    for _ in 0..n {
        t.builtins
            .push(ply_eval::Builtin::from_name(&Symbol::new(lines.next()?))?);
    }
    let n = count(lines.next()?, "fields")?;
    for _ in 0..n {
        t.fields.push(Symbol::new(lines.next()?));
    }
    let n = count(lines.next()?, "shapes")?;
    for _ in 0..n {
        let line = lines.next()?;
        t.shapes
            .push(line.split_whitespace().map(Symbol::new).collect::<Vec<_>>());
    }
    let n = count(lines.next()?, "calls")?;
    for _ in 0..n {
        t.calls.push(lines.next()?.to_string());
    }
    if lines.next()? != "text" {
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
        let text = "Word f(void) {\n  return @@c1@@;\n}\ntext\n";

        let (back, out) = decode(&encode(text, &t)).expect("the encoding round trips");
        assert_eq!(back, text, "the text came back changed");
        assert_eq!(out.fields, t.fields);
        assert_eq!(out.shapes, t.shapes);
        assert_eq!(out.calls, t.calls);
        assert_eq!(out.builtins, t.builtins);
        assert_eq!(out.consts.len(), 3);
        assert!(matches!(out.consts[0], Value::Unit));
        assert_eq!(
            format!("{:?}", out.consts[1]),
            format!("{:?}", Value::str("hello\nworld"))
        );
    }
}
