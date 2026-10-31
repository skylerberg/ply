pub fn told(text: std::io::Result<String>) -> Vec<u8> {
    match text {
        Ok(text) => [&[1u8][..], text.as_bytes()].concat(),
        Err(e) => [&[0u8][..], e.to_string().as_bytes()].concat(),
    }
}

pub fn found(bytes: Option<impl AsRef<[u8]>>) -> Vec<u8> {
    match bytes {
        Some(bytes) => [&[1u8][..], bytes.as_ref()].concat(),
        None => vec![0],
    }
}

pub fn read_result_or_none(
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

pub fn parts(parts: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for part in parts {
        out.extend((part.len() as u32).to_be_bytes());
        out.extend(*part);
    }
    out
}

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

pub fn int(n: i64) -> [u8; 8] {
    n.to_be_bytes()
}

pub fn int_of(bytes: &[u8]) -> Option<i64> {
    Some(i64::from_be_bytes(bytes.try_into().ok()?))
}
