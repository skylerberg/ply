use crate::fixture::Compiled;
use ply_eval::{Compiled as _, Diagnostic, Entered, Machine, Span, Symbol, Value, codes};

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
    (value, tier.steps())
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

/// A tier's steps are its last entry's own: an entry the memo answers ran no body, so it made none.
#[test]
fn a_constant_the_memo_answers_counts_no_steps() {
    let c = Compiled::new(SOURCE);
    let (machine, tier) = c.machine_and_tier();
    let mut machine = machine.with_max_calls(BUDGET);
    let mut constant = || {
        let value = machine
            .call("m.constant", vec![], Span::DUMMY)
            .unwrap_or_else(|d| panic!("`m.constant` raised: {d:#?}"));
        (value, tier.steps())
    };
    let (value, ran) = constant();
    assert_eq!(value, Value::Int(700));
    assert!(ran > 0, "the first entry did not run `constant`'s body");
    assert_eq!(
        constant(),
        (Value::Int(700), 0),
        "the memo's answer reported the {ran} calls of the entry before it"
    );
}

/// `parameterized`'s steps: an `Int` is no memo word, so every entry runs the body.
fn ran(machine: &mut Machine<'_>, tier: &ply_codegen::Bodies) -> u64 {
    machine
        .call("m.parameterized", vec![Value::Int(0)], Span::DUMMY)
        .unwrap_or_else(|d| panic!("`m.parameterized` raised: {d:#?}"));
    let steps = tier.steps();
    assert!(steps > 0, "`m.parameterized` ran no body");
    steps
}

/// Every way the seam declines an offer before running a body counts none, whatever came before.
#[test]
fn a_declined_entry_counts_no_steps() {
    let c = Compiled::new(SOURCE);
    let (machine, tier) = c.machine_and_tier();
    let mut machine = machine.with_max_calls(BUDGET);
    // A name the unit never compiled, then a compiled one offered the wrong number of arguments.
    for (name, args) in [("m.absent", vec![]), ("m.parameterized", vec![])] {
        let before = ran(&mut machine, &tier);
        machine
            .call(name, args, Span::DUMMY)
            .expect_err("the seam declines the offer");
        assert_eq!(
            tier.steps(),
            0,
            "declining `{name}` reported the {before} calls of the entry before it"
        );
    }
    let before = ran(&mut machine, &tier);
    tier.while_entered(|| machine.call("m.parameterized", vec![Value::Int(0)], Span::DUMMY))
        .expect_err("an entry that arrives while another runs is declined");
    assert_eq!(
        tier.steps(),
        0,
        "the reentrant decline reported the {before} calls of the entry before it"
    );
    assert_eq!(
        tier.declines(),
        ply_codegen::Declines {
            not_compiled: 1,
            arity: 1,
            reentered: 1,
            ..Default::default()
        }
    );
}

/// A constant, and a body whose `simulate` region leaves a record and performs its tasks' atoms.
const REGION: &str = r#"
pub fn constant() -> Int = 7

pub fn raced() -> Int / {sim.read} = simulate {
  let a = task.spawn(|| 1);
  let b = task.spawn(|| 2);
  task.join(a) + task.join(b)
}
"#;

fn called(machine: &mut Machine<'_>, name: &str) -> Value {
    machine
        .call(name, vec![], Span::DUMMY)
        .unwrap_or_else(|d| panic!("`{name}` raised: {d:#?}"))
}

#[test]
fn a_constant_the_memo_answers_after_a_region_reports_no_record() {
    let c = Compiled::new(REGION);
    let (mut machine, tier) = c.machine_and_tier();
    assert_eq!(called(&mut machine, "m.constant"), Value::Int(7));
    assert_eq!(called(&mut machine, "m.raced"), Value::Int(3));
    assert!(
        machine.simulated().is_some(),
        "`raced` left no record of its region"
    );
    assert_eq!(called(&mut machine, "m.constant"), Value::Int(7));
    assert_eq!(tier.steps(), 0, "`constant` was not answered from the memo");
    assert!(
        machine.simulated().is_none(),
        "the memo's answer reported the region of the entry before it"
    );
}

/// Entries nothing reads after, as a prover's are: an entry no body ran for reports none of them.
#[test]
fn an_entry_no_body_ran_for_reports_nothing_of_the_one_before_it() {
    let c = Compiled::new(REGION);
    let (_machine, tier) = c.machine_and_tier();
    let enter = |name: &str| tier.enter_whole(&Symbol::new(name), &[], BUDGET);
    assert!(matches!(
        enter("m.constant"),
        Entered::Answered(Value::Int(7))
    ));
    assert!(matches!(enter("m.raced"), Entered::Answered(Value::Int(3))));
    assert!(
        tier.steps() > 0 && tier.simulated().is_some() && !tier.take_performed().is_empty(),
        "`raced` did not leave its calls, its record and its atoms"
    );
    let reports_nothing = |entry: &str| {
        assert_eq!(tier.steps(), 0, "{entry} reported `raced`'s calls");
        assert!(
            tier.simulated().is_none(),
            "{entry} reported `raced`'s region"
        );
        assert!(
            tier.take_performed().is_empty(),
            "{entry} reported `raced`'s atoms"
        );
    };
    assert!(matches!(enter("m.raced"), Entered::Answered(Value::Int(3))));
    assert!(matches!(
        enter("m.constant"),
        Entered::Answered(Value::Int(7))
    ));
    reports_nothing("the memo's answer to `m.constant`");
    assert!(matches!(enter("m.raced"), Entered::Answered(Value::Int(3))));
    assert!(matches!(enter("m.absent"), Entered::Declined));
    reports_nothing("the decline of `m.absent`");
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
