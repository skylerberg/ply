//! A binary of its own: it installs a `#[global_allocator]`.

/// This test binary's pack is the checkout it was built in, and what each test reads of it is traced.
#[ctor::ctor(unsafe)]
fn pack() {
    ply_machine::tested::installed(
        std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")),
        concat!(env!("CARGO_PKG_NAME"), "::", env!("CARGO_CRATE_NAME")),
    );
}

mod counting;

#[global_allocator]
static ALLOCATOR: counting::Counting = counting::Counting;

mod bridge_reuse;
mod cell_write_cost;
mod fixture_open_cost;
mod region_arena_cost;
mod region_reclamation_audit;
