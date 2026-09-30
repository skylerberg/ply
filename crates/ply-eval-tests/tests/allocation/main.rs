//! A binary of its own: it installs a `#[global_allocator]`.

mod counting;

#[global_allocator]
static ALLOCATOR: counting::Counting = counting::Counting;

mod bridge_reuse;
mod cell_write_cost;
mod fixture_open_cost;
mod frame_cost;
mod link_reuse;
mod region_arena_cost;
mod region_reclamation_audit;
