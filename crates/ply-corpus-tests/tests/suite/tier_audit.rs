//! The statistical gate on the prover's tiers, as CI runs it: the corpus program's `tiers`, whose
//! criteria are the gate — no draw contradicts a proof, and enough proofs were drawn from.

use crate::support::{corpus, document, measured, outcome, row};

#[test]
fn every_proof_a_generated_corpus_produces_survives_a_wide_sample() {
    let dir = tempfile::tempdir().unwrap();
    let out = corpus(dir.path(), &["tiers", "--out", "tiers", "--json"]);
    assert!(
        out.status.success(),
        "the audit refused:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = document(&out);
    for seed in 1..=6 {
        let audited = row(&report, &format!("seed {seed}"));
        assert_eq!(
            outcome(audited),
            "pass",
            "seed {seed}: a proof a sampled run contradicts is a defect in Ply: {audited:#}"
        );
    }
    let total = row(&report, "audited");
    assert_eq!(outcome(total), "pass", "{total:#}");
    eprintln!(
        "{} generated proofs re-drawn at 1,000 cases across 8 roots",
        measured(total, "proved")
    );
    assert_eq!(report["ok"].as_bool(), Some(true), "{report:#}");
}
