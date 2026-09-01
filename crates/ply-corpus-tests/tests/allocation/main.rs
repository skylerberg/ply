//! Every test in this crate that counts allocations, as one binary: a
//! `#[global_allocator]` is a whole-binary decision, so each of these used to
//! be a test target of its own, and in this crate a test target links a
//! two-hundred-crate graph.
//!
//! They share one allocator rather than three near-copies of it, which is safe
//! because `counting`'s counters are `thread_local!` and libtest gives each
//! test its own thread.
//!
//! `tests/r4_value_construction.rs` and `tests/w6_alloc_sites.rs` stay
//! binaries of their own, and they are not two more copies of this: their
//! allocator walks and symbolicates a stack on every allocation to attribute
//! it to the `ply_*` frame that asked. No test in here should pay for that.
//! They are also not copies of *each other* — one keys a site by allocation
//! size and the whole frame chain, the other by the site alone — so folding
//! them together would mean one of them measuring through the other's key.

mod counting;

#[global_allocator]
static ALLOCATOR: counting::Counting = counting::Counting;

mod frame_cost;
mod w6_report_allocations;
mod w6_request_cost;
