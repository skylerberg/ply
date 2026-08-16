//! A nullary pure definition is a constant, and both engines remember it.
//!
//! The rule is read off the **published** row, so these tests pin the three
//! signatures that decide it: no parameters and an empty row is remembered, a
//! parameter is not, and a declared row the body never performs is not either.
//!
//! The recursion budget is what makes the memo observable at all. Nothing else
//! about it is: a constant evaluated once and a constant evaluated twice differ
//! in no value, no atom and no trace, which is the whole argument that
//! remembering it is a substitution rather than a change. What they do differ
//! in is how many calls are pending underneath, so a budget the second
//! evaluation would exceed is the one place the difference has a name — and
//! both engines have to agree about it or `--engine both` reports `E0503` on
//! every program with a pure nullary definition in it.

use ply_core::{CheckOutput, check_program};
use ply_eval::{Interp, Machine, Value};
use ply_span::{SourceId, Span, codes};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::{Resolved, resolve};

struct Compiled {
    program: Program,
    resolved: Resolved,
    check: CheckOutput,
}

fn compile(source: &str) -> Compiled {
    let inputs = vec![(SourceId(0), ModuleName::from_dotted("m"), source)];
    let program = match ply_syntax::parse_program(inputs) {
        Ok(p) => p,
        Err(d) => panic!("did not parse: {d:#?}"),
    };
    let resolved = match resolve(&program) {
        Ok(r) => r,
        Err(d) => panic!("did not resolve: {d:#?}"),
    };
    let check = match check_program(&program, &resolved) {
        Ok(c) => c,
        Err(d) => panic!("did not typecheck: {d:#?}"),
    };
    Compiled {
        program,
        resolved,
        check,
    }
}

/// `deep` costs 700 pending calls and `nest` costs one per step, so a budget of
/// 1000 admits either alone and refuses `nest(400)` with a second `deep` under
/// it. Every probe below is `<constant>() + nest(400)`: the first term is
/// evaluated at the top, where it always fits, and the second reaches the same
/// definition from a depth where only a remembered value does.
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

/// Both engines, one budget, one entry point.
fn probe(c: &Compiled, name: &str) -> [Result<Value, ply_span::Diagnostic>; 2] {
    let args = || vec![Value::Int(400)];
    let mut interp = Interp::new(&c.program, &c.resolved, &c.check).with_max_calls(BUDGET);
    let walked = interp.call(name, args(), Span::DUMMY);
    let mut machine = Machine::new(&c.program, &c.resolved, &c.check).with_max_calls(BUDGET);
    let stepped = machine.call(name, args(), Span::DUMMY);
    [walked, stepped]
}

#[test]
fn a_nullary_pure_definition_is_evaluated_once_and_both_engines_agree() {
    let c = compile(SOURCE);
    for answered in probe(&c, "m.probe_constant") {
        match answered {
            Ok(value) => assert_eq!(value, Value::Int(1400)),
            Err(d) => panic!("the remembered constant did not survive the depth: {d:#?}"),
        }
    }
}

#[test]
fn a_definition_with_a_parameter_is_not_a_constant_however_dead_the_parameter_is() {
    let c = compile(SOURCE);
    for answered in probe(&c, "m.probe_parameterized") {
        let d = answered.expect_err("a parameterized definition must be re-evaluated");
        assert_eq!(d.code, codes::RUNTIME_ERROR);
    }
}

#[test]
fn a_declared_row_the_body_never_performs_still_refuses_the_memo() {
    let c = compile(SOURCE);
    for answered in probe(&c, "m.probe_over_declared") {
        let d = answered.expect_err("the published row is what decides, not the body's");
        assert_eq!(d.code, codes::RUNTIME_ERROR);
    }
}

/// The rule has to keep the atoms: a nullary definition that performs is
/// re-evaluated, and its handler sees every one of its calls.
#[test]
fn a_nullary_definition_that_performs_is_evaluated_on_every_call() {
    let c = compile(
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
    let mut interp = Interp::new(&c.program, &c.resolved, &c.check);
    if let Err(d) = interp.eval_test(0) {
        panic!("the tree-walker dropped a perform: {d:#?}");
    }
    let mut machine = Machine::new(&c.program, &c.resolved, &c.check);
    if let Err(d) = machine.eval_test(0) {
        panic!("the machine dropped a perform: {d:#?}");
    }
}

/// A constant reached from inside a handler is still the same constant, and a
/// handler that discharges an effect leaves the definition around it pure — so
/// the value a second call sees has to be the first's.
#[test]
fn a_constant_built_behind_a_handler_is_remembered_with_its_value_intact() {
    let c = compile(
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
    let mut interp = Interp::new(&c.program, &c.resolved, &c.check);
    if let Err(d) = interp.eval_test(0) {
        panic!("the tree-walker disagreed: {d:#?}");
    }
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
