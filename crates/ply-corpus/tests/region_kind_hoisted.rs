//! The region-kind analysis does not run inside a measured window.
//!
//! `w6-alloc` and `w6_alloc_sites.rs` both run a warm pass before they arm
//! their counters, on the stated ground that "lazily built machine state is not
//! charged to the count". That was false for one item: the region-kind analysis
//! was a field of the `Machine`, and both harnesses build a fresh `Machine` per
//! call — so the warm pass warmed a machine that was then dropped, and the
//! counted window paid a whole-program traversal that no request causes.
//!
//! Measured, before and after, with `./target/release/w6-alloc --repo .
//! --requests N`: **1,122.3 → 1,081.8** at 200 requests and **972.0 → 961.9**
//! at 800, a delta that halves as the window doubles because the work was one
//! per `Machine` rather than one per request. Only the second half of each pair
//! is re-takeable from this tree; the first is what the pre-hoist tree printed
//! and nothing here can reproduce it.
//!
//! `w6_alloc_sites.rs`'s two-window fit is where that shape is read off
//! directly. **Run today it puts `ply_eval::region_kind` at 0.0 allocations per
//! request and 0 per `Machine`** — which is the whole claim, and is what
//! `cargo test -p ply-corpus --release --test w6_alloc_sites -- --nocapture`
//! prints under "the two hoist candidates".
//!
//! > **Corrected.** This paragraph read "it puts `ply_eval::region_kind` at
//! > **0.0 allocations per request** and 8,103 per `Machine`, and 8,103/200 is
//! > the 40.5 the figure above moved by." The arithmetic is right and the
//! > present tense is not: 8,103 is the **pre-hoist** intercept, so a reader
//! > running the command beside it sees 0 and concludes the fit is broken. The
//! > pre-hoist intercept is not re-takeable from this tree; what is, is that the
//! > intercept is now zero.
//!
//! So what is asserted here is the mechanism rather than the number: the
//! analysis belongs to the program, the harness's warm pass fills it, and the
//! machine built for the counted window is handed the filled one.

use ply_corpus::w3;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives two levels below the repository root")
        .to_path_buf()
}

fn service() -> w3::Loaded {
    ply_corpus::w6_run::program(&repo()).expect("the service must compile")
}

#[test]
fn the_harnesss_warm_pass_fills_the_analysis_the_counted_window_would_have_paid_for() {
    let loaded = service();
    assert!(
        loaded.shared_region_kinds().get().is_none(),
        "the analysis ran at load, before any machine asked for it"
    );

    // Exactly what `w6-alloc` does before it zeroes its counters.
    let request = ply_corpus::w6_run::head();
    loaded
        .over_sim(vec![vec![request]])
        .expect("the service serves one connection");

    let filled = loaded
        .shared_region_kinds()
        .get()
        .expect("serving a connection opens a region, so the analysis has run")
        .len();
    assert!(filled > 0, "the service declares no region");
}

/// Two machines over one program hold one analysis. An equal one would mean it
/// was inferred twice, which is the cost this exists to remove.
#[test]
fn every_machine_over_one_program_holds_one_analysis() {
    let loaded = service();
    let first = loaded.machine();
    let second = loaded.machine();
    assert!(
        std::ptr::eq(first.region_kinds(), second.region_kinds()),
        "the second machine over the same program inferred its own region kinds"
    );
}
