//! A synthetic but realistic Ply project, at whatever scale a benchmark needs,
//! and a harness that says which compiler phase the time went to.
//!
//! A corpus the compiler rejects is worthless, so generation is not finished
//! until the corpus has been parsed, checked, hashed and run — see [`verify`].

pub mod bench;
pub mod build;
pub mod emit;
pub mod measure;
pub mod model;
pub mod pipeline;
pub mod rng;
pub mod simulate;
pub mod spec;
pub mod write;

pub use spec::CorpusSpec;

use anyhow::{Result, bail};
use ply_eval::{EngineChoice, Plan};
use ply_store::Store;
use std::path::Path;

#[derive(Clone, Debug)]
pub struct Verified {
    pub definitions: usize,
    pub tests: usize,
    pub passed: usize,
    pub failed: usize,
    pub groups: usize,
    pub largest_group: usize,
    /// Tests whose footprint carries `sim.read` — the ones whose result is a
    /// function of the definition set *and* a seed.
    pub seeded: usize,
}

/// Compiles and runs a corpus with the real crates. This is the only thing that
/// can tell the generator its reference evaluator still agrees with `ply-eval`,
/// so it runs by default rather than behind a flag.
///
/// It runs on both engines, because generated programs are the corpus most
/// likely to reach a shape no hand-written test covers, and a divergence found
/// anywhere else is found later.
pub fn verify(root: &Path) -> Result<Verified> {
    let front = pipeline::front(root)?;
    let mut store = Store::open(root)?;
    store.clear()?;

    let selection = ply_test::select(&front.check, &front.hashes, &store, &Plan::default());
    let report = ply_test::run(
        &selection,
        &front.program,
        &front.resolved,
        &front.check,
        &front.hashes,
        &mut store,
        EngineChoice::Both,
        ply_test::Search::of(&selection),
    );

    if report.failed > 0 {
        let shown: Vec<String> = report
            .failures
            .iter()
            .take(3)
            .map(|f| format!("{}: {}", f.key, f.diagnostic.message))
            .collect();
        bail!(
            "{} of {} generated tests failed — the reference evaluator disagrees with `ply-eval`:\n  {}",
            report.failed,
            selection.total,
            shown.join("\n  ")
        );
    }

    Ok(Verified {
        definitions: front.check.defs.len(),
        tests: front.check.tests.len(),
        passed: report.passed,
        failed: report.failed,
        groups: selection.groups.len(),
        largest_group: selection.groups.iter().map(|g| g.len()).max().unwrap_or(0),
        seeded: front
            .check
            .tests
            .iter()
            .filter(|t| ply_test::is_seeded(&t.footprint))
            .count(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::generate;

    /// The whole point of the crate, at a size a unit test can afford: a corpus
    /// the real compiler accepts and the real scheduler runs green.
    #[test]
    fn a_generated_corpus_compiles_and_every_test_passes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        let spec = CorpusSpec {
            seed: 12,
            modules: 8,
            defs_per_module: 12,
            tests: 40,
            depth: 3,
            ..CorpusSpec::default()
        };
        write::write(&root, &spec, &generate(&spec)).unwrap();

        let verified = verify(&root).unwrap();
        assert_eq!(verified.failed, 0);
        assert_eq!(verified.passed, verified.tests);
        assert!(
            verified.tests >= 40,
            "only {} tests reached the runner",
            verified.tests
        );
    }

    /// Tests that write a shared resource have to end up in different groups,
    /// or the corpus is not exercising the scheduler at all.
    #[test]
    fn contended_resources_force_more_than_one_concurrency_group() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        let spec = CorpusSpec {
            seed: 12,
            modules: 8,
            defs_per_module: 12,
            tests: 60,
            depth: 3,
            tables: 3,
            regions: 2,
            ..CorpusSpec::default()
        };
        write::write(&root, &spec, &generate(&spec)).unwrap();

        let verified = verify(&root).unwrap();
        assert_eq!(verified.failed, 0);
        assert!(
            verified.groups >= 2,
            "every test landed in one group, so the conflict graph is trivial"
        );
        assert!(
            verified.largest_group < verified.tests,
            "one group holds every test, so nothing was serialized"
        );
    }

    #[test]
    fn several_seeds_all_produce_corpora_that_compile_and_pass() {
        for seed in [1u64, 2, 3, 99] {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("corpus");
            let spec = CorpusSpec {
                seed,
                modules: 5,
                defs_per_module: 8,
                tests: 16,
                depth: 2,
                ..CorpusSpec::default()
            };
            write::write(&root, &spec, &generate(&spec)).unwrap();
            let verified = verify(&root)
                .unwrap_or_else(|e| panic!("seed {seed} produced a corpus that fails: {e:#}"));
            assert_eq!(verified.failed, 0, "seed {seed}");
        }
    }

    #[test]
    fn a_corpus_with_no_effects_still_compiles() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        let spec = CorpusSpec {
            seed: 7,
            modules: 4,
            defs_per_module: 6,
            tests: 8,
            depth: 2,
            effect_fraction: 0.0,
            ..CorpusSpec::default()
        };
        write::write(&root, &spec, &generate(&spec)).unwrap();
        let verified = verify(&root).unwrap();
        assert_eq!(verified.failed, 0);
        assert_eq!(
            verified.groups, 1,
            "pure tests never conflict, so one group is right"
        );
    }

    #[test]
    fn a_corpus_that_is_all_effects_still_compiles() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        let spec = CorpusSpec {
            seed: 8,
            modules: 4,
            defs_per_module: 6,
            tests: 12,
            depth: 2,
            effect_fraction: 1.0,
            ..CorpusSpec::default()
        };
        write::write(&root, &spec, &generate(&spec)).unwrap();
        assert_eq!(verify(&root).unwrap().failed, 0);
    }

    #[test]
    fn a_single_module_corpus_is_still_a_corpus() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        let spec = CorpusSpec {
            seed: 9,
            modules: 1,
            defs_per_module: 10,
            tests: 6,
            depth: 1,
            ..CorpusSpec::default()
        };
        write::write(&root, &spec, &generate(&spec)).unwrap();
        assert_eq!(verify(&root).unwrap().failed, 0);
    }

    fn concurrent_spec(density: f64) -> CorpusSpec {
        CorpusSpec {
            seed: 31,
            modules: 4,
            defs_per_module: 6,
            tests: 8,
            depth: 2,
            concurrent_tests: 4,
            tasks_per_test: 3,
            steps_per_task: 2,
            conflict_density: density,
            ..CorpusSpec::default()
        }
    }

    fn verify_at(density: f64) -> Verified {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        let spec = concurrent_spec(density);
        write::write(&root, &spec, &generate(&spec)).unwrap();
        verify(&root).unwrap_or_else(|e| panic!("density {density} does not compile: {e:#}"))
    }

    /// A generated concurrent test has to be green under **every** interleaving,
    /// or the corpus cannot be the fixed point a reduction is measured against:
    /// a failure would be reported as a race rather than as a bad generator.
    #[test]
    fn a_concurrent_corpus_compiles_and_passes_at_every_density() {
        for density in [0.0, 0.5, 1.0] {
            let verified = verify_at(density);
            assert_eq!(verified.failed, 0, "density {density}");
            assert_eq!(verified.passed, verified.tests, "density {density}");
            assert_eq!(
                verified.seeded, 4,
                "density {density}: a `simulate` test must carry `sim.read`"
            );
        }
    }

    /// `sim.read` names an input no test can write, so adding simulated tests to
    /// an otherwise-isolated corpus must not serialize anything.
    #[test]
    fn concurrent_tests_do_not_change_how_the_suite_is_scheduled() {
        let plain = {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("corpus");
            let spec = CorpusSpec {
                concurrent_tests: 0,
                ..concurrent_spec(0.5)
            };
            write::write(&root, &spec, &generate(&spec)).unwrap();
            verify(&root).unwrap()
        };
        let with_concurrency = verify_at(0.5);

        assert_eq!(plain.seeded, 0);
        assert_eq!(with_concurrency.tests, plain.tests + 4);
        assert_eq!(
            with_concurrency.groups, plain.groups,
            "four simulated tests changed the group count"
        );
    }

    /// The corpus exists to measure incremental selection, so the property it
    /// measures has to hold on the corpus itself.
    #[test]
    fn a_second_run_over_an_unchanged_corpus_selects_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        let spec = CorpusSpec {
            seed: 21,
            modules: 4,
            defs_per_module: 8,
            tests: 12,
            depth: 2,
            nondet_fraction: 0.0,
            ..CorpusSpec::default()
        };
        write::write(&root, &spec, &generate(&spec)).unwrap();
        verify(&root).unwrap();

        let front = pipeline::front(&root).unwrap();
        let store = Store::open(&root).unwrap();
        let selection = ply_test::select(&front.check, &front.hashes, &store, &Plan::default());
        let nondet = front.check.tests.iter().filter(|t| t.nondet).count();
        assert_eq!(
            selection.to_run.len(),
            nondet,
            "an unchanged corpus re-selected {} tests",
            selection.to_run.len()
        );
    }
}
