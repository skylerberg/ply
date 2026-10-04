//! One binary to save link time; a test reading process-global state needs a binary of its own.

/// This test binary's pack is the checkout it was built in, as `ply`'s is the one appended to it.
#[ctor::ctor(unsafe)]
fn pack() {
    ply_pack::install_checkout(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../.."
    )));
}

mod db_in_ply;
mod pg_client;
mod random_host;
mod shared_state;
mod support;
mod unit;
