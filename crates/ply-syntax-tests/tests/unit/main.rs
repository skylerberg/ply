//! `ply-syntax`'s unit tests, in a package of their own so they compile at
//! opt-level 0 against the optimised rlib. The module tree mirrors
//! `crates/ply-syntax/src`; `parser` holds what `src/tests.rs` did, the tests
//! of the parser through its dumper.

mod lexer;
mod numerics;
mod parser;
mod resolve;
