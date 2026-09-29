//! `ply-corpus`'s unit tests: a module for each harness in `crates/ply-corpus/src`, and `bench`
//! and `real` for the corpus program's own subcommands, run the way `benches/corpus.sh` runs them.

mod bench;
mod measure;
mod payload;
mod pipeline;
mod r4;
mod real;
mod regions;
mod rng;
mod serve;
mod simulate;
mod w3;
mod w4;
mod w5;
mod w6;

use crate::support::generate;
use ply_corpus::pipeline::{Front, front};
use ply_corpus::run_on_tier;
use ply_eval::Plan;
use ply_store::Store;
use std::path::Path;

/// Every test the module declares: what a caller with no program means by "run them all".
fn visible_of(check: &ply_ty::CheckOutput) -> Vec<usize> {
    (0..check.tests.len()).collect()
}

#[test]
fn a_generated_corpus_compiles_and_every_test_passes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    generate(
        &root,
        &[
            "--seed",
            "12",
            "--modules",
            "8",
            "--defs-per-module",
            "12",
            "--tests",
            "40",
            "--depth",
            "3",
        ],
    );

    let verified = verify(&root).unwrap();
    assert_eq!(verified.failed, 0);
    assert_eq!(verified.passed, verified.tests);
    assert!(
        verified.tests >= 40,
        "only {} tests reached the runner",
        verified.tests
    );
}

#[test]
fn several_seeds_all_produce_corpora_that_compile_and_pass() {
    for seed in ["1", "2", "3", "99"] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        generate(
            &root,
            &[
                "--seed",
                seed,
                "--modules",
                "5",
                "--defs-per-module",
                "8",
                "--tests",
                "16",
                "--depth",
                "2",
            ],
        );
        let verified = verify(&root)
            .unwrap_or_else(|e| panic!("seed {seed} produced a corpus that fails: {e:#}"));
        assert_eq!(verified.failed, 0, "seed {seed}");
    }
}

#[test]
fn a_corpus_with_no_effects_still_compiles() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    generate(
        &root,
        &[
            "--seed",
            "7",
            "--modules",
            "4",
            "--defs-per-module",
            "6",
            "--tests",
            "8",
            "--depth",
            "2",
            "--effect-fraction",
            "0",
        ],
    );
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
    generate(
        &root,
        &[
            "--seed",
            "8",
            "--modules",
            "4",
            "--defs-per-module",
            "6",
            "--tests",
            "12",
            "--depth",
            "2",
            "--effect-fraction",
            "1",
        ],
    );
    assert_eq!(verify(&root).unwrap().failed, 0);
}

#[test]
fn a_single_module_corpus_is_still_a_corpus() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    generate(
        &root,
        &[
            "--seed",
            "9",
            "--modules",
            "1",
            "--defs-per-module",
            "10",
            "--tests",
            "6",
            "--depth",
            "1",
        ],
    );
    assert_eq!(verify(&root).unwrap().failed, 0);
}

/// Four `simulate` tests of three tasks, two steps each, at `density`, over a small corpus.
fn concurrent(root: &Path, density: &str, concurrent_tests: &str) {
    generate(
        root,
        &[
            "--seed",
            "31",
            "--modules",
            "4",
            "--defs-per-module",
            "6",
            "--tests",
            "8",
            "--depth",
            "2",
            "--concurrent-tests",
            concurrent_tests,
            "--tasks-per-test",
            "3",
            "--steps-per-task",
            "2",
            "--conflict-density",
            density,
        ],
    );
}

fn verify_at(density: &str) -> Verified {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    concurrent(&root, density, "4");
    verify(&root).unwrap_or_else(|e| panic!("density {density} does not compile: {e:#}"))
}

#[test]
fn a_concurrent_corpus_compiles_and_passes_at_every_density() {
    for density in ["0", "0.5", "1"] {
        let verified = verify_at(density);
        assert_eq!(verified.failed, 0, "density {density}");
        assert_eq!(verified.passed, verified.tests, "density {density}");
        assert_eq!(
            verified.seeded, 4,
            "density {density}: a `simulate` test must carry `sim.read`"
        );
    }
}

/// `sim.read` names an input no test can write, so it serializes nothing.
#[test]
fn concurrent_tests_do_not_change_how_the_suite_is_scheduled() {
    let plain = {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        concurrent(&root, "0.5", "0");
        verify(&root).unwrap()
    };
    let with_concurrency = verify_at("0.5");

    assert_eq!(plain.seeded, 0);
    assert_eq!(with_concurrency.tests, plain.tests + 4);
    assert_eq!(
        with_concurrency.groups, plain.groups,
        "four simulated tests changed the group count"
    );
}

/// A corpus whose definitions carry an obligation at `fraction`, with `specimens` specimens a
/// module.
fn specified(root: &Path, fraction: &str, specimens: &str) {
    generate(
        root,
        &[
            "--seed",
            "17",
            "--modules",
            "5",
            "--defs-per-module",
            "8",
            "--tests",
            "16",
            "--depth",
            "2",
            "--spec-fraction",
            fraction,
            "--specimens-per-module",
            specimens,
        ],
    );
}

fn front_of(fraction: &str, specimens: &str, dir: &std::path::Path) -> Front {
    let root = dir.join("corpus");
    specified(&root, fraction, specimens);
    front(&root).unwrap()
}

#[test]
fn a_specified_corpus_compiles_and_every_test_still_passes() {
    for (fraction, specimens) in [("0", "3"), ("0.5", "3"), ("1", "0"), ("1", "4")] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        specified(&root, fraction, specimens);
        let verified =
            verify(&root).unwrap_or_else(|e| panic!("density {fraction}/{specimens} fails: {e:#}"));
        assert_eq!(verified.failed, 0);
        assert_eq!(verified.passed, verified.tests);
    }
}

#[test]
fn raising_the_spec_density_changes_no_definition_hash() {
    let bare = tempfile::tempdir().unwrap();
    let specified = tempfile::tempdir().unwrap();
    let bare = front_of("0", "0", bare.path());
    let specified = front_of("1", "0", specified.path());

    assert_eq!(bare.hashes.defs.len(), specified.hashes.defs.len());
    for (name, hash) in &bare.hashes.defs {
        assert_eq!(
            specified.hashes.defs.get(name),
            Some(hash),
            "`{name}` moved when a clause was attached to it"
        );
    }
    assert_eq!(bare.hashes.tests, specified.hashes.tests);
}

#[test]
fn attaching_a_spec_changes_no_footprint_and_no_concurrency_group() {
    let bare = tempfile::tempdir().unwrap();
    let specified = tempfile::tempdir().unwrap();
    let bare = front_of("0", "0", bare.path());
    let specified = front_of("1", "0", specified.path());

    for (name, def) in &bare.check.defs {
        let other = specified
            .check
            .defs
            .get(name)
            .expect("the same definitions");
        assert_eq!(
            def.footprint.to_string(),
            other.footprint.to_string(),
            "`{name}`'s footprint moved when a clause was attached"
        );
    }
    for (a, b) in bare.check.tests.iter().zip(&specified.check.tests) {
        assert_eq!(a.footprint.to_string(), b.footprint.to_string());
        assert_eq!(a.nondet, b.nondet);
    }
}

#[test]
fn the_manifest_reports_the_obligations_the_corpus_actually_carries() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    specified(&root, "0.5", "3");
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("corpus.json")).unwrap()).unwrap();
    let specs = &manifest["specs"];

    assert_eq!(specs["specimens"].as_i64().unwrap(), 3 * 5);
    assert_eq!(specs["laws"].as_i64().unwrap(), 3 * 5);
    assert_eq!(
        specs["obligations"].as_i64().unwrap(),
        specs["decided"].as_i64().unwrap()
            + specs["sampled"].as_i64().unwrap()
            + specs["gaps"].as_i64().unwrap()
    );
    assert_eq!(
        specs["specified_definitions"].as_i64().unwrap()
            + specs["unspecified_definitions"].as_i64().unwrap(),
        manifest["definitions"].as_i64().unwrap() + specs["specimens"].as_i64().unwrap()
    );
    assert!(
        specs["decided"].as_i64().unwrap() > 0
            && specs["sampled"].as_i64().unwrap() > 0
            && specs["gaps"].as_i64().unwrap() > 0
    );
}

#[derive(Clone, Debug)]
struct Verified {
    tests: usize,
    passed: usize,
    failed: usize,
    groups: usize,
    /// Tests whose footprint carries `sim.read`, so their result depends on a seed.
    seeded: usize,
}

/// Compiles and runs a corpus with the real crates. It lives here rather than in the library
/// because nothing but these tests asked for it, and a `select` the library no longer makes is a
/// `select` the tests should own until the decision moves to the corpus's own program.
fn verify(root: &Path) -> anyhow::Result<Verified> {
    let front = front(root)?;
    let mut store = Store::open(root)?;
    store.clear()?;

    let selection = ply_test::fresh(&front.check, &visible_of(&front.check), &Plan::default());
    let report = run_on_tier(
        &front,
        &selection,
        &mut store,
        ply_test::Search::of(&selection),
        ply_test::Hosting::hermetic(),
    );

    if report.failed > 0 {
        let shown: Vec<String> = report
            .failures
            .iter()
            .take(3)
            .map(|f| format!("{}: {}", f.key, f.diagnostic.message))
            .collect();
        panic!(
            "{} of {} generated tests failed — the reference evaluator disagrees with `ply-eval`:\n  {}",
            report.failed,
            selection.total,
            shown.join("\n  ")
        );
    }

    Ok(Verified {
        tests: front.check.tests.len(),
        passed: report.passed,
        failed: report.failed,
        groups: selection.groups.len(),
        seeded: front
            .check
            .tests
            .iter()
            .filter(|t| ply_test::is_seeded(&t.footprint))
            .count(),
    })
}
