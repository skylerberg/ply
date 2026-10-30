//! The operating system's entropy, which `std.random`'s natives draw: `/dev/urandom` is the only
//! source a run has.
//!
//! The prelude's `random` is the scheduler's, handled with values a seed decides so a simulation
//! is reproducible.

use ply_core::{Diagnostic, codes};
// `fill` is the trait's method, not the struct's.
use ring::rand::SecureRandom;

/// Sixty-three random bits, the sign bit clear, so a draw is never negative.
pub fn next() -> Result<i64, Diagnostic> {
    let mut bytes = [0u8; 8];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| no_entropy())?;
    Ok((u64::from_be_bytes(bytes) >> 1) as i64)
}

/// `size` random bytes, held so that what drops them zeroes them.
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
