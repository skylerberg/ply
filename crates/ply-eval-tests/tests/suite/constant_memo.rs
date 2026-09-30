use crate::fixture::Compiled;
use ply_eval::{Diagnostic, Span, Value, codes};

/// Admits `deep` (700 pending calls) or `nest` alone, not `nest(400)` with a `deep` under it.
const BUDGET: usize = 1000;

// Each definition answers a list: the tier remembers only a constant that answers a boxed word.
const SOURCE: &str = r#"
effect store {
  read peek() -> Int
}

fn deep(n: Int) -> Int = if n <= 0 { 0 } else { 1 + deep(n - 1) }

fn total(xs: List<Int>) -> Int = fold(xs, 0, |sum, x| sum + x)

pub fn constant() -> List<Int> = [deep(700)]

pub fn parameterized(ignored: Int) -> List<Int> = [deep(700)]

// Performs nothing; the row is the published claim and is what decides this.
pub fn over_declared() -> List<Int> / {store.read} = [deep(700)]

fn nest_constant(n: Int) -> Int = if n <= 0 { total(constant()) } else { nest_constant(n - 1) + 0 }

fn nest_parameterized(n: Int) -> Int =
  if n <= 0 { total(parameterized(0)) } else { nest_parameterized(n - 1) + 0 }

fn nest_over_declared(n: Int) -> Int / {store.read} =
  if n <= 0 { total(over_declared()) } else { nest_over_declared(n - 1) + 0 }

pub fn probe_constant(n: Int) -> Int = total(constant()) + nest_constant(n)

pub fn probe_parameterized(n: Int) -> Int = total(parameterized(0)) + nest_parameterized(n)

pub fn probe_over_declared(n: Int) -> Int / {store.read} =
  total(over_declared()) + nest_over_declared(n)
"#;

fn probe(c: &Compiled, name: &str) -> Result<Value, Diagnostic> {
    let mut machine = c.machine().with_max_calls(BUDGET);
    machine.call(name, vec![Value::Int(400)], Span::DUMMY)
}

/// The budget's own refusal: a second `deep` under `nest` was evaluated rather than remembered.
#[track_caller]
fn assert_over_budget(d: &Diagnostic) {
    assert_eq!(d.code, codes::RUNTIME_ERROR, "{d:?}");
    assert_eq!(
        d.message,
        format!("recursion limit of {BUDGET} nested calls exceeded"),
        "{d:?}"
    );
}

/// What the refusals below are measured against: the same shape, remembered, fits the budget.
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
    assert_over_budget(&d);
}

#[test]
fn a_declared_row_the_body_never_performs_still_refuses_the_memo() {
    let c = Compiled::new(SOURCE);
    let d = probe(&c, "m.probe_over_declared")
        .expect_err("the published row is what decides, not the body's");
    assert_over_budget(&d);
}
