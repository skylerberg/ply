use ply_core::host::{
    Determinism, Grants, HostAnswer, HostBinding, HostHandler, HostOp, HostRegistry, HostRequest,
    HostResource, HostRuntime, Linearity, MachineId, Pending,
};
use ply_core::sched::*;
use ply_core::sim::TaskId;
use ply_core::{Diagnostic, Resource, SimId, Span, Symbol, TaskHandle, Value, codes};
use std::sync::Arc;

type Sched = Scheduler<usize, Value, Value>;
type Choice = Turn<usize, Value, Value>;

/// No scheduler decision looks inside a continuation, so a bare id stands in for a suspended task.
fn suspended() -> usize {
    0
}

/// What a task hears, as the release's code reads it: an answer the scheduler gives itself or a
/// task's answer as it is, and `Some`, `None` and a select's pair around what a task passed.
fn as_heard(reply: &Reply<Value>) -> Value {
    let some = |v: Value| Value::ctor("Some", vec![v]);
    let none = || Value::ctor("None", Vec::new());
    match reply {
        Reply::Value(v) | Reply::Item(v) => v.clone(),
        Reply::Maybe(m) => m.clone().map_or_else(none, some),
        Reply::Chosen(None) => none(),
        Reply::Chosen(Some((at, got))) => some(Value::Record(Arc::new(
            ply_core::Fields::from_unsorted(vec![
                (Symbol::new("_0"), Value::Int(*at as i64)),
                (Symbol::new("_1"), got.clone().map_or_else(none, some)),
            ]),
        ))),
    }
}

/// A `Turn` has no `Debug`, so an expected refusal is unwrapped here rather than by `expect_err`.
fn refused(turn: Result<Choice, Diagnostic>, why: &str) -> Diagnostic {
    match turn {
        Ok(_) => panic!("the scheduler handed out a task: {why}"),
        Err(diagnostic) => diagnostic,
    }
}

/// A binding that serves something; these tests need only that it is bound.
fn binding() -> HostBinding {
    let mut registry = HostRegistry::new();
    registry.register(
        HostOp {
            effect: Symbol::new("db"),
            op: Symbol::new("get"),
            resource: HostResource::Only(Resource::Named(Symbol::new("users"))),
            determinism: Determinism::Nondeterministic,
            linearity: Linearity::AtMostOnce,
            blocking: false,
            secrets: false,
            path: "test::handler",
        },
        Arc::new(Never),
    );
    registry
        .granted(Grants::default())
        .expect("the fixture binds")
}

struct Never;

impl HostHandler for Never {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        Err(
            Diagnostic::error(codes::INTERNAL_ERROR, "the test handler was called")
                .primary(req.span, "here"),
        )
    }
}

/// Owns no token, so it fails loudly if a task ever waits.
struct Idle;

impl HostRuntime for Idle {
    fn watch(&self, pending: &Pending) -> Result<(), Diagnostic> {
        Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("the idle runtime was asked to watch `{pending}`"),
        ))
    }

    fn resolved(&self) -> Vec<(u64, Result<Value, Diagnostic>)> {
        Vec::new()
    }

    fn park(&self) -> Result<(), Diagnostic> {
        Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            "the idle runtime was asked to wait",
        ))
    }

    fn block_on(&self, _: Pending) -> Result<Value, Diagnostic> {
        Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            "the idle runtime was asked to wait",
        ))
    }
}

fn production() -> Sched {
    let binding = binding();
    let permit = HostPolicy::of(&binding).expect("a bound binding mints a permit");
    Scheduler::production(SimId(0), Span::DUMMY, permit, MachineId::next())
}

#[test]
fn an_unbound_binding_mints_no_permit() {
    assert!(HostPolicy::of(&HostBinding::unbound()).is_none());
    assert!(HostPolicy::of(&HostBinding::default()).is_none());
    assert!(HostPolicy::of(&binding()).is_some());
}

#[test]
fn the_production_scheduler_starves_nobody() {
    let mut sched = production();
    let Turn::Run { task, .. } = sched.next_host(&Idle).expect("the root is enabled") else {
        panic!("expected the root's step");
    };
    assert_eq!(task, ROOT);
    sched.spawn(Value::Unit, Span::DUMMY);
    sched.spawn(Value::Unit, Span::DUMMY);
    sched.suspend(suspended(), Value::Unit).expect("running");

    let mut order = Vec::new();
    for _ in 0..9 {
        let Turn::Run { task, .. } = sched.next_host(&Idle).expect("three are ready") else {
            panic!("expected a step");
        };
        order.push(task.0);
        sched.suspend(suspended(), Value::Unit).expect("running");
    }
    assert_eq!(order, vec![1, 2, 0, 1, 2, 0, 1, 2, 0]);
}

/// The `task.*` that opened the region is still unanswered, so the root starts running.
#[test]
fn a_lazily_opened_region_roots_on_the_control_that_opened_it() {
    let mut sched = production().rooted_running().expect("nothing has run yet");
    assert_eq!(sched.current(), Some(ROOT));

    // Answered through the same path every later perform takes, so `spawn` means one thing.
    let child = sched.spawn(Value::Unit, Span::DUMMY);
    let id = child.id();
    sched
        .suspend(suspended(), Value::Task(child))
        .expect("the root is running");

    let Turn::Run { task, resumption } = sched.next_host(&Idle).expect("the root is enabled")
    else {
        panic!("expected a step");
    };
    assert_eq!(task, ROOT);
    assert!(
        matches!(&resumption, Resumption::Resume { value: Reply::Value(Value::Task(handle)), .. } if handle.id() == id),
        "a lazily-opened root resumes with the answer, never evaluates a body it does not have"
    );

    let err = sched
        .rooted_running()
        .err()
        .expect("the region has already begun");
    assert_eq!(err.code, codes::INTERNAL_ERROR);
}

/// A runtime that is stopping and has nothing outstanding.
struct Stopping {
    parks: std::cell::Cell<u32>,
    expire_after: u32,
}

impl HostRuntime for Stopping {
    fn watch(&self, pending: &Pending) -> Result<(), Diagnostic> {
        Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("the stopping runtime was asked to watch `{pending}`"),
        ))
    }

    fn resolved(&self) -> Vec<(u64, Result<Value, Diagnostic>)> {
        Vec::new()
    }

    fn park(&self) -> Result<(), Diagnostic> {
        self.parks.set(self.parks.get() + 1);
        Ok(())
    }

    fn block_on(&self, _: Pending) -> Result<Value, Diagnostic> {
        Err(Diagnostic::error(codes::INTERNAL_ERROR, "not reached"))
    }

    fn stopping(&self) -> bool {
        true
    }

    fn drain_expired(&self) -> Option<Diagnostic> {
        (self.parks.get() >= self.expire_after)
            .then(|| Diagnostic::warning(codes::DRAIN_INCOMPLETE, "the drain deadline expired"))
    }
}

/// Two tasks each waiting on the other, with no host wait and no clock.
fn deadlock(sched: &mut Sched) {
    let other = sched.spawn(Value::Unit, Span::DUMMY);
    sched
        .join(suspended(), &other, Span::DUMMY)
        .expect("the root is running");
    let Turn::Run { task, .. } = sched.next_host(&Idle).expect("the spawned task is enabled")
    else {
        panic!("expected the spawned task's first step");
    };
    assert_eq!(task, other.id());
    sched
        .join(
            suspended(),
            &TaskHandle::unowned(SimId(0), ROOT),
            Span::DUMMY,
        )
        .expect("the spawned task is running");
}

#[test]
fn a_stopping_region_with_nothing_outstanding_drains_rather_than_deadlocking() {
    let mut sched = production();
    let Turn::Run { .. } = sched
        .next_host(&Stopping {
            parks: std::cell::Cell::new(0),
            expire_after: 3,
        })
        .expect("the root is enabled")
    else {
        panic!("expected the root's step");
    };
    // Nothing enabled and nothing waiting on the host: `err_host_deadlock`'s exact condition.
    deadlock(&mut sched);

    let runtime = Stopping {
        parks: std::cell::Cell::new(0),
        expire_after: 3,
    };
    let err = refused(
        sched.next_host(&runtime),
        "the drain deadline ends the region",
    );
    assert_eq!(
        err.code,
        codes::DRAIN_INCOMPLETE,
        "a stopping region ends on its deadline, not on `E0414`: {}",
        err.message
    );
    assert!(
        runtime.parks.get() >= 3,
        "the scheduler parked {} times before the deadline, so it never waited",
        runtime.parks.get()
    );
}

#[test]
fn a_park_that_woke_on_a_stop_is_not_counted_as_fruitless() {
    let mut sched = production();
    let Turn::Run { .. } = sched
        .next_host(&Stopping {
            parks: std::cell::Cell::new(0),
            expire_after: u32::MAX,
        })
        .expect("the root is enabled")
    else {
        panic!("expected the root's step");
    };
    deadlock(&mut sched);

    let runtime = Stopping {
        parks: std::cell::Cell::new(0),
        expire_after: FRUITLESS_PARKS + 8,
    };
    let err = refused(sched.next_host(&runtime), "the drain deadline ends it");
    assert_eq!(
        err.code,
        codes::DRAIN_INCOMPLETE,
        "{} parks past the fruitless bound reported `{}` instead",
        runtime.parks.get(),
        err.code
    );
    assert!(runtime.parks.get() > FRUITLESS_PARKS);
}

/// So the stopping exemption is not a hole.
#[test]
fn a_region_that_is_not_stopping_still_deadlocks() {
    let mut sched = production();
    let Turn::Run { .. } = sched.next_host(&Idle).expect("the root is enabled") else {
        panic!("expected the root's step");
    };
    deadlock(&mut sched);
    let err = refused(sched.next_host(&Idle), "nothing can make progress");
    assert_eq!(err.code, codes::DEADLOCK);
}

/// Finishes each task the region hands out until it hands out the root, and answers how the root
/// resumes.
fn until_root(sched: &mut Sched, rt: &dyn HostRuntime) -> Resumption<usize, Value, Value> {
    loop {
        let Turn::Run { task, resumption } = sched.next_host(rt).expect("a task is enabled") else {
            panic!("the region ended while its root was still running");
        };
        if task == ROOT {
            return resumption;
        }
        sched
            .finish(Value::Int(task.0 as i64))
            .expect("the task is running");
    }
}

fn root_step(sched: &mut Sched, rt: &dyn HostRuntime) {
    let Turn::Run { task, .. } = sched.next_host(rt).expect("the root is enabled") else {
        panic!("expected the root's step");
    };
    assert_eq!(task, ROOT);
}

#[test]
fn a_production_region_keeps_only_the_tasks_that_can_still_run_or_be_joined() {
    let mut sched = production();
    root_step(&mut sched, &Idle);
    for round in 0..200 {
        drop(sched.spawn(Value::Unit, Span::DUMMY));
        let joined = sched.spawn(Value::Unit, Span::DUMMY);
        let target = joined.id();
        sched
            .suspend(suspended(), Value::Unit)
            .expect("the root is running");
        until_root(&mut sched, &Idle);
        sched
            .join(suspended(), &joined, Span::DUMMY)
            .expect("a task whose handle is held is kept");
        drop(joined);
        let Resumption::Resume { value, .. } = until_root(&mut sched, &Idle) else {
            panic!("the root resumes from its join");
        };
        assert_eq!(value, Reply::Item(Value::Int(target.0 as i64)));
        assert_eq!(
            sched.tasks(),
            1,
            "round {round} left a finished task behind"
        );
    }
}

/// Host state is keyed by task, so a reused id would hand a new task a retired one's state.
#[test]
fn a_retired_tasks_id_is_never_handed_out_again() {
    let mut sched = production();
    root_step(&mut sched, &Idle);
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..100 {
        let handle = sched.spawn(Value::Unit, Span::DUMMY);
        assert!(
            seen.insert(handle.id()),
            "{} was handed out twice",
            handle.id()
        );
        drop(handle);
        sched
            .suspend(suspended(), Value::Unit)
            .expect("the root is running");
        until_root(&mut sched, &Idle);
        assert_eq!(sched.tasks(), 1, "the task retired before the next spawn");
    }
}

/// A select that does not wait answers at once; one that waits to send is woken by the receive of
/// the task it spawned, and both hear what went.
#[test]
fn a_select_waits_and_is_woken_by_the_task_that_takes_what_it_sends() {
    let mut sched = production();
    root_step(&mut sched, &Idle);
    sched.channel(suspended(), 0).expect("the root is running");
    let Resumption::Resume {
        value: Reply::Value(Value::Chan(meet)),
        ..
    } = until_root(&mut sched, &Idle)
    else {
        panic!("the root resumes from making a channel");
    };
    sched
        .select(suspended(), vec![(meet, None)], false, Span::DUMMY)
        .expect("the root is running");
    let Resumption::Resume { value, .. } = until_root(&mut sched, &Idle) else {
        panic!("the root resumes from a select that does not wait");
    };
    assert_eq!(value, Reply::Chosen(None));

    let receiver = sched.spawn(Value::Unit, Span::DUMMY);
    sched
        .select(
            suspended(),
            vec![(meet, Some(Value::Int(5)))],
            true,
            Span::DUMMY,
        )
        .expect("the root is running");
    let Turn::Run { task, .. } = sched.next_host(&Idle).expect("the receiver is enabled") else {
        panic!("the region ended with its root waiting");
    };
    assert_eq!(task, receiver.id());
    sched
        .recv(suspended(), &meet, Span::DUMMY)
        .expect("the receiver is running");
    let mut heard = Vec::new();
    for _ in 0..2 {
        let Turn::Run { task, resumption } = sched.next_host(&Idle).expect("a task is enabled")
        else {
            panic!("the region ended before both had heard");
        };
        let Resumption::Resume { value, .. } = resumption else {
            panic!("{task} was not answered");
        };
        heard.push((task == ROOT, value));
        sched
            .suspend(suspended(), Value::Unit)
            .expect("the task is running");
    }
    heard.sort_by_key(|(root, _)| *root);
    assert_eq!(
        heard[0].1,
        Reply::Maybe(Some(Value::Int(5))),
        "the receiver takes what was sent"
    );
    assert_eq!(heard[1].1, Reply::Chosen(Some((0, Some(Value::Int(5))))));
}

/// A task cancelled before its join and one cancelled under it fail the join alike: as a raise in
/// the joiner, whose own handlers answer it, and never as the region's failure.
#[test]
fn joining_a_task_already_cancelled_resumes_the_joiner_with_a_raise() {
    let mut sched = production();
    root_step(&mut sched, &Idle);
    let target = sched.spawn(Value::Unit, Span::DUMMY);
    let stopped = sched
        .cancel(suspended(), &target, Span::DUMMY)
        .expect("the root is running");
    assert_eq!(
        stopped.unstarted,
        Some(Value::Unit),
        "the task never started"
    );
    let Resumption::Resume { value, .. } = until_root(&mut sched, &Idle) else {
        panic!("the root resumes from its cancel");
    };
    assert_eq!(value, Reply::Value(Value::Bool(true)));
    sched
        .join(suspended(), &target, Span::DUMMY)
        .expect("a join of a cancelled task is the joiner's to answer");
    let Resumption::Raise { failure, .. } = until_root(&mut sched, &Idle) else {
        panic!("the join of a cancelled task did not raise in the joiner");
    };
    assert!(
        failure.message.contains("was cancelled"),
        "{}",
        failure.message
    );
}

#[test]
fn a_kept_handle_keeps_its_finished_task_joinable_and_every_join_answers() {
    let mut sched = production();
    root_step(&mut sched, &Idle);
    let kept: Vec<TaskHandle> = (0..8)
        .map(|_| sched.spawn(Value::Unit, Span::DUMMY))
        .collect();
    for _ in 0..4 {
        sched
            .suspend(suspended(), Value::Unit)
            .expect("the root is running");
        until_root(&mut sched, &Idle);
    }
    assert_eq!(
        sched.tasks(),
        9,
        "a finished task whose handle is held stays"
    );
    for handle in &kept {
        for join in 0..2 {
            sched
                .join(suspended(), handle, Span::DUMMY)
                .expect("the task is kept");
            let Resumption::Resume { value, .. } = until_root(&mut sched, &Idle) else {
                panic!("the root resumes from its join");
            };
            assert_eq!(
                value,
                Reply::Item(Value::Int(handle.id().0 as i64)),
                "join {join} of {}",
                handle.id()
            );
        }
    }
    drop(kept);
    sched
        .suspend(suspended(), Value::Unit)
        .expect("the root is running");
    until_root(&mut sched, &Idle);
    assert_eq!(
        sched.tasks(),
        1,
        "the last handles going retired their tasks"
    );
}

/// Records each task the region retired, and the machine it ran on.
#[derive(Default)]
struct Ends {
    ended: std::cell::RefCell<Vec<(MachineId, TaskId)>>,
}

impl HostRuntime for Ends {
    fn watch(&self, pending: &Pending) -> Result<(), Diagnostic> {
        Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("nothing here parks, yet `{pending}` was watched"),
        ))
    }

    fn resolved(&self) -> Vec<(u64, Result<Value, Diagnostic>)> {
        Vec::new()
    }

    fn park(&self) -> Result<(), Diagnostic> {
        Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            "nothing here waits",
        ))
    }

    fn block_on(&self, _: Pending) -> Result<Value, Diagnostic> {
        Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            "nothing here waits",
        ))
    }

    fn end_task(&self, machine: MachineId, task: TaskId) {
        self.ended.borrow_mut().push((machine, task));
    }
}

#[test]
fn retiring_a_task_ends_its_host_state_once_and_a_held_task_is_not_retired() {
    let machine = MachineId::next();
    let permit = HostPolicy::of(&binding()).expect("a bound binding mints a permit");
    let mut sched: Sched = Scheduler::production(SimId(0), Span::DUMMY, permit, machine);
    let rt = Ends::default();
    root_step(&mut sched, &rt);
    let forgotten = sched.spawn(Value::Unit, Span::DUMMY).id();
    let kept = sched.spawn(Value::Unit, Span::DUMMY);
    sched
        .suspend(suspended(), Value::Unit)
        .expect("the root is running");
    until_root(&mut sched, &rt);
    assert_eq!(*rt.ended.borrow(), vec![(machine, forgotten)]);

    let held = kept.id();
    drop(kept);
    for _ in 0..2 {
        sched
            .suspend(suspended(), Value::Unit)
            .expect("the root is running");
        until_root(&mut sched, &rt);
    }
    assert_eq!(
        *rt.ended.borrow(),
        vec![(machine, forgotten), (machine, held)]
    );
}

#[test]
fn a_region_hands_back_the_bodies_of_the_tasks_it_never_started() {
    let mut sched = production();
    root_step(&mut sched, &Idle);
    let started = sched.spawn(Value::Int(1), Span::DUMMY);
    let _never = sched.spawn(Value::Int(2), Span::DUMMY);
    sched
        .suspend(suspended(), Value::Unit)
        .expect("the root is running");
    let Turn::Run {
        task,
        resumption: Resumption::Start { .. },
    } = sched.next_host(&Idle).expect("a spawned task is enabled")
    else {
        panic!("the first spawned task starts");
    };
    assert_eq!(task, started.id());
    sched
        .suspend(suspended(), Value::Unit)
        .expect("the started task is running");
    assert_eq!(sched.unstarted(), vec![Value::Int(2)]);
    assert!(sched.unstarted().is_empty(), "a body is handed back once");
}

/// Resolves every watched token at the next look, newest first, beside a token nothing here
/// parked on, as one a region of this machine that already ended left behind.
#[derive(Default)]
struct Backwards {
    watched: std::cell::RefCell<Vec<u64>>,
}

const STALE: u64 = 999;

impl HostRuntime for Backwards {
    fn watch(&self, pending: &Pending) -> Result<(), Diagnostic> {
        self.watched.borrow_mut().push(pending.token);
        Ok(())
    }

    fn resolved(&self) -> Vec<(u64, Result<Value, Diagnostic>)> {
        let mut resolved: Vec<(u64, Result<Value, Diagnostic>)> = self
            .watched
            .take()
            .into_iter()
            .rev()
            .map(|token| (token, Ok(Value::Int(token as i64 * 10))))
            .collect();
        resolved.push((STALE, Ok(Value::Int(-1))));
        resolved
    }

    fn park(&self) -> Result<(), Diagnostic> {
        Ok(())
    }

    fn block_on(&self, _: Pending) -> Result<Value, Diagnostic> {
        Err(Diagnostic::error(codes::INTERNAL_ERROR, "a task parks"))
    }
}

#[test]
fn each_parked_task_wakes_with_what_its_own_token_resolved_to() {
    let rt = Backwards::default();
    let mut sched = production();
    root_step(&mut sched, &rt);
    let children: Vec<TaskHandle> = (0..3)
        .map(|_| sched.spawn(Value::Unit, Span::DUMMY))
        .collect();
    let parked = |sched: &mut Sched, token: u64| {
        sched
            .park_on_host(
                suspended(),
                Pending {
                    token,
                    label: "test",
                },
                Span::DUMMY,
                &rt,
            )
            .expect("the task is running")
    };
    parked(&mut sched, 100);
    for _ in &children {
        let Turn::Run { task, .. } = sched.next_host(&rt).expect("a child starts") else {
            panic!("expected a child's first step");
        };
        parked(&mut sched, 200 + task.0);
    }

    let mut woke = std::collections::BTreeMap::new();
    loop {
        match sched.next_host(&rt).expect("every task wakes") {
            Turn::Complete(_) => break,
            Turn::Run {
                task,
                resumption: Resumption::Resume { value, .. },
            } => {
                woke.insert(task, as_heard(&value));
                sched.finish(Value::Unit).expect("the task is running");
            }
            Turn::Run { task, .. } => panic!("{task} ran without its token resolving"),
        }
    }
    let mut expected = std::collections::BTreeMap::from([(ROOT, Value::Int(1000))]);
    for child in &children {
        let id = child.id();
        expected.insert(id, Value::Int((200 + id.0 as i64) * 10));
    }
    assert_eq!(woke, expected);
}

#[test]
fn a_task_cannot_park_on_a_token_its_runtime_did_not_mint() {
    let mut sched = production();
    root_step(&mut sched, &Idle);
    let err = sched
        .park_on_host(
            suspended(),
            Pending {
                token: 7,
                label: "stray",
            },
            Span::DUMMY,
            &Idle,
        )
        .expect_err("the idle runtime mints nothing");
    assert_eq!(err.code, codes::INTERNAL_ERROR);
    assert_eq!(
        sched.current(),
        Some(ROOT),
        "the refused task was parked anyway"
    );
}

/// A runtime whose clock a test reads and moves. A park on a deadline moves it there, as the
/// host's would wait.
#[derive(Default)]
struct Ticking {
    now: std::cell::Cell<i64>,
    /// Each deadline the region parked until, in order.
    parked_until: std::cell::RefCell<Vec<i64>>,
}

impl HostRuntime for Ticking {
    fn watch(&self, pending: &Pending) -> Result<(), Diagnostic> {
        Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("nothing here parks on a token, yet `{pending}` was watched"),
        ))
    }

    fn resolved(&self) -> Vec<(u64, Result<Value, Diagnostic>)> {
        Vec::new()
    }

    fn park(&self) -> Result<(), Diagnostic> {
        Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            "a region with a sleeper was parked on no deadline",
        ))
    }

    fn block_on(&self, _: Pending) -> Result<Value, Diagnostic> {
        Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            "nothing here waits",
        ))
    }

    fn now(&self) -> Result<i64, Diagnostic> {
        Ok(self.now.get())
    }

    fn park_until(&self, deadline: i64) -> Result<(), Diagnostic> {
        self.parked_until.borrow_mut().push(deadline);
        self.now.set(self.now.get().max(deadline));
        Ok(())
    }
}

fn sleeps(sched: &mut Sched, nanos: i64, rt: &dyn HostRuntime) {
    sched
        .sleep_on_host(suspended(), nanos, Span::DUMMY, rt)
        .expect("the task is running");
}

fn next_task(sched: &mut Sched, rt: &dyn HostRuntime) -> (TaskId, Resumption<usize, Value, Value>) {
    match sched.next_host(rt) {
        Ok(Turn::Run { task, resumption }) => (task, resumption),
        Ok(Turn::Complete(_)) => panic!("the region ended with a task still to run"),
        Err(d) => panic!("no task could run: {}", d.message),
    }
}

#[test]
fn a_sleeping_production_task_is_parked_alone_and_the_others_run() {
    let rt = Ticking::default();
    let mut sched = production();
    root_step(&mut sched, &rt);
    let beside = sched.spawn(Value::Unit, Span::DUMMY);
    sleeps(&mut sched, 100, &rt);
    for _ in 0..5 {
        let (task, _) = next_task(&mut sched, &rt);
        assert_eq!(task, beside.id(), "the sleeper ran before its deadline");
        sched.suspend(suspended(), Value::Unit).expect("running");
    }

    // The deadline comes while the other task is still runnable, and the sleeper has the next turn.
    rt.now.set(100);
    let (task, resumption) = next_task(&mut sched, &rt);
    assert_eq!(task, ROOT);
    assert!(
        matches!(
            resumption,
            Resumption::Resume {
                value: Reply::Value(Value::Unit),
                ..
            }
        ),
        "a sleep answers `Unit`"
    );
    assert!(
        rt.parked_until.borrow().is_empty(),
        "the thread waited while a task could run: {:?}",
        rt.parked_until.borrow()
    );
}

#[test]
fn with_nothing_runnable_a_production_region_waits_for_its_earliest_deadline() {
    let rt = Ticking::default();
    let mut sched = production();
    root_step(&mut sched, &rt);
    let late = sched.spawn(Value::Unit, Span::DUMMY);
    let early = sched.spawn(Value::Unit, Span::DUMMY);
    sleeps(&mut sched, 300, &rt);
    assert_eq!(next_task(&mut sched, &rt).0, late.id());
    sleeps(&mut sched, 200, &rt);
    assert_eq!(next_task(&mut sched, &rt).0, early.id());
    sleeps(&mut sched, 100, &rt);

    let mut woke = Vec::new();
    loop {
        match sched.next_host(&rt).expect("each sleeper wakes") {
            Turn::Complete(_) => break,
            Turn::Run { task, .. } => {
                woke.push((task, rt.now.get()));
                sched.finish(Value::Unit).expect("the task is running");
            }
        }
    }
    assert_eq!(woke, vec![(early.id(), 100), (late.id(), 200), (ROOT, 300)]);
    assert_eq!(*rt.parked_until.borrow(), vec![100, 200, 300]);
}

#[test]
fn cancelling_a_sleeping_production_task_lets_go_of_its_deadline() {
    let rt = Ticking::default();
    let mut sched = production();
    root_step(&mut sched, &rt);
    let sleeper = sched.spawn(Value::Unit, Span::DUMMY);
    sched
        .suspend(suspended(), Value::Unit)
        .expect("the root is running");
    assert_eq!(next_task(&mut sched, &rt).0, sleeper.id());
    sleeps(&mut sched, 1_000, &rt);

    assert_eq!(next_task(&mut sched, &rt).0, ROOT);
    let stopped = sched
        .cancel(suspended(), &sleeper, Span::DUMMY)
        .expect("the root is running");
    assert!(stopped.stopped);

    let (task, resumption) = next_task(&mut sched, &rt);
    assert_eq!(task, sleeper.id());
    assert!(
        matches!(resumption, Resumption::Cancel { .. }),
        "a cancelled sleeper only unwinds"
    );
    sched.finish_cancelled().expect("the sleeper is running");

    // The root sleeps past where the cancelled task would have woken.
    assert_eq!(next_task(&mut sched, &rt).0, ROOT);
    sleeps(&mut sched, 2_000, &rt);
    assert_eq!(next_task(&mut sched, &rt).0, ROOT);
    assert_eq!(
        *rt.parked_until.borrow(),
        vec![2_000],
        "the region waited on a deadline nothing sleeps until"
    );
}

#[test]
fn a_cancel_inside_a_release_leaves_a_production_sleeper_asleep_until_its_deadline() {
    let rt = Ticking::default();
    let mut sched = production();
    root_step(&mut sched, &rt);
    let sleeper = sched.spawn(Value::Unit, Span::DUMMY);
    sched
        .suspend(suspended(), Value::Unit)
        .expect("the root is running");
    assert_eq!(next_task(&mut sched, &rt).0, sleeper.id());
    assert_eq!(sched.shield(Shield::Release), Some(sleeper.id()));
    sleeps(&mut sched, 500, &rt);

    assert_eq!(next_task(&mut sched, &rt).0, ROOT);
    let stopped = sched
        .cancel(suspended(), &sleeper, Span::DUMMY)
        .expect("the root is running");
    assert!(stopped.stopped);
    assert_eq!(next_task(&mut sched, &rt).0, ROOT);
    sched
        .await_task(suspended(), &sleeper, Span::DUMMY)
        .expect("the root is running");

    let (task, resumption) = next_task(&mut sched, &rt);
    assert_eq!(task, sleeper.id());
    assert!(
        matches!(resumption, Resumption::Resume { .. }),
        "the release's sleep was cut short"
    );
    assert_eq!(*rt.parked_until.borrow(), vec![500]);
    assert!(
        sched.unshield(sleeper.id(), Shield::Release),
        "the cancel lands when the release returns"
    );
    sched.finish_cancelled().expect("the sleeper is running");
    let (task, resumption) = next_task(&mut sched, &rt);
    assert_eq!(task, ROOT);
    assert!(
        matches!(
            &resumption,
            Resumption::Resume {
                value: Reply::Maybe(None),
                ..
            }
        ),
        "an await of a cancelled task hears `None`"
    );
}

#[test]
fn a_production_sleep_of_no_span_is_a_yield() {
    let rt = Ticking::default();
    let mut sched = production();
    root_step(&mut sched, &rt);
    let beside = sched.spawn(Value::Unit, Span::DUMMY);
    for nanos in [0, -5] {
        sleeps(&mut sched, nanos, &rt);
        assert_eq!(next_task(&mut sched, &rt).0, beside.id());
        sched.suspend(suspended(), Value::Unit).expect("running");
        assert_eq!(next_task(&mut sched, &rt).0, ROOT);
    }
    assert!(rt.parked_until.borrow().is_empty());
}

#[test]
fn a_production_sleep_needs_a_runtime_that_keeps_time() {
    let mut sched = production();
    root_step(&mut sched, &Idle);
    let err = sched
        .sleep_on_host(suspended(), 5, Span::DUMMY, &Idle)
        .expect_err("the idle runtime reads no clock");
    assert_eq!(err.code, codes::INTERNAL_ERROR);
    assert_eq!(
        sched.current(),
        Some(ROOT),
        "the refused task was put to sleep anyway"
    );
}
