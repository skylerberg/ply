use crate::fixture::port_front;
use ply_eval::compiled::{Compiled, Entered};
use ply_eval::host::{HostRuntime, MachineId, Pending};
use ply_eval::{Analysis, DefHash, Diagnostic, Machine, Span, Symbol, Value, codes};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

const PROGRAM: &str = "fn main() -> Int = 3\n\ntest \"main answers\" { assert_eq(main(), 3) }\n";

/// A tier whose every entry leaves a span open in its body, which it says as the body ends.
struct Leaves {
    program: DefHash,
    answer: Box<dyn Fn() -> Entered>,
    left: RefCell<Vec<Diagnostic>>,
}

impl Leaves {
    fn ran(&self) -> Entered {
        self.left.borrow_mut().push(warning("the body's"));
        (self.answer)()
    }
}

impl Compiled for Leaves {
    fn describes(&self, program: DefHash) -> bool {
        self.program == program
    }

    fn enter(&self, _: &Symbol, _: &[Value], _: usize) -> Option<Value> {
        None
    }

    fn enter_test(&self, _: &Symbol, _: usize) -> Entered {
        self.ran()
    }

    fn enter_whole(&self, _: &Symbol, _: &[Value], _: usize) -> Entered {
        self.ran()
    }

    fn take_teardown(&self) -> Vec<Diagnostic> {
        self.left.take()
    }
}

/// A runtime that warns as every entry point it is told of ends, counting them where a test reads.
struct Warns {
    ended: Arc<AtomicU32>,
}

impl HostRuntime for Warns {
    fn watch(&self, _: &Pending) -> Result<(), Diagnostic> {
        Ok(())
    }

    fn resolved(&self) -> Vec<(u64, Result<Value, Diagnostic>)> {
        Vec::new()
    }

    fn park(&self) -> Result<(), Diagnostic> {
        Ok(())
    }

    fn block_on(&self, _: Pending) -> Result<Value, Diagnostic> {
        Ok(Value::Unit)
    }

    fn end_entry_point(&self, _: MachineId) -> Vec<Diagnostic> {
        self.ended.fetch_add(1, Ordering::Relaxed);
        vec![warning("the runtime's")]
    }
}

fn warning(whose: &str) -> Diagnostic {
    Diagnostic::warning(
        codes::SPAN_ABANDONED,
        format!("{whose} spans were still open"),
    )
}

fn messages(warnings: &[Diagnostic]) -> Vec<&str> {
    warnings.iter().map(|d| d.message.as_str()).collect()
}

/// Both halves of an entry's ending, in the order they end: the body's, then the entry point's.
fn both() -> Vec<&'static str> {
    vec![
        "the body's spans were still open",
        "the runtime's spans were still open",
    ]
}

/// The machine, and how many entry points its runtime has been told ended.
fn machine_over(
    front: &Analysis,
    answer: impl Fn() -> Entered + 'static,
) -> (Machine<'_>, Arc<AtomicU32>) {
    let tier = Rc::new(Leaves {
        program: front.hashes_digest,
        answer: Box::new(answer),
        left: RefCell::new(Vec::new()),
    });
    let mut machine = Machine::new(front, tier).expect("the tier describes the program");
    let ended = Arc::new(AtomicU32::new(0));
    let counted = Arc::clone(&ended);
    machine.set_host_runtime(Arc::new(move || {
        Rc::new(Warns {
            ended: Arc::clone(&counted),
        }) as Rc<dyn HostRuntime>
    }));
    (machine, ended)
}

#[test]
fn a_test_hands_back_what_its_body_and_its_entry_point_warned_of_as_it_ended() {
    let front = port_front(&[("", PROGRAM)]);
    let (mut machine, _) = machine_over(&front, || Entered::Answered(Value::Unit));
    let (answer, warnings) = machine.eval_test(0).into_parts();
    assert!(answer.is_ok(), "{answer:?}");
    assert_eq!(messages(&warnings), both());
    assert!(
        warnings.iter().all(|w| w.code == codes::SPAN_ABANDONED),
        "{warnings:?}"
    );
}

#[test]
fn each_call_hands_back_its_own_warnings_and_none_of_the_entry_before_it() {
    let front = port_front(&[("", PROGRAM)]);
    let (mut machine, ended) = machine_over(&front, || Entered::Answered(Value::Int(3)));
    for entry in 1..=2 {
        let (answer, warnings) = machine.call("main", Vec::new(), Span::DUMMY).into_parts();
        assert_eq!(answer.ok(), Some(Value::Int(3)), "entry {entry}");
        assert_eq!(messages(&warnings), both(), "entry {entry}");
    }
    assert_eq!(ended.load(Ordering::Relaxed), 2);
}

/// A body that raises never reaches the `trace.exit` it was heading for, which is when its spans
/// are most worth the warning.
#[test]
fn a_raise_hands_back_its_warnings_beside_the_diagnostic() {
    let front = port_front(&[("", PROGRAM)]);
    let (mut machine, _) = machine_over(&front, || {
        Entered::Raised(Diagnostic::error(codes::RUNTIME_ERROR, "the body gave up"))
    });
    let (answer, warnings) = machine.call("main", Vec::new(), Span::DUMMY).into_parts();
    assert_eq!(
        answer.expect_err("the body raised").message,
        "the body gave up"
    );
    assert_eq!(messages(&warnings), both());
}

#[test]
fn an_entry_refused_before_it_ran_ended_nothing_and_warns_of_nothing() {
    let front = port_front(&[("", PROGRAM)]);
    let (mut machine, ended) = machine_over(&front, || panic!("a refused entry reached the tier"));
    let (answer, warnings) = machine.eval_test(7).into_parts();
    assert_eq!(
        answer.expect_err("there is no eighth test").code,
        codes::INTERNAL_ERROR
    );
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        ended.load(Ordering::Relaxed),
        0,
        "no entry point began, so none ended"
    );
}
