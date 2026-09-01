//! Every test in this crate that measures the allocator, as one binary: a
//! `#[global_allocator]` is a whole-binary decision, so each of these used to
//! be a test target of its own and cargo linked seven of them.
//!
//! They share one allocator rather than seven near-copies of it. That is safe
//! for the same reason it was already safe for the several tests inside each
//! of those binaries: `counting`'s counters are `thread_local!`, and libtest
//! gives each test its own thread.
//!
//! `tests/seam_census.rs` is still a binary of its own, and that is a real
//! difference rather than an oversight — it reads `ply_eval::census`, whose
//! accumulator is a process-wide `static` that every test in a binary writes.

mod counting;

#[global_allocator]
static ALLOCATOR: counting::Counting = counting::Counting;

mod accumulator_shape;
mod cell_write_cost;
mod fixture_open_cost;
mod link_reuse;
mod literal_sharing;
mod lowering_sharing;
mod region_arena_cost;
mod region_reclamation_audit;
