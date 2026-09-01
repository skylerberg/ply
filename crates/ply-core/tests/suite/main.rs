//! Every integration test in this crate, as one binary.
//!
//! Cargo links a separate binary per .rs directly under `tests/`, and the
//! link dominates the build. These are modules of one target instead; the
//! tests and their names are unchanged apart from the module prefix.
//!
//! A test that reads process-global state stays a binary of its own alongside
//! this one, because everything else in here runs in the same process and in
//! parallel. Two kinds so far: a `#[global_allocator]`, where the count would
//! include the other tests' allocations, and `ply_eval::census`, whose
//! accumulator is a `static` the whole binary writes to.

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
