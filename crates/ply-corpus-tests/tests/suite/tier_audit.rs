//! The statistical gate on the prover's tiers, as CI runs it: the corpus program's `tiers`, one
//! seed a test so the seeds spread over nextest's processes. A seed's row is the whole gate for its
//! corpus — no draw contradicts a proof, and the corpus carries its share of the proofs the audit
//! must draw from — so every seed passing here is the audit passing.

use crate::support::{corpus, document, measured, outcome, row};

/// The seeds `tiers` takes unless told otherwise, each audited by one test below.
const SEEDS: u64 = 6;

fn audit(seed: u64) {
    let dir = tempfile::tempdir().unwrap();
    let seed_text = seed.to_string();
    let out = corpus(
        dir.path(),
        &["tiers", "--out", "tiers", "--seed", &seed_text, "--json"],
    );
    assert!(
        out.status.success(),
        "the audit refused:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = document(&out);
    assert_eq!(
        report["seeds"].as_u64(),
        Some(SEEDS),
        "the tests here are every seed the audit takes, one each: {report:#}"
    );
    assert_eq!(
        report["rows"].as_array().map(Vec::len),
        Some(1),
        "only the seed asked for is audited: {report:#}"
    );
    let audited = row(&report, &format!("seed {seed}"));
    assert_eq!(
        outcome(audited),
        "pass",
        "seed {seed}: a proof a sampled run contradicts is a defect in Ply, and a corpus short of \
         its share of proofs is no gate: {audited:#}"
    );
    eprintln!(
        "seed {seed}: {} generated proofs re-drawn at {} cases from each of {} roots",
        measured(audited, "proved"),
        report["cases"],
        report["roots"]
    );
    assert_eq!(report["ok"].as_bool(), Some(true), "{report:#}");
}

#[test]
fn every_proof_of_seed_1_survives_a_wide_sample() {
    audit(1);
}

#[test]
fn every_proof_of_seed_2_survives_a_wide_sample() {
    audit(2);
}

#[test]
fn every_proof_of_seed_3_survives_a_wide_sample() {
    audit(3);
}

#[test]
fn every_proof_of_seed_4_survives_a_wide_sample() {
    audit(4);
}

#[test]
fn every_proof_of_seed_5_survives_a_wide_sample() {
    audit(5);
}

#[test]
fn every_proof_of_seed_6_survives_a_wide_sample() {
    audit(6);
}
