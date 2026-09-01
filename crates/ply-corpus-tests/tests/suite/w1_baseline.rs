//! The before column of W2's headline number has to be a twin, not a guess.

use ply_corpus::serve::{Endpoint, Parser};
use ply_eval::Plan;
use std::path::PathBuf;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn the_reconstructed_parser_passes_every_test_the_shipped_one_does() {
    let endpoint = Endpoint::open(&repo()).expect("`examples/hello.ply` is where it was");
    let dir = tempfile::tempdir().expect("a temp dir");
    std::fs::write(
        dir.path().join("hello.ply"),
        endpoint
            .whole(Parser::W1Folds)
            .expect("the rewrite applies"),
    )
    .unwrap();

    let loaded = match ply_cli::driver::load_full(dir.path()) {
        Ok(loaded) => loaded,
        Err(e) => panic!(
            "the reconstruction does not compile: {}",
            e.diagnostics
                .iter()
                .map(|d| format!("{}: {}", d.code, d.message))
                .collect::<Vec<_>>()
                .join("\n  ")
        ),
    };

    let mut store = ply_store::Store::open(dir.path()).expect("a cache");
    let selection = ply_test::select(&loaded.check, &loaded.hashes, &store, &Plan::default());
    assert!(
        selection.total >= 16,
        "the example declares {} tests; this comparison is worth what they cover",
        selection.total
    );

    let report = ply_test::run(
        &selection,
        &loaded.program,
        &loaded.resolved,
        &loaded.check,
        &loaded.hashes,
        &mut store,
        false,
        ply_test::Search::of(&selection),
        ply_test::Hosting::hermetic(),
    );

    assert_eq!(
        report.failed,
        0,
        "the reconstruction is not a twin: {}",
        report
            .failures
            .iter()
            .map(|f| format!("{}: {}", f.key, f.diagnostic.message))
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}
