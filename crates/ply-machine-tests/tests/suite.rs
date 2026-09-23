//! The nested-entry capability, end to end: a program performs `machine.load`/`machine.enter`
//! over a project on disk, and another program has run — its output captured, its ending a value
//! the caller reads, and the load incremental over the project's own store.

use ply_eval::host::HostRegistry;
use ply_eval::{BackendKind, BackendSpec, Machine, Provider, Value};
use ply_span::{SourceId, Span};
use ply_ty::Front;
use std::collections::HashMap;
use std::sync::Arc;

/// The outer program: one `main` that loads the root it is handed, enters `inner.main`, and
/// answers with how that ended. The effect, and the record shapes crossing it, are the program's
/// own declarations.
const OUTER: &str = r#"
nondet effect machine {
  read load[m](root: String) -> Result<Summary, Refusal>
  read reload[m]() -> Result<Summary, Refusal>
  write enter[m](entry: String, argv: List<String>) -> Ended
  write drop[m]() -> Unit
}

type Summary = { modules: List<String>, definitions: Int, tests: Int, warnings: List<String> }
type Refusal = { diagnostics: List<String> }
type Ended = {
  out: List<String>,
  err: List<String>,
  exit: Option<Int>,
  value: Option<String>,
  raised: Option<String>,
}

fn main(root: String) -> Ended / {machine.load[m], machine.enter[m], machine.drop[m]} = {
  let loaded = machine.load[m](root);
  match loaded {
    Ok(_summary) -> {
      let ended = machine.enter[m]("inner.main", ["--flag", "value"]);
      machine.drop[m]();
      ended
    },
    Err(refusal) -> {
      machine.drop[m]();
      {
        out: [],
        err: [],
        exit: None,
        value: None,
        raised: match list_at(refusal.diagnostics, 0) {
          Some(first) -> Some(first),
          None -> Some("no diagnostic"),
        },
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

fn entered(inner: &str) -> Value {
    let project = project(inner);
    let front = front_of(OUTER);
    let texts: HashMap<String, String> =
        [("m".to_string(), OUTER.to_string())].into_iter().collect();
    let unit = ply_codegen::Unit::over_front(&front, texts).expect("this host has a C toolchain");
    let mut machine = Machine::new(&front);
    machine.set_compiled(unit.attach(&BackendSpec {
        kind: BackendKind::C,
    }));
    let mut registry = HostRegistry::new();
    ply_machine::register(&mut registry);
    let binding = registry.bind(&front.check).expect("the machine ops bind");
    machine.set_host_binding(Arc::new(binding));
    machine
        .call(
            "m.main",
            vec![Value::str(project.path().display().to_string())],
            Span::DUMMY,
        )
        .expect("the outer main ran")
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

fn strings(value: &Value) -> Vec<String> {
    let Ok(items) = value.as_list(Span::DUMMY, "a list of lines") else {
        panic!("a list of lines");
    };
    items
        .iter()
        .map(|item| item.as_str(Span::DUMMY, "a line").unwrap().to_string())
        .collect()
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
import std.process (process)

fn main() -> Int / {process.args[proc], process.out[proc]} = {
  let args = process.args[proc]();
  process.out[proc]("inner says hello");
  process.out[proc](int_to_string(len(args)) ++ " args came with it");
  40 + 2
}
"#;

#[test]
fn a_program_loads_and_enters_a_program() {
    let answer = entered(INNER);
    assert_eq!(
        strings(field(&answer, "out")),
        vec!["inner says hello", "2 args came with it"],
        "the nested program's lines are the caller's, not the process's"
    );
    assert_eq!(strings(field(&answer, "err")), Vec::<String>::new());
    assert_eq!(option_int(field(&answer, "exit")), None);
    assert_eq!(option_text(field(&answer, "value")).as_deref(), Some("42"));
    assert_eq!(option_text(field(&answer, "raised")), None);
}

#[test]
fn a_nested_program_may_choose_the_exit_code() {
    let answer = entered(
        r#"
import std.process (process)

fn main() -> Unit / {process.exit[proc]} = process.exit[proc](7)
"#,
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
    let raised = option_text(field(&answer, "raised")).expect("the raise is reported");
    assert!(raised.contains("the inner program's own bug"), "{raised}");
    assert_eq!(option_int(field(&answer, "exit")), None);
}

#[test]
fn a_program_that_does_not_check_is_refused_with_its_diagnostics() {
    let answer = entered("fn main() -> Int = unknown_name\n");
    let raised = option_text(field(&answer, "raised")).expect("the refusal is reported");
    assert!(raised.contains("unknown_name"), "{raised}");
}

/// A load, an entry, then a reload after the tree moved: the second answer is the new
/// program's, and the store the first load wrote is what the second read.
const OUTER_TWICE: &str = r#"
nondet effect machine {
  read load[m](root: String) -> Result<Summary, Refusal>
  read reload[m]() -> Result<Summary, Refusal>
  write enter[m](entry: String, argv: List<String>) -> Ended
  write drop[m]() -> Unit
}

type Summary = { modules: List<String>, definitions: Int, tests: Int, warnings: List<String> }
type Refusal = { diagnostics: List<String> }
type Ended = {
  out: List<String>,
  err: List<String>,
  exit: Option<Int>,
  value: Option<String>,
  raised: Option<String>,
}

fn main(root: String) -> Option<String> / {machine.load[m], machine.enter[m]} = {
  let _loaded = machine.load[m](root);
  (machine.enter[m]("inner.main", [])).value
}

fn again() -> Option<String> / {machine.reload[m], machine.enter[m], machine.drop[m]} = {
  let _again = machine.reload[m]();
  let value = (machine.enter[m]("inner.main", [])).value;
  machine.drop[m]();
  value
}
"#;

#[test]
fn a_reload_after_an_edit_enters_the_new_program() {
    let project = tempfile::tempdir().expect("a temporary directory");
    let inner = project.path().join("inner.ply");
    std::fs::write(
        &inner,
        "fn main() -> Int = 1
",
    )
    .unwrap();

    let front = front_of(OUTER_TWICE);
    let texts: HashMap<String, String> = [("m".to_string(), OUTER_TWICE.to_string())]
        .into_iter()
        .collect();
    let unit = ply_codegen::Unit::over_front(&front, texts).expect("this host has a C toolchain");

    let mut registry = HostRegistry::new();
    ply_machine::register(&mut registry);
    let binding = registry.bind(&front.check).expect("the machine ops bind");
    let binding = Arc::new(binding);

    let call = |entry: &str, arg: Option<String>| {
        let mut machine = Machine::new(&front);
        machine.set_compiled(unit.attach(&BackendSpec {
            kind: BackendKind::C,
        }));
        machine.set_host_binding(Arc::clone(&binding));
        let args = arg.iter().map(Value::str).collect::<Vec<Value>>();
        machine
            .call(entry, args, Span::DUMMY)
            .expect("the entry ran")
    };

    let root = project.path().display().to_string();
    let first = call("m.main", Some(root));
    assert_eq!(option_text(&first).as_deref(), Some("1"));
    assert!(
        project.path().join(".ply-cache").is_dir(),
        "the load wrote its store"
    );

    std::fs::write(
        &inner,
        "fn main() -> Int = 2
",
    )
    .unwrap();
    let second = call("m.again", None);
    assert_eq!(
        option_text(&second).as_deref(),
        Some("2"),
        "the reload read the edited program"
    );
}
