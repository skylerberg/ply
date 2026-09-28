//! The shipped allocation figure is what the launcher measures, and the request path has not got
//! worse since it was taken.
//!
//! The figure comes from `w6-alloc`, which is the only instrument that produces it: a launcher run
//! with `--count-allocs` over the service's own entry. This test runs that taker rather than
//! counting anything itself, so the number checked and the number shipped cannot come from two
//! different windows — which is what they did while the harness counted in its own process.

use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives two levels below the repository root")
        .to_path_buf()
}

/// The figure this tree measures: the taker's own output, at the window the shipped one uses.
fn measured() -> serde_json::Value {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_w6-alloc"))
        .args(["--repo", &repo().display().to_string(), "--requests", "200"])
        .output()
        .expect("the taker runs");
    assert!(
        out.status.success(),
        "the taker exited {}:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("the taker prints the figure it writes")
}

fn shipped() -> serde_json::Value {
    let path = repo().join(ply_corpus::w6_run::ALLOCATION_FILE);
    serde_json::from_str(
        &std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} is the request-path figure: {e}", path.display())),
    )
    .unwrap_or_else(|e| panic!("{} is what `w6-alloc --out` writes: {e}", path.display()))
}

const RETAKE: &str =
    "Re-take it: `./target/release/w6-alloc --repo . --requests 200 --out benches/w6-alloc.json`.";

#[test]
fn the_shipped_figure_is_what_the_launcher_measures() {
    let shipped = shipped();
    let measured = measured();
    assert_eq!(
        shipped["route"], measured["route"],
        "the figure is of one route and the taker measures another"
    );
    assert_eq!(
        shipped["requests"], measured["requests"],
        "the figure is taken at one window and this test measured another"
    );
    assert_eq!(
        shipped["response_bytes"], measured["response_bytes"],
        "the answer's size is part of what the figure describes"
    );
    for what in ["allocations_per_request", "bytes_per_request"] {
        let claimed = shipped[what].as_f64().expect("a number");
        let got = measured[what].as_f64().expect("a number");
        assert!(
            got <= claimed * 1.01,
            "`{}` caps one {} request at {claimed:.2} {what} and this tree makes {got:.2}. Lower \
             it, or {RETAKE}",
            ply_corpus::w6_run::ALLOCATION_FILE,
            shipped["route"].as_str().unwrap_or("served"),
        );
    }
}
