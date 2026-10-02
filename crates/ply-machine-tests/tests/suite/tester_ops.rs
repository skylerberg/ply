//! The `tester` family as a program drives it: a unit over a project, the binding over it, each
//! test run once on the thread that asks, and what the binding came to. Which tests run, what their
//! results establish and what the cache keeps are the program's; this checks the runtime carries out
//! what it is asked and answers what it saw.

use ply_eval::host::HostRegistry;
use ply_eval::{Machine, Provider, Span, Value};
use std::sync::Arc;

/// The family as a program declares it, and the helpers both programs report with.
const DECLARED: &str = r#"
import std.json (Json)
import std.value (Value)

nondet effect tester {
  write configure[r](options: Options) -> Unit
  write unit[r](front: Front, build: Bool, hosted: Bool) -> Result<Int, List<Diag>>
  write bound[r]() -> Result<List<Diag>, List<Diag>>
  read hosted[r]() -> Hosted
  write ended[r]() -> Unit
  read executed[r](unit: Int, index: Int) -> Executed
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

"#;

/// One run over a project: its tests and the binding.
const ONE_RUN: &str = r#"
fn main(root: String, front: Front) -> String / {
  tester.configure[r], tester.unit[r], tester.bound[r], tester.hosted[r], tester.ended[r],
  tester.executed[r],
} = {
  tester.configure[r](options(root));
  match tester.unit[r](front, true, true) {
    Err(_) -> "no unit",
    Ok(u) -> match tester.bound[r]() {
      Err(_) -> "unbound",
      Ok(_) -> {
        let adds = tester.executed[r](u, 0);
        let wrong = tester.executed[r](u, 1);
        let peeks = tester.executed[r](u, 2);
        let h = tester.hosted[r]();
        tester.ended[r]();
        adds.status ++ " " ++ wrong.status ++ " " ++ peeks.status ++ " "
          ++ int_to_string(peeks.usage.performs) ++ " " ++ int_to_string(adds.usage.performs) ++ " "
          ++ h.label ++ " " ++ (if h.backend.fragment > 0 { "compiled" } else { "empty" })
      },
    },
  }
}
"#;

/// Two runs over two projects, neither ended: each one's unit and its first test.
const TWO_RUNS: &str = r#"
fn first_of(root: String, front: Front) -> String / {
  tester.configure[r], tester.unit[r], tester.executed[r],
} = {
  tester.configure[r](options(root));
  match tester.unit[r](front, true, false) {
    Err(_) -> "no unit",
    Ok(u) -> int_to_string(u) ++ " " ++ tester.executed[r](u, 0).status,
  }
}

fn main(root: String, front: Front, other: String, other_front: Front) -> String / {
  tester.configure[r], tester.unit[r], tester.executed[r],
} = first_of(root, front) ++ " | " ++ first_of(other, other_front)
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

/// A machine over `DECLARED` and `main`, bound to the tester operations the program declares.
fn driving(main: &str) -> Machine<'static> {
    ply_codegen::c::producer::ensure_default();
    let answered = ply_codegen::c::producer::checked_front_with_std(&[(
        "m".to_string(),
        format!("{DECLARED}{main}"),
    )])
    .expect("the driving program checks");
    let unit =
        ply_codegen::Unit::over_front(&answered.front, answered.modules.into_iter().collect())
            .expect("this host has a C toolchain");
    let front: &'static ply_eval::Front = Box::leak(Box::new(answered.front));
    let mut machine = Machine::new(front, unit.attach()).expect("the unit is this program's");
    let mut registry = HostRegistry::new();
    // Only what this program declares: a family's operation the program does not declare is a
    // registration the binding refuses.
    for (op, handler) in ply_machine::tester::Session::new().lent() {
        if op.op.as_str() != "interleaved" {
            registry.register(op, handler);
        }
    }
    let binding = registry.bind(&front.check).expect("the tester ops bind");
    machine.set_host_binding(Arc::new(binding));
    machine
}

fn root_and_front(project: &tempfile::TempDir) -> [Value; 2] {
    [
        Value::str(project.path().display().to_string()),
        crate::fixture::handed(project.path()),
    ]
}

fn answer_of(machine: &mut Machine<'_>, args: Vec<Value>) -> Value {
    machine
        .call("m.main", args, Span::DUMMY)
        .into_parts()
        .0
        .expect("the driving program ran")
}

#[test]
fn a_program_runs_its_tests_through_the_family_and_files_what_it_decided() {
    let project = crate::fixture::project(PROJECT);
    let answer = answer_of(&mut driving(ONE_RUN), root_and_front(&project).to_vec());
    // The runtime runs exactly what it is asked and says how each ended; a handled operation is
    // performed all the same, and a test's count starts over with it.
    assert_eq!(
        answer,
        Value::str("passed failed passed 3 0 hermetic compiled"),
        "the program saw: {answer:?}"
    );
}

/// A configuration begins a run whether or not the last one ended: the last run's units and binding
/// go.
#[test]
fn a_configuration_begins_a_run_over_the_project_it_names() {
    let first = crate::fixture::project("test \"holds\" { assert(true) }\n");
    let second = crate::fixture::project("test \"breaks\" { assert(false) }\n");
    let mut args = root_and_front(&first).to_vec();
    args.extend(root_and_front(&second));
    let answer = answer_of(&mut driving(TWO_RUNS), args);
    assert_eq!(
        answer,
        Value::str("0 passed | 0 failed"),
        "the program saw: {answer:?}"
    );
}
