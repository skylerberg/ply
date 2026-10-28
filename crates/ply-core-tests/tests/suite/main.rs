//! One binary: cargo links one per `.rs` directly under `tests/`, and linking dominates the build.

mod armed;
mod host_boundary;
mod position_invariance;
mod simulated_handlers;
mod unit;
