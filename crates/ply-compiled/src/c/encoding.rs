use ply_core::Symbol;

pub(super) fn encode_fields(fields: &[Symbol]) -> String {
    let mut out = format!("fields {}\n", fields.len());
    for f in fields {
        out.push_str(&format!("{f}\n"));
    }
    out
}

pub(super) fn decode_fields(s: &str, at: &mut usize) -> Option<Vec<Symbol>> {
    let n = count(line(s, at)?, "fields")?;
    let mut fields = Vec::with_capacity(n);
    for _ in 0..n {
        fields.push(Symbol::new(line(s, at)?));
    }
    Some(fields)
}

pub(super) fn line<'a>(s: &'a str, at: &mut usize) -> Option<&'a str> {
    let rest = s.get(*at..)?;
    let end = rest.find('\n')?;
    *at += end + 1;
    Some(&rest[..end])
}

pub(super) fn count(line: &str, label: &str) -> Option<usize> {
    line.strip_prefix(label)?.trim().parse().ok()
}
