//! `ply-machine`'s unit tests: the modules of `crates/ply-machine/src`, one file each.

/// This test binary's pack is the checkout it was built in, as `ply`'s is the one appended to it.
#[ctor::ctor(unsafe)]
fn pack() {
    ply_machine::tested::installed(
        std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")),
        concat!(env!("CARGO_PKG_NAME"), "::", env!("CARGO_CRATE_NAME")),
    );
}

mod answered;
mod config;
mod drive;
mod hosts;
mod load;
mod payload;
mod policy;
