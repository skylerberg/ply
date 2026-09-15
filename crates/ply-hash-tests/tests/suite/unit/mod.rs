//! `ply-hash`'s unit tests, in a package of their own so they compile at
//! opt-level 0 against the optimised rlib. The module tree mirrors
//! `crates/ply-hash/src`; `hash` holds what `src/tests.rs` did, the tests of
//! hashing itself.

mod hash;
mod numerics;
