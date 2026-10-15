//! One binary: cargo links one per `.rs` directly under `tests/`, and linking dominates the build.

mod armed;
mod host_boundary;
mod host_trust_audit;
mod map_order;
mod position_invariance;
mod region_boundary_audit;
mod region_isolation_audit;
mod secrets;
mod simulated_handlers;
mod unit;
mod use_after_free_audit;
mod value_semantics_audit;
