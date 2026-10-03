use crate::fixture::port_front;
use ply_eval::compiled::*;
use ply_eval::evaluator::Machine;
use ply_eval::{Analysis, DefHash, Diagnostic, Plain, Span, Symbol, Value, codes, slot};
use std::rc::Rc;

struct Checked {
    front: Analysis,
}

fn checked_source(source: &str) -> Checked {
    Checked {
        front: port_front(&[("m", source)]),
    }
}

struct Roots {
    program: DefHash,
    entered: Box<dyn Fn() -> Entered>,
}

impl Compiled for Roots {
    fn describes(&self, program: DefHash) -> bool {
        self.program == program
    }

    fn enter(&self, _: &Symbol, _: &[Value], _: usize) -> Option<Value> {
        None
    }

    fn enter_test(&self, _: &Symbol, _: usize) -> Entered {
        (self.entered)()
    }
}

/// A machine whose tier enters every test root the way `entered` says and declines every call.
fn machine_under(c: &Checked, entered: impl Fn() -> Entered + 'static) -> Machine<'_> {
    let tier = Rc::new(Roots {
        program: c.front.hashes_digest,
        entered: Box::new(entered),
    });
    Machine::new(&c.front, tier).expect("the tier describes the program")
}

fn first_test_under(
    c: &Checked,
    entered: impl Fn() -> Entered + 'static,
) -> (Result<(), Diagnostic>, (u64, u64)) {
    let mut machine = machine_under(c, entered);
    let (outcome, _) = machine.eval_test(0).into_parts();
    (outcome, machine.compiled_counts())
}

/// A test whose assertion fails: `double(21)` is 42.
const DOUBLE_DOUBLES: &str =
    "fn double(x: Int) -> Int = x * 2\n\ntest \"double doubles\" { assert_eq(double(21), 43) }\n";

fn assertion_raised() -> Entered {
    Entered::Raised(Diagnostic::error(codes::RUNTIME_ERROR, "assertion failed"))
}

#[test]
fn a_test_root_the_backend_raised_in_keeps_the_machines_diagnostic_when_it_raises_too() {
    let c = checked_source(DOUBLE_DOUBLES);
    let (outcome, _) = first_test_under(&c, assertion_raised);
    let d = outcome.expect_err("the assertion fails in the machine");
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    assert_eq!(d.message, "assertion failed");
}

#[test]
fn a_tier_built_from_another_program_is_refused_before_anything_runs() {
    let c = checked_source(DOUBLE_DOUBLES);
    let tier = Rc::new(Roots {
        program: DefHash::of(b"another program"),
        entered: Box::new(|| panic!("a refused tier was entered")),
    });
    let refused = Machine::new(&c.front, tier)
        .err()
        .expect("a machine is never built on another program's tier");
    assert_eq!(refused.code, codes::INTERNAL_ERROR, "{refused:?}");
    assert!(
        refused.message.contains("built from another"),
        "{refused:?}"
    );
}

/// Nothing the program wrote ran, so the failure is Ply's and no assertion on the program's own
/// codes can be met by it.
#[test]
fn a_test_root_the_tier_did_not_run_is_plys_defect_and_counted_as_declined() {
    let c = checked_source(DOUBLE_DOUBLES);
    let (outcome, counts) = first_test_under(&c, || Entered::Declined);
    let d = outcome.expect_err("a declined test answers nothing");
    assert_eq!(d.code, codes::INTERNAL_ERROR, "{d:?}");
    assert!(
        d.message.starts_with("the C backend declined to enter"),
        "{d:?}"
    );
    assert_eq!(counts, (0, 1));
}

/// The checker refuses a test whose body is not `Unit`, so a value is Ply's defect rather than a
/// decline: the tier did run the test.
#[test]
fn a_test_root_that_answers_a_value_is_plys_defect_and_counted_as_entered() {
    let c = checked_source(DOUBLE_DOUBLES);
    let (outcome, counts) = first_test_under(&c, || Entered::Answered(Value::Int(7)));
    let d = outcome.expect_err("a test answers `()`");
    assert_eq!(d.code, codes::INTERNAL_ERROR, "{d:?}");
    assert!(
        d.message.contains(&format!("answered `{}`", slot(0))),
        "{d:?}"
    );
    assert_eq!(&*d.values, &[Plain::Int(7)], "{d:?}");
    assert_eq!(counts, (1, 0));
}

#[test]
fn an_entry_point_the_tier_declined_is_plys_defect_and_counted_as_declined() {
    let c = checked_source(DOUBLE_DOUBLES);
    let mut machine = machine_under(&c, || panic!("no test is entered here"));
    let d = machine
        .call("double", vec![Value::Int(21)], Span::DUMMY)
        .into_parts()
        .0
        .expect_err("a declined entry answers nothing");
    assert_eq!(d.code, codes::INTERNAL_ERROR, "{d:?}");
    assert!(
        d.message.starts_with("the C backend declined to enter"),
        "{d:?}"
    );
    assert_eq!(machine.compiled_counts(), (0, 1));
}
