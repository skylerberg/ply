//! Every integration test in this crate, as one binary: cargo links one per
//! `.rs` directly under `tests/`, and the link dominates the build.
//!
//! A test that reads process-global state keeps a binary of its own beside this
//! one, because everything in here shares a process and runs in parallel — a
//! `#[global_allocator]`, whose count would include every other test's, and
//! `ply_eval::census`, whose accumulator is a `static` the whole binary writes.

mod fixture;

mod bit_operators;
mod byte_builtins;
mod derivation;
mod effect_sets;
mod effect_sets_audit;
mod iterate_builtin;
mod list_builtins;
mod map_keys;
mod record_update;
mod region_escape_audit;
mod regions;
mod secrets;
mod shipped_modules;
mod try_op;
