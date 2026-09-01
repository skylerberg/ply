//! Observability as an effect, checked from the outside.
//!
//! The claims ADR 0015 §1 makes are claims about a *row* and about a
//! *substitution*, and neither is visible from inside `ply-host`. This file is
//! where they are asserted the way a user would see them: `ply check --types`
//! prints which channels a definition records on, `ply test --explain` puts two
//! channels in one concurrency group and two definitions on one channel in two,
//! a `det` test that reaches an unhandled `trace` operation does not compile,
//! and a test that installs the twin has an empty row and is cached.

use assert_cmd::prelude::*;
use ply_span::codes;
use serde_json::Value;
use std::path::Path;
use std::process::Command;

fn write(dir: &Path, rel: &str, text: &str) {
    std::fs::write(dir.join(rel), text).unwrap();
}

fn ply(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ply").unwrap();
    cmd.arg("--color").arg("never").current_dir(dir);
    cmd
}

fn output(out: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn json(dir: &Path, args: &[&str]) -> Value {
    let out = ply(dir).args(args).output().unwrap();
    serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .unwrap_or_else(|e| panic!("{e}: {}", output(&out)))
}

/// Two endpoints recording on two channels, and the clause set a test installs
/// to collect what they recorded. The `with_cell` is **inside** each test, which
/// is the whole of ADR 0015 §7's first row: one collector around a suite is one
/// shared cell, and two tests asserting on it are coupled exactly as W4's pooled
/// connection coupled two the footprint graph believed were disjoint.
const SERVICE: &str = r#"
import std.trace
import std.trace (trace)

pub fn place_order(sku: String) -> Int / {trace.write[orders]} {
  let span = trace.enter[orders]("place_order", map_new());
  trace.count[orders]("orders_placed", 1, map_new());
  trace.exit[orders](span, trace::Ok);
  1
}

pub fn restock(sku: String) -> Int / {trace.write[items]} {
  trace.event[items](trace::Info, "restocked", map_new());
  2
}

test "what place_order records is a claim a test can assert on" {
  with_cell[collector](trace::sink()) { s -> {
    handle {
      assert_eq(place_order("BOLT-1"), 1)
    } with {
      trace.enter[orders](n, fs) -> {
        let out = trace::enter_step(cell_get(s), "orders", n, fs);
        cell_set(s, out.sink);
        out.span
      },
      trace.exit[orders](sp, o) -> {
        let out = trace::exit_step(cell_get(s), sp, o);
        cell_set(s, out.sink);
        assert(out.ok)
      },
      trace.count[orders](n, d, fs) ->
        cell_set(s, trace::count_step(cell_get(s), "orders", n, d, fs)),
    };
    let rs = trace::drain(cell_get(s));
    assert_eq(len(rs), 3);
    assert_eq(trace::counter_total(rs, "orders_placed"), 1);
    assert_eq(len(trace::on_channel(rs, "items")), 0)
  } }
}

test "what restock records is recorded on its own channel" {
  with_cell[collector](trace::sink()) { s -> {
    handle {
      assert_eq(restock("BOLT-1"), 2)
    } with {
      trace.event[items](l, n, fs) ->
        cell_set(s, trace::event_step(cell_get(s), "items", l, n, fs)),
    };
    assert_eq(len(trace::on_channel(trace::drain(cell_get(s)), "items")), 1)
  } }
}
"#;

/// A row says what a function records, and it says it per channel. That is the
/// sentence the whole milestone rests on, and `ply check --types` is where a
/// reader sees it with no flag.
#[test]
fn a_functions_row_names_the_channels_it_records_on() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "app.ply", SERVICE);

    let out = ply(dir.path()).args(["check", "--types"]).output().unwrap();
    // Whitespace-insensitive, because a long row wraps and the claim is about
    // the atoms rather than about the column the printer chose.
    let text: String = output(&out)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        text.contains("place_order : (String) -> Int / {std.trace.trace.write[orders]}"),
        "{text}"
    );
    assert!(
        text.contains("restock : (String) -> Int / {std.trace.trace.write[items]}"),
        "{text}"
    );
}

/// The reason the resource is a channel rather than a singleton. Two definitions
/// recording on two channels do not conflict; two on one channel do, and the
/// existing conflict graph is what serialises them — no new mechanism.
#[test]
fn two_channels_do_not_conflict_and_two_definitions_on_one_channel_do() {
    let dir = tempfile::tempdir().unwrap();
    // Host-backed, so the atoms survive into the schedule rather than being
    // discharged by a handler. `--host` is what makes these two tests reach the
    // sink, which is the arrangement the conflict graph exists to protect.
    write(
        dir.path(),
        "app.ply",
        r#"
import std.trace
import std.trace (trace)

test/nondet "records on orders" {
  trace.event[orders](trace::Info, "a", map_new())
}

test/nondet "records on orders too" {
  trace.event[orders](trace::Info, "b", map_new())
}

test/nondet "records on items" {
  trace.event[items](trace::Info, "c", map_new())
}
"#,
    );

    let explained = json(dir.path(), &["test", "--host", "--explain", "--json"]);
    let selection = &explained["selection"];
    let group_of = |name: &str| -> i64 {
        selection["tests"]
            .as_array()
            .unwrap_or_else(|| panic!("no selection block: {explained:#}"))
            .iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("no test `{name}`: {explained:#}"))["group"]
            .as_i64()
            .expect("a group")
    };
    assert_ne!(
        group_of("records on orders"),
        group_of("records on orders too"),
        "two tests recording on one channel conflict and are serialised: {explained:#}"
    );
    assert_eq!(
        group_of("records on orders"),
        group_of("records on items"),
        "two channels do not conflict, so these two run beside each other: {explained:#}"
    );
    // Three tests, two groups. One singleton `trace.write` would have made it
    // one group of three, which is the cost §1.2 refuses.
    assert_eq!(selection["parallelism"]["groups"], 2, "{explained:#}");
}

/// The span stack survives the scheduler, end to end and through the real
/// production scheduler rather than through a Rust unit test.
///
/// Two tasks each open a span, yield, open a second inside it, yield again, and
/// close both. The stack is the **driver's, per task**, so each inner span nests
/// under its own outer one — and the check is against the tree the record list
/// itself implies, so it would catch a driver that got the links right by
/// accident of ordering.
///
/// A stack keyed on the machine alone would put `b` inside `a`. A stack the
/// *program* maintained would need the span to survive a `task.yield`, which is
/// a continuation the program does not hold.
#[test]
fn two_tasks_interleaving_spans_nest_into_their_own_and_not_each_others() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "app.ply",
        r#"
import std.trace
import std.trace (trace)

fn work(name: String) -> Int / {trace.write[http], task.write} {
  let outer = trace.enter[http](name, map_new());
  task.yield();
  let inner = trace.enter[http](string_concat(name, "-inner"), map_new());
  task.yield();
  trace.exit[http](inner, trace::Ok);
  trace.exit[http](outer, trace::Ok);
  1
}

pub fn main() -> Int / {trace.write[http], task.write} {
  let a = task.spawn(|| work("a"));
  let b = task.spawn(|| work("b"));
  task.join(a) + task.join(b)
}
"#,
    );

    let out = ply(dir.path())
        .args(["run", "--host", "--trace", "json"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let records: Vec<Value> = stderr
        .lines()
        .filter(|line| line.starts_with('{'))
        .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}")))
        .collect();
    assert_eq!(
        records.len(),
        8,
        "two spans per task, entered and exited: {stderr}"
    );

    let name_of = |id: &Value| -> String {
        records
            .iter()
            .find(|r| &r["span"] == id && r["kind"] == "enter")
            .map_or_else(
                || "-".to_string(),
                |r| r["name"].as_str().unwrap().to_string(),
            )
    };
    let parent_of = |name: &str| -> String {
        let record = records
            .iter()
            .find(|r| r["name"] == name && r["kind"] == "enter")
            .unwrap_or_else(|| panic!("no `{name}`: {stderr}"));
        name_of(&record["parent"])
    };
    assert_eq!(parent_of("a"), "-", "{stderr}");
    assert_eq!(
        parent_of("b"),
        "-",
        "task 2's span is not inside task 1's: {stderr}"
    );
    assert_eq!(parent_of("a-inner"), "a", "{stderr}");
    assert_eq!(parent_of("b-inner"), "b", "{stderr}");
    assert!(
        records.iter().all(|r| r["outcome"] != "abandoned"),
        "every span was closed by the task that opened it: {stderr}"
    );
}

/// A continuation resumed later cannot corrupt the span tree, and the mechanism
/// that stops it is one already in the boundary rather than one this milestone
/// added.
///
/// `trace.*` registers `Linearity::AtMostOnce` because replaying a continuation
/// across an event writes the event twice, and a duplicated span in a log is a
/// wrong answer about what happened rather than a missing one. A second `resume`
/// over a captured span is therefore `E0426` naming the operation — before it is
/// performed a second time, so the count is one rather than two.
#[test]
fn a_continuation_resumed_twice_across_a_span_is_e0426() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "app.ply",
        r#"
import std.trace
import std.trace (trace)

effect ask { read pick() -> Int }

fn body() -> Int / {trace.write[http], ask.read} {
  let n = ask.pick();
  let span = trace.enter[http]("work", map_new());
  trace.exit[http](span, trace::Ok);
  n
}

pub fn main() -> Int / {trace.write[http]} =
  handle { body() } with {
    ask.pick() resume k -> k(1) + k(2),
  }
"#,
    );
    let report = json(dir.path(), &["run", "--host", "--trace", "json", "--json"]);
    let codes: Vec<&str> = report["diagnostics"]
        .as_array()
        .unwrap_or_else(|| panic!("{report:#}"))
        .iter()
        .filter_map(|d| d["code"].as_str())
        .collect();
    assert!(
        codes.contains(&codes::HOST_CONTINUATION_RESUMED),
        "a replayed span is a wrong answer about what happened: {report:#}"
    );
}

/// `nondet` on the declaration is load-bearing: a `det` test that reaches an
/// unhandled `trace` operation does not compile, with `--host` and without it,
/// and the only way to make it compile is to install a collecting handler.
#[test]
fn a_det_test_reaching_an_unhandled_trace_operation_is_e0412() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "app.ply",
        r#"
import std.trace
import std.trace (trace)

test "this cannot be deterministic" {
  trace.event[orders](trace::Info, "x", map_new())
}
"#,
    );
    for args in [
        ["test", "--json"].as_slice(),
        ["test", "--host", "--json"].as_slice(),
    ] {
        let report = json(dir.path(), args);
        let codes: Vec<&str> = report["diagnostics"]
            .as_array()
            .unwrap_or_else(|| panic!("no diagnostics with {args:?}: {report:#}"))
            .iter()
            .filter_map(|d| d["code"].as_str())
            .collect();
        assert!(
            codes.contains(&codes::NONDET_IN_DET_TEST),
            "with {args:?}, expected E0412: {report:#}"
        );
    }
}

/// The substitution argument, end to end. A test that installs the twin has an
/// empty row: it is `det`, it is cached, it runs without `--host`, and its
/// second run is a cache hit.
#[test]
fn a_twin_backed_tracing_test_is_det_cached_and_hermetic() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "app.ply", SERVICE);

    let first = json(dir.path(), &["test", "--json"]);
    assert_eq!(first["exit_code"], 0, "{first:#}");
    assert_eq!(first["summary"]["passed"], 2, "{first:#}");
    assert_eq!(first["summary"]["failed"], 0, "{first:#}");
    assert_eq!(
        first["summary"]["cached"], 0,
        "a first run has nothing cached: {first:#}"
    );
    // Hermetic: no `--host` was passed and nothing refused.
    assert!(
        !output(&ply(dir.path()).args(["test"]).output().unwrap()).contains("E0424"),
        "a twin-backed test must not reach the boundary"
    );

    let second = json(dir.path(), &["test", "--json"]);
    assert_eq!(
        second["summary"]["cached"], 2,
        "a trace-collecting test is a `det` test and re-runs never: {second:#}"
    );
    assert_eq!(second["summary"]["passed"], 0, "{second:#}");
}

/// `--trace off` binds a real, listed handler rather than an empty registry, and
/// the listing names *that* handler — because "the run is discarding records" and
/// "the run is writing JSON" are different facts and a trusted computing base
/// that confused them would be lying about itself.
#[test]
fn ply_hosts_lists_the_sink_per_channel_and_names_the_one_that_serves_the_run() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "app.ply", SERVICE);

    let listing = json(dir.path(), &["hosts", "--host", "--trace", "off", "--json"]);
    let rows = listing["hosts"]
        .as_array()
        .unwrap_or_else(|| panic!("{listing:#}"));
    let traced: Vec<&Value> = rows
        .iter()
        .filter(|r| r["effect"] == "std.trace.trace")
        .collect();
    assert!(
        !traced.is_empty(),
        "the sink is a member of the trusted computing base: {listing:#}"
    );
    assert!(
        traced
            .iter()
            .all(|r| r["handler"] == "ply_host::trace::discard"),
        "`--trace off` is a listed handler, not an empty registry: {listing:#}"
    );
    // One row per channel the program actually records on, never a `*`.
    let atoms: Vec<&str> = traced.iter().filter_map(|r| r["atom"].as_str()).collect();
    assert!(
        atoms.contains(&"std.trace.trace.write[orders]")
            && atoms.contains(&"std.trace.trace.write[items]"),
        "the listing expands to the channels the program uses: {atoms:?}"
    );
    assert!(
        traced.iter().all(|r| r["deterministic"] == false
            && r["linearity"] == "at_most_once"
            && r["blocking"] == false),
        "{listing:#}"
    );
    // Twelve rows: six operations over the two channels this program records
    // on. One singleton `trace.write` would have been six.
    assert_eq!(traced.len(), 12, "{listing:#}");
}

/// One JSON object per line, on **stderr**, while `--json` owns stdout. A trace
/// line interleaved into the document would destroy it, and a consumer that has
/// to tolerate that is a consumer that cannot use `--json` at all.
#[test]
fn a_trace_line_goes_to_stderr_and_leaves_the_json_document_on_stdout_intact() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "app.ply",
        r#"
import std.trace
import std.trace (trace)

pub fn main() -> Int / {trace.write[orders]} {
  let span = trace.enter[orders]("main", map_new());
  trace.event[orders](trace::Info, "working",
    map_insert(map_new(), "sku", trace::FText("BOLT-1")));
  trace.exit[orders](span, trace::Ok);
  0
}
"#,
    );
    let out = ply(dir.path())
        .args(["run", "--host", "--trace", "json", "--json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let document: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not one document: {e}: {stdout}"));
    assert_eq!(document["exit_code"], 0, "{document:#}");

    let stderr = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<Value> = stderr
        .lines()
        .filter(|line| line.starts_with('{'))
        .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}")))
        .collect();
    assert_eq!(lines.len(), 3, "an enter, an event and an exit: {stderr}");
    assert_eq!(lines[0]["kind"], "enter");
    assert_eq!(lines[0]["channel"], "orders");
    assert_eq!(lines[1]["fields"]["sku"], "BOLT-1");
    assert_eq!(lines[2]["kind"], "exit");
    assert_eq!(lines[2]["outcome"], "ok");
    assert_eq!(lines[2]["span"], lines[0]["span"]);
}
