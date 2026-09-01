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
