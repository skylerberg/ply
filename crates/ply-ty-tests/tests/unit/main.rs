//! `ply-ty`'s unit tests, in a package of their own so they compile at
//! opt-level 0 against the optimised rlib. The module tree mirrors
//! `crates/ply-ty/src`.

mod front;
mod parse;
mod print;
mod ty;
