//! One binary: cargo links one per `.rs` directly under `tests/`, and linking dominates the build.

mod fixture;

mod constant_memo;
mod determinism_audit;
mod hoist_staleness_audit;
mod host_boundary;
mod host_linearity_audit;
mod host_trust_audit;
mod map_order;
mod position_invariance_g1;
mod record_update_reuse;
mod reference_cycles;
mod region_boundary_audit;
mod region_isolation_audit;
mod region_meaning_adversarial;
mod resumption_snapshot_audit;
mod secrets;
mod simulated_handlers;
mod unit;
mod use_after_free_audit;
mod value_semantics_audit;
mod vertical_slice;
