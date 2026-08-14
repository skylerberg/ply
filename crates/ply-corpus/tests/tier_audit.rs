//! The differential tier audit, over generated corpora.
//!
//! `ply-cli`'s copy of this audit runs over `examples/` and four fixtures, which
//! is every proof a human wrote. ADR 0007 §11 asks for the generated corpus too,
//! and it is the half that matters for a prover: a hand-written corpus exercises
//! the shapes its author thought of, and a generated one reaches call graphs
//! nobody chose. A `proved` obligation that a sampled run refutes is a defect in
//! Ply, and this is where one would first show up.
//!
//! **A raise counts.** Ply's arithmetic is `checked_*` and its recursion is
//! bounded, so an obligation that is valid over ℤ and over total function
//! symbols but wrong about Ply cannot surface as a refutation — it surfaces as
//! `Gap::Raised`. An audit that looked only for `Discharge::Refuted` would look
//! straight past the exact defect it was written for.
//!
//! Seeds are swept because one seed is one call graph, exactly as
//! `differential_sweep.rs` argues for the two engines.

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
            if prover.discharge_with(obligation, &ProvePlan::default()).tier() != Some(Tier::Proved)
            {
                continue;
            }
            audited += 1;
            // A refutation and a raise are both defects. A vacuity is not: it
            // is a claim about the sample, because a guard the prover showed
            // valid can still reject every drawn tuple and the proved path
            // established its own guard.
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
///
/// A proof claims that every input satisfying the guard has an answer and that
/// the answer is `true`. A refutation denies the second; a raise denies the
/// first, and denying the first is the shape ADR 0007 §5.1(a)'s disclosed
/// ℤ-versus-`i64` divergence has to take, because nothing in Ply wraps.
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
