//! The served subcommands end to end, run the way `benches/corpus.sh` runs them: the desk and the C
//! floor started as children of the corpus program, found by the port each says it listens on,
//! loaded by the Ply client, and stopped. No database: the desk serves its twin.
//!
//! Only the rows whose criterion is about correctness are asserted — every request answered, the
//! floor answering the desk's bytes, each handshake timed apart, each server stopping on its signal —
//! and what a request allocates, which is a count. A rate on a shared runner is a reading, not a
//! verdict this test can own.

use crate::support::{document, measured, outcome, repo, row, served};
use serde_json::Value;
use std::path::Path;

fn run(dir: &Path, args: &[&str]) -> Value {
    let out = served(dir, args);
    assert!(
        out.status.success(),
        "`{}` failed:\n{}\n{}",
        args.join(" "),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    document(&out)
}

fn rows(report: &Value) -> Vec<Value> {
    report["rows"]
        .as_array()
        .expect("a report carries its rows")
        .clone()
}

fn criterion(row: &Value) -> &str {
    row["criterion"]["text"].as_str().unwrap_or_default()
}

/// The rows a correct run passes whatever the machine: answered, agreed, handshook, stopped.
fn correctness(row: &Value) -> bool {
    let c = criterion(row);
    c.starts_with("every request answered 200")
        || c == "the desk's status, fields and body"
        || c.contains("SIGTERM")
}

fn assert_correct(report: &Value) {
    let checked: Vec<Value> = rows(report).into_iter().filter(correctness).collect();
    assert!(!checked.is_empty(), "no correctness row: {report:#}");
    for row in checked {
        assert_eq!(outcome(&row), "pass", "{row:#}");
    }
}

fn named<'a>(rows: &'a [Value], prefix: &str) -> Vec<&'a Value> {
    rows.iter()
        .filter(|row| row["name"].as_str().unwrap_or_default().starts_with(prefix))
        .collect()
}

#[test]
fn the_socket_bench_serves_the_desk_and_its_floor() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo();
    let repo = repo.to_str().expect("the checkout's path is text");

    let serve = run(
        dir.path(),
        &[
            "serve",
            "--repo",
            repo,
            "--sections",
            "layers,scans,load",
            "--requests",
            "8",
            "--concurrency",
            "2",
            "--repeats",
            "1",
            "--iterations",
            "20",
            "--json",
        ],
    );
    assert_correct(&serve);
    let taken = rows(&serve);
    assert_eq!(
        named(&taken, "layer handled").len(),
        1,
        "the layers table has its whole-request rung: {serve:#}"
    );
    assert!(
        !named(&taken, "scans, 0 filler field(s)").is_empty(),
        "the scans table has its first head: {serve:#}"
    );
    for prefix in [
        "desk, one connection at a time, 1 client",
        "desk, a task per connection, 2 clients",
        "floor, one connection at a time, 1 client",
        "floor, a thread per connection, 2 clients",
    ] {
        let found = named(&taken, prefix);
        assert_eq!(found.len(), 1, "{prefix}: {serve:#}");
        assert_eq!(measured(found[0], "asked"), 8.0, "{prefix}: {serve:#}");
        assert_eq!(measured(found[0], "answered"), 8.0, "{prefix}: {serve:#}");
    }

    let w3 = run(
        dir.path(),
        &[
            "w3",
            "--repo",
            repo,
            "--sections",
            "keep-alive,tls",
            "--accept",
            "task-per-connection",
            "--ladder-requests",
            "8",
            "--ladder-concurrency",
            "2",
            "--repeats",
            "1",
            "--json",
        ],
    );
    assert_correct(&w3);
    let taken = rows(&w3);
    for per_conn in ["1 request a connection", "100 requests a connection"] {
        assert_eq!(named(&taken, per_conn).len(), 1, "{per_conn}: {w3:#}");
    }
    let secured = named(&taken, "TLS, ");
    assert_eq!(
        secured.len(),
        6,
        "three ladders at two client counts: {w3:#}"
    );
    for row in secured {
        assert!(
            measured(row, "handshakes") > 0.0,
            "a TLS row times its handshakes: {row:#}"
        );
    }
}

/// The ladder with no database: its in-process rungs taken, the twin and the floor served, the rungs
/// postgres would carry saying why they are not, and the desk as it is written counted over two
/// served windows and held to the figure `benches/w6-alloc.json` ships.
#[test]
fn the_ladder_serves_the_desk_and_holds_the_shipped_allocation_figure() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo();
    let repo = repo.to_str().expect("the checkout's path is text");

    let ladder = run(
        dir.path(),
        &[
            "w6-ladder",
            "--repo",
            repo,
            "--repeats",
            "1",
            "--served-repeats",
            "1",
            "--concurrency",
            "1,2",
            "--per-conn",
            "4",
            "--requests-per-point",
            "8",
            "--json",
        ],
    );
    assert_correct(&ladder);
    let taken = rows(&ladder);
    for layer in ["call", "endpoint", "framing", "routing", "machine"] {
        let rung = row(&ladder, &format!("layer {layer}"));
        assert!(
            measured(rung, "with") > 0.0,
            "the in-process `{layer}` rung was timed: {rung:#}"
        );
    }
    for prefix in [
        "the twin in plaintext, /health, 1 client",
        "the twin in plaintext, /health, 2 clients",
        "the floor, one connection at a time, /health, 2 clients",
    ] {
        let found = named(&taken, prefix);
        assert_eq!(found.len(), 1, "{prefix}: {ladder:#}");
        assert_eq!(
            measured(found[0], "answered"),
            measured(found[0], "asked"),
            "{prefix}: {ladder:#}"
        );
    }
    for untaken in [
        "layer socket",
        "layer tls",
        "layer database",
        "layer tracing",
        "total",
        "residue",
        "share",
    ] {
        let judged = row(&ladder, untaken);
        assert_eq!(outcome(judged), "inconclusive", "{judged:#}");
        assert!(
            judged["verdict"]["why"]
                .as_str()
                .unwrap_or_default()
                .contains("no database"),
            "a rung postgres would carry says so: {judged:#}"
        );
    }

    let allocations = row(&ladder, "allocations, one /health request");
    assert_eq!(
        outcome(allocations),
        "pass",
        "the request path allocates more than `benches/w6-alloc.json` ships, or the figure is not \
         one this tree takes. Lower it, or re-take it: `benches/corpus.sh w6-ladder --sections alloc \
         --out benches`, or `.github/workflows/bench.yml`. {allocations:#}"
    );
}
