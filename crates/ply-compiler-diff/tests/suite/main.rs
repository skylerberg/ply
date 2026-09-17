//! Every differential in this crate, as one binary: cargo links one per `.rs`
//! directly under `tests/`, and each link is the whole workspace with its DWARF.

mod agreement;
mod derive;
mod effects;
mod fields;
mod hash;
mod infer;
mod lexer_agreement;
mod resolve;
mod rewrite;
