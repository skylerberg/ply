//! The region-kind analysis is shared across the machines over one program.
//!
//! Under tier-only (ADR 0048) `Loaded`'s machines run on a compiled tier, which handles regions in
//! its own emitted C and never asks the machine for the Rust `region_kind::infer` analysis. That
//! analysis is now the interpreted `eval_expr` path's alone (the prover's applier), so serving a
//! connection no longer fills it — the harness's warm pass, which existed to hoist that inference
//! out of `w6-alloc`'s measured window, hoists nothing the tier would pay for there. What remains
//! worth pinning is the sharing: two machines over one program hold one analysis, not two.

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
