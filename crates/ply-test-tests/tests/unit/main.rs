//! `ply-test`'s unit tests, in a package of their own so they compile at
//! opt-level 0 against the optimised rlib. The module tree mirrors
//! `crates/ply-test/src` without the `tests` level; `runner` holds what the
//! crate root's own `tests.rs` held.

mod bisect;
mod diagnose;
mod key;
mod region;
mod runner;
mod schedule;
mod sim;
mod slice;
