//! Every test in this crate that counts allocations, as one binary: a
//! `#[global_allocator]` is a whole-binary decision, so each of these used to
//! be a test target of its own, and in this crate a test target links a
//! two-hundred-crate graph.
//!
//! They share one allocator rather than a copy of it per module, which is safe
//! because `counting`'s counters are `thread_local!` and libtest gives each
//! test its own thread.

mod counting;

#[global_allocator]
static ALLOCATOR: counting::Counting = counting::Counting;

mod frame_cost;
mod w6_report_allocations;
mod w6_request_cost;
