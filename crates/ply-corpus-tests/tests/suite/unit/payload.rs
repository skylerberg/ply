//! `payload`, end to end at sizes small enough for a test: the measurement programs verified by the
//! product, a derived codec and `Map` priced in-process, `map_keys` compared across processes, and
//! derivation priced by the product's own reports.

use crate::support::{corpus, document, measured, outcome, row};

#[test]
fn payload_prices_its_codec_maps_and_derivation_and_every_criterion_holds() {
    let dir = tempfile::tempdir().unwrap();
    let out = corpus(
        dir.path(),
        &[
            "payload",
            "--lines",
            "2",
            // Enough rounds that a microsecond clock sees each half of a decode on its own.
            "--iterations",
            "50",
            "--shape",
            "2:0,2:64",
            "--entries",
            "16",
            "--types",
            "4",
            "--types-per-module",
            "4",
            "--processes",
            "2",
            "--repeats",
            "1",
            "--json",
        ],
    );
    assert!(
        out.status.success(),
        "payload refused:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = document(&out);

    // Every measurement program compiles and its own tests pass, run by the product.
    let fixtures = row(&report, "fixtures");
    assert_eq!(outcome(fixtures), "pass", "{fixtures:#}");

    let json = row(&report, "json 2");
    assert_eq!(outcome(json), "pass", "{json:#}");
    assert!(measured(json, "payload") > 0.0);

    // Widening a field grows the bytes and not the fields; each half of a decode was timed apart.
    let narrow = row(&report, "shape 2:0");
    let wide = row(&report, "shape 2:64");
    for shape in [narrow, wide] {
        assert_eq!(outcome(shape), "pass", "{shape:#}");
    }
    assert_eq!(measured(narrow, "fields"), measured(wide, "fields"));
    assert_eq!(
        measured(wide, "payload"),
        measured(narrow, "payload") + 128.0,
        "two lines each widened by 64 bytes"
    );

    let map = row(&report, "map 16");
    assert_eq!(outcome(map), "pass", "{map:#}");

    let order = row(&report, "order");
    assert_eq!(outcome(order), "pass", "{order:#}");
    assert_eq!(measured(order, "processes"), 2.0);

    // The same types with and without a `derive`: the derived variant adds definitions and no test.
    let derive = row(&report, "derive 4");
    assert_eq!(outcome(derive), "pass", "{derive:#}");
    assert_eq!(measured(derive, "plain_tests"), 4.0, "{derive:#}");
    assert!(measured(derive, "derived_cache") > 0.0, "{derive:#}");

    assert_eq!(report["ok"].as_bool(), Some(true), "{report:#}");
    // Its fixtures are written under the build output and taken back out, and nothing is written
    // beside the tree it was started in.
    let names = |at: &std::path::Path| -> Vec<String> {
        std::fs::read_dir(at)
            .map(|entries| {
                entries
                    .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    };
    let left = names(&dir.path().join("target/corpus/payload"));
    assert!(left.is_empty(), "payload left scratch behind: {left:?}");
    let beside: Vec<String> = names(dir.path())
        .into_iter()
        .filter(|n| n != "target")
        .collect();
    assert!(
        beside.is_empty(),
        "payload wrote beside the tree: {beside:?}"
    );
}

#[test]
fn a_shape_that_is_not_two_counts_is_refused_before_anything_runs() {
    let dir = tempfile::tempdir().unwrap();
    let out = corpus(dir.path(), &["payload", "--shape", "2:x"]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("2:x"), "{stderr}");
}
