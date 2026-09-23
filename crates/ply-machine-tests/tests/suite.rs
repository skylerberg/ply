//! The nested-entry capability, end to end: a program performs `machine.load`/`machine.enter`
//! and another program has run, its output captured, its ending a value the caller reads.

use ply_eval::host::HostRegistry;
use ply_eval::{BackendKind, BackendSpec, Machine, Provider, Value};
use ply_span::{SourceId, Span};
use ply_ty::Front;
use std::collections::HashMap;
use std::sync::Arc;

/// The outer program: one `main` that loads the source it is handed as module `inner`, enters
/// its `inner.main`, and answers with how that ended. The effect, and the record shapes crossing
/// it, are the program's own declarations.
const OUTER: &str = r#"
nondet effect machine {
  read load[m](modules: List<Module>) -> Result<Summary, Refusal>
  write enter[m](entry: String, argv: List<String>) -> Ended
  write drop[m]() -> Unit
}

type Module = { name: String, text: String }
type Summary = { modules: List<String>, definitions: Int, tests: Int }
type Refusal = { diagnostics: List<String> }
type Ended = {
  out: List<String>,
  err: List<String>,
  exit: Option<Int>,
  value: Option<String>,
  raised: Option<String>,
}

fn main(source: String) -> Ended / {machine.load[m], machine.enter[m], machine.drop[m]} = {
  let loaded = machine.load[m]([{ name: "inner", text: source }]);
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

fn entered(inner: &str) -> Value {
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
        .call("m.main", vec![Value::str(inner)], Span::DUMMY)
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
