//! Keeps `target/debug/ply` built. Cargo builds a package's binaries only for
//! that package's own integration tests, and the suite that drives the binary
//! lives in `crates/ply-cli-tests`. Without an integration test here, that
//! suite would find no `ply` to run.

use assert_cmd::Command;

#[test]
fn the_binary_is_built_for_the_suite_beside_this_crate() {
    Command::cargo_bin("ply")
        .expect("a `ply` binary")
        .args(["std", "--digest"])
        .assert()
        .success();
}
