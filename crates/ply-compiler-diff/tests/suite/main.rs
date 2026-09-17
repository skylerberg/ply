//! Every differential in this crate, as one binary: cargo links one per `.rs`
//! directly under `tests/`, and each link is the whole workspace with its DWARF.

mod agreement;
mod effects;
mod fields;
mod lexer_agreement;
