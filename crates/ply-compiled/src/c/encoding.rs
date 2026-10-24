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
    let mut out = format!("builtins {}\n", builtins.len());
    for b in builtins {
        out.push_str(&format!("{}\n", b.name()));
    }
    out.push_str(&format!("fields {}\n", fields.len()));
    for f in fields {
        out.push_str(&format!("{f}\n"));
    }
    out
}

/// Inverse of [`encode_pools`]; leaves the cursor after them, and after the record shapes a unit
/// whose records the runtime laid out lists after them, which no unit's code reads now.
pub(super) fn decode_pools(s: &str, at: &mut usize) -> Option<Pools> {
    let mut t = Pools::default();
    let n = count(line(s, at)?, "builtins")?;
    for _ in 0..n {
        t.builtins
            .push(Builtin::from_name(&Symbol::new(line(s, at)?))?);
    }
    let n = count(line(s, at)?, "fields")?;
    for _ in 0..n {
        t.fields.push(Symbol::new(line(s, at)?));
    }
    skipped(s, at, "shapes")?;
    Some(t)
}

/// Steps past the table `label` where the text opens one here, a row a line.
pub(super) fn skipped(s: &str, at: &mut usize, label: &str) -> Option<()> {
    for _ in 0..section(s, at, label)? {
        line(s, at)?;
    }
    Some(())
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
