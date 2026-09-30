//! `ply-corpus`'s tests: the generator's corpora, judged by the product itself; a module for each
//! subcommand the corpus program runs, driven the way `benches/corpus.sh` drives it; and a module for
//! each harness still in `crates/ply-corpus/src`.

mod bench;
mod measure;
mod payload;
mod real;
mod regions;
mod simulate;

use crate::support::{generate, product, product_document};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

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

    let verified = verify(&root);
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
        assert_eq!(verify(&root).failed, 0, "seed {seed}");
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
    let verified = verify(&root);
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
    assert_eq!(verify(&root).failed, 0);
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
    assert_eq!(verify(&root).failed, 0);
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
    verify(&root)
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
        verify(&root)
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

#[test]
fn a_specified_corpus_compiles_and_every_test_still_passes() {
    for (fraction, specimens) in [("0", "3"), ("0.5", "3"), ("1", "0"), ("1", "4")] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        specified(&root, fraction, specimens);
        let verified = verify(&root);
        assert_eq!(verified.failed, 0, "density {fraction}/{specimens}");
        assert_eq!(verified.passed, verified.tests);
    }
}

/// `ply <command> . --json` over a corpus, as the document it wrote.
fn reported(root: &Path, command: &str) -> Value {
    let out = product(root, &[command, ".", "--json", "--color", "never"]);
    assert!(
        out.status.success(),
        "`ply {command}` failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    product_document(&out)
}

/// A specified corpus at `fraction` with no specimens, generated under `dir`, as `ply` reports it.
fn specified_at(fraction: &str, dir: &Path, command: &str) -> Value {
    let root = dir.join("corpus");
    specified(&root, fraction, "0");
    reported(&root, command)
}

fn hashes_by_name(report: &Value) -> BTreeMap<String, Value> {
    report["definitions"]
        .as_array()
        .expect("definitions is an array")
        .iter()
        .map(|d| {
            (
                d["name"].as_str().unwrap_or("").to_string(),
                d["hash"].clone(),
            )
        })
        .collect()
}

#[test]
fn raising_the_spec_density_changes_no_definition_hash() {
    let bare = tempfile::tempdir().unwrap();
    let specified = tempfile::tempdir().unwrap();
    let bare = specified_at("0", bare.path(), "hash");
    let specified = specified_at("1", specified.path(), "hash");

    let (bare_defs, specified_defs) = (hashes_by_name(&bare), hashes_by_name(&specified));
    assert_eq!(bare_defs.len(), specified_defs.len());
    for (name, hash) in &bare_defs {
        assert_eq!(
            specified_defs.get(name),
            Some(hash),
            "`{name}` moved when a clause was attached to it"
        );
    }
    let tests = |r: &Value| -> Vec<Value> {
        r["tests"]
            .as_array()
            .expect("tests is an array")
            .iter()
            .map(|t| t["hash"].clone())
            .collect()
    };
    assert_eq!(tests(&bare), tests(&specified));
}

#[test]
fn attaching_a_spec_changes_no_footprint_and_no_concurrency_group() {
    let bare = tempfile::tempdir().unwrap();
    let specified = tempfile::tempdir().unwrap();
    let bare = specified_at("0", bare.path(), "check");
    let specified = specified_at("1", specified.path(), "check");

    let footprints = |r: &Value| -> BTreeMap<String, Value> {
        r["definitions"]
            .as_array()
            .expect("definitions is an array")
            .iter()
            .map(|d| {
                (
                    d["name"].as_str().unwrap_or("").to_string(),
                    d["footprint"].clone(),
                )
            })
            .collect()
    };
    let (bare_defs, specified_defs) = (footprints(&bare), footprints(&specified));
    for (name, footprint) in &bare_defs {
        assert_eq!(
            specified_defs.get(name),
            Some(footprint),
            "`{name}`'s footprint moved when a clause was attached"
        );
    }
    let tests = |r: &Value| -> Vec<(Value, Value)> {
        r["tests"]
            .as_array()
            .expect("tests is an array")
            .iter()
            .map(|t| (t["footprint"].clone(), t["nondet"].clone()))
            .collect()
    };
    assert_eq!(tests(&bare), tests(&specified));
}

#[test]
fn the_manifest_reports_the_obligations_the_corpus_actually_carries() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    specified(&root, "0.5", "3");
    let manifest: Value =
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
    tests: u64,
    passed: u64,
    failed: u64,
    groups: usize,
    /// Tests the product says are handed a seed, so their result depends on one.
    seeded: usize,
}

/// A corpus compiled and every one of its tests run by the product, nothing read from a cache, as
/// the report it wrote. A failure here is the reference evaluator disagreeing with the runtime.
fn verify(root: &Path) -> Verified {
    let out = product(
        root,
        &["test", ".", "--json", "--no-cache", "--color", "never"],
    );
    let report = product_document(&out);
    assert!(
        out.status.success(),
        "the generated tests failed — the reference evaluator disagrees with the runtime:\n{:#}",
        report["failures"]
    );
    let count = |v: &Value| v.as_u64().unwrap_or_else(|| panic!("not a count: {v}"));
    Verified {
        tests: count(&report["selection"]["total"]),
        passed: count(&report["summary"]["passed"]),
        failed: count(&report["summary"]["failed"]),
        groups: report["selection"]["groups"]
            .as_array()
            .expect("groups is an array")
            .len(),
        seeded: report["selection"]["tests"]
            .as_array()
            .expect("tests is an array")
            .iter()
            .filter(|t| t["seeded"].as_bool() == Some(true))
            .count(),
    }
}
