//! `ply-cli`'s unit tests, in a package of their own so they compile at this
//! package's opt-level and link the library's rlib instead of re-codegenning
//! everything they reach at the library's. The module tree mirrors
//! `crates/ply-cli/src`.

mod artifact;
mod cli;
mod commands;
mod config;
mod db;
mod hosts;
mod load;
mod migrate;
mod signature;
mod simulation;
mod style;
mod warm;
