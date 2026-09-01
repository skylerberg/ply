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

mod db_driver;
mod db_transaction_audit;
mod host_park;
mod support;
mod w5_drain_audit;
mod w5_shared_state;
mod w5_shutdown;
