//! Every differential in this crate, as one binary: cargo links one per `.rs`
//! directly under `tests/`, and each link is the whole workspace with its DWARF.

mod agreement;
mod derive;
mod diag;
mod effects;
mod emit;
mod emit_diff;
mod fields;
mod front;
mod hash;
mod infer;
mod lexer_agreement;
mod lower;
mod lower_diff;
mod resolve;
mod rewrite;
