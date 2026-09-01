//! The differential tier audit, over generated corpora.

use ply_cli::engine::Prover;
use ply_cli::load::load;
use ply_cli::obligations;
use ply_corpus::{CorpusSpec, build::generate, write};
use ply_prove::{Discharge, Gap, ProvePlan, Tier};

#[test]
fn every_proof_a_generated_corpus_produces_survives_a_wide_sample() {
    let wide = ProvePlan {
        cases: 1_000,
        roots: (0..8).collect(),
        ..ProvePlan::default()
    };
    let mut audited = 0;
    for seed in 1..=6u64 {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        let spec = CorpusSpec {
            seed,
            modules: 6,
            defs_per_module: 10,
            tests: 20,
            depth: 3,
            spec_fraction: 0.4,
            specimens_per_module: 3,
            ..CorpusSpec::default()
        };
        write::write(&root, &spec, &generate(&spec)).unwrap();

        let loaded = load(&root).expect("a generated corpus compiles");
        let hashes = loaded.hashes.clone();
        let collected = obligations::collect(&loaded.program, &loaded.check, &hashes);
        let prover = Prover::new(&loaded.program, &loaded.resolved, &loaded.check);
        for obligation in &collected.obligations {
            if prover
                .discharge_with(obligation, &ProvePlan::default())
                .tier()
                != Some(Tier::Proved)
            {
                continue;
            }
            audited += 1;
            // A refutation and a raise are both defects.
            if let Some(defect) = disagreement(&prover.resample(obligation, &wide)) {
                panic!(
                    "seed {seed}: `{}` is reported `proved` and a sampled run {defect} — a \
                     defect in Ply",
                    obligation.owner
                );
            }
        }
    }
    eprintln!("{audited} generated proofs re-sampled at 1,000 cases across 8 roots");
    assert!(audited >= 100, "only {audited} proofs were audited");
}

/// How a sampled run contradicts a proof, or `None` when it does not.
pub fn disagreement(discharge: &Discharge) -> Option<String> {
    match discharge {
        Discharge::Refuted(counterexample) => Some(format!(
            "refutes it at {}",
            counterexample
                .bindings
                .iter()
                .map(|b| format!("{} = {}", b.name, b.rendered))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        Discharge::Unattempted(Gap::Raised {
            bindings,
            diagnostic,
        }) => Some(format!(
            "raises `{}` at {}",
            diagnostic.message,
            bindings
                .iter()
                .map(|b| format!("{} = {}", b.name, b.rendered))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        _ => None,
    }
}
