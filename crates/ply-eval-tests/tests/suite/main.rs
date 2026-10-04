//! One binary: cargo links one per `.rs` directly under `tests/`, and linking dominates the build.

/// This test binary's pack is the checkout it was built in, as `ply`'s is the one appended to it.
#[ctor::ctor(unsafe)]
fn pack() {
    ply_machine::tested::installed(
        std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")),
        concat!(env!("CARGO_PKG_NAME"), "::", env!("CARGO_CRATE_NAME")),
    );
}

mod fixture;

mod armed;
mod call_memo;
mod constant_memo;
mod determinism_audit;
mod hoist_staleness_audit;
mod host_boundary;
mod host_linearity_audit;
mod host_trust_audit;
mod map_order;
mod position_invariance;
mod record_update_reuse;
mod reference_cycles;
mod region_boundary_audit;
mod region_isolation_audit;
mod region_meaning_adversarial;
mod region_stacks_audit;
mod secrets;
mod simulated_handlers;
mod spawn_handlers;
mod unit;
mod use_after_free_audit;
mod value_semantics_audit;
mod vertical_slice;
