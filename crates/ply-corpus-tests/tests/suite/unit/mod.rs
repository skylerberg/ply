//! `ply-corpus`'s unit tests, in a package of their own so they compile at
//! opt-level 0 against the optimised rlib. The module tree mirrors
//! `crates/ply-corpus/src` without the `tests` level; the crate root's own
//! tests are here.

mod bench;
mod build;
mod emit;
mod measure;
mod model;
mod payload;
mod pipeline;
mod r4;
mod regions;
mod rng;
mod serve;
mod simulate;
mod spec;
mod w3;
mod w4;
mod w5;
mod w6;
mod write;

use ply_corpus::build::generate;
use ply_corpus::emit::emit;
use ply_corpus::pipeline::{Front, front};
use ply_corpus::write::{read_manifest, write};
use ply_corpus::{CorpusSpec, Verified, verify};
use ply_eval::Plan;
use ply_store::Store;

/// The whole point of the crate, at a size a unit test can afford: a corpus the real compiler
/// accepts and the real scheduler runs green.
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
    write(&root, &spec, &generate(&spec)).unwrap();

    let verified = verify(&root).unwrap();
    assert_eq!(verified.failed, 0);
    assert_eq!(verified.passed, verified.tests);
    assert!(
        verified.tests >= 40,
        "only {} tests reached the runner",
        verified.tests
    );
}

/// Tests that write a shared resource have to end up in different groups, or the corpus is not
/// exercising the scheduler at all.
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
    write(&root, &spec, &generate(&spec)).unwrap();

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
        write(&root, &spec, &generate(&spec)).unwrap();
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
    write(&root, &spec, &generate(&spec)).unwrap();
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
    write(&root, &spec, &generate(&spec)).unwrap();
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
    write(&root, &spec, &generate(&spec)).unwrap();
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
    write(&root, &spec, &generate(&spec)).unwrap();
    verify(&root).unwrap_or_else(|e| panic!("density {density} does not compile: {e:#}"))
}

/// A generated concurrent test has to be green under **every** interleaving, or the corpus
/// cannot be the fixed point a reduction is measured against: a failure would be reported as a
/// race rather than as a bad generator.
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

/// `sim.read` names an input no test can write, so adding simulated tests to an
/// otherwise-isolated corpus must not serialize anything.
#[test]
fn concurrent_tests_do_not_change_how_the_suite_is_scheduled() {
    let plain = {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        let spec = CorpusSpec {
            concurrent_tests: 0,
            ..concurrent_spec(0.5)
        };
        write(&root, &spec, &generate(&spec)).unwrap();
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

fn specified_spec(fraction: f64, specimens: usize) -> CorpusSpec {
    CorpusSpec {
        seed: 17,
        modules: 5,
        defs_per_module: 8,
        tests: 16,
        depth: 2,
        spec_fraction: fraction,
        specimens_per_module: specimens,
        ..CorpusSpec::default()
    }
}

fn front_of(spec: &CorpusSpec, dir: &std::path::Path) -> Front {
    let root = dir.join("corpus");
    write(&root, spec, &generate(spec)).unwrap();
    front(&root).unwrap()
}

/// A corpus the compiler rejects is worthless, and a specified one is a corpus the compiler has
/// more to reject.
#[test]
fn a_specified_corpus_compiles_and_every_test_still_passes() {
    for (fraction, specimens) in [(0.0, 3), (0.5, 3), (1.0, 0), (1.0, 4)] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        let spec = specified_spec(fraction, specimens);
        write(&root, &spec, &generate(&spec)).unwrap();
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
    let bare = front_of(&specified_spec(0.0, 0), bare.path());
    let specified = front_of(&specified_spec(1.0, 0), specified.path());

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

/// And the consequence that makes the invariant worth having: a spec edit selects nothing.
#[test]
fn attaching_a_spec_to_every_definition_selects_no_test() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    let bare = specified_spec(0.0, 0);
    write(&root, &bare, &generate(&bare)).unwrap();
    verify(&root).unwrap();

    // Written over the top rather than through `write::write`, which clears the directory and
    // would take the cache the test is about with it.
    for file in emit(&generate(&specified_spec(1.0, 0))) {
        std::fs::write(root.join(&file.path), &file.text).unwrap();
    }

    let front = front(&root).unwrap();
    let store = Store::open(&root).unwrap();
    let selection = ply_test::select(
        &front.check,
        &front.hashes,
        &store,
        &Plan::default(),
        &ply_test::Engine::Evaluator,
    );
    let nondet = front.check.tests.iter().filter(|t| t.nondet).count();
    assert_eq!(
        selection.to_run.len(),
        nondet,
        "a spec edit selected {} tests",
        selection.to_run.len()
    );
}

/// A claim is not an effect, so it cannot join a footprint, so it cannot change which tests may
/// run concurrently.
#[test]
fn attaching_a_spec_changes_no_footprint_and_no_concurrency_group() {
    let bare = tempfile::tempdir().unwrap();
    let specified = tempfile::tempdir().unwrap();
    let bare = front_of(&specified_spec(0.0, 0), bare.path());
    let specified = front_of(&specified_spec(1.0, 0), specified.path());

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

/// The counts a measurement reads back.
#[test]
fn the_manifest_reports_the_obligations_the_corpus_actually_carries() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    let spec = specified_spec(0.5, 3);
    let corpus = generate(&spec);
    let written = write(&root, &spec, &corpus).unwrap();
    let specs = &written.manifest.specs;

    assert_eq!(specs.specimens, corpus.specimens.len());
    assert_eq!(specs.laws, corpus.laws.len());
    assert_eq!(
        specs.obligations,
        specs.decided + specs.sampled + specs.gaps
    );
    assert_eq!(
        specs.specified_definitions + specs.unspecified_definitions,
        corpus.defs.len() + corpus.specimens.len()
    );
    assert!(specs.decided > 0 && specs.sampled > 0 && specs.gaps > 0);
    assert_eq!(
        read_manifest(&root).unwrap().specs,
        *specs,
        "the profile does not survive the manifest"
    );
}

/// The corpus exists to measure incremental selection, so the property it measures has to hold
/// on the corpus itself.
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
    write(&root, &spec, &generate(&spec)).unwrap();
    verify(&root).unwrap();

    let front = front(&root).unwrap();
    let store = Store::open(&root).unwrap();
    let selection = ply_test::select(
        &front.check,
        &front.hashes,
        &store,
        &Plan::default(),
        &ply_test::Engine::Evaluator,
    );
    let nondet = front.check.tests.iter().filter(|t| t.nondet).count();
    assert_eq!(
        selection.to_run.len(),
        nondet,
        "an unchanged corpus re-selected {} tests",
        selection.to_run.len()
    );
}
