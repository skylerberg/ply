use crate::support::{product, product_document, repo};
use ply_corpus::serve::{Endpoint, Parser};

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

    let out = product(
        dir.path(),
        &["test", ".", "--json", "--no-cache", "--color", "never"],
    );
    let report = product_document(&out);
    assert!(
        out.status.success() && report["summary"]["failed"].as_u64() == Some(0),
        "the reconstruction is not a twin: {:#}\n{:#}",
        report["failures"],
        report["diagnostics"]
    );
    let total = report["selection"]["total"].as_u64().unwrap_or(0);
    assert!(
        total >= 16,
        "the example declares {total} tests; this comparison is worth what they cover"
    );
}
