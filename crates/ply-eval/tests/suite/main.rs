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
mod cell_arena_wiring;
mod constant_memo;
mod ctor_value_sharing;
mod determinism_audit;
mod differential_corpus;
mod equivalence_audit;
mod exploration_soundness;
mod hoist_staleness_audit;
mod host_boundary;
mod host_linearity_audit;
mod host_trust_audit;
mod list_builtins;
mod map_builtins;
mod map_order;
mod ownership_checker_armed;
mod ownership_checker_oracle;
mod reference_counting_audit;
mod reference_counting_cost;
mod reference_cycles;
mod region_boundary_audit;
mod region_isolation_audit;
mod region_kind_inference;
mod region_kind_sharing;
mod region_meaning_adversarial;
mod region_meaning_audit;
mod region_reclamation_census;
mod region_wiring_audit;
mod resumption_semantics_audit;
mod resumption_snapshot_audit;
mod secrets;
mod simulated_handlers;
mod simulation;
mod stdlib_accumulator_cost;
mod transaction_scope_audit;
mod use_after_free_audit;
mod value_semantics_audit;
mod vertical_slice;
