//! The nested-entry capability, end to end: a program performs `machine.load`/`machine.bound`/
//! `machine.enter` over a project on disk, and another program has run — its ending a value the
//! caller reads.
//!
//! One binary. The engine's own unit tests, one per module of `crates/ply-machine/src`, are in
//! `tests/unit`.

mod claims;
mod fixture;
mod front_read;
mod prover_soundness_audit;
mod replay;
mod selector_reads;
mod tiers;

use ply_eval::host::HostRegistry;
use ply_eval::{Machine, Provider, Value};
use ply_span::{SourceId, Span};
use ply_ty::Front;
use std::collections::HashMap;
use std::sync::Arc;

/// The outer program: load the root it is handed, bind and enter `inner.main`, answer with how
/// that ended. The effect, and the record shapes crossing it, are the program's own declarations.
const OUTER: &str = r#"
nondet effect machine {
  write configure[m](options: Options) -> Unit
  read load[m](root: String, front: Option<Front>) -> Result<Target, Refusal>
  read reload[m](front: Front) -> Result<Target, Refusal>
  read bound[m](entry: String) -> Result<Bound, Refusal>
  write enter[m]() -> Ended
  read call[m](name: String, args: List<Value>) -> Result<Value, Raised>
  read accounting[m]() -> Accounting
  write drop[m]() -> Unit
}

type Accounting = { steps: Int, micros: Int, counters: Counters }
type Raised = { code: String, message: String }
type Value = | VUnit | VBool(Bool) | VInt(Int) | VStr(String) | VList(List<Value>)

type Options = { host: Bool, trace: TraceOpts }
type TraceOpts = { sink: String, level: String }

type At = { module: Int, start: Int, end: Int }
type Main = { name: String, module: String, path: String, at: At }
type Module = { name: String, path: String, at: At }
type Place = { path: String, text: Bytes }
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

type Target = | Project(Program) | Deployed(Artifact)
type Program = {
  root: String,
  files: List<String>,
  places: List<Place>,
  mains: List<Main>,
  modules: List<Module>,
}
type Artifact = {
  path: String,
  digest: String,
  entry: String,
  definitions: Int,
  unit: Bool,
  warnings: List<Diag>,
}

type Signals = { names: List<String>, lead_ms: Int, drain_ms: Int }
type Bound = {
  hermetic: Bool,
  operations: Int,
  digest: String,
  config: Option<String>,
  trace: Option<String>,
  database: Option<String>,
  signals: Option<Signals>,
  warnings: List<Diag>,
}

type Front = {
  dump: Bytes,
  files: List<{ path: String, name: String, text: Bytes }>,
  read_ms: Int,
  front_ms: Int,
  file_ms: Int,
  cached: Bool,
}
type Refusal = { diags: List<Diag>, places: List<Place>, artifact: Option<String> }

type Counters = { updates: Int, updates_in_place: Int, in_place: Option<Decimal>, cycles: Int }
type Stopping = {
  signal: Option<String>,
  listeners: Int,
  connections: Int,
  scopes: Int,
  elapsed_ms: Int,
}
type Teardown = {
  lead_ms: Int,
  drain_ms: Int,
  transactions_rolled_back: Int,
  connections_closed: Int,
  spans_abandoned: Int,
  problems: List<String>,
}
type Trace = { events: Int, spans: Int, abandoned: Int, flushed: Bool }
type Json = | Null

type Ended = {
  exit: Option<Int>,
  value: Option<String>,
  raised: Option<Diag>,
  counters: Counters,
  cycles: List<Diag>,
  stopping: Option<Stopping>,
  teardown: Teardown,
  trace: Option<Trace>,
  handshakes: List<String>,
  hosts: Json,
  configuration: Json,
}

fn main(root: String, front: Front) -> Ended / {machine.load[m], machine.bound[m], machine.enter[m], machine.drop[m]} = {
  match machine.load[m](root, Some(front)) {
    Ok(_t) -> {
      match machine.bound[m]("inner.main") {
        Ok(_b) -> {
          let ended = machine.enter[m]();
          machine.drop[m]();
          ended
        },
        Err(why) -> {
          machine.drop[m]();
          match list_at(why.diags, 0) {
            Some(first) -> panic(string_of_bytes(first.message)),
            None -> panic("the inner program binds, without a diagnostic"),
          }
        },
      }
    },
    Err(refusal) -> {
      machine.drop[m]();
      match list_at(refusal.diags, 0) {
        Some(first) -> panic(string_of_bytes(first.message)),
        None -> panic("refused without a diagnostic"),
      }
    },
  }
}
"#;

fn front_of(source: &str) -> Front {
    let named = vec![("m".to_string(), source.to_string())];
    let ids = vec![SourceId(0)];
    ply_codegen::c::producer::ensure_default();
    ply_codegen::c::producer::checked_front(&named, &ids).expect("the outer program checks")
}

/// The inner program's home: a directory with `inner.ply` in it.
fn project(inner: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a temporary directory");
    std::fs::write(dir.path().join("inner.ply"), inner).unwrap();
    dir
}

fn entered_with(inner: &str, host: bool) -> Value {
    let project = project(inner);
    let front = front_of(OUTER);
    let texts: HashMap<String, String> =
        [("m".to_string(), OUTER.to_string())].into_iter().collect();
    let unit = ply_codegen::Unit::over_front(&front, texts).expect("this host has a C toolchain");
    let mut machine = Machine::new(&front);
    machine.set_compiled(unit.attach());
    let mut registry = HostRegistry::new();
    ply_machine::register_with(
        &mut registry,
        ply_machine::drive::RunOptions {
            host,
            ..Default::default()
        },
    );
    let binding = registry.bind(&front.check).expect("the machine ops bind");
    machine.set_host_binding(Arc::new(binding));
    machine
        .call(
            "m.main",
            vec![
                Value::str(project.path().display().to_string()),
                crate::fixture::handed(project.path()),
            ],
            Span::DUMMY,
        )
        .expect("the outer main ran")
}

fn entered(inner: &str) -> Value {
    entered_with(inner, false)
}

fn field<'a>(value: &'a Value, name: &str) -> &'a Value {
    let Value::Record(fields) = value else {
        panic!("the answer is a record, not {}", value.type_name());
    };
    fields
        .iter()
        .find(|(key, _)| key.as_str() == name)
        .map(|(_, value)| value)
        .unwrap_or_else(|| panic!("the answer holds `{name}`"))
}

fn option_text(value: &Value) -> Option<String> {
    match value {
        Value::Ctor { name, args } if name.as_str() == "Some" => args
            .first()
            .map(|v| v.as_str(Span::DUMMY, "text").unwrap().to_string()),
        Value::Ctor { name, .. } if name.as_str() == "None" => None,
        other => panic!("an Option, not {}", other.type_name()),
    }
}

fn option_int(value: &Value) -> Option<i64> {
    match value {
        Value::Ctor { name, args } if name.as_str() == "Some" => args
            .first()
            .map(|v| v.as_int(Span::DUMMY, "a code").unwrap()),
        Value::Ctor { name, .. } if name.as_str() == "None" => None,
        other => panic!("an Option, not {}", other.type_name()),
    }
}

const INNER: &str = r#"
fn main() -> Int = 40 + 2
"#;

#[test]
fn a_program_loads_binds_and_enters_a_program() {
    let answer = entered(INNER);
    assert_eq!(option_text(field(&answer, "value")).as_deref(), Some("42"));
    assert_eq!(option_int(field(&answer, "exit")), None);
    assert_eq!(option_text(field(&answer, "raised")), None);
}

#[test]
fn a_nested_program_may_choose_the_exit_code() {
    let answer = entered_with(
        r#"
import std.process (process)

fn main() -> Unit / {process.exit[proc]} = process.exit[proc](7)
"#,
        true,
    );
    assert_eq!(option_int(field(&answer, "exit")), Some(7));
    assert_eq!(option_text(field(&answer, "value")), None);
    assert_eq!(option_text(field(&answer, "raised")), None);
}

#[test]
fn a_nested_raise_is_a_value_not_an_unwind() {
    let answer = entered(
        r#"
fn main() -> Int = panic("the inner program's own bug")
"#,
    );
    // A raise is a diagnostic value; the outer program does not read inside it.
    assert!(option_int(field(&answer, "exit")).is_none());
    assert_eq!(option_text(field(&answer, "value")), None);
    let raised = field(&answer, "raised");
    assert!(
        matches!(raised, Value::Ctor { name, .. } if name.as_str() == "Some"),
        "the raise is reported, not unwound"
    );
}

#[test]
fn a_program_that_does_not_check_is_refused_with_its_diagnostics() {
    // The outer program panics with the refusal's first message; the outer machine raises it.
    let front = front_of(OUTER);
    let texts: HashMap<String, String> =
        [("m".to_string(), OUTER.to_string())].into_iter().collect();
    let unit = ply_codegen::Unit::over_front(&front, texts).expect("this host has a C toolchain");
    let mut machine = Machine::new(&front);
    machine.set_compiled(unit.attach());
    let mut registry = HostRegistry::new();
    ply_machine::register(&mut registry);
    let binding = registry.bind(&front.check).expect("the machine ops bind");
    machine.set_host_binding(Arc::new(binding));
    let project = project("fn main() -> Int = unknown_name\n");
    let raised = machine
        .call(
            "m.main",
            vec![
                Value::str(project.path().display().to_string()),
                crate::fixture::handed(project.path()),
            ],
            Span::DUMMY,
        )
        .expect_err("the outer main raises the refusal's message");
    assert!(raised.message.contains("unknown_name"), "{raised}");
}

/// A load, a bound entry, then a reload after the tree moved: the second answer is the new
/// program's.
const OUTER_TWICE: &str = r#"
nondet effect machine {
  write configure[m](options: Options) -> Unit
  read load[m](root: String, front: Option<Front>) -> Result<Target, Refusal>
  read reload[m](front: Front) -> Result<Target, Refusal>
  read bound[m](entry: String) -> Result<Bound, Refusal>
  write enter[m]() -> Ended
  read call[m](name: String, args: List<Value>) -> Result<Value, Raised>
  read accounting[m]() -> Accounting
  write drop[m]() -> Unit
}

type Accounting = { steps: Int, micros: Int, counters: Counters }
type Raised = { code: String, message: String }
type Value = | VUnit | VBool(Bool) | VInt(Int) | VStr(String) | VList(List<Value>)

type Options = { host: Bool, trace: TraceOpts }
type TraceOpts = { sink: String, level: String }

type At = { module: Int, start: Int, end: Int }
type Main = { name: String, module: String, path: String, at: At }
type Module = { name: String, path: String, at: At }
type Place = { path: String, text: Bytes }
type Label = { module: Int, start: Int, end: Int, primary: Bool, text: Bytes }
type Diag = {
  code: Bytes,
  notes: Int,
  labels: List<Label>,
  text: Bytes,
  message: Bytes,
  notes_text: List<Bytes>,
  severity: Bytes,
  fixes: Int,
}

type Target = | Project(Program) | Deployed(Artifact)
type Program = {
  root: String,
  files: List<String>,
  places: List<Place>,
  mains: List<Main>,
  modules: List<Module>,
}
type Artifact = {
  path: String,
  digest: String,
  entry: String,
  definitions: Int,
  unit: Bool,
  warnings: List<Diag>,
}

type Signals = { names: List<String>, lead_ms: Int, drain_ms: Int }
type Bound = {
  hermetic: Bool,
  operations: Int,
  digest: String,
  config: Option<String>,
  trace: Option<String>,
  database: Option<String>,
  signals: Option<Signals>,
  warnings: List<Diag>,
}

type Front = {
  dump: Bytes,
  files: List<{ path: String, name: String, text: Bytes }>,
  read_ms: Int,
  front_ms: Int,
  file_ms: Int,
  cached: Bool,
}
type Refusal = { diags: List<Diag>, places: List<Place>, artifact: Option<String> }

type Ended = { exit: Option<Int>, value: Option<String>, raised: Option<Diag>, rest: Int }

fn once() -> Option<String> / {machine.bound[m], machine.enter[m]} = {
  let _b = machine.bound[m]("inner.main");
  (machine.enter[m]()).value
}

fn main(root: String, front: Front) -> Option<String> / {machine.load[m], machine.bound[m], machine.enter[m]} = {
  let _loaded = machine.load[m](root, Some(front));
  once()
}

fn again(front: Front) -> Option<String> / {machine.reload[m], machine.bound[m], machine.enter[m], machine.drop[m]} = {
  let _again = machine.reload[m](front);
  let value = once();
  machine.drop[m]();
  value
}
"#;

#[test]
fn a_reload_after_an_edit_enters_the_new_program() {
    let project = tempfile::tempdir().expect("a temporary directory");
    let inner = project.path().join("inner.ply");
    std::fs::write(&inner, "fn main() -> Int = 1\n").unwrap();

    let front = front_of(OUTER_TWICE);
    let texts: HashMap<String, String> = [("m".to_string(), OUTER_TWICE.to_string())]
        .into_iter()
        .collect();
    let unit = ply_codegen::Unit::over_front(&front, texts).expect("this host has a C toolchain");

    let mut registry = HostRegistry::new();
    ply_machine::register_with(&mut registry, ply_machine::drive::RunOptions::default());
    let binding = Arc::new(registry.bind(&front.check).expect("the machine ops bind"));

    let call = |entry: &str, args: Vec<Value>| {
        let mut machine = Machine::new(&front);
        machine.set_compiled(unit.attach());
        machine.set_host_binding(Arc::clone(&binding));
        machine
            .call(entry, args, Span::DUMMY)
            .expect("the entry ran")
    };

    let root = project.path().display().to_string();
    let first = call(
        "m.main",
        vec![Value::str(root), crate::fixture::handed(project.path())],
    );
    assert_eq!(option_text(&first).as_deref(), Some("1"));
    assert!(
        !project.path().join(".ply-cache").exists(),
        "a machine's load reads the answer it was handed and files nothing"
    );

    std::fs::write(&inner, "fn main() -> Int = 2\n").unwrap();
    let second = call("m.again", vec![crate::fixture::handed(project.path())]);
    assert_eq!(
        option_text(&second).as_deref(),
        Some("2"),
        "the reload read the edited program"
    );
}

/// `configure` before `load`: the program parsed the line and the machine reads the record. A
/// configured `--host` binds the nested program's `process`, which a hermetic run refuses.
#[test]
fn a_configured_machine_binds_what_the_options_say() {
    let outer = r#"
nondet effect machine {
  write configure[m](options: Options) -> Unit
  read load[m](root: String, front: Option<Front>) -> Result<Target, Refusal>
  read reload[m](front: Front) -> Result<Target, Refusal>
  read bound[m](entry: String) -> Result<Bound, Refusal>
  write enter[m]() -> Ended
  read call[m](name: String, args: List<Value>) -> Result<Value, Raised>
  read accounting[m]() -> Accounting
  write drop[m]() -> Unit
}

type Accounting = { steps: Int, micros: Int, counters: Counters }
type Counters = { updates: Int, updates_in_place: Int, in_place: Option<Decimal>, cycles: Int }
type Raised = { code: String, message: String }
type Value = | VUnit | VBool(Bool) | VInt(Int) | VStr(String) | VList(List<Value>)

type TlsCred = { name: String, cert: String, key: String }
type Named = { name: String, path: String }
type DbOpts = {
  url: Option<String>,
  pool: Option<Int>,
  acquire_ms: Option<Int>,
  statement_ms: Option<Int>,
  idle_txn_ms: Option<Int>,
  connect_ms: Option<Int>,
  statement_cache: Option<Int>,
  schema: Option<String>,
}
type ConfigOpts = { set: List<String>, files: List<String>, schema: Option<String> }
type TraceOpts = { sink: String, level: String }

type Options = {
  host: Bool,
  json: Bool,
  tls: List<TlsCred>,
  trust: List<String>,
  fs: List<Named>,
  exec: List<Named>,
  allow: List<String>,
  db: DbOpts,
  config: ConfigOpts,
  trace: TraceOpts,
  drain_ms: Int,
  drain_lead_ms: Int,
  steps: Int,
  timeout: Int,
  seed: Option<String>,
  backend: Option<String>,
  profile: String,
  argv: List<String>,
}

type At = { module: Int, start: Int, end: Int }
type Main = { name: String, module: String, path: String, at: At }
type Module = { name: String, path: String, at: At }
type Place = { path: String, text: Bytes }
type Label = { module: Int, start: Int, end: Int, primary: Bool, text: Bytes }
type Diag = {
  code: Bytes,
  notes: Int,
  labels: List<Label>,
  text: Bytes,
  message: Bytes,
  notes_text: List<Bytes>,
  severity: Bytes,
  fixes: Int,
}
type Target = | Project(Program) | Deployed(Artifact)
type Program = {
  root: String,
  files: List<String>,
  places: List<Place>,
  mains: List<Main>,
  modules: List<Module>,
}
type Artifact = {
  path: String,
  digest: String,
  entry: String,
  definitions: Int,
  unit: Bool,
  warnings: List<Diag>,
}
type Signals = { names: List<String>, lead_ms: Int, drain_ms: Int }
type Bound = {
  hermetic: Bool,
  operations: Int,
  digest: String,
  config: Option<String>,
  trace: Option<String>,
  database: Option<String>,
  signals: Option<Signals>,
  warnings: List<Diag>,
}
type Front = {
  dump: Bytes,
  files: List<{ path: String, name: String, text: Bytes }>,
  read_ms: Int,
  front_ms: Int,
  file_ms: Int,
  cached: Bool,
}
type Refusal = { diags: List<Diag>, places: List<Place>, artifact: Option<String> }
type Ended = { exit: Option<Int>, value: Option<String>, raised: Option<Diag>, rest: Int }

fn opts(host: Bool) -> Options =
  {
    host: host,
    json: false,
    tls: [],
    trust: [],
    fs: [],
    exec: [],
    allow: [],
    db: {
      url: None,
      pool: None,
      acquire_ms: None,
      statement_ms: None,
      idle_txn_ms: None,
      connect_ms: None,
      statement_cache: None,
      schema: None,
    },
    config: { set: [], files: [], schema: None },
    trace: { sink: "json", level: "info" },
    drain_ms: 30000,
    drain_lead_ms: 0,
    steps: 0,
    timeout: 0,
    seed: None,
    backend: None,
    profile: "development",
    argv: [],
  }

fn main(root: String, front: Front) -> Bool / {machine.configure[m], machine.load[m], machine.bound[m], machine.enter[m], machine.drop[m]} = {
  machine.configure[m](opts(true));
  match machine.load[m](root, Some(front)) {
    Err(_) -> false,
    Ok(_t) -> {
      let bound = machine.bound[m]("inner.main");
      match bound {
        Err(_) -> false,
        Ok(b) -> {
          let ended = machine.enter[m]();
          machine.drop[m]();
          !b.hermetic && ended.value == Some("77")
        },
      }
    },
  }
}
"#;

    let project = tempfile::tempdir().expect("a temporary directory");
    std::fs::write(project.path().join("inner.ply"), "fn main() -> Int = 77\n").unwrap();

    let front = front_of(outer);
    let texts: HashMap<String, String> =
        [("m".to_string(), outer.to_string())].into_iter().collect();
    let unit = ply_codegen::Unit::over_front(&front, texts).expect("this host has a C toolchain");
    let mut machine = Machine::new(&front);
    machine.set_compiled(unit.attach());
    let mut registry = HostRegistry::new();
    ply_machine::register(&mut registry);
    let binding = registry.bind(&front.check).expect("the machine ops bind");
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
        .expect("the outer main ran");
    assert_eq!(answer.to_string(), "true", "the configured host bound");
}

// --- `machine.call` ----------------------------------------------------------

/// The outer program: load the root it is handed, bind `inner.main`, and call `inner.double`
/// with one argument. The machine module here is `m`, so the values it is handed are `m.VInt`
/// and the like.
const OUTER_CALL: &str = r#"
nondet effect machine {
  write configure[m](options: Options) -> Unit
  read load[m](root: String, front: Option<Front>) -> Result<Target, Refusal>
  read reload[m](front: Front) -> Result<Target, Refusal>
  read bound[m](entry: String) -> Result<Bound, Refusal>
  write enter[m]() -> Ended
  read call[m](name: String, args: List<Value>) -> Result<Value, Raised>
  read accounting[m]() -> Accounting
  write drop[m]() -> Unit
}

type Accounting = { steps: Int, micros: Int, counters: Counters }
type Counters = { updates: Int, updates_in_place: Int, in_place: Option<Decimal>, cycles: Int }
type Options = Unit
type Target = Unit
type Bound = Unit
type Front = {
  dump: Bytes,
  files: List<{ path: String, name: String, text: Bytes }>,
  read_ms: Int,
  front_ms: Int,
  file_ms: Int,
  cached: Bool,
}
type Refusal = Unit
type Ended = Unit
type Raised = { code: String, message: String }
type Value = | VUnit | VBool(Bool) | VInt(Int) | VStr(String) | VList(List<Value>)
type Answer = { value: Int, steps: Int, reset: Int, raised_steps: Int }

fn main(root: String, front: Front) -> Answer / {machine.load[m], machine.bound[m], machine.call[m], machine.accounting[m], machine.drop[m]} = {
  match machine.load[m](root, Some(front)) {
    Ok(_) -> match machine.bound[m]("inner.main") {
      Ok(_) -> {
        let doubled = machine.call[m]("inner.double", [VInt(21)]);
        let first = machine.accounting[m]();
        let again = machine.accounting[m]();
        let raised = machine.call[m]("inner.boom", []);
        let after_raised = machine.accounting[m]();
        machine.drop[m]();
        match doubled {
          Ok(v) -> match v {
            VInt(i) -> match raised {
              Err(r) -> if string_contains(r.message, "oh no") {
                { value: i, steps: first.steps, reset: again.steps, raised_steps: after_raised.steps }
              } else { { value: 0 - 4, steps: 0, reset: 0, raised_steps: 0 } },
              Ok(_) -> { value: 0 - 3, steps: 0, reset: 0, raised_steps: 0 },
            },
            _ -> { value: 0 - 2, steps: 0, reset: 0, raised_steps: 0 },
          },
          Err(_) -> { value: 0 - 1, steps: 0, reset: 0, raised_steps: 0 },
        }
      },
      Err(_) -> { value: 0 - 5, steps: 0, reset: 0, raised_steps: 0 },
    },
    Err(_) -> { value: 0 - 6, steps: 0, reset: 0, raised_steps: 0 },
  }
}
"#;

const INNER_CALL: &str = r#"
fn main() -> Int = 0

pub fn double(x: Int) -> Int = x * 2

pub fn boom() -> Int = panic("oh no")
"#;

#[test]
fn a_call_enters_a_definition_with_arguments_and_answers_its_value() {
    let project = project(INNER_CALL);
    let front = front_of(OUTER_CALL);
    let texts: HashMap<String, String> = [("m".to_string(), OUTER_CALL.to_string())]
        .into_iter()
        .collect();
    let unit = ply_codegen::Unit::over_front(&front, texts).expect("this host has a C toolchain");
    let mut machine = Machine::new(&front);
    machine.set_compiled(unit.attach());
    let mut registry = HostRegistry::new();
    ply_machine::register_with_for(
        &mut registry,
        ply_machine::drive::RunOptions {
            host: false,
            ..Default::default()
        },
        "m",
    );
    let binding = registry.bind(&front.check).expect("the machine ops bind");
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
        .expect("the outer main ran");
    let Value::Record(fields) = &answer else {
        panic!("the outer program answers a record, not {answer}");
    };
    let field = |name: &str| {
        fields
            .iter()
            .find(|(key, _)| key.as_str() == name)
            .map(|(_, value)| value)
            .unwrap_or_else(|| panic!("the answer carries `{name}`"))
    };
    assert_eq!(field("value"), &Value::Int(42));
    // The runtime counted the calls the definition made, and the read reset the count.
    let Value::Int(steps) = field("steps") else {
        panic!("steps is an int");
    };
    assert!(*steps > 0, "the call's steps were not counted: {answer}");
    assert_eq!(
        field("reset"),
        &Value::Int(0),
        "reading the accounting did not reset it"
    );
    // A call that raised did work too, and that work is counted.
    let Value::Int(raised_steps) = field("raised_steps") else {
        panic!("raised_steps is an int");
    };
    assert!(
        *raised_steps > 0,
        "the raising call's steps were not counted: {answer}"
    );
}
