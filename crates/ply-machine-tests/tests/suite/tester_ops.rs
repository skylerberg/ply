//! The `tester` family as a program drives it: a unit over a project, the binding over it, each
//! test run once on the thread that asks, what the binding came to, and the result cache read and
//! written under keys the program names. Which tests run and what their results establish are the
//! program's; this checks the runtime carries out what it is asked and answers what it saw.

use ply_eval::host::HostRegistry;
use ply_eval::{Machine, Provider, Span, Value};
use std::sync::Arc;

const OUTER: &str = r#"
import std.json (Json)
import std.value (Value)

nondet effect tester {
  write configure[r](options: Options) -> Unit
  write unit[r](front: Front, build: Bool, hosted: Bool) -> Result<Int, List<Diag>>
  write bound[r]() -> Result<List<Diag>, List<Diag>>
  read hosted[r]() -> Hosted
  write ended[r]() -> Unit
  read executed[r](unit: Int, index: Int) -> Executed
  read opened[r]() -> List<Diag>
  read outcomes[r](keys: List<String>) -> List<Option<String>>
  read seen[r](hashes: List<String>) -> List<Bool>
  read baselines[r](keys: List<String>) -> List<Option<PassRecord>>
  write filed[r](filing: Filing) -> Filed
}

type Label = { module: Int, start: Int, end: Int, primary: Bool, text: Bytes }
type Fix = { title: Bytes, edits: List<Edit> }
type Edit = { module: Int, start: Int, end: Int, text: Bytes }
type Diag = {
  code: Bytes,
  notes: Int,
  labels: List<Label>,
  text: Bytes,
  message: Bytes,
  notes_text: List<Bytes>,
  severity: Bytes,
  fixes: List<Fix>,
}
type Raised = { diag: Diag, values: List<Value> }
type Use = {
  duration_us: Int,
  host: Bool,
  entries: Int,
  declines: Int,
  performs: Int,
  teardown: List<Diag>,
}
type Executed = { status: String, failure: Option<Raised>, usage: Use }
type Compiled = {
  name: String,
  fragment: Int,
  offered: Int,
  converted_in: Int,
  converted_out: Int,
  units: Option<Int>,
  analysis_nanos: Option<Int>,
  codegen_nanos: Option<Int>,
  unbuilt: Int,
}
type Hosted = {
  hermetic: Bool,
  label: String,
  operations: Int,
  digest: String,
  handshakes: List<String>,
  hosts: Json,
  reaches: List<Int>,
  cores: Int,
  backend: Compiled,
}
type Named = { name: String, hash: String }
type PassRecord = { test: String, closure: List<Named>, decls: List<Named> }
type Filing = {
  passes: List<String>,
  records: List<{ key: String, record: PassRecord }>,
  seen: List<String>,
}
type Filed = { unflushed: Option<String>, warnings: List<Diag> }
type Front = {
  dump: Bytes,
  files: List<{ path: String, name: String, text: Bytes }>,
  read_ms: Int,
  front_ms: Int,
  file_ms: Int,
  cached: Bool,
}
type Paths = { name: String, path: String }
type Cred = { name: String, cert: String, key: String }
type ConfigOpts = { set: List<String>, files: List<String>, schema: Option<String> }
type Options = {
  path: String,
  steps: Int,
  timeout: Int,
  profile: String,
  host: Bool,
  tls: List<Cred>,
  trust: List<String>,
  fs: List<Paths>,
  exec: List<Paths>,
  allow: List<String>,
  config: ConfigOpts,
}

fn options(root: String) -> Options =
  {
    path: root,
    steps: 1000000000,
    timeout: 60000,
    profile: "development",
    host: false,
    tls: [],
    trust: [],
    fs: [],
    exec: [],
    allow: [],
    config: { set: [], files: [], schema: None },
  }

fn key(digit: String) -> String = fold(range(0, 64), "", |acc: String, i: Int| acc ++ digit)

fn said(xs: List<Option<String>>) -> String =
  fold(xs, "", |acc: String, x: Option<String>|
    acc ++ (if acc == "" { "" } else { "," }) ++ (match x { Some(w) -> w, None -> "none" }))

fn flags(xs: List<Bool>) -> String =
  fold(xs, "", |acc: String, x: Bool|
    acc ++ (if acc == "" { "" } else { "," }) ++ (if x { "yes" } else { "no" }))

fn held(xs: List<Option<PassRecord>>) -> String =
  fold(xs, "", |acc: String, x: Option<PassRecord>|
    acc ++ (if acc == "" { "" } else { "," }) ++ (match x { Some(r) -> r.test, None -> "none" }))

fn main(root: String, front: Front) -> String / {
  tester.configure[r], tester.unit[r], tester.bound[r], tester.hosted[r], tester.ended[r],
  tester.executed[r], tester.opened[r], tester.outcomes[r], tester.seen[r], tester.baselines[r],
  tester.filed[r],
} = {
  tester.configure[r](options(root));
  let opened = tester.opened[r]();
  match tester.unit[r](front, true, true) {
    Err(_) -> "no unit",
    Ok(u) -> match tester.bound[r]() {
      Err(_) -> "unbound",
      Ok(_) -> {
        let adds = tester.executed[r](u, 0);
        let wrong = tester.executed[r](u, 1);
        let peeks = tester.executed[r](u, 2);
        let h = tester.hosted[r]();
        let filed = tester.filed[r]({
          passes: [key("a")],
          records: [
            {
              key: "m.adds",
              record: { test: key("a"), closure: [{ name: "m.add", hash: key("b") }], decls: [] },
            },
          ],
          seen: [key("b")],
        });
        let back = said(tester.outcomes[r]([key("a"), key("c")]));
        let records = held(tester.baselines[r](["m.adds", "m.wrong"]));
        let known = flags(tester.seen[r]([key("b"), key("c")]));
        tester.ended[r]();
        adds.status ++ " " ++ wrong.status ++ " " ++ peeks.status ++ " "
          ++ int_to_string(peeks.usage.performs) ++ " " ++ int_to_string(adds.usage.performs) ++ " "
          ++ h.label ++ " " ++ (if h.backend.fragment > 0 { "compiled" } else { "empty" }) ++ " "
          ++ (match filed.unflushed { Some(_) -> "unflushed", None -> "flushed" }) ++ " "
          ++ back ++ " " ++ records ++ " " ++ known ++ " "
          ++ int_to_string(len(opened))
      },
    },
  }
}
"#;

const PROJECT: &str = r#"
effect disk {
  read peek[r](key: Int) -> Int
}

fn add(a: Int, b: Int) -> Int = a + b

test "adds" { assert_eq(add(1, 2), 3) }

test "wrong" { assert_eq(add(1, 2), 4) }

test "peeks" {
  let n = handle {
    disk.peek[log](1) + disk.peek[log](2) + disk.peek[log](3)
  } with {
    disk.peek[log](k) -> k,
  };
  assert_eq(n, 6)
}
"#;

#[test]
fn a_program_runs_its_tests_through_the_family_and_files_what_it_decided() {
    let project = crate::fixture::project(PROJECT);
    ply_codegen::c::producer::ensure_default();
    let answered =
        ply_codegen::c::producer::checked_front_with_std(&[("m".to_string(), OUTER.to_string())])
            .expect("the driving program checks");
    let unit =
        ply_codegen::Unit::over_front(&answered.front, answered.modules.into_iter().collect())
            .expect("this host has a C toolchain");
    let front = answered.front;
    let mut machine = Machine::new(&front, unit.attach()).expect("the unit is this program's");
    let mut registry = HostRegistry::new();
    for (op, handler) in ply_machine::tester::Session::new().lent() {
        registry.register(op, handler);
    }
    let binding = registry.bind(&front.check).expect("the tester ops bind");
    machine.set_host_binding(Arc::new(binding));
    let answer = machine
        .call(
            "m.main",
            vec![
                Value::str(project.path().display().to_string()),
                crate::fixture::handed(project.path()),
            ],
            Span::DUMMY,
        )
        .into_parts()
        .0
        .expect("the driving program ran");
    // The runtime runs exactly what it is asked and says how each ended; a handled operation is
    // performed all the same, and a test's count starts over with it. What was filed reads back
    // under the keys the program named, and nothing else does.
    let expected = format!(
        "passed failed passed 3 0 hermetic compiled flushed passed,none {},none yes,no 0",
        "a".repeat(64)
    );
    assert_eq!(answer, Value::str(expected), "the program saw: {answer:?}");
}
