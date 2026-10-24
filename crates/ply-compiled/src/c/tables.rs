//! The ids a loaded unit's failure sites name their roots by.

/// The id a body's failure sites name their root by: a digest of its name, so a body's C does not
/// depend on which other roots the unit holds. Non-negative, since the C stores it in an `int64_t`.
pub fn root_id(name: &str) -> u64 {
    let hash = blake3::hash(name.as_bytes());
    let bytes: [u8; 8] = hash.as_bytes()[..8]
        .try_into()
        .expect("a digest has eight bytes");
    u64::from_le_bytes(bytes) >> 1
}
