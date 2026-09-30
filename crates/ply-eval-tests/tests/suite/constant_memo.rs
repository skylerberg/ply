use crate::fixture::Compiled;
use ply_eval::{Diagnostic, Span, Value, codes};

/// Admits `deep` (700 pending calls) or `nest` alone, not `nest(400)` with a `deep` under it.
const BUDGET: usize = 1000;

/// How many times `reads` reads `constant`.
const READS: i64 = 1000;

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

pub fn reads(n: Int, sum: Int) -> Int = if n <= 0 { sum } else { reads(n - 1, sum + constant()) }

pub fn reads_literal(n: Int, sum: Int) -> Int =
  if n <= 0 { sum } else { reads_literal(n - 1, sum + 700) }
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

/// `name`'s answer and the calls the tier counted, on a tier whose memo no earlier entry filled.
fn counted(c: &Compiled, name: &str, args: Vec<Value>) -> (Value, u64) {
    let (machine, tier) = c.machine_and_tier();
    let mut machine = machine.with_max_calls(BUDGET);
    let value = machine
        .call(name, args, Span::DUMMY)
        .unwrap_or_else(|d| panic!("`{name}` raised: {d:#?}"));
    (value, ply_eval::Compiled::steps(&*tier))
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

/// Counted in calls, never timed: all `reads` spends beyond `reads_literal` is `constant`'s body.
#[test]
fn a_constant_read_many_times_runs_its_body_once() {
    let c = Compiled::new(SOURCE);
    let (value, body) = counted(&c, "m.constant", vec![]);
    assert_eq!(value, Value::Int(700));
    assert!(body > 0, "the tier counted no call of `constant`'s body");
    let args = || vec![Value::Int(READS), Value::Int(0)];
    let (literal, looped) = counted(&c, "m.reads_literal", args());
    let (read, spent) = counted(&c, "m.reads", args());
    assert_eq!(read, literal);
    assert_eq!(
        spent,
        looped + body,
        "{READS} reads ran `constant`'s body of {body} calls {} times, not once",
        spent.saturating_sub(looped) / body.max(1)
    );
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
