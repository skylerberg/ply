//! `ply-core`'s unit tests, in a package of their own so they compile at
//! opt-level 0 against the optimised rlib. The module tree mirrors
//! `crates/ply-core/src` without the `tests` level; `infer` holds what
//! `src/tests.rs` held, the tests of the checker.

mod env;
mod infer;
mod numerics;
mod print;
mod scc;
mod ty;
mod unify;
