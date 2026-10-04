//! `ply-machine`'s unit tests: the modules of `crates/ply-machine/src`, one file each.

/// This test binary's pack is the checkout it was built in, as `ply`'s is the one appended to it.
#[ctor::ctor(unsafe)]
fn pack() {
    ply_pack::install_checkout(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../.."
    )));
}

mod answered;
mod config;
mod drive;
mod hosts;
mod load;
mod payload;
mod policy;
