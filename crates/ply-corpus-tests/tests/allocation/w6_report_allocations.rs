//! The staleness guard that cannot be blamed on a machine.

use crate::counting::charge;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives two levels below the repository root")
        .to_path_buf()
}

/// The first comma-grouped integer in `text` that is followed by `after`.
fn number_before(text: &str, after: &str) -> Option<f64> {
    let at = text.find(after)?;
    let head = &text[..at];
    let digits: String = head
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '.')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    digits.replace(',', "").trim().parse().ok()
}

/// What one `/health` request allocates, in a 200-request window.
fn per_request() -> (f64, f64) {
    let loaded = ply_corpus::w6_run::program(&repo()).expect("the service compiles");
    let request = ply_corpus::w6_run::head();
    // One warm pass, so lazily-built machine state is not charged to the count.
    loaded
        .over_sim(vec![vec![request.clone()]])
        .expect("the service serves one connection");

    const N: usize = 200;
    let script: Vec<Vec<Vec<u8>>> = (0..N).map(|_| vec![request.clone()]).collect();
    let (_, allocs, bytes) = charge(|| loaded.over_sim(script).expect("the service serves"));
    (allocs as f64 / N as f64, bytes as f64 / N as f64)
}

/// What the shipped report says a request allocates, against what one does.
#[test]
fn the_shipped_allocation_evidence_still_describes_this_request_path() {
    let text = std::fs::read_to_string(repo().join("benches/w6-ladder.json"))
        .expect("the ladder file is what the lever's size is published in");
    let report: ply_corpus::w6::Report =
        serde_json::from_str(&text).expect("the ladder file is a W6 report");
    let alternative = report
        .alternatives
        .iter()
        .find(|a| a.name.contains("boxing"))
        .expect("the report prices boxing and allocation as a lever");
    let claimed_allocs =
        number_before(&alternative.what, " times").expect("the lever states an allocation count");
    let claimed_mb =
        number_before(&alternative.what, " MB").expect("the lever states a byte count");

    let (per_request, bytes) = per_request();
    let mb_per_request = bytes / 1e6;

    println!(
        "the report says {claimed_allocs:.0} allocations and {claimed_mb:.2} MB per /health \
         request; this tree makes {per_request:.0} and {mb_per_request:.3} MB"
    );

    let ratio = claimed_allocs / per_request;
    assert!(
        (0.5..=2.0).contains(&ratio),
        "`benches/w6-ladder.json` publishes {claimed_allocs:.0} allocations per /health request \
         and this tree makes {per_request:.0} — {ratio:.1}x apart. Allocation counts do not vary \
         with a machine, so the file and the program are not describing the same request path, \
         and every share, projection and reopen threshold read off that file is about a program \
         that is not here. Re-take it: `ply-corpus w6-ladder --repo . --db <url> --machine <name> \
         --postgres <version> --out benches/w6-ladder.json`, which runs `w6-alloc` for this \
         number itself."
    );
}

/// The figures `w6-alloc` wrote.
fn shipped_figures() -> ply_corpus::w6_run::Allocation {
    let path = repo().join(ply_corpus::w6_run::ALLOCATION_FILE);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} is the request-path figure: {e}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("{} is what `w6-alloc --out` writes: {e}", path.display()))
}

const RETAKE: &str =
    "Re-take it: `./target/release/w6-alloc --repo . --requests 200 --out benches/w6-alloc.json`.";

/// What the shipped figures say a request allocates, against what one does.
#[test]
fn the_shipped_figures_still_describe_this_request_path() {
    let figures = shipped_figures();
    let (allocs, bytes) = per_request();
    println!(
        "`{}` says {:.2} allocations and {:.2} bytes per {} request; this tree makes {allocs:.2} \
         and {bytes:.2}",
        ply_corpus::w6_run::ALLOCATION_FILE,
        figures.allocations_per_request,
        figures.bytes_per_request,
        figures.route
    );
    for (what, claimed, measured) in [
        ("allocations", figures.allocations_per_request, allocs),
        ("bytes", figures.bytes_per_request, bytes),
    ] {
        let drift = (claimed - measured).abs() / measured;
        assert!(
            drift <= 0.01,
            "`{}` says one {} request makes {claimed:.2} {what} and this tree makes \
             {measured:.2} — {:.1}% apart. {RETAKE}",
            ply_corpus::w6_run::ALLOCATION_FILE,
            figures.route,
            drift * 100.0
        );
    }
}
