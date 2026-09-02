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

/// What `README.md` says a request allocates, against what one does.
#[test]
fn the_readme_still_describes_this_request_path() {
    let text = std::fs::read_to_string(repo().join("README.md")).expect("the repository ships one");
    let marker = "One `/health` request makes";
    let at = text.find(marker).unwrap_or_else(|| {
        panic!(
            "`README.md` no longer contains \"{marker}\", so the sentence this guards was moved \
             or reworded. Point this test at wherever the request-path allocation count now \
             lives, or delete it and say in `docs/ONBOARDING.md` §7 that no test reads a prose \
             document again."
        )
    });
    let claimed_allocs = number_before(&text[at..], " allocations")
        .expect("the sentence states an allocation count before the word `allocations`");
    let claimed_bytes = number_before(&text[at..], " bytes")
        .expect("the sentence states a byte count before the word `bytes`");

    let (allocs, bytes) = per_request();
    println!(
        "`README.md` says {claimed_allocs:.0} allocations and {claimed_bytes:.0} bytes per \
         /health request; this tree makes {allocs:.2} and {bytes:.2}"
    );

    for (what, claimed, measured) in [
        ("allocations", claimed_allocs, allocs),
        ("bytes", claimed_bytes, bytes),
    ] {
        let drift = (claimed - measured).abs() / measured;
        assert!(
            drift <= 0.01,
            "`README.md` §\"Where this is not competitive\" says one /health request makes \
             {claimed:.0} {what} and this tree \
             makes {measured:.2} — {:.1}% apart. That sentence is present tense about this tree \
             and it has gone stale twice, the second time inside the block correcting the first. \
             Re-take it: `./target/release/w6-alloc --repo . --requests 200`.",
            drift * 100.0
        );
    }
}
