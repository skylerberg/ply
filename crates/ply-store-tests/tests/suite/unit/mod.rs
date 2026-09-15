//! `ply-store`'s unit tests, in a package of their own so they compile at
//! opt-level 0 against the optimised rlib. The module tree mirrors
//! `crates/ply-store/src`; `store` holds what `src/tests.rs` did, the tests of
//! `Store` itself.

mod canonical;
mod diag;
mod schema;
mod store;
