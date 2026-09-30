use crate::fixture::port_front;
use ply_eval::compiled::*;
use ply_eval::evaluator::Machine;
use ply_eval::{DefHash, Diagnostic, Front, Symbol, Value, codes};
use std::rc::Rc;

struct Checked {
    front: Front,
}

impl Checked {
    fn machine(&self) -> Machine<'_> {
        Machine::new(&self.front)
    }
}

fn checked_source(source: &str) -> Checked {
    Checked {
        front: port_front(&[("", source)]),
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

fn first_test_under(
    c: &Checked,
    entered: impl Fn() -> Entered + 'static,
) -> (Result<(), Diagnostic>, (u64, u64)) {
    let mut machine = c.machine();
    machine.set_compiled(Rc::new(Roots {
        program: c.front.hashes_digest,
        entered: Box::new(entered),
    }));
    let outcome = machine.eval_test(0);
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
