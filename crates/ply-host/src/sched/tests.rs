//! The production scheduler, driven exactly as the machine drives it, against a
//! host runtime backed by real threads.
//!
//! The runtime here is a test double only in the sense that it computes integers
//! instead of reading sockets. The waiting is real: a job runs on a thread of its
//! own and the scheduler learns about it through the same `poll`/`park` pair a
//! socket handler would use. A starvation test against a fake that resolves
//! immediately would prove nothing, which is the whole reason it is not one.

use super::*;
use ply_eval::sched::{Policy, ROOT, Resumption, Turn};
use ply_eval::{Continuation, Env, HostRegistry, Pending, Prompt, Stack, TaskId, Value};
use ply_span::SourceId;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

/// A continuation is control, and none of the scheduler's decisions look inside
/// one, so a captured empty segment is a faithful stand-in for a suspended task.
fn suspended() -> Continuation {
    let prompt = std::rc::Rc::new(Prompt {
        clauses: std::rc::Rc::new(Vec::new()),
        effects: std::rc::Rc::new(Vec::new()),
        ret: None,
        env: Env::empty(),
        module: 0,
        span: Span::DUMMY,
    });
    Stack::new().push_prompt(prompt).capture(1, 0).0
}

// ---------------------------------------------------------------- the runtime

#[derive(Default)]
struct Slots {
    outstanding: BTreeSet<u64>,
    done: BTreeMap<u64, i64>,
}

/// One thread per outstanding operation, a `Condvar` to wake the machine's
/// thread, and no `Value` anywhere near either: a job produces an `i64` and the
/// polling thread — the machine's — builds the `Value`.
struct Threads {
    slots: Mutex<Slots>,
    finished: Condvar,
    next: Mutex<u64>,
}

impl Threads {
    fn new() -> Arc<Threads> {
        Arc::new(Threads {
            slots: Mutex::new(Slots::default()),
            finished: Condvar::new(),
            // Token 0 is never minted, so a zeroed `Pending` is a token nothing
            // owns rather than the first job.
            next: Mutex::new(1),
        })
    }

    /// Starts a job that answers `value` after `delay` and hands back the token
    /// the performing task parks on.
    fn submit(self: &Arc<Threads>, delay: Duration, value: i64) -> Pending {
        let token = {
            let mut next = lock(&self.next);
            let token = *next;
            *next += 1;
            token
        };
        lock(&self.slots).outstanding.insert(token);
        let shared = Arc::clone(self);
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            lock(&shared.slots).done.insert(token, value);
            shared.finished.notify_all();
        });
        Pending {
            token,
            label: "test-job",
        }
    }

    fn outstanding(&self) -> usize {
        lock(&self.slots).outstanding.len()
    }
}

impl HostRuntime for Threads {
    fn poll(&self, pending: &Pending) -> Result<Option<Value>, Diagnostic> {
        let mut slots = lock(&self.slots);
        match slots.done.remove(&pending.token) {
            Some(value) => {
                slots.outstanding.remove(&pending.token);
                Ok(Some(Value::Int(value)))
            }
            None if slots.outstanding.contains(&pending.token) => Ok(None),
            None => Err(Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!("nothing minted host token {}", pending.token),
            )),
        }
    }

    fn park(&self) -> Result<(), Diagnostic> {
        let mut slots = lock(&self.slots);
        if slots.outstanding.is_empty() {
            return Err(Diagnostic::error(
                codes::INTERNAL_ERROR,
                "the host runtime was asked to wait with no operation outstanding",
            ));
        }
        while slots.done.is_empty() {
            slots = match self.finished.wait(slots) {
                Ok(slots) => slots,
                Err(poisoned) => poisoned.into_inner(),
            };
        }
        Ok(())
    }

    fn block_on(&self, pending: Pending) -> Result<Value, Diagnostic> {
        loop {
            if let Some(value) = self.poll(&pending)? {
                return Ok(value);
            }
            self.park()?;
        }
    }
}

/// A runtime whose `park` returns without ever resolving anything, which is the
/// one failure a scheduler must not answer with a hot spin.
struct NeverResolves;

impl HostRuntime for NeverResolves {
    fn poll(&self, _: &Pending) -> Result<Option<Value>, Diagnostic> {
        Ok(None)
    }

    fn park(&self) -> Result<(), Diagnostic> {
        Ok(())
    }

    fn block_on(&self, _: Pending) -> Result<Value, Diagnostic> {
        Err(Diagnostic::error(codes::INTERNAL_ERROR, "nothing resolves"))
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ---------------------------------------------------------------- the harness

/// What a task does, in the order it does it. The harness plays these against
/// the scheduler exactly as the machine would: every action but `Mark` ends the
/// task's step.
#[derive(Clone)]
enum Act {
    Mark(&'static str),
    Yield,
    /// Spawn the script at this index.
    Spawn(usize),
    Join(u32),
    /// Park on a host operation that answers after this delay.
    Wait(Duration),
    Fail,
}

type Program = Vec<Vec<Act>>;

struct Run {
    /// `(task, mark)` in the order the marks were reached.
    marks: Vec<(u32, &'static str)>,
    steps: u32,
}

impl Run {
    fn of(&self, task: u32) -> Vec<&'static str> {
        self.marks
            .iter()
            .filter(|(t, _)| *t == task)
            .map(|(_, m)| *m)
            .collect()
    }

    fn position(&self, mark: &str) -> Option<usize> {
        self.marks.iter().position(|(_, m)| *m == mark)
    }
}

fn run(program: &Program, rt: &dyn HostRuntime) -> Result<Run, Diagnostic> {
    let mut sched = Scheduler::production(SimId(0), Span::DUMMY, permit());
    let mut marks = Vec::new();
    let mut steps = 0u32;
    // Which script each task runs, and how far into it that task has got.
    let mut script: Vec<usize> = vec![0];
    let mut pc: Vec<usize> = vec![0];

    loop {
        match sched.next_host(rt)? {
            Turn::Complete(_) => return Ok(Run { marks, steps }),
            Turn::Run { task, resumption } => {
                steps += 1;
                let at = task.0 as usize;
                if let Resumption::Start { body, .. } = &resumption {
                    let index = match body {
                        Value::Int(i) => *i as usize,
                        _ => return Err(broken("a spawned body is a script index")),
                    };
                    while script.len() <= at {
                        script.push(0);
                        pc.push(0);
                    }
                    script[at] = index;
                }
                loop {
                    let Some(act) = program[script[at]].get(pc[at]).cloned() else {
                        sched.finish(Value::Int(task.0 as i64))?;
                        break;
                    };
                    pc[at] += 1;
                    match act {
                        Act::Mark(m) => {
                            marks.push((task.0, m));
                            continue;
                        }
                        Act::Yield => sched.suspend(suspended(), Value::Unit)?,
                        Act::Spawn(index) => {
                            let id = sched.spawn(Value::Int(index as i64), Vec::new(), Span::DUMMY);
                            while script.len() <= id.0 as usize {
                                script.push(0);
                                pc.push(0);
                            }
                            sched.suspend(suspended(), Value::Task(id))?;
                        }
                        Act::Join(id) => sched.join(suspended(), TaskId(id), Span::DUMMY)?,
                        Act::Wait(delay) => {
                            let pending = jobs().submit(delay, task.0 as i64);
                            sched.park_on_host(suspended(), pending, Span::DUMMY)?;
                        }
                        Act::Fail => {
                            return Err(sched.fail(
                                Diagnostic::error(codes::RUNTIME_ERROR, "the task failed")
                                    .primary(Span::DUMMY, "here"),
                                &ply_eval::Seed::root(0),
                            ));
                        }
                    }
                    break;
                }
            }
        }
    }
}

// `Act::Wait` needs the same runtime the run is driven by, and threading one
// through every arm of the harness would say less than this does: there is one
// runtime per test, built by `with_jobs`.
thread_local! {
    static JOBS: std::cell::RefCell<Option<Arc<Threads>>> = const { std::cell::RefCell::new(None) };
}

fn jobs() -> Arc<Threads> {
    JOBS.with(|j| {
        j.borrow()
            .clone()
            .expect("a run that waits is driven under `with_jobs`")
    })
}

fn with_jobs<T>(f: impl FnOnce(&Arc<Threads>) -> T) -> T {
    let rt = Threads::new();
    JOBS.with(|j| *j.borrow_mut() = Some(Arc::clone(&rt)));
    let out = f(&rt);
    JOBS.with(|j| *j.borrow_mut() = None);
    out
}

fn broken(message: &str) -> Diagnostic {
    Diagnostic::error(codes::INTERNAL_ERROR, message.to_string())
}

/// A `Turn` and a `Scheduler` hold control, which has no `Debug` and wants none,
/// so an expected refusal is unwrapped here rather than through `expect_err`.
fn refused<T>(outcome: Result<T, Diagnostic>, why: &str) -> Diagnostic {
    match outcome {
        Ok(_) => panic!("the scheduler answered instead of refusing: {why}"),
        Err(diagnostic) => diagnostic,
    }
}

// --------------------------------------------------------------- the bindings

fn check(source: &str) -> ply_core::CheckOutput {
    let module = ply_syntax::parse(SourceId(0), source).expect("the fixture parses");
    ply_core::check_module(&module).expect("the fixture typechecks")
}

/// A program whose footprint contains `task.write`, so the `Any` registrations
/// have an atom to resolve against.
const SPAWNS: &str = r#"
fn spin() -> Unit / {task.write} = task.yield()
"#;

fn registry() -> HostRegistry {
    let mut registry = HostRegistry::new();
    for (op, handler) in registrations() {
        registry.register(op, handler);
    }
    registry
}

fn bound() -> HostBinding {
    registry()
        .bind(&check(SPAWNS))
        .expect("the task registrations resolve against a program that spawns")
}

fn permit() -> HostPolicy {
    HostPolicy::of(&bound()).expect("a bound binding mints a permit")
}

// ----------------------------------------------------------------- the claims

/// The exclusion that matters most, and the one a convention would get wrong: a
/// hermetic run cannot *build* a production scheduler, so a test cannot acquire
/// real threads by accident however it is written.
#[test]
fn a_hermetic_binding_mints_no_permit_and_opens_no_region() {
    let hermetic = HostBinding::hermetic_with(registry());
    assert!(hermetic.is_hermetic());
    assert!(
        HostPolicy::of(&hermetic).is_none(),
        "a hermetic binding minted a permit for the production scheduler"
    );

    let err = refused(open(&hermetic, SimId(0), Span::DUMMY), "nothing is bound");
    assert_eq!(err.code, codes::HERMETIC_BOUNDARY);
    assert!(
        err.notes.iter().any(|n| n.contains("simulate")),
        "the refusal does not name the seeded alternative: {:?}",
        err.notes
    );
    assert!(
        err.notes
            .iter()
            .any(|n| n.contains("ply_host::sched::spawn")),
        "the refusal does not name the handler that would have served it: {:?}",
        err.notes
    );
}

/// `HostBinding::default()` is the hermetic one, so the path a caller reaches by
/// not thinking about it is the one that refuses.
#[test]
fn the_default_binding_is_the_one_that_refuses() {
    assert!(HostPolicy::of(&HostBinding::default()).is_none());
}

#[test]
fn a_bound_binding_opens_a_production_region() {
    let sched = open(&bound(), SimId(0), Span::DUMMY).expect("bound");
    assert_eq!(sched.policy(), Policy::Host);
    assert!(!sched.records_steps());
    assert!(sched.holds(ROOT));
}

/// A production region answers `task` and nothing else. `clock.now` served from
/// the seeded table would give a real run virtual time starting at zero — a
/// wrong answer that no assertion goes red on.
#[test]
fn a_production_region_answers_task_and_not_the_clock() {
    let sched = open(&bound(), SimId(0), Span::DUMMY).expect("bound");
    for op in TASK_OPS {
        assert!(sched.answers("task", op), "task.{op}");
    }
    for (effect, op) in [("clock", "now"), ("clock", "sleep"), ("random", "next")] {
        assert!(
            !sched.answers(effect, op),
            "a production region claimed to answer {effect}.{op}"
        );
    }
}

/// The registry is the trusted computing base, and these three lines are what a
/// reviewer reads. Each claim is checkable: `task.*` performs nothing outside the
/// program, so it is repeatable and non-blocking, and `task` is `nondet` in the
/// prelude, so `Nondeterministic` passes `E0423` rather than needing a source
/// edit.
#[test]
fn the_task_registrations_are_what_ply_hosts_prints() {
    let listing = bound().listing().clone();
    let rows: Vec<String> = listing.rows.iter().map(|r| r.to_string()).collect();
    assert_eq!(rows, vec!["task.join", "task.spawn", "task.yield"]);
    for row in &listing.rows {
        assert_eq!(row.atom.to_string(), "task.write");
        assert!(!row.deterministic);
        assert!(!row.linearity.is_linear());
        assert!(!row.blocking);
        assert!(row.declared_nondet);
    }
}

/// An `Any` registration that resolves to nothing is a driver that is idle
/// rather than wrong. This registry is compiled into every program, and most
/// programs never spawn a task.
#[test]
fn a_program_that_never_spawns_binds_cleanly_and_lists_nothing() {
    let binding = registry()
        .bind(&check("fn f(x: Int) -> Int = x"))
        .expect("an idle scheduler is not an error");
    assert!(binding.listing().is_empty());
    assert!(binding.footprint().is_empty());
}

/// A `task.*` perform is answered by opening a region, never by a handler. If it
/// ever reaches one, that is a dispatch defect, and a handler that quietly
/// answered would be a program whose concurrency did not happen.
#[test]
fn dispatching_a_task_perform_to_the_handler_is_refused() {
    let binding = bound();
    let bound_op = binding
        .resolve(&Symbol::new("task"), &Symbol::new("spawn"), None)
        .expect("the triple is bound");
    let request = HostRequest {
        machine: ply_eval::host::MachineId(1),
        atom: bound_op.atom.clone(),
        op: bound_op.op,
        args: &[],
        span: Span::DUMMY,
        task: None,
        declared: None,
    };
    let err = bound_op
        .handler
        .call(&NeverResolves, &request)
        .err()
        .expect("the handler refuses");
    assert_eq!(err.code, codes::INTERNAL_ERROR);
    assert!(err.message.contains("task.spawn"), "{}", err.message);
}

/// Many tasks, all making progress, all finishing, with the region delivering
/// its body's value only once the last of them has ended.
#[test]
fn many_concurrent_tasks_all_make_progress() {
    const WORKERS: usize = 64;
    let mut program: Program = vec![Vec::new()];
    for i in 0..WORKERS {
        program[0].push(Act::Spawn(i + 1));
        program.push(vec![
            Act::Mark("start"),
            Act::Yield,
            Act::Mark("middle"),
            Act::Yield,
            Act::Mark("end"),
        ]);
    }
    for i in 0..WORKERS {
        program[0].push(Act::Join(i as u32 + 1));
    }
    program[0].push(Act::Mark("joined"));

    let run = with_jobs(|rt| run(&program, &**rt))
        .unwrap_or_else(|e| panic!("every task finishes: {}", e.message));
    for worker in 1..=WORKERS as u32 {
        assert_eq!(
            run.of(worker),
            vec!["start", "middle", "end"],
            "@{worker} did not run to completion in its own order"
        );
    }
    assert_eq!(run.marks.last(), Some(&(0, "joined")));

    // Concurrent rather than merely complete. A scheduler that ran each worker
    // to completion before starting the next — which is what strict
    // lowest-numbered-first degenerates to, since a yielding task is ready again
    // at once — leaves exactly one hand-off per worker. Anything well above that
    // is the interleaving.
    let handoffs = run.marks.windows(2).filter(|w| w[0].0 != w[1].0).count();
    assert!(
        handoffs > WORKERS,
        "{handoffs} hand-offs across {WORKERS} workers: they ran one at a time"
    );
}

/// ADR 0008 §8, as a test rather than as a claim: a task waiting on a real
/// operation on a real thread must not stop the others from being stepped.
///
/// The assertion is not that the others finished — they would even under a
/// stalled scheduler, once the waiter woke — but that they finished *before* it
/// did. A scheduler that ran the blocking operation on its own thread would put
/// `blocked` first every time.
#[test]
fn a_blocking_operation_does_not_starve_the_tasks_beside_it() {
    let program: Program = vec![
        vec![
            Act::Spawn(1),
            Act::Spawn(2),
            Act::Spawn(3),
            Act::Join(1),
            Act::Join(2),
            Act::Join(3),
        ],
        vec![Act::Wait(Duration::from_millis(120)), Act::Mark("blocked")],
        vec![Act::Yield, Act::Mark("a"), Act::Yield, Act::Mark("a2")],
        vec![Act::Yield, Act::Mark("b"), Act::Yield, Act::Mark("b2")],
    ];
    let run = with_jobs(|rt| run(&program, &**rt))
        .unwrap_or_else(|e| panic!("every task finishes: {}", e.message));
    let blocked = run.position("blocked").expect("the waiter woke");
    for mark in ["a", "a2", "b", "b2"] {
        let at = run.position(mark).expect("the runnable tasks ran");
        assert!(
            at < blocked,
            "`{mark}` ran after the blocking operation returned: {:?}",
            run.marks
        );
    }
}

/// Every outstanding job is accounted for at the end. A token left outstanding
/// is a thread the run never collected, which is the shape a leak takes here.
#[test]
fn every_host_token_a_run_parks_on_is_collected() {
    let program: Program = vec![
        vec![Act::Spawn(1), Act::Spawn(2), Act::Join(1), Act::Join(2)],
        vec![Act::Wait(Duration::from_millis(5)), Act::Mark("one")],
        vec![Act::Wait(Duration::from_millis(15)), Act::Mark("two")],
    ];
    with_jobs(|rt| {
        run(&program, &**rt).unwrap_or_else(|e| panic!("both wake: {}", e.message));
        assert_eq!(rt.outstanding(), 0);
    });
}

/// A failure in one task ends the region, names the task, and keeps answering
/// with the same diagnostic — so a caller that keeps driving cannot turn a
/// failure into a hang, and cannot get a second, different answer out of it.
#[test]
fn a_task_failing_stops_the_region_and_names_it() {
    let program: Program = vec![
        vec![Act::Spawn(1), Act::Spawn(2), Act::Join(1), Act::Join(2)],
        vec![Act::Mark("a"), Act::Fail],
        vec![Act::Yield, Act::Mark("b")],
    ];
    let err = refused(with_jobs(|rt| run(&program, &**rt)), "a task failed");
    assert_eq!(err.code, codes::RUNTIME_ERROR);
    assert!(
        err.notes.iter().any(|n| n.contains("@1")),
        "the failure does not name the task: {:?}",
        err.notes
    );
    assert!(
        !err.notes.iter().any(|n| n.contains("replay with seed")),
        "a host-scheduled failure offered a seed that would replay nothing: {:?}",
        err.notes
    );
}

#[test]
fn a_failed_production_region_answers_with_its_failure_forever() {
    let rt = Threads::new();
    let mut sched = Scheduler::production(SimId(0), Span::DUMMY, permit());
    let Turn::Run { .. } = sched.next_host(&*rt).expect("the root is enabled") else {
        panic!("expected the root's step");
    };
    sched.fail(
        Diagnostic::error(codes::RUNTIME_ERROR, "boom"),
        &ply_eval::Seed::root(0),
    );
    for _ in 0..4 {
        let err = refused(sched.next_host(&*rt), "the region is over");
        assert_eq!(err.code, codes::RUNTIME_ERROR);
    }
}

/// A join cycle has no host operation that could ever break it, so it is a
/// deadlock rather than a park that never returns.
#[test]
fn a_join_cycle_deadlocks_rather_than_parking_forever() {
    let program: Program = vec![vec![Act::Spawn(1), Act::Join(1)], vec![Act::Join(0)]];
    let err = refused(with_jobs(|rt| run(&program, &**rt)), "nothing can run");
    assert_eq!(err.code, codes::DEADLOCK);
    assert!(err.message.contains("host-scheduled"), "{}", err.message);
    let waits: Vec<&str> = err.labels.iter().map(|l| l.message.as_str()).collect();
    assert!(waits.iter().any(|m| m.contains("@0 waits here for @1")));
    assert!(waits.iter().any(|m| m.contains("@1 waits here for @0")));
}

/// A hot spin is the one failure mode a scheduler must not have: it burns a core
/// and reports nothing, which is indistinguishable from working.
#[test]
fn a_runtime_whose_park_never_resolves_is_named_rather_than_spun_on() {
    let mut sched = Scheduler::production(SimId(0), Span::DUMMY, permit());
    let Turn::Run { .. } = sched
        .next_host(&NeverResolves)
        .expect("the root is enabled")
    else {
        panic!("expected the root's step");
    };
    sched
        .park_on_host(
            suspended(),
            Pending {
                token: 1,
                label: "forever",
            },
            Span::DUMMY,
        )
        .expect("the root is running");
    let err = refused(sched.next_host(&NeverResolves), "nothing will ever resolve");
    assert_eq!(err.code, codes::INTERNAL_ERROR);
    assert!(err.message.contains("park"), "{}", err.message);
}

/// A production region is unbounded by default — a server is supposed to keep
/// scheduling — and bounded when a caller asks, which is what turns a livelock
/// in a `--host` test into a diagnostic.
#[test]
fn a_production_region_spends_a_budget_only_when_one_was_set() {
    let mut forever = vec![Act::Yield; 64];
    forever.push(Act::Mark("unreachable"));
    let program: Program = vec![forever];

    let unbounded = with_jobs(|rt| run(&program, &**rt))
        .unwrap_or_else(|e| panic!("64 yields is not a livelock: {}", e.message));
    assert_eq!(unbounded.steps, 65);

    let mut sched = Scheduler::production(SimId(0), Span::DUMMY, permit()).with_step_budget(4);
    let rt = Threads::new();
    for _ in 0..4 {
        let Turn::Run { .. } = sched.next_host(&*rt).expect("within the budget") else {
            panic!("expected a step");
        };
        sched.suspend(suspended(), Value::Unit).expect("running");
    }
    let err = refused(sched.next_host(&*rt), "the budget is spent");
    assert_eq!(err.code, codes::DEADLOCK);
    assert!(
        err.message.contains("4 scheduling steps"),
        "{}",
        err.message
    );
}
