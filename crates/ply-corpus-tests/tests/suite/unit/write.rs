use ply_corpus::build::generate;
use ply_corpus::model::DefId;
use ply_corpus::spec::CorpusSpec;
use ply_corpus::write::{pick, read_manifest, reverse_edges, transitive_dependents, write};
use std::collections::BTreeSet;

fn spec() -> CorpusSpec {
    CorpusSpec {
        seed: 2,
        modules: 6,
        defs_per_module: 8,
        tests: 12,
        depth: 3,
        ..CorpusSpec::default()
    }
}

#[test]
fn a_written_corpus_round_trips_through_its_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    let corpus = generate(&spec());
    let written = write(&root, &spec(), &corpus).unwrap();

    let read = read_manifest(&root).unwrap();
    assert_eq!(read.spec, spec());
    assert_eq!(read.definitions, written.manifest.definitions);
    assert_eq!(read.files, corpus.modules.len() + 2);
    assert!(root.join("core/prim.ply").exists());
}

#[test]
fn regenerating_over_a_corpus_is_allowed_and_over_anything_else_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    let corpus = generate(&spec());
    write(&root, &spec(), &corpus).unwrap();
    write(&root, &spec(), &corpus).expect("a corpus directory may be regenerated");

    let precious = dir.path().join("precious");
    std::fs::create_dir_all(&precious).unwrap();
    std::fs::write(precious.join("thesis.txt"), "years of work").unwrap();
    let err = write(&precious, &spec(), &corpus).unwrap_err();
    assert!(err.to_string().contains("refusing to overwrite"));
    assert!(precious.join("thesis.txt").exists());
}

#[test]
fn the_hub_edit_reaches_more_definitions_than_the_leaf_edit() {
    let corpus = generate(&spec());
    let callers = reverse_edges(&corpus);
    let tested: BTreeSet<DefId> = corpus.tests.iter().map(|t| t.root).collect();
    let hub = pick(&corpus, &callers, &tested, true).unwrap();
    let leaf = pick(&corpus, &callers, &tested, false).unwrap();
    assert!(
        transitive_dependents(&callers, hub).len() > transitive_dependents(&callers, leaf).len()
    );
}

#[test]
fn every_edit_sites_needle_occurs_exactly_once_in_its_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    let corpus = generate(&spec());
    let written = write(&root, &spec(), &corpus).unwrap();

    for site in [&written.manifest.hub_edit, &written.manifest.leaf_edit] {
        let text = std::fs::read_to_string(root.join(&site.path)).unwrap();
        assert_eq!(
            text.matches(&site.find).count(),
            1,
            "`{}` is not unique",
            site.find
        );
    }
}

#[test]
fn the_rename_target_is_a_corpus_wide_unique_symbol() {
    let corpus = generate(&spec());
    let names: BTreeSet<&str> = corpus.defs.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names.len(), corpus.defs.len());
}
