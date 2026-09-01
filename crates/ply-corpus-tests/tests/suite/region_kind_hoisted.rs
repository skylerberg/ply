//! The region-kind analysis does not run inside a measured window.

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

/// Two machines over one program hold one analysis.
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
