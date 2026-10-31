//! The flat forms a native returns structured data in, which `std.internal.native` reads. Told:
//! byte 1 and then the text, or byte 0 and then why the system said nothing. Found: byte 1 and then
//! the bytes, or the one byte 0. Parts: each part behind its count, four bytes, most significant
//! first. An integer is eight bytes, most significant first.

/// `text` told.
pub fn told(text: std::io::Result<String>) -> Vec<u8> {
    match text {
        Ok(text) => [&[1u8][..], text.as_bytes()].concat(),
        Err(e) => [&[0u8][..], e.to_string().as_bytes()].concat(),
    }
}

/// `bytes` found.
pub fn found(bytes: Option<impl AsRef<[u8]>>) -> Vec<u8> {
    match bytes {
        Some(bytes) => [&[1u8][..], bytes.as_ref()].concat(),
        None => vec![0],
    }
}

/// What `read` appends to a buffer of `capacity` bytes, found, or none where it fails: read in
/// after the leading byte, so its bytes are not copied again.
pub fn found_read(
    capacity: usize,
    read: impl FnOnce(&mut Vec<u8>) -> std::io::Result<usize>,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(capacity.saturating_add(1));
    out.push(1);
    match read(&mut out) {
        Ok(_) => out,
        Err(_) => vec![0],
    }
}

/// `parts` one after another, each behind its count.
pub fn parts(parts: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for part in parts {
        out.extend((part.len() as u32).to_be_bytes());
        out.extend(*part);
    }
    out
}

/// The parts `flat` holds; `None` where a count runs past its end.
pub fn parts_of(flat: &[u8]) -> Option<Vec<&[u8]>> {
    let mut parts = Vec::new();
    let mut rest = flat;
    while !rest.is_empty() {
        let (count, after) = rest.split_first_chunk::<4>()?;
        let count = u32::from_be_bytes(*count) as usize;
        if count > after.len() {
            return None;
        }
        let (part, after) = after.split_at(count);
        parts.push(part);
        rest = after;
    }
    Some(parts)
}

/// `n` as eight bytes.
pub fn int(n: i64) -> [u8; 8] {
    n.to_be_bytes()
}

/// The integer eight bytes are; `None` for any other count of them.
pub fn int_of(bytes: &[u8]) -> Option<i64> {
    Some(i64::from_be_bytes(bytes.try_into().ok()?))
}
