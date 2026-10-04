//! Keeps `target/debug/ply` built and packed. Cargo builds a package's binaries only for that
//! package's own integration tests, and the suites that drive the binary are Ply packages, the
//! CLI's in `crates/ply-cli-tests/ply` among them. Without an integration test here, they would
//! find no `ply` to run.

use assert_cmd::Command;
use std::path::Path;

#[test]
fn the_binary_is_built_and_packed_for_the_suite_beside_this_crate() {
    let binary = assert_cmd::cargo::cargo_bin("ply");
    let repo = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
    let pack = ply_pack::Pack::of_checkout(repo).expect("the checkout packs");
    ply_pack::append(&binary, &pack).expect("the binary is packed");
    Command::new(&binary)
        .args(["std", "--digest"])
        .assert()
        .success();
}
