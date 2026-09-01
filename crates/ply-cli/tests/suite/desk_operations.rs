//! `examples/desk.ply` as an operator meets it: what its types say about liveness and readiness,
//! which channels each endpoint records on, and where a credential is allowed to go.

use assert_cmd::prelude::*;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

struct Run {
    stdout: String,
    stderr: String,
    ok: bool,
}

impl Run {
    fn of(args: &[&str]) -> Run {
        let out = Command::cargo_bin("ply")
            .expect("the binary is built")
            .arg("--color")
            .arg("never")
            .args(args)
            .output()
            .expect("ply runs");
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            ok: out.status.success(),
        }
    }

    fn all(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }

    #[track_caller]
    fn says(&self, needle: &str) {
        assert!(
            self.all().contains(needle),
            "the run never mentioned `{needle}`\n\n{}",
            self.all()
        );
    }

    #[track_caller]
    fn silent_about(&self, needle: &str) {
        assert!(
            !self.all().contains(needle),
            "the run mentioned `{needle}`, which it must not\n\n{}",
            self.all()
        );
    }
}

/// One definition's block from `ply check --types`: the signature line and the row lines that
/// follow it, joined.
fn signature_of(output: &str, name: &str) -> String {
    let mut lines = output.lines().skip_while(|l| {
        let trimmed = l.trim_start();
        !(trimmed.starts_with(&format!("{name} ")) && trimmed.contains(" : "))
    });
    let first = lines.next().unwrap_or_else(|| {
        panic!("`ply check --types` printed no signature for `{name}`\n\n{output}")
    });
    let mut block = first.to_string();
    for line in lines {
        // A continuation is indented past the name column and carries no `:` separator of its own.
        if line.trim_start().starts_with('/')
            || (line.starts_with("      ") && !line.contains(" : "))
        {
            block.push(' ');
            block.push_str(line.trim());
        } else {
            break;
        }
    }
    block
}

fn desk_types() -> String {
    let run = Run::of(&[
        "check",
        "--types",
        repo("examples/desk.ply").to_str().unwrap(),
    ]);
    assert!(run.ok, "`ply check --types` failed\n\n{}", run.all());
    run.stdout
}

// --- liveness and readiness -------------------------------------------------

/// ADR 0015 §6.1, and the reason the two routes exist rather than one.
#[test]
fn health_has_no_row_and_ready_names_what_it_verifies() {
    let types = desk_types();

    let health = signature_of(&types, "health");
    assert!(
        !health.contains('/'),
        "liveness must reach nothing, so that no outage can make it fail: {health}"
    );

    let ready = signature_of(&types, "ready");
    assert!(
        ready.contains("std.db.db.read[items]"),
        "a readiness route that does not reach the store checks nothing: {ready}"
    );
    assert!(
        ready.contains("std.signal.signal.read"),
        "readiness must also mean `nobody has asked this instance to stop`: {ready}"
    );
    assert!(
        !ready.contains("write"),
        "a readiness probe that wrote anything would be a load generator with a \
         two-second period: {ready}"
    );
}

// --- observability as an effect ---------------------------------------------

/// The sibling of "which tables does this route touch", which is what W5 buys: the row says which
/// channels an endpoint records on.
#[test]
fn a_row_says_which_channels_an_endpoint_records_on() {
    let types = desk_types();

    for (endpoint, channels) in [
        ("place_order", ["orders", "items"]),
        ("cancel_order", ["orders", "items"]),
    ] {
        let row = signature_of(&types, endpoint);
        for channel in channels {
            assert!(
                row.contains(&format!("std.trace.trace.write[{channel}]")),
                "`{endpoint}` must publish the `{channel}` channel it records on: {row}"
            );
        }
    }

    for quiet in [
        "list_items",
        "featured",
        "get_item",
        "list_orders",
        "get_order",
    ] {
        let row = signature_of(&types, quiet);
        assert!(
            !row.contains("std.trace"),
            "`{quiet}` records nothing, and its row must say so: {row}"
        );
    }

    // The request span is the serving layer's, not an endpoint's, so `http` appears exactly where
    // the span is opened and nowhere below it.
    assert!(signature_of(&types, "dispatch").contains("std.trace.trace.write[http]"));
    assert!(!signature_of(&types, "place_order").contains("trace.write[http]"));
}

/// A singleton `trace.write` would put every recording test in one concurrency group.
#[test]
fn two_channels_are_two_atoms_rather_than_one_recording_capability() {
    let types = desk_types();
    let row = signature_of(&types, "move_stock");
    assert!(
        row.contains("std.trace.trace.write[items]") && !row.contains("trace.write[orders]"),
        "the shelf's own movement records on the shelf's channel and no other: {row}"
    );
}

// --- configuration ----------------------------------------------------------

/// ADR 0015 §3.6: configuration is read at start-up and is a value thereafter.
#[test]
fn only_the_entry_point_reads_settings_and_only_one_route_reads_a_credential() {
    let types = desk_types();

    let main = signature_of(&types, "main");
    assert!(main.contains("std.config.config.read[server]"), "{main}");
    assert!(
        main.contains("std.config.config.read[credentials]"),
        "{main}"
    );

    // The serving layer carries the credential namespace and never the settings namespace: the port
    // was resolved before a socket existed.
    let dispatch = signature_of(&types, "dispatch");
    assert!(
        dispatch.contains("std.config.config.read[credentials]"),
        "{dispatch}"
    );
    assert!(
        !dispatch.contains("config.read[server]"),
        "a request must not re-read the deployment's settings: {dispatch}"
    );

    for below in ["place_order", "list_items", "ready", "health"] {
        assert!(
            !signature_of(&types, below).contains("std.config"),
            "`{below}` must not consult configuration; the entry point already did"
        );
    }
}

// --- the credential ---------------------------------------------------------

/// The headline, checked over a whole run rather than over one call.
#[test]
fn the_desks_credential_reaches_no_line_of_a_whole_test_run() {
    let dir = tempfile::tempdir().expect("a temp dir");
    std::fs::copy(repo("examples/desk.ply"), dir.path().join("desk.ply"))
        .expect("the example is copied");

    let run = Run::of(&["test", "--json", dir.path().to_str().unwrap()]);
    assert!(run.ok, "the desk's suite must be green\n\n{}", run.all());
    // Not in the `--json` document, not in a diff, not in a diagnostic, not in a trace field — over
    // sixty-odd tests that each drive the check end to end.
    run.silent_about("twin-key-not-a-credential");

    // Not in the result cache either, which is the one that would matter for a *failing* assertion:
    // a failure report is stored, and `Value::render` is `Secret(****)` before it gets there.
    let results = std::fs::read(dir.path().join(".ply-cache/results.json")).unwrap_or_default();
    assert!(
        !String::from_utf8_lossy(&results).contains("twin-key-not-a-credential"),
        "a credential must not reach the result cache, which never forgets"
    );

    // And the hole, pinned: a literal *is* in the front-end store, because a literal is part of the
    // definition it was written in.
    let store = std::fs::read(dir.path().join(".ply-cache/frontend.dat")).unwrap_or_default();
    assert!(
        String::from_utf8_lossy(&store).contains("twin-key-not-a-credential"),
        "ADR 0015 §2.5 (1) says a source literal enters the store; if that stopped \
         being true the sentence in the ADR needs rewriting, not this test"
    );
}

/// ADR 0015 §2.3 as a fixture: every route out of a `Secret` is a compile error, and this is the
/// list.
#[test]
fn every_route_out_of_a_secret_is_a_compile_error() {
    let run = Run::of(&[
        "check",
        repo("tests/fixtures/secret_containment.ply")
            .to_str()
            .unwrap(),
    ]);
    assert!(
        !run.ok,
        "the containment fixture must not compile — every line in it is a leak\n\n{}",
        run.all()
    );
    // `++`, a trace field and a SQL parameter are `E0201`; there is no pattern that binds the
    // payload, which is `E0101`; and a law cannot quantify over one, which is `E0418`.
    for code in ["E0201", "E0101", "E0418"] {
        run.says(code);
    }

    // Derivation is a second run because it refuses *before* inference: one file holding both would
    // report `E0206` and hide the five refusals above it.
    let derived = Run::of(&[
        "check",
        repo("tests/fixtures/secret_not_derivable.ply")
            .to_str()
            .unwrap(),
    ]);
    assert!(!derived.ok, "{}", derived.all());
    derived.says("`json` cannot be derived");
    derived.says("`ord` cannot be derived");
}

// --- what a run is, and stays -----------------------------------------------

/// The invariant W5 must not regress, over a corpus that now has `trace`, `config`, `signal` and
/// `Secret` rows in it: the desk's suite is hermetic without `--host`, and says so.
#[test]
fn the_desks_suite_is_hermetic_without_host_and_says_so() {
    let dir = tempfile::tempdir().expect("a temp dir");
    std::fs::copy(repo("examples/desk.ply"), dir.path().join("desk.ply"))
        .expect("the example is copied");

    let first = Run::of(&["test", "--explain", dir.path().to_str().unwrap()]);
    assert!(first.ok, "{}", first.all());
    // Every one of the desk's tests is region-isolated, including the ones that assert on trace
    // records and the ones that supply a credential: the twins are values in region-scoped cells,
    // so nothing reaches the boundary and nothing is `isolation: host`.
    first.says("isolated 68 of 68");
    first.silent_about("isolation: host");

    // And every one of them is cached, for the same reason: a twin-backed tracing test's row is
    // empty after the region, so it is `det`, and its second run is a cache hit rather than a
    // re-run.
    let second = Run::of(&["test", dir.path().to_str().unwrap()]);
    assert!(second.ok, "{}", second.all());
    second.says("0 passed, 68 cached");
}

/// The spans, the counters and the credential check did not cost the desk its specifications.
#[test]
fn the_desks_laws_still_hold_over_a_service_that_records_and_authenticates() {
    let run = Run::of(&["prove", repo("examples/desk.ply").to_str().unwrap()]);
    assert!(run.ok, "{}", run.all());
    run.says("7 held");
    run.says("2 proved");
    // The two placement laws now drive the whole route — the key check, the decode and the span —
    // so a credential that could not be verified would refute them rather than being invisible to
    // them.
    run.says("a placement the shelf can cover moves the shelf by exactly the drawdown");
}

// --- what an operator reads before starting it ------------------------------

/// ADR 0015 §6.5's two new blocks, over the desk.
#[test]
fn hosts_prints_where_records_go_which_channels_exist_and_what_a_signal_does() {
    let desk = repo("examples/desk.ply");
    let desk = desk.to_str().unwrap();
    let run = Run::of(&["hosts", desk, "--host", "--trace", "json"]);
    assert!(run.ok, "{}", run.all());

    run.says("observability");
    run.says("sink       ply_host::trace::json → stderr · level info");
    // Three channels, and they are the desk's own: `http` for the request span, `orders` and
    // `items` for the two tables.
    run.says("channels   http items orders");
    run.says("spans      per-task stack · closed at end_entry_point");

    run.says("shutdown");
    run.says("signals    INT TERM · lead 0ms · drain 30000ms · second signal exits 130/143");

    // `--trace off` is a handler and not an absence, so it is named; and a level on a discarding
    // sink is a distinction with no consequence, so it is not.
    let off = Run::of(&["hosts", desk, "--host", "--trace", "off"]);
    assert!(off.ok, "{}", off.all());
    off.says("sink       ply_host::trace::discard → nothing");
    off.silent_about("→ nothing · level");
}

/// ADR 0015 §6.5's digest rule, which is the whole reason the blocks are hashed rather than only
/// printed: a structural change to the trusted computing base breaks CI, and a deployment's own
/// configuration does not.
#[test]
fn the_hosts_digest_moves_with_the_sink_and_the_drain_and_not_with_a_value() {
    let desk = repo("examples/desk.ply");
    let desk = desk.to_str().unwrap();
    let digest = |extra: &[&str]| -> String {
        let mut args = vec![
            "hosts",
            desk,
            "--host",
            "--config-schema",
            "desk.config",
            "--set",
            "DESK_API_KEY=fixture-not-a-credential",
            "--digest",
        ];
        args.extend_from_slice(extra);
        let run = Run::of(&args);
        assert!(run.ok, "{}", run.all());
        run.stdout.trim().to_string()
    };

    let base = digest(&[]);
    assert!(base.starts_with("b3:"), "got {base}");

    // Structural: which sink, at which level, and how long a drain waits.
    assert_ne!(base, digest(&["--trace", "text"]), "the sink path");
    assert_ne!(base, digest(&["--trace", "off"]), "the sink path");
    assert_ne!(base, digest(&["--trace-level", "warn"]), "the level");
    assert_ne!(base, digest(&["--drain-ms", "60000"]), "the drain window");
    assert_ne!(base, digest(&["--drain-lead-ms", "2000"]), "the lead");

    // Not structural: what a key resolved to.
    assert_eq!(base, digest(&["--set", "DESK_PORT=9999"]));
    assert_eq!(base, digest(&["--set", "DESK_API_KEY=a-different-secret"]));

    // And the credential is in none of it, printed or hashed.
    let listing = Run::of(&[
        "hosts",
        desk,
        "--host",
        "--config-schema",
        "desk.config",
        "--set",
        "DESK_API_KEY=fixture-not-a-credential",
    ]);
    assert!(listing.ok, "{}", listing.all());
    listing.silent_about("fixture-not-a-credential");
    listing.says("DESK_API_KEY=**** (--set)");
}
