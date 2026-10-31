pub fn root_id(name: &str) -> u64 {
    let hash = blake3::hash(name.as_bytes());
    let bytes: [u8; 8] = hash.as_bytes()[..8]
        .try_into()
        .expect("a digest has eight bytes");
    u64::from_le_bytes(bytes) >> 1
}
