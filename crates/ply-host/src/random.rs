use ply_core::{Diagnostic, codes};
use ring::rand::SecureRandom;

pub fn next() -> Result<i64, Diagnostic> {
    let mut bytes = [0u8; 8];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| no_entropy())?;
    Ok((u64::from_be_bytes(bytes) >> 1) as i64)
}

pub fn drawn(size: usize) -> Result<zeroize::Zeroizing<Vec<u8>>, Diagnostic> {
    let mut bytes = zeroize::Zeroizing::new(vec![0u8; size]);
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| no_entropy())?;
    Ok(bytes)
}

fn no_entropy() -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        "the operating system gave this run no entropy",
    )
    .note("`std.random` reads the system's, and there is no second source")
}
