//! A nullary pure definition is a constant, and the evaluator remembers it.

use crate::fixture::Compiled;
use ply_eval::{Machine, Value};
use ply_span::{Span, codes};

/// `deep` costs 700 pending calls and `nest` costs one per step, so a budget of 1000 admits either
/// alone and refuses `nest(400)` with a second `deep` under it.
const BUDGET: usize = 1000;

const SOURCE: &str = r#"
effect store {
  read peek() -> Int
}

fn deep(n: Int) -> Int = if n <= 0 { 0 } else { 1 + deep(n - 1) }

pub fn constant() -> Int = deep(700)

pub fn parameterized(ignored: Int) -> Int = deep(700)

// Performs nothing; the row is the published claim and is what decides this.
pub fn over_declared() -> Int / {store.read} = deep(700)

fn nest_constant(n: Int) -> Int = if n <= 0 { constant() } else { nest_constant(n - 1) + 0 }

fn nest_parameterized(n: Int) -> Int =
  if n <= 0 { parameterized(0) } else { nest_parameterized(n - 1) + 0 }

fn nest_over_declared(n: Int) -> Int / {store.read} =
  if n <= 0 { over_declared() } else { nest_over_declared(n - 1) + 0 }

pub fn probe_constant(n: Int) -> Int = constant() + nest_constant(n)

pub fn probe_parameterized(n: Int) -> Int = parameterized(0) + nest_parameterized(n)

pub fn probe_over_declared(n: Int) -> Int / {store.read} =
  over_declared() + nest_over_declared(n)
"#;

/// One budget, one entry point.
fn probe(c: &Compiled, name: &str) -> Result<Value, ply_span::Diagnostic> {
    let mut machine = Machine::new(&c.program, &c.resolved, &c.check).with_max_calls(BUDGET);
    machine.call(name, vec![Value::Int(400)], Span::DUMMY)
}

#[test]
fn a_nullary_pure_definition_is_evaluated_once() {
    let c = Compiled::new(SOURCE);
    match probe(&c, "m.probe_constant") {
        Ok(value) => assert_eq!(value, Value::Int(1400)),
        Err(d) => panic!("the remembered constant did not survive the depth: {d:#?}"),
    }
}

#[test]
fn a_definition_with_a_parameter_is_not_a_constant_however_dead_the_parameter_is() {
    let c = Compiled::new(SOURCE);
    let d = probe(&c, "m.probe_parameterized")
        .expect_err("a parameterized definition must be re-evaluated");
    assert_eq!(d.code, codes::RUNTIME_ERROR);
}

#[test]
fn a_declared_row_the_body_never_performs_still_refuses_the_memo() {
    let c = Compiled::new(SOURCE);
    let d = probe(&c, "m.probe_over_declared")
        .expect_err("the published row is what decides, not the body's");
    assert_eq!(d.code, codes::RUNTIME_ERROR);
}

/// The rule has to keep the atoms: a nullary definition that performs is re-evaluated, and its
/// handler sees every one of its calls.
#[test]
fn a_nullary_definition_that_performs_is_evaluated_on_every_call() {
    let c = Compiled::new(
        r#"
effect counter {
  write bump() -> Unit
}

fn tick() -> Unit / {counter.write} = counter.bump()

test "a nullary effectful definition performs once per call" {
  with_cell[hits](0) { c ->
    handle {
      tick();
      tick();
      tick();
      assert_eq(cell_get(c), 3)
    } with {
      counter.bump() -> cell_set(c, cell_get(c) + 1),
    }
  }
}
"#,
    );
    assert_eq!(c.check.tests.len(), 1);
    let mut machine = Machine::new(&c.program, &c.resolved, &c.check);
    if let Err(d) = machine.eval_test(0) {
        panic!("the machine dropped a perform: {d:#?}");
    }
}

/// A constant reached from inside a handler is still the same constant, and a handler that
/// discharges an effect leaves the definition around it pure — so the value a second call sees has
/// to be the first's.
#[test]
fn a_constant_built_behind_a_handler_is_remembered_with_its_value_intact() {
    let c = Compiled::new(
        r#"
effect seed {
  read grain(i: Int) -> Int
}

fn raw() -> List<Int> / {seed.read} = [seed.grain(1), seed.grain(2), seed.grain(3)]

pub fn table() -> List<Int> = handle { raw() } with { seed.grain(i) -> i * 10 }

test "the table is the same list however many times it is asked for" {
  assert_eq(table(), [10, 20, 30]);
  assert_eq(table(), [10, 20, 30]);
  assert_eq(len(table()) + len(table()), 6)
}
"#,
    );
    let mut machine = Machine::new(&c.program, &c.resolved, &c.check);
    if let Err(d) = machine.eval_test(0) {
        panic!("the machine disagreed: {d:#?}");
    }
    let first = machine
        .call("m.table", Vec::new(), Span::DUMMY)
        .expect("the constant is callable from outside a test");
    let second = machine
        .call("m.table", Vec::new(), Span::DUMMY)
        .expect("and callable again");
    assert_eq!(first.render(), "[10, 20, 30]");
    assert_eq!(first, second);
}
