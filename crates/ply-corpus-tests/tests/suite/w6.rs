//! The W6 evidence the tree ships: measurements and what took them, judged by the corpus program
//! rather than by anything written into the files.

use crate::support::{corpus, document, outcome, repo, row};

/// Every W6 file under `benches/`, which is where a re-take writes them.
const SHIPPED: &[&str] = &["benches/w6-ladder.json", "benches/w6-alloc.json"];

/// What only a judgement carries: a criterion or a verdict, and the words either is made of.
const JUDGING: &[&str] = &[
    "verdict",
    "criterion",
    "criteria",
    "outcome",
    "rule",
    "bound",
    "ok",
];

#[test]
fn neither_shipped_measurement_file_carries_a_verdict_or_a_criterion() {
    for name in SHIPPED {
        let path = repo().join(name);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("`{name}` is W6 evidence the tree ships: {e}"));
        let value: serde_json::Value =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("`{name}` is not JSON: {e}"));
        let mut stack = vec![&value];
        while let Some(node) = stack.pop() {
            match node {
                serde_json::Value::Object(fields) => {
                    for key in fields.keys() {
                        assert!(
                            !JUDGING.contains(&key.as_str()),
                            "`{name}` carries `{key}`, which judges a measurement rather than being one"
                        );
                    }
                    stack.extend(fields.values());
                }
                serde_json::Value::Array(items) => stack.extend(items.iter()),
                _ => {}
            }
        }
    }
}

#[test]
fn the_shipped_ladder_is_what_the_command_writes_and_is_judged_rung_by_rung() {
    let dir = tempfile::tempdir().unwrap();
    let out = corpus(dir.path(), &["w6", "--json"]);
    assert!(
        out.status.success(),
        "`w6` could not judge `benches/w6-ladder.json`, which a re-take replaces: `benches/corpus.sh \
         w6-ladder --db <url> --out benches`, or `.github/workflows/bench.yml`:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = document(&out);
    let written = row(&report, "benches/w6-ladder.json");
    assert_eq!(
        outcome(written),
        "pass",
        "`benches/w6-ladder.json` is not what `w6-ladder --out` writes, so a hand has been in it: \
         {written:#}"
    );
    for layer in [
        "call", "endpoint", "framing", "routing", "machine", "socket", "tls", "database", "tracing",
    ] {
        let judged = row(&report, &format!("layer {layer}"));
        assert!(
            !judged["criterion"]["text"]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "every rung carries its criterion: {judged:#}"
        );
    }
    for summary in ["total", "residue", "share"] {
        row(&report, summary);
    }
}
