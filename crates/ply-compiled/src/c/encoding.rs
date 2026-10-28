//! The pools a unit names, written into its C and read back from it: one encoding, so what a unit
//! says about itself is what loading it reads.

use ply_core::{Builtin, Symbol};

/// The pools a unit's positions index, each in the unit's order.
#[derive(Default)]
pub(super) struct Pools {
    pub builtins: Vec<Builtin>,
    pub fields: Vec<Symbol>,
}

pub(super) fn encode_pools(builtins: &[Builtin], fields: &[Symbol]) -> String {
    let mut out = String::new();
    if !builtins.is_empty() {
        out.push_str(&format!("builtins {}\n", builtins.len()));
        for b in builtins {
            out.push_str(&format!("{}\n", b.name()));
        }
    }
    out.push_str(&format!("fields {}\n", fields.len()));
    for f in fields {
        out.push_str(&format!("{f}\n"));
    }
    out
}

/// Inverse of [`encode_pools`]; leaves the cursor after them. Only a builder the release built
/// names builtins, which it calls through [`crate::rt::rt_builtin_encoded`].
pub(super) fn decode_pools(s: &str, at: &mut usize) -> Option<Pools> {
    let mut t = Pools::default();
    let mut next = line(s, at)?;
    if let Some(n) = count(next, "builtins") {
        for _ in 0..n {
            t.builtins
                .push(Builtin::from_name(&Symbol::new(line(s, at)?))?);
        }
        next = line(s, at)?;
    }
    let n = count(next, "fields")?;
    for _ in 0..n {
        t.fields.push(Symbol::new(line(s, at)?));
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

pub(super) fn count(line: &str, label: &str) -> Option<usize> {
    line.strip_prefix(label)?.trim().parse().ok()
}
