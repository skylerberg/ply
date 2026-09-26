use crate::support::generate as generate_corpus;
use ply_corpus::measure::{scheduling, throughput};
use ply_corpus::spec::CorpusSpec;

use std::path::Path;

fn corpus_at(root: &Path) {
    let spec = CorpusSpec {
        seed: 4,
        modules: 5,
        defs_per_module: 6,
        tests: 10,
        depth: 2,
        ..CorpusSpec::default()
    };
    generate_corpus(root, &spec);
}

#[test]
fn a_pass_reports_what_it_ran() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    corpus_at(&root);

    let t = throughput(&root, 1).unwrap();
    assert!(t.pass.steady_pass_millis > 0.0);
    assert!(t.pass.performs > 0, "the corpus performed no atom");
}

/// The machine lowers on first call, so setup must not be read as interpreter speed.
#[test]
fn setup_is_reported_apart_from_evaluation() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    corpus_at(&root);

    let t = throughput(&root, 9).unwrap();
    assert!(
        t.pass.first_pass_millis >= t.pass.steady_pass_millis * 0.5,
        "a first pass of {} ms against a steady {} ms",
        t.pass.first_pass_millis,
        t.pass.steady_pass_millis
    );
}

#[test]
fn scheduling_reports_the_group_count_the_shared_tests_alone_need() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    corpus_at(&root);

    let s = scheduling(&root).unwrap();
    assert_eq!(s.isolated + s.shared, s.tests);
    assert_eq!(s.groups, s.shared_groups.max(1));
}
