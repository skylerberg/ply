//! How Ply prints a digest.

/// `b3:` and a digest's first twelve hex characters: every digest Ply prints for a person to
/// compare is written this way.
pub fn short(digest: &[u8; 32]) -> String {
    let mut out = String::from("b3:");
    for byte in &digest[..6] {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}
