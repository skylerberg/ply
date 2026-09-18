//! One binary for every integration test, because the link dominates the build. A test that
//! reads process-global state (a `#[global_allocator]`, `ply_eval::census`) needs its own binary.

mod bodies;
mod format_audit;
mod obligations;
mod unit;
