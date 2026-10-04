//! One binary. A test that reads process-global state (`#[global_allocator]`) needs a binary of its own.

/// This test binary's pack is the checkout it was built in, as `ply`'s is the one appended to it.
#[ctor::ctor(unsafe)]
fn pack() {
    ply_machine::tested::installed(
        std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")),
        concat!(env!("CARGO_PKG_NAME"), "::", env!("CARGO_CRATE_NAME")),
    );
}

mod emitted;
mod fixture;
mod fragment;
mod hazards;
mod kernel;
mod number_types;
mod unit;
