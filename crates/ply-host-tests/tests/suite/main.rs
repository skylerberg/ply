//! One binary to save link time; a test reading process-global state needs a binary of its own.

/// This test binary's pack is the checkout it was built in, as `ply`'s is the one appended to it.
#[ctor::ctor(unsafe)]
fn pack() {
    ply_machine::tested::installed(
        std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")),
        concat!(env!("CARGO_PKG_NAME"), "::", env!("CARGO_CRATE_NAME")),
    );
}

mod db_in_ply;
mod password_host;
mod pg_client;
mod random_host;
mod shared_state;
mod sqlite_host;
mod support;
mod unit;
