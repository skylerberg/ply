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

mod artifact;
mod backend;
mod cli;
mod config_cli;
mod db_cli;
mod derivation_audit;
mod derivation_determinism_audit;
mod desk_operations;
mod determinism_audit;
mod effect_set_selection;
mod effect_sets;
mod failure_classification_audit;
mod http_endpoint;
mod incremental;
mod incremental_audit;
mod json_endpoint;
mod map_cache;
mod map_law;
mod modules_hash_audit;
mod numerics;
mod prove;
mod prover_soundness_audit;
mod refcount_counters;
mod regressions;
mod routing_audit;
mod stdlib;
mod text;
mod tiers;
mod tls_cli;
mod w2_derivation_audit;
mod w2_prover_audit;
mod w2_stdlib_audit;
mod w3_http_audit;
mod w5_shutdown;
mod w5_trace_audit;
