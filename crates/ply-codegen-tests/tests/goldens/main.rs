//! The goldens that hold the port without a reference, in a binary of their own.
//!
//! Not in `suite`: everything there shares one process and runs in parallel, and these enter
//! the port for every program of every corpus. Put beside `suite`'s tests they disturbed
//! `fragment`'s, which read process-global producer state — the hazard `suite/main.rs`
//! already names for `ply_eval::census` and the allocator, and which a run found rather than
//! a compile (ADR 0052 §2).
//!
//! These came from `ply-compiler-diff` with their fixtures, so that deleting that crate with
//! the parser does not delete the gate.

mod harness;
mod derive;
mod hash;
mod infer;
mod resolve;
mod rewrite;
