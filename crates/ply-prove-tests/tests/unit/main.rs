//! `ply-prove`'s unit tests, in a package of their own so they compile at
//! opt-level 0 against the optimised rlib. The module tree mirrors
//! `crates/ply-prove/src` without the `tests` level; `tiers` holds `lib.rs`'s own.

mod concurrency;
mod domain;
mod key;
mod numerics;
mod property;
mod prove;
mod shrink;
mod tiers;
