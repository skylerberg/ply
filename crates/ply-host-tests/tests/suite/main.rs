//! Every integration test in this crate, as one binary: cargo links one per
//! `.rs` directly under `tests/`, and the link dominates the build.
//!
//! A test that reads process-global state keeps a binary of its own beside this
//! one, because everything in here shares a process and runs in parallel — a
//! `#[global_allocator]`, whose count would include every other test's, and
//! `ply_eval::census`, whose accumulator is a `static` the whole binary writes.

mod db_driver;
mod db_transaction_audit;
mod host_park;
mod support;
mod w5_drain_audit;
mod w5_shared_state;
mod w5_shutdown;
