//! `seam_census.rs` stays a binary of its own: `ply_eval::census` is a process-wide static.

mod counting;

#[global_allocator]
static ALLOCATOR: counting::Counting = counting::Counting;

mod cell_write_cost;
mod fixture_open_cost;
mod link_reuse;
mod region_arena_cost;
mod region_reclamation_audit;
