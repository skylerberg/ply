//! A transaction is a handler, so its two hazards are the machine's rather than a driver's, and
//! both are audited here.

use crate::fixture::Compiled;
use ply_eval::host::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRegistry, HostRequest, HostResource,
    HostRuntime, Linearity, MachineId, Pending,
};
use ply_eval::{Machine, TaskId, Value};
use ply_span::{Diagnostic, Symbol, codes};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The `db` effect and the `transaction` scope, in the shape `std.db` ships them.
const DB: &str = r#"
nondet effect db {
  write execute[t](row: Int) -> Int
  write begin() -> Int
  write commit() -> Int
  write abort() -> Int
  write rollback(reason: String) -> Unit
}

fn transaction<a | e>(body: () -> a / e) -> Result<a, String> / {db.write | e} =
  handle {
    db.begin();
    let value = body();
    db.commit();
    Ok(value)
  } with {
    db.rollback(reason) resume k -> {
      db.abort();
      Err(reason)
    },
  }
"#;

/// What the run performed, in order.
#[derive(Default)]
struct Journal {
    lines: Mutex<Vec<String>>,
}

impl Journal {
    fn lines(&self) -> Vec<String> {
        self.lines.lock().expect("no panic held the lock").clone()
    }

    fn count(&self, what: &str) -> usize {
        self.lines().iter().filter(|line| *line == what).count()
    }
}

impl HostHandler for Journal {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let line = match &req.atom.resource {
            ply_core::ty::Resource::Named(table) => format!("{}[{table}]", req.op.op),
            ply_core::ty::Resource::Singleton => req.op.op.to_string(),
        };
        let ordinal = {
            let mut lines = self.lines.lock().expect("no panic held the lock");
            lines.push(line);
            lines.len()
        };
        Ok(HostAnswer::Value(Value::Int(ordinal as i64)))
    }
}

/// Records the task each operation was performed by, which is the identity a driver keys an open
/// scope on.
#[derive(Default)]
struct Performers {
    seen: Mutex<Vec<(String, Option<TaskId>)>>,
}

impl HostHandler for Performers {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        self.seen
            .lock()
            .expect("no panic held the lock")
            .push((req.op.op.to_string(), req.task));
        Ok(HostAnswer::Value(Value::Int(0)))
    }
}

fn op(effect: &str, name: &str, resource: HostResource) -> HostOp {
    HostOp {
        effect: Symbol::new(effect),
        op: Symbol::new(name),
        resource,
        determinism: Determinism::Nondeterministic,
        // The whole point: every one of these is a real write, so the linearity counter is armed
        // for the rollback tests rather than dormant.
        linearity: Linearity::AtMostOnce,
        blocking: false,
        secrets: false,
        path: "test::db",
    }
}

fn registry(handler: Arc<dyn HostHandler>, tasks: bool) -> HostRegistry {
    let mut registry = HostRegistry::new();
    registry.register(op("db", "execute", HostResource::Any), handler.clone());
    for name in ["begin", "commit", "abort"] {
        registry.register(
            op(
                "db",
                name,
                HostResource::Only(ply_core::ty::Resource::Singleton),
            ),
            handler.clone(),
        );
    }
    if tasks {
        for name in ["spawn", "join", "yield"] {
            let mut declaration = op("task", name, HostResource::Any);
            declaration.linearity = Linearity::Repeatable;
            registry.register(declaration, handler.clone());
        }
    }
    registry
}

/// A runtime that answers nothing and counts what the machine tells it.
#[derive(Default)]
struct Bookkeeper {
    ends: AtomicU64,
    fails: bool,
    /// The machine the last teardown named.
    machines: std::sync::Mutex<Option<MachineId>>,
}

impl Bookkeeper {
    fn ends(&self) -> u64 {
        self.ends.load(Ordering::SeqCst)
    }

    fn machine(&self) -> Option<MachineId> {
        *self.machines.lock().expect("no panic holds this")
    }
}

impl HostRuntime for Bookkeeper {
    fn poll(&self, _: &Pending) -> Result<Option<Value>, Diagnostic> {
        Ok(Some(Value::Int(0)))
    }

    fn park(&self) -> Result<(), Diagnostic> {
        Ok(())
    }

    fn block_on(&self, _: Pending) -> Result<Value, Diagnostic> {
        Ok(Value::Int(0))
    }

    fn end_entry_point(&self, machine: MachineId) -> Result<(), Diagnostic> {
        // Recorded, because the identity a handler keys scoped state on is half the point of the
        // hook: a teardown that could not tell two entry points apart would roll back the wrong
        // one's transaction.
        *self.machines.lock().expect("no panic holds this") = Some(machine);
        self.ends.fetch_add(1, Ordering::SeqCst);
        match self.fails {
            false => Ok(()),
            true => Err(Diagnostic::error(
                codes::RUNTIME_ERROR,
                "a connection was discarded rather than returned to the pool",
            )),
        }
    }
}

struct Run {
    outcome: Result<(), Diagnostic>,
    journal: Arc<Journal>,
}

impl Run {
    #[track_caller]
    fn refused(&self, code: &str) -> &Diagnostic {
        let d = self
            .outcome
            .as_ref()
            .expect_err("the program was expected to be refused");
        assert_eq!(d.code, code, "{}: {}", d.code, d.message);
        d
    }

    #[track_caller]
    fn passed(&self) {
        if let Err(d) = &self.outcome {
            panic!("the test was expected to pass: {} {}", d.code, d.message);
        }
    }
}

fn run(body: &str) -> Run {
    run_with(body, false)
}

fn run_with(body: &str, tasks: bool) -> Run {
    let source = format!("{DB}{body}");
    let compiled = Compiled::named("t", &source);
    let journal = Arc::new(Journal::default());
    let binding = registry(journal.clone() as Arc<dyn HostHandler>, tasks)
        .bind(&compiled.check)
        .unwrap_or_else(|d| panic!("the registry binds: {d:#?}"));
    let mut machine = Machine::new(&compiled.program, &compiled.resolved, &compiled.check);
    machine.set_host_binding(Arc::new(binding));
    let outcome = machine.eval_test(0);
    Run { outcome, journal }
}

/// The claim, against a run where the counter is armed: the statement after the rollback never
/// executes, the commit never executes, and nothing is refused.
#[test]
fn a_rollback_discards_the_continuation_and_trips_no_linearity_rule() {
    let run = run(r#"
test/nondet "rollback" {
  let out = transaction(|| {
    db.execute[items](1);
    db.rollback("out of stock");
    db.execute[items](2);
    7
  });
  assert_eq(out, Err("out of stock"))
}
"#);
    run.passed();
    assert_eq!(
        run.journal.lines(),
        ["begin", "execute[items]", "abort"],
        "the second statement and the commit are in the continuation that was dropped"
    );
}

/// The same discard from two calls deep, which is the shape a program actually writes: the rollback
/// is not a lexical `return` and the site that performs it cannot see the `handle` that answers it.
#[test]
fn a_rollback_two_calls_deep_discards_its_callers() {
    let run = run(r#"
fn charge() -> Int / {db.write, db.write[items]} = {
  db.execute[items](1);
  reserve();
  db.execute[items](3);
  3
}

fn reserve() -> Unit / {db.write, db.write[items]} = db.rollback("no stock")

test/nondet "deep" {
  assert_eq(transaction(|| charge()), Err("no stock"))
}
"#);
    run.passed();
    assert_eq!(run.journal.lines(), ["begin", "execute[items]", "abort"]);
}

/// A nested transaction is a second `handle`, and the innermost one answers.
#[test]
fn an_inner_rollback_is_answered_by_the_inner_scope() {
    let run = run(r#"
test/nondet "nested" {
  let out = transaction(|| {
    db.execute[items](1);
    let inner = transaction(|| {
      db.execute[items](2);
      db.rollback("inner");
      db.execute[items](3);
      0
    });
    db.execute[items](4);
    inner
  });
  assert_eq(out, Ok(Err("inner")))
}
"#);
    run.passed();
    assert_eq!(
        run.journal.lines(),
        [
            "begin",
            "execute[items]",
            "begin",
            "execute[items]",
            "abort",
            "execute[items]",
            "commit"
        ]
    );
}

/// The contrast, and the reason the tests above prove anything: in the same program shape, with the
/// same bound handlers, a clause that resumes **twice** after a host operation is refused.
#[test]
fn resuming_twice_over_the_same_boundary_is_still_refused() {
    let run = run(r#"
effect retry {
  read ask() -> Int
}

test/nondet "twice" {
  handle {
    db.begin();
    let n = retry.ask();
    db.execute[items](n)
  } with {
    retry.ask() resume k -> k(1) + k(2)
  }
}
"#);
    run.refused(codes::HOST_CONTINUATION_RESUMED);
    assert_eq!(
        run.journal.count("begin"),
        1,
        "`BEGIN` was issued exactly once, and the replay was refused before a second"
    );
}

/// `db.begin` is `AtMostOnce`, so a continuation captured *before* a transaction opened cannot be
/// resumed a second time.
#[test]
fn a_continuation_captured_before_begin_cannot_be_replayed_over_it() {
    let run = run(r#"
effect retry {
  read ask() -> Int
}

test/nondet "replayed" {
  handle {
    let n = retry.ask();
    transaction(|| db.execute[items](n))
  } with {
    retry.ask() resume k -> { k(1); k(2) }
  }
}
"#);
    run.refused(codes::HOST_CONTINUATION_RESUMED);
    assert_eq!(run.journal.count("begin"), 1);
}

/// Outside a scheduler region the identity is `None`, and that is one identity rather than an
/// absence of one: the entry point is a single thread of control and a scope it opened belongs to
/// it.
#[test]
fn an_operation_outside_a_region_carries_the_entry_point_as_its_performer() {
    let source = format!(
        "{DB}{}",
        r#"
test/nondet "alone" {
  assert_eq(transaction(|| db.execute[items](1)), Ok(0))
}
"#
    );
    let compiled = Compiled::named("t", &source);
    let performers = Arc::new(Performers::default());
    let binding = registry(performers.clone() as Arc<dyn HostHandler>, false)
        .bind(&compiled.check)
        .expect("the registry binds");
    let mut machine = Machine::new(&compiled.program, &compiled.resolved, &compiled.check);
    machine.set_host_binding(Arc::new(binding));
    machine.eval_test(0).expect("it passes");

    let seen = performers.seen.lock().expect("no panic held the lock");
    assert_eq!(seen.len(), 3, "begin, execute, commit");
    assert!(
        seen.iter().all(|(_, task)| task.is_none()),
        "no region was opened, so every operation is the entry point's: {seen:?}"
    );
}

/// Inside one, it is the task that ran the statement — which is what lets a driver refuse a
/// `commit` performed by a task that does not own the scope.
#[test]
fn an_operation_inside_a_region_carries_the_task_that_performed_it() {
    let source = format!(
        "{DB}{}",
        r#"
test/nondet "spawned" {
  let a = task.spawn(|| db.execute[items](1));
  task.join(a);
  ()
}
"#
    );
    let compiled = Compiled::named("t", &source);
    let performers = Arc::new(Performers::default());
    let binding = registry(performers.clone() as Arc<dyn HostHandler>, true)
        .bind(&compiled.check)
        .expect("the registry binds");
    let mut machine = Machine::new(&compiled.program, &compiled.resolved, &compiled.check);
    machine.set_host_binding(Arc::new(binding));
    machine.eval_test(0).expect("it passes");

    let seen = performers.seen.lock().expect("no panic held the lock");
    let executed = seen
        .iter()
        .find(|(name, _)| name == "execute")
        .expect("the spawned task ran its statement");
    assert!(
        executed.1.is_some(),
        "a statement performed inside a region names the task that performed it: {seen:?}"
    );
}

fn ends_after(body: &str, budget: Option<usize>) -> (Arc<Bookkeeper>, Result<(), Diagnostic>) {
    let source = format!("{DB}{body}");
    let compiled = Compiled::named("t", &source);
    let journal = Arc::new(Journal::default());
    let binding = registry(journal as Arc<dyn HostHandler>, false)
        .bind(&compiled.check)
        .expect("the registry binds");
    let mut machine = Machine::new(&compiled.program, &compiled.resolved, &compiled.check);
    if let Some(calls) = budget {
        machine = machine.with_max_calls(calls);
    }
    machine.set_host_binding(Arc::new(binding));
    let bookkeeper = Arc::new(Bookkeeper::default());
    machine.set_host_runtime(std::rc::Rc::new(BookkeeperHandle(bookkeeper.clone())));
    let outcome = machine.eval_test(0);
    (bookkeeper, outcome)
}

/// `Rc<dyn HostRuntime>` is what the machine takes and `Arc` is what a test keeps a handle through,
/// so the two are bridged rather than the counter being made thread-local.
struct BookkeeperHandle(Arc<Bookkeeper>);

impl HostRuntime for BookkeeperHandle {
    fn poll(&self, pending: &Pending) -> Result<Option<Value>, Diagnostic> {
        self.0.poll(pending)
    }

    fn park(&self) -> Result<(), Diagnostic> {
        self.0.park()
    }

    fn block_on(&self, pending: Pending) -> Result<Value, Diagnostic> {
        self.0.block_on(pending)
    }

    fn end_entry_point(&self, machine: MachineId) -> Result<(), Diagnostic> {
        self.0.end_entry_point(machine)
    }
}

/// The identity the hook carries, which is what stops one entry point's teardown from ending
/// another's transaction.
#[test]
fn a_teardown_names_the_machine_whose_entry_point_ended() {
    let (first, _) = ends_after(
        r#"
test/nondet "one" {
  transaction(|| db.execute[items](1));
  ()
}
"#,
        None,
    );
    let (second, _) = ends_after(
        r#"
test/nondet "two" {
  transaction(|| db.execute[items](1));
  ()
}
"#,
        None,
    );
    let one = first.machine().expect("the teardown named a machine");
    let two = second.machine().expect("the teardown named a machine");
    assert_ne!(
        one, two,
        "two machines share an identity, so one entry point's teardown would end the other's scope"
    );
}

#[test]
fn an_entry_point_that_returned_a_value_ends_once() {
    let (book, outcome) = ends_after(
        r#"
test/nondet "value" {
  transaction(|| db.execute[items](1));
  ()
}
"#,
        None,
    );
    outcome.expect("it passes");
    assert_eq!(book.ends(), 1);
}

/// The path that needs the hook.
#[test]
fn an_entry_point_that_raised_inside_a_transaction_still_ends() {
    let (book, outcome) = ends_after(
        r#"
test/nondet "raised" {
  transaction(|| {
    db.execute[items](1);
    assert(1 == 2);
    0
  });
  ()
}
"#,
        None,
    );
    let failure = outcome.expect_err("the assertion inside the body fails the test");
    assert_eq!(failure.code, codes::ASSERTION_FAILED);
    assert_eq!(
        book.ends(),
        1,
        "the scope the raise left open is the whole reason this hook exists"
    );
}

/// And the budget path, which is neither a value nor a diagnostic the program wrote: a run that
/// spent its call budget has left whatever it was holding.
#[test]
fn an_entry_point_that_spent_its_budget_still_ends() {
    let (book, outcome) = ends_after(
        r#"
fn forever(n: Int) -> Int / {db.write[items]} = {
  db.execute[items](n);
  forever(n + 1)
}

test/nondet "budget" {
  transaction(|| forever(0));
  ()
}
"#,
        Some(8),
    );
    outcome.expect_err("the budget is spent");
    assert_eq!(book.ends(), 1);
}

/// A failure while closing is the *run's* fault and not the program's: the program asked for
/// nothing and did nothing wrong, and attributing a discarded connection to whichever test was
/// running would send a reader looking for a defect in their own program.
#[test]
fn a_teardown_failure_is_a_warning_and_not_the_entry_points_verdict() {
    let source = format!(
        "{DB}{}",
        r#"
test/nondet "value" {
  transaction(|| db.execute[items](1));
  ()
}
"#
    );
    let compiled = Compiled::named("t", &source);
    let journal = Arc::new(Journal::default());
    let binding = registry(journal as Arc<dyn HostHandler>, false)
        .bind(&compiled.check)
        .expect("the registry binds");
    let mut machine = Machine::new(&compiled.program, &compiled.resolved, &compiled.check);
    machine.set_host_binding(Arc::new(binding));
    let bookkeeper = Arc::new(Bookkeeper {
        ends: AtomicU64::new(0),
        fails: true,
        machines: Mutex::new(None),
    });
    machine.set_host_runtime(std::rc::Rc::new(BookkeeperHandle(bookkeeper.clone())));

    machine.eval_test(0).expect("the test still passes");
    assert_eq!(bookkeeper.ends(), 1);
    let warnings = machine.take_teardown_warnings();
    assert_eq!(warnings.len(), 1);
    assert!(
        warnings[0].message.contains("discarded"),
        "the warning names what was lost"
    );
    assert!(
        machine.take_teardown_warnings().is_empty(),
        "and it is reported once rather than again after every later test"
    );
}
