//! Every integration test in this crate, as one binary: cargo links one per
//! `.rs` directly under `tests/`, and the link dominates the build.
//!
//! A test that reads process-global state keeps a binary of its own beside this
//! one, because everything in here shares a process and runs in parallel — a
//! `#[global_allocator]`, whose count would include every other test's, and
//! `ply_eval::census`, whose accumulator is a `static` the whole binary writes.

mod fixture;

mod audit_coverage;
mod bisect_audit;
mod classification_audit;
mod effect_set_scheduling_audit;
mod host_scheduler_audit;
mod host_selection_audit;
mod host_trust_audit;
mod hybrid;
mod isolation_audit;
mod obligations;
mod region_fixture_cost;
