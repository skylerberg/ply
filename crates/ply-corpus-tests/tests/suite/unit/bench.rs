use ply_corpus::bench::{Mutation, Options, apply, run};
use ply_corpus::build::generate;
use ply_corpus::spec::CorpusSpec;
use ply_corpus::write::{Manifest, write};
use std::path::Path;

fn corpus_at(root: &Path) -> Manifest {
    let spec = CorpusSpec {
        seed: 4,
        modules: 5,
        defs_per_module: 6,
        tests: 10,
        depth: 2,
        ..CorpusSpec::default()
    };
    write(root, &spec, &generate(&spec)).unwrap().manifest
}

#[test]
fn a_mutation_is_undone_even_when_the_measurement_fails() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    let manifest = corpus_at(&root);

    let before: Vec<String> = ply_corpus::pipeline::discover(&root)
        .unwrap()
        .iter()
        .map(|p| std::fs::read_to_string(p).unwrap())
        .collect();

    let applied = apply(&root, &Mutation::Edit(manifest.leaf_edit.clone())).unwrap();
    let during: Vec<String> = ply_corpus::pipeline::discover(&root)
        .unwrap()
        .iter()
        .map(|p| std::fs::read_to_string(p).unwrap())
        .collect();
    assert_ne!(before, during, "the edit changed nothing");

    applied.undo().unwrap();
    let after: Vec<String> = ply_corpus::pipeline::discover(&root)
        .unwrap()
        .iter()
        .map(|p| std::fs::read_to_string(p).unwrap())
        .collect();
    assert_eq!(before, after);
}

#[test]
fn a_rename_touches_every_file_that_mentions_the_symbol() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    let manifest = corpus_at(&root);

    let applied = apply(
        &root,
        &Mutation::Rename(
            manifest.rename.symbol.clone(),
            manifest.rename.replacement.clone(),
        ),
    )
    .unwrap();
    assert!(!applied.files.is_empty());
    for path in ply_corpus::pipeline::discover(&root).unwrap() {
        let text = std::fs::read_to_string(&path).unwrap();
        let bare = text
            .replace(&manifest.rename.replacement, "")
            .contains(&manifest.rename.symbol);
        assert!(!bare, "`{}` still names the old symbol", path.display());
    }
    applied.undo().unwrap();
}

/// With more than one repeat, an edit scenario has to re-prove its dependents every time: if
/// the cache is not restored, every repeat after the first is a warm run and the fastest one is
/// reported.
#[test]
fn an_edit_scenario_reselects_on_every_repeat() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    corpus_at(&root);

    let report = run(
        &root,
        &Options {
            repeats: 2,
            backend: None,
        },
    )
    .unwrap();
    let named = |name: &str| {
        report
            .scenarios
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("{name}"))
    };
    let warm = named("warm");
    let hub = named("edit-hub");
    assert!(
        hub.tests_selected > warm.tests_selected,
        "editing a hub selected {} tests, no more than an unchanged run's {}",
        hub.tests_selected,
        warm.tests_selected
    );
}

/// The headline invariant, measured rather than asserted in the abstract.
#[test]
fn renaming_selects_no_more_than_an_unchanged_run() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    corpus_at(&root);

    let report = run(
        &root,
        &Options {
            repeats: 1,
            backend: None,
        },
    )
    .unwrap();
    let warm = report.scenarios.iter().find(|s| s.name == "warm").unwrap();
    let rename = report
        .scenarios
        .iter()
        .find(|s| s.name == "rename")
        .unwrap();
    assert_eq!(rename.tests_selected, warm.tests_selected);
    assert_eq!(rename.failed, 0);
}

#[test]
fn a_stale_edit_site_is_an_error_rather_than_a_silent_no_op() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    let mut manifest = corpus_at(&root);
    manifest.leaf_edit.find = "fn definitely_not_here() -> Int = 0\n".to_string();

    let err = apply(&root, &Mutation::Edit(manifest.leaf_edit)).unwrap_err();
    assert!(err.to_string().contains("occurs 0 times"));
}
