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
