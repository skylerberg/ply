//! The field names a unit's bodies read, written into its C and read back from it: one encoding, so
//! what a unit says about itself is what loading it reads.

use ply_core::Symbol;

pub(super) fn encode_fields(fields: &[Symbol]) -> String {
    let mut out = format!("fields {}\n", fields.len());
    for f in fields {
        out.push_str(&format!("{f}\n"));
    }
    out
}

/// Inverse of [`encode_fields`]; leaves the cursor after them.
pub(super) fn decode_fields(s: &str, at: &mut usize) -> Option<Vec<Symbol>> {
    let n = count(line(s, at)?, "fields")?;
    let mut fields = Vec::with_capacity(n);
    for _ in 0..n {
        fields.push(Symbol::new(line(s, at)?));
    }
    Some(fields)
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
