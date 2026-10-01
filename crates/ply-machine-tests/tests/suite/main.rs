//! The nested-entry capability, end to end: a program performs `machine.load`/`machine.bound`/
//! `machine.enter` over a project on disk, and another program has run — its ending a value the
//! caller reads.
//!
//! One binary. The engine's own unit tests, one per module of `crates/ply-machine/src`, are in
//! `tests/unit`.

mod claims;
mod fixture;
mod prover_runs;
mod replay;
mod reused;
mod selector_reads;
mod strategy;

use ply_eval::host::HostRegistry;
use ply_eval::{Front, Machine, Provider, Span, Value};
use std::sync::Arc;

/// The outer program: load the root it is handed, bind and enter `inner.main`, answer with how
/// that ended. The effect, and the record shapes crossing it, are the program's own declarations.
const OUTER: &str = r#"
import std.value (Value, VInt)

nondet effect machine {
  write configure[m](options: Options) -> Unit
  read load[m](root: String, front: Option<Front>, keep: Option<String>) -> Result<Target, Refusal>
  read reuse[m](root: String, walked: Walked) -> Option<Target>
  read reload[m](front: Front) -> Result<Target, Refusal>
  read bound[m](entry: String) -> Result<Bound, Refusal>
  write enter[m]() -> Ended
  read call[m](name: String, args: List<Value>) -> Called
  read accounting[m]() -> Accounting
  write drop[m]() -> Unit
}

type Accounting = { steps: Int, micros: Int, counters: Counters }
type Raised = { diag: Diag, values: List<Value> }
type Called = { answer: Result<Value, Raised>, warnings: List<Diag> }

type Options = { host: Bool, trace: TraceOpts }
type TraceOpts = { sink: String, level: String }

type At = { module: Int, start: Int, end: Int }
type Main = { name: String, module: String, path: String, at: At }
type Module = { name: String, path: String, at: At }
type Place = { path: String, text: Bytes }
type Walked = { key: String, modules: List<Place>, manifests: List<Place> }
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
  value: Option<Value>,
  raised: Option<Raised>,
  counters: Counters,
  cycles: List<Diag>,
  warnings: List<Diag>,
  stopping: Option<Stopping>,
  teardown: Teardown,
  trace: Option<Trace>,
  handshakes: List<String>,
  hosts: Json,
  configuration: Json,
}

fn main(root: String, front: Front) -> Ended / {machine.load[m], machine.bound[m], machine.enter[m], machine.drop[m]} = {
  match machine.load[m](root, Some(front), None) {
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

/// The program checked with the standard library it imports, and compiled.
fn built(source: &str) -> (Front, &'static ply_codegen::Unit) {
    ply_codegen::c::producer::ensure_default();
    let answered =
        ply_codegen::c::producer::checked_front_with_std(&[("m".to_string(), source.to_string())])
            .expect("the outer program checks");
    let unit =
        ply_codegen::Unit::over_front(&answered.front, answered.modules.into_iter().collect())
            .expect("this host has a C toolchain");
    (answered.front, unit)
}

/// The inner program's home: a directory with `inner.ply` in it.
fn project(inner: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a temporary directory");
    std::fs::write(dir.path().join("inner.ply"), inner).unwrap();
    dir
}

fn entered_with(inner: &str, host: bool) -> Value {
    driven(
        OUTER,
        inner,
        ply_machine::drive::RunOptions {
            host,
            ..Default::default()
        },
    )
}

/// `outer`'s answer over a root holding `inner`, its machine ops configured as `options` says.
fn driven(outer: &str, inner: &str, options: ply_machine::drive::RunOptions) -> Value {
    let project = project(inner);
    let (front, unit) = built(outer);
    let mut machine =
        Machine::new(&front, unit.attach()).expect("the unit was compiled from this program");
    let mut registry = HostRegistry::new();
    ply_machine::register_with(&mut registry, options);
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
        .into_parts()
        .0
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

fn option_value(value: &Value) -> Option<&Value> {
    match value {
        Value::Ctor { name, args } if name.as_str() == "Some" => args.first(),
        Value::Ctor { name, .. } if name.as_str() == "None" => None,
        other => panic!("an Option, not {}", other.type_name()),
    }
}

/// A nested program's `Int` answer, as `std.value` carries it.
fn vint(n: i64) -> Value {
    Value::ctor("std.value.VInt", vec![Value::Int(n)])
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
    assert_eq!(option_value(field(&answer, "value")), Some(&vint(42)));
    assert_eq!(option_int(field(&answer, "exit")), None);
    assert_eq!(option_value(field(&answer, "raised")), None);
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
    assert_eq!(option_value(field(&answer, "value")), None);
    assert_eq!(option_value(field(&answer, "raised")), None);
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
    assert_eq!(option_value(field(&answer, "value")), None);
    let raised = match field(&answer, "raised") {
        Value::Ctor { name, args } if name.as_str() == "Some" => field(&args[0], "diag"),
        other => panic!("the raise is reported, not unwound: {other:?}"),
    };
    let text = |name: &str| {
        let bytes = field(raised, name)
            .as_bytes(Span::DUMMY, name)
            .expect("a diagnostic's fields are bytes");
        String::from_utf8_lossy(bytes).into_owned()
    };
    assert_eq!(text("code"), ply_eval::codes::RUNTIME_ERROR);
    assert!(
        text("message").contains("the inner program's own bug"),
        "the inner program's panic is what was raised: {}",
        text("message")
    );
}

/// The outer program: bind `inner.main` and call it, answering how the call ended.
const OUTER_CALLS_MAIN: &str = r#"
import std.value (Value, VInt)

nondet effect machine {
  write configure[m](options: Options) -> Unit
  read load[m](root: String, front: Option<Front>, keep: Option<String>) -> Result<Target, Refusal>
  read reuse[m](root: String, walked: Walked) -> Option<Target>
  read reload[m](front: Front) -> Result<Target, Refusal>
  read bound[m](entry: String) -> Result<Bound, Refusal>
  write enter[m]() -> Ended
  read call[m](name: String, args: List<Value>) -> Called
  read accounting[m]() -> Accounting
  write drop[m]() -> Unit
}

type Accounting = { steps: Int, micros: Int, counters: Counters }
type Counters = { updates: Int, updates_in_place: Int, in_place: Option<Decimal>, cycles: Int }
type Options = Unit
type Target = Unit
type Walked = Unit
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
type Label = { module: Int, start: Int, end: Int, primary: Bool, text: Bytes }
type Edit = { module: Int, start: Int, end: Int, text: Bytes }
type Fix = { title: Bytes, edits: List<Edit> }
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
type Called = { answer: Result<Value, Raised>, warnings: List<Diag> }

fn main(root: String, front: Front) -> Called / {machine.load[m], machine.bound[m], machine.call[m], machine.drop[m]} = {
  let _loaded = machine.load[m](root, Some(front), None);
  let _bound = machine.bound[m]("inner.main");
  let called = machine.call[m]("inner.main", []);
  machine.drop[m]();
  called
}
"#;

const LEAVES_A_SPAN: &str = r#"
import std.trace
import std.trace (trace)

fn main() -> Int / {trace.write[orders]} = {
  let order = trace.enter[orders]("order", map_new());
  3
}
"#;

/// Bound to the host, whose trace facility closes what an entry leaves open, with a sink that
/// writes nothing.
fn hosted_quietly() -> ply_machine::drive::RunOptions {
    ply_machine::drive::RunOptions {
        host: true,
        trace: ply_machine::trace::TraceOptions::silent(),
        ..Default::default()
    }
}

/// The code and message of each diagnostic in a list the machine answered.
fn diags(list: &Value) -> Vec<(String, String)> {
    let Value::List(items) = list else {
        panic!("diagnostics are a list, not {}", list.type_name());
    };
    let text = |d: &Value, name: &str| {
        let bytes = field(d, name)
            .as_bytes(Span::DUMMY, name)
            .expect("a diagnostic's fields are bytes");
        String::from_utf8_lossy(bytes).into_owned()
    };
    items
        .iter()
        .map(|d| (text(d, "code"), text(d, "message")))
        .collect()
}

#[track_caller]
fn warns_of_the_open_span(warnings: &Value) {
    let warned = diags(warnings);
    assert_eq!(warned.len(), 1, "{warned:?}");
    assert_eq!(warned[0].0, ply_eval::codes::SPAN_ABANDONED, "{warned:?}");
    assert!(warned[0].1.contains("`order` on `orders`"), "{warned:?}");
}

/// What `ply run` reads once the entry is over.
#[test]
fn an_entry_that_left_a_span_open_ends_with_w0609_beside_its_value() {
    let answer = driven(OUTER, LEAVES_A_SPAN, hosted_quietly());
    assert_eq!(option_value(field(&answer, "value")), Some(&vint(3)));
    warns_of_the_open_span(field(&answer, "warnings"));
}

#[test]
fn a_call_that_left_a_span_open_answers_w0609_beside_its_value() {
    let answer = driven(OUTER_CALLS_MAIN, LEAVES_A_SPAN, hosted_quietly());
    match field(&answer, "answer") {
        Value::Ctor { name, args } if name.as_str() == "Ok" => {
            assert_eq!(args.first(), Some(&vint(3)));
        }
        other => panic!("the call answered its value, not {other:?}"),
    }
    warns_of_the_open_span(field(&answer, "warnings"));
}

#[test]
fn a_program_that_does_not_check_is_refused_with_its_diagnostics() {
    // The outer program panics with the refusal's first message; the outer machine raises it.
    let (front, unit) = built(OUTER);
    let mut machine =
        Machine::new(&front, unit.attach()).expect("the unit was compiled from this program");
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
        .into_parts()
        .0
        .expect_err("the outer main raises the refusal's message");
    assert!(raised.message.contains("unknown_name"), "{raised}");
}

/// A load, a bound entry, then a reload after the tree moved: the second answer is the new
/// program's.
const OUTER_TWICE: &str = r#"
import std.value (Value, VInt)

nondet effect machine {
  write configure[m](options: Options) -> Unit
  read load[m](root: String, front: Option<Front>, keep: Option<String>) -> Result<Target, Refusal>
  read reuse[m](root: String, walked: Walked) -> Option<Target>
  read reload[m](front: Front) -> Result<Target, Refusal>
  read bound[m](entry: String) -> Result<Bound, Refusal>
  write enter[m]() -> Ended
  read call[m](name: String, args: List<Value>) -> Called
  read accounting[m]() -> Accounting
  write drop[m]() -> Unit
}

type Accounting = { steps: Int, micros: Int, counters: Counters }
type Counters = { updates: Int, updates_in_place: Int, in_place: Option<Decimal>, cycles: Int }
type Raised = { diag: Diag, values: List<Value> }
type Called = { answer: Result<Value, Raised>, warnings: List<Diag> }

type Options = { host: Bool, trace: TraceOpts }
type TraceOpts = { sink: String, level: String }

type At = { module: Int, start: Int, end: Int }
type Main = { name: String, module: String, path: String, at: At }
type Module = { name: String, path: String, at: At }
type Place = { path: String, text: Bytes }
type Walked = { key: String, modules: List<Place>, manifests: List<Place> }
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

type Ended = { exit: Option<Int>, value: Option<Value>, raised: Option<Raised>, rest: Int }

fn once() -> Option<Value> / {machine.bound[m], machine.enter[m]} = {
  let _b = machine.bound[m]("inner.main");
  (machine.enter[m]()).value
}

fn main(root: String, front: Front) -> Option<Value> / {machine.load[m], machine.bound[m], machine.enter[m]} = {
  let _loaded = machine.load[m](root, Some(front), None);
  once()
}

fn again(front: Front) -> Option<Value> / {machine.reload[m], machine.bound[m], machine.enter[m], machine.drop[m]} = {
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

    let (front, unit) = built(OUTER_TWICE);

    let mut registry = HostRegistry::new();
    ply_machine::register_with(&mut registry, ply_machine::drive::RunOptions::default());
    let binding = Arc::new(registry.bind(&front.check).expect("the machine ops bind"));

    let call = |entry: &str, args: Vec<Value>| {
        let mut machine =
            Machine::new(&front, unit.attach()).expect("the unit was compiled from this program");
        machine.set_host_binding(Arc::clone(&binding));
        machine
            .call(entry, args, Span::DUMMY)
            .into_parts()
            .0
            .expect("the entry ran")
    };

    let root = project.path().display().to_string();
    let first = call(
        "m.main",
        vec![Value::str(root), crate::fixture::handed(project.path())],
    );
    assert_eq!(option_value(&first), Some(&vint(1)));
    assert!(
        !project.path().join(".ply-cache").exists(),
        "a machine's load reads the answer it was handed and files nothing"
    );

    std::fs::write(&inner, "fn main() -> Int = 2\n").unwrap();
    let second = call("m.again", vec![crate::fixture::handed(project.path())]);
    assert_eq!(
        option_value(&second),
        Some(&vint(2)),
        "the reload read the edited program"
    );
}

/// `configure` before `load`: the program parsed the line and the machine reads the record. A
/// configured `--host` binds the nested program's `process`, which a hermetic run refuses.
#[test]
fn a_configured_machine_binds_what_the_options_say() {
    let outer = r#"
import std.value (Value, VInt)

nondet effect machine {
  write configure[m](options: Options) -> Unit
  read load[m](root: String, front: Option<Front>, keep: Option<String>) -> Result<Target, Refusal>
  read reuse[m](root: String, walked: Walked) -> Option<Target>
  read reload[m](front: Front) -> Result<Target, Refusal>
  read bound[m](entry: String) -> Result<Bound, Refusal>
  write enter[m]() -> Ended
  read call[m](name: String, args: List<Value>) -> Called
  read accounting[m]() -> Accounting
  write drop[m]() -> Unit
}

type Accounting = { steps: Int, micros: Int, counters: Counters }
type Counters = { updates: Int, updates_in_place: Int, in_place: Option<Decimal>, cycles: Int }
type Raised = { diag: Diag, values: List<Value> }
type Called = { answer: Result<Value, Raised>, warnings: List<Diag> }

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
type Walked = { key: String, modules: List<Place>, manifests: List<Place> }
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
type Ended = { exit: Option<Int>, value: Option<Value>, raised: Option<Raised>, rest: Int }

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
  match machine.load[m](root, Some(front), None) {
    Err(_) -> false,
    Ok(_t) -> {
      let bound = machine.bound[m]("inner.main");
      match bound {
        Err(_) -> false,
        Ok(b) -> {
          let ended = machine.enter[m]();
          machine.drop[m]();
          !b.hermetic && ended.value == Some(VInt(77))
        },
      }
    },
  }
}
"#;

    let project = tempfile::tempdir().expect("a temporary directory");
    std::fs::write(project.path().join("inner.ply"), "fn main() -> Int = 77\n").unwrap();

    let (front, unit) = built(outer);
    let mut machine =
        Machine::new(&front, unit.attach()).expect("the unit was compiled from this program");
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
        .into_parts()
        .0
        .expect("the outer main ran");
    assert_eq!(answer, Value::Bool(true), "the configured host bound");
}

// --- `machine.call` ----------------------------------------------------------

/// The outer program: load the root it is handed, bind `inner.main`, and call `inner.double`
/// with one argument. The machine module here is `m`, so the values it is handed are `m.VInt`
/// and the like.
const OUTER_CALL: &str = r#"
import std.value (Value, VInt)

nondet effect machine {
  write configure[m](options: Options) -> Unit
  read load[m](root: String, front: Option<Front>, keep: Option<String>) -> Result<Target, Refusal>
  read reuse[m](root: String, walked: Walked) -> Option<Target>
  read reload[m](front: Front) -> Result<Target, Refusal>
  read bound[m](entry: String) -> Result<Bound, Refusal>
  write enter[m]() -> Ended
  read call[m](name: String, args: List<Value>) -> Called
  read accounting[m]() -> Accounting
  write drop[m]() -> Unit
}

type Accounting = { steps: Int, micros: Int, counters: Counters }
type Counters = { updates: Int, updates_in_place: Int, in_place: Option<Decimal>, cycles: Int }
type Options = Unit
type Target = Unit
type Walked = Unit
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
type Label = { module: Int, start: Int, end: Int, primary: Bool, text: Bytes }
type Edit = { module: Int, start: Int, end: Int, text: Bytes }
type Fix = { title: Bytes, edits: List<Edit> }
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
type Called = { answer: Result<Value, Raised>, warnings: List<Diag> }
type Answer = { value: Int, steps: Int, reset: Int, raised_steps: Int }

fn main(root: String, front: Front) -> Answer / {machine.load[m], machine.bound[m], machine.call[m], machine.accounting[m], machine.drop[m]} = {
  match machine.load[m](root, Some(front), None) {
    Ok(_) -> match machine.bound[m]("inner.main") {
      Ok(_) -> {
        let doubled = machine.call[m]("inner.double", [VInt(21)]);
        let first = machine.accounting[m]();
        let again = machine.accounting[m]();
        let raised = machine.call[m]("inner.boom", []);
        let after_raised = machine.accounting[m]();
        machine.drop[m]();
        match doubled.answer {
          Ok(v) -> match v {
            VInt(i) -> match raised.answer {
              Err(r) -> if bytes_index_of(r.diag.message, b"oh no") != None {
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
    let (front, unit) = built(OUTER_CALL);
    let mut machine =
        Machine::new(&front, unit.attach()).expect("the unit was compiled from this program");
    let mut registry = HostRegistry::new();
    ply_machine::register_with(
        &mut registry,
        ply_machine::drive::RunOptions {
            host: false,
            ..Default::default()
        },
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
        .into_parts()
        .0
        .expect("the outer main ran");
    let Value::Record(fields) = &answer else {
        panic!("the outer program answers a record, not {answer:?}");
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
    assert!(*steps > 0, "the call's steps were not counted: {answer:?}");
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
        "the raising call's steps were not counted: {answer:?}"
    );
}

/// The outer program: call the constant `inner.constant` twice, reading the accounting after
/// each; its twin twice under one read; then a name the inner program never defined.
const OUTER_TOTAL: &str = r#"
import std.value (Value, VInt)

nondet effect machine {
  write configure[m](options: Options) -> Unit
  read load[m](root: String, front: Option<Front>, keep: Option<String>) -> Result<Target, Refusal>
  read reuse[m](root: String, walked: Walked) -> Option<Target>
  read reload[m](front: Front) -> Result<Target, Refusal>
  read bound[m](entry: String) -> Result<Bound, Refusal>
  write enter[m]() -> Ended
  read call[m](name: String, args: List<Value>) -> Called
  read accounting[m]() -> Accounting
  write drop[m]() -> Unit
}

type Accounting = { steps: Int, micros: Int, counters: Counters }
type Counters = { updates: Int, updates_in_place: Int, in_place: Option<Decimal>, cycles: Int }
type Options = Unit
type Target = Unit
type Walked = Unit
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
type Label = { module: Int, start: Int, end: Int, primary: Bool, text: Bytes }
type Edit = { module: Int, start: Int, end: Int, text: Bytes }
type Fix = { title: Bytes, edits: List<Edit> }
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
type Called = { answer: Result<Value, Raised>, warnings: List<Diag> }
type Spent = { answers: List<Int>, ran: Int, remembered: Int, both: Int, declined: Int }

fn spent() -> Int / {machine.accounting[m]} = (machine.accounting[m]()).steps

fn answered(c: Called) -> Int =
  match c.answer { Ok(v) -> match v { VInt(i) -> i, _ -> 0 - 2 }, Err(_) -> 0 - 1 }

fn main(root: String, front: Front) -> Spent / {machine.load[m], machine.bound[m], machine.call[m], machine.accounting[m], machine.drop[m]} = {
  let _loaded = machine.load[m](root, Some(front), None);
  let _bound = machine.bound[m]("inner.main");
  let first = answered(machine.call[m]("inner.constant", []));
  let ran = spent();
  let again = answered(machine.call[m]("inner.constant", []));
  let remembered = spent();
  let twin = answered(machine.call[m]("inner.twin", []));
  let twin_again = answered(machine.call[m]("inner.twin", []));
  let both = spent();
  let absent = answered(machine.call[m]("inner.absent", []));
  let declined = spent();
  machine.drop[m]();
  {
    answers: [first, again, twin, twin_again, absent],
    ran: ran,
    remembered: remembered,
    both: both,
    declined: declined,
  }
}
"#;

const INNER_TOTAL: &str = r#"
fn main() -> Int = 0

fn deep(n: Int) -> Int = if n <= 0 { 0 } else { 1 + deep(n - 1) }

pub fn constant() -> Int = deep(50)

pub fn twin() -> Int = deep(50)
"#;

/// A driver totals each entry's own steps: a memo's answer and a declined call add none, so a
/// constant run and then answered from the memo costs one run of its body, not two.
#[test]
fn a_memo_answer_and_a_decline_add_no_steps_to_the_accounting() {
    let project = project(INNER_TOTAL);
    let (front, unit) = built(OUTER_TOTAL);
    let mut machine =
        Machine::new(&front, unit.attach()).expect("the unit was compiled from this program");
    let mut registry = HostRegistry::new();
    ply_machine::register_with(&mut registry, ply_machine::drive::RunOptions::default());
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
        .into_parts()
        .0
        .expect("the outer main ran");
    let int = |value: &Value| {
        value
            .as_int(Span::DUMMY, "a count")
            .unwrap_or_else(|d| panic!("{d}: {answer:?}"))
    };
    let Value::List(answers) = field(&answer, "answers") else {
        panic!("the answers are a list: {answer:?}");
    };
    // `inner.absent` is declined, which the machine answers as a raise.
    assert_eq!(
        answers.iter().map(int).collect::<Vec<_>>(),
        [50, 50, 50, 50, -1],
        "{answer:?}"
    );
    let ran = int(field(&answer, "ran"));
    assert!(
        ran > 0,
        "the first call's steps were not counted: {answer:?}"
    );
    assert_eq!(
        int(field(&answer, "remembered")),
        0,
        "the memo's answer added the call before it to the accounting: {answer:?}"
    );
    assert_eq!(
        int(field(&answer, "both")),
        ran,
        "a run and the memo's answer after it were totalled as two runs: {answer:?}"
    );
    assert_eq!(
        int(field(&answer, "declined")),
        0,
        "the declined call added the call before it to the accounting: {answer:?}"
    );
}
