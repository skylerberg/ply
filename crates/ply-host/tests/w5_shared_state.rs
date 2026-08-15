//! An adversarial audit of the three host states W5 adds, against ADR 0015 §7.
//!
//! W4's worst defect was one shared structure serving a whole run: a pooled
//! connection coupled two tests the footprint graph believed were disjoint, and
//! the scheduler could not have prevented it. §7 accounts for the three new
//! states — a tracing sink, a configuration snapshot and a stop flag — and this
//! file is the check on that account rather than a restatement of it.
//!
//! The question each test asks is the same one: **can one entry point observe a
//! value that is a function of what a footprint-disjoint entry point did?** A
//! `trace.write[c]` on channel `orders` and one on channel `items` do not
//! conflict, so `ply test` places them in one concurrency group and runs them
//! side by side on two rayon workers, each driving a machine of its own. Every
//! `MachineId` below is one of those workers.
//!
//! Two of the answers are `no` and are pinned here so a later change has to move
//! them on purpose. Two are `yes`, and those tests are written to *characterise*
//! the coupling rather than to assert it is acceptable: they name what is shared,
//! show the value that moves, and say what a caller would see. Reading one of
//! them as a blessing would be reading it backwards.

// A `Value::Record` holds `Arc<BTreeMap<Symbol, Value>>` and a `Value` is not
// `Send`; that is `ply-eval`'s design and this is the same allow, for the same
// reason, that `ply-host` itself carries.
#![allow(clippy::arc_with_non_send_sync)]

use ply_core::ty::{EffectAtom, Resource};
use ply_eval::host::{
    HostAnswer, HostOp, HostRegistry, HostRequest, HostRuntime, MachineId, Pending,
};
use ply_eval::{TaskId, Value};
use ply_host::config::{Key, Shape, Snapshot, Sources, Spec};
use ply_host::signal::{Accepting, Bounds, Shutdown, Signal};
use ply_host::tcp::{Net, TcpHost};
use ply_host::trace::sink::Recording;
use ply_host::trace::{Clock, Kept, Kind, Level, Op, Outcome, Sink, Trace};
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::Mode;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Driving one trace driver as several entry points
// ---------------------------------------------------------------------------

struct NoRuntime;

impl HostRuntime for NoRuntime {
    fn poll(&self, _: &Pending) -> Result<Option<Value>, Diagnostic> {
        unreachable!("`trace` never answers `Pending`")
    }
    fn park(&self) -> Result<(), Diagnostic> {
        unreachable!("`trace` never answers `Pending`")
    }
    fn block_on(&self, _: Pending) -> Result<Value, Diagnostic> {
        unreachable!("`trace` never answers `Pending`")
    }
}

/// A clock that ascends by one per read, so a stamp is a count and a golden
/// assertion has nothing that moves with the wall clock in it.
#[derive(Default)]
struct Ticking(AtomicI64);

impl Clock for Ticking {
    fn micros(&self) -> i64 {
        self.0.fetch_add(1, Ordering::Relaxed) + 1
    }
}

/// One driver, and the declarations a bound run would dispatch through.
///
/// The declarations come out of `register` rather than being written here, so
/// this drives the same rows `ply hosts` prints. A second declaration written
/// for the test would be a test of something else.
struct Driver {
    trace: Arc<Trace>,
    sink: Arc<Recording>,
    declarations: Vec<HostOp>,
}

fn driver() -> Driver {
    let sink = Arc::new(Recording::new(Level::Debug));
    let trace = Arc::new(Trace::with_clock(
        Arc::clone(&sink) as Arc<dyn Sink>,
        Arc::new(Ticking::default()),
    ));
    let mut registry = HostRegistry::new();
    ply_host::trace::register(&mut registry, Arc::clone(&trace));
    Driver {
        declarations: registry.ops().cloned().collect(),
        trace,
        sink,
    }
}

/// One entry point of a run, which is what a `ply test` worker is.
#[derive(Clone, Copy)]
struct EntryPoint {
    machine: MachineId,
    task: Option<TaskId>,
}

fn entry_point() -> EntryPoint {
    EntryPoint {
        machine: MachineId::next(),
        task: None,
    }
}

impl EntryPoint {
    fn in_task(self, task: u32) -> EntryPoint {
        EntryPoint {
            task: Some(TaskId(task)),
            ..self
        }
    }
}

impl Driver {
    fn perform(
        &self,
        who: EntryPoint,
        op: Op,
        channel: &str,
        args: &[Value],
    ) -> Result<Value, Diagnostic> {
        let index = Op::ALL
            .iter()
            .position(|candidate| *candidate == op)
            .expect("every operation is registered");
        let handler = ply_host::trace::handler(op, Arc::clone(&self.trace));
        let request = HostRequest {
            atom: EffectAtom::new(
                Symbol::new(ply_host::trace::EFFECT),
                Resource::Named(Symbol::new(channel)),
                Mode::Write,
            ),
            op: &self.declarations[index],
            args,
            span: Span::DUMMY,
            machine: who.machine,
            task: who.task,
            declared: None,
        };
        match handler.call(&NoRuntime, &request)? {
            HostAnswer::Value(value) => Ok(value),
            HostAnswer::Pending(_) => panic!("`trace` never waits on a peer"),
        }
    }

    fn enter(&self, who: EntryPoint, channel: &str, name: &str) -> Value {
        self.perform(
            who,
            Op::Enter,
            channel,
            &[Value::str(name), Value::empty_map()],
        )
        .expect("`trace.enter` never refuses")
    }

    fn exit(&self, who: EntryPoint, channel: &str, span: Value) -> Result<Value, Diagnostic> {
        self.perform(who, Op::Exit, channel, &[span, ctor("Ok", Vec::new())])
    }

    fn event(&self, who: EntryPoint, channel: &str, name: &str) -> Value {
        self.perform(
            who,
            Op::Event,
            channel,
            &[
                ctor("Info", Vec::new()),
                Value::str(name),
                Value::empty_map(),
            ],
        )
        .expect("`trace.event` never refuses")
    }

    fn records(&self) -> Vec<Kept> {
        self.sink.records()
    }
}

fn ctor(name: &str, args: Vec<Value>) -> Value {
    Value::ctor(format!("std.trace.{name}"), args)
}

/// A `Span` as `trace.enter` answered it.
fn span_id(value: &Value) -> i64 {
    match value {
        Value::Record(fields) => match fields.get(&Symbol::new("id")) {
            Some(Value::Int(id)) => *id,
            other => panic!("a `Span` carries an `Int` id, not {other:?}"),
        },
        other => panic!("`trace.enter` answers a record, not {}", other.type_name()),
    }
}

/// A `Span` a program built for itself, which is what §1.3 says an id is: an
/// ordinary record, forgeable, and `E0445` when it names nothing this task holds.
fn forged(id: i64, channel: &str) -> Value {
    Value::Record(Arc::new(
        [
            (Symbol::new("id"), Value::Int(id)),
            (Symbol::new("channel"), Value::str(channel)),
        ]
        .into_iter()
        .collect(),
    ))
}

fn rendered(diagnostic: &Diagnostic) -> String {
    let mut text = format!("{}: {}", diagnostic.code, diagnostic.message);
    for note in &diagnostic.notes {
        text.push_str("\n  = ");
        text.push_str(note);
    }
    text
}

// ---------------------------------------------------------------------------
// The tracing sink: what is not shared
// ---------------------------------------------------------------------------

/// The property §7 rests on: the span *stack* is keyed on the machine and the
/// task, so two entry points recording on channels that do not conflict cannot
/// nest into each other.
///
/// Asserted through the records rather than through the table, because the
/// records are what a reader of the log has: an event's `span` and a span's
/// `parent` are the whole of what says which request a line belongs to.
#[test]
fn two_entry_points_on_disjoint_channels_never_nest_into_each_other() {
    let d = driver();
    let a = entry_point();
    let b = entry_point();

    let outer_a = d.enter(a, "orders", "place_order");
    // `b` opens and closes a whole span while `a`'s is open. If the stack were
    // keyed on anything coarser than the owner, `b`'s span would take `a`'s as
    // its parent and `b`'s exit would close `a`'s.
    let outer_b = d.enter(b, "items", "list_items");
    d.event(b, "items", "scanned");
    d.exit(b, "items", outer_b.clone())
        .expect("`b` closes its own span");
    d.event(a, "orders", "priced");
    d.exit(a, "orders", outer_a.clone())
        .expect("`a`'s span is still open and still `a`'s");

    let records = d.records();
    let by_name = |name: &str| {
        records
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("no record named `{name}`"))
            .clone()
    };

    assert_eq!(
        by_name("list_items").parent,
        0,
        "`b`'s span took `a`'s as its parent, which is one request's timing under another's"
    );
    assert_eq!(
        by_name("scanned").span,
        span_id(&outer_b),
        "`b`'s event landed in the wrong span"
    );
    assert_eq!(
        by_name("priced").span,
        span_id(&outer_a),
        "`a`'s event landed in the wrong span, so `b`'s exit closed `a`'s span"
    );
    // Every span closed `Ok`: an `Abandoned` here would mean one entry point's
    // exit had swept up the other's.
    let outcomes: Vec<&Outcome> = records
        .iter()
        .filter(|r| r.kind == Kind::Exit)
        .map(|r| &r.outcome)
        .collect();
    assert_eq!(outcomes, [&Outcome::Ok, &Outcome::Ok]);
}

/// The teardown half of the same property. `end_entry_point` is called on every
/// exit path from an entry point, and `ply test` reaches it once per test over
/// one shared driver — so a teardown that emptied the table would close a span a
/// test running beside it is still writing into, and neither test would see it.
#[test]
fn a_teardown_closes_only_the_entry_point_that_ended() {
    let d = driver();
    let a = entry_point();
    let b = entry_point();

    let _ = d.enter(a, "orders", "a_outer");
    let _ = d.enter(a, "orders", "a_inner");
    let b_span = d.enter(b, "items", "b_outer");
    assert_eq!(d.trace.open_spans(), 3);

    let warning = d
        .trace
        .end_entry_point(a.machine)
        .expect("`a` left two spans open, which is `W0609`");
    assert_eq!(warning.code, codes::SPAN_ABANDONED);
    assert!(
        warning.message.contains("a_inner"),
        "`W0609` names the innermost span of the entry point that ended: {}",
        warning.message
    );
    assert!(
        !warning.message.contains('2') || !warning.message.contains("b_outer"),
        "the warning counted a span belonging to another entry point: {}",
        warning.message
    );
    assert_eq!(
        d.trace.open_spans(),
        1,
        "`b`'s span was swept up by `a`'s teardown"
    );

    // And `b` can still close its own, which it could not if `a`'s teardown had
    // taken it.
    d.exit(b, "items", b_span)
        .expect("`b`'s span survived another entry point's teardown");
    assert_eq!(d.trace.open_spans(), 0);
}

/// The question ADR 0015 §1.3 answers with "the handler keeps the stack, per
/// task": a task that is suspended across other tasks' whole span lifetimes
/// resumes into the span it opened, and not into whichever one was opened last.
///
/// This is the shape a scheduler region actually has — a task performs, is
/// descheduled at the perform, and the next thing the driver sees is another
/// task — so the driver never observes the two as one thread of control.
#[test]
fn a_task_resumed_later_records_under_the_span_it_opened() {
    let d = driver();
    let one = entry_point();
    let two = one.in_task(2);
    let one = one.in_task(1);

    let held = d.enter(one, "orders", "held");
    // Task two runs a whole span to completion while task one is suspended.
    let theirs = d.enter(two, "orders", "theirs");
    d.event(two, "orders", "in_theirs");
    d.exit(two, "orders", theirs)
        .expect("task two closes its own");
    // Task one resumes. Its next record must be in `held` and nothing else.
    d.event(one, "orders", "resumed");
    d.exit(one, "orders", held.clone())
        .expect("task one's span is still open");

    let records = d.records();
    let resumed = records
        .iter()
        .find(|r| r.name == "resumed")
        .expect("the resumed event was written");
    assert_eq!(
        resumed.span,
        span_id(&held),
        "a continuation resumed after another task's span attached to the wrong span"
    );
    assert_eq!(
        resumed.parent, 0,
        "the resumed event's span is at depth zero, so its parent is 0"
    );
}

// ---------------------------------------------------------------------------
// The tracing sink: what *is* shared
// ---------------------------------------------------------------------------

/// **A finding, characterised.** Span ids are minted from one counter per
/// driver, and one driver serves a whole run — so the `Span` value a program
/// receives from `trace.enter` is a function of how many spans every *other*
/// entry point in the run has opened, including entry points whose channel does
/// not conflict with this one and which therefore run concurrently.
///
/// ADR 0015 §7 argued the sink was safe because `trace.write[c]` is a write per
/// channel and the conflict graph serialises two tests on one channel. That
/// argument covers the *records*. It did not cover the id counter, which was
/// shared across channels — and an id is a value that crosses back into the
/// program: `Span` is an ordinary record precisely so a program can put its id
/// in a field.
///
/// What that cost a caller, before the fix: a host-backed test that asserts on
/// a span id, or on a log line containing one, passed at `--jobs 1` and failed
/// at `--jobs 8`, while the footprint graph said the two tests were disjoint.
/// That is W4's pooled connection with a different noun. The counter is now per
/// entry point, so the same call answers the same id whatever is running beside
/// it, and this test is what would fail if it ever went back.
#[test]
fn a_span_id_a_program_receives_does_not_move_with_a_disjoint_entry_points_work() {
    // Alone: the first span of this entry point is 1.
    let d = driver();
    let alone = span_id(&d.enter(entry_point(), "orders", "place_order"));

    // Beside a footprint-disjoint entry point that opened two spans on another
    // channel first. Same program, same channel, same call.
    let d = driver();
    let other = entry_point();
    let _ = d.enter(other, "items", "list_items");
    let _ = d.enter(other, "items", "featured");
    let beside = span_id(&d.enter(entry_point(), "orders", "place_order"));

    assert_eq!(alone, 1);
    assert_eq!(
        beside, 1,
        "the id counter is per entry point, so a disjoint entry point's work is invisible"
    );

    // And within one entry point they still ascend and are never reused, which
    // is the property the counter existed for.
    let d = driver();
    let mine = entry_point();
    let first = span_id(&d.enter(mine, "orders", "a"));
    let second = span_id(&d.enter(mine, "orders", "b"));
    let third = span_id(&d.enter(mine.in_task(1), "orders", "c"));
    assert_eq!(
        [first, second, third],
        [1, 2, 3],
        "one entry point's ids ascend across its tasks"
    );
}

/// **The same finding where it reaches a verdict.** `E0445 SPAN_UNBALANCED` is
/// the program's fault, attributed and bisected like any other program failure —
/// so its text is what a failure report carries and what a reader acts on. Which
/// of the three refusals it is, and therefore what it says, used to be computed
/// from the *whole driver's* table: the run-global id counter decided "never
/// opened" from "already closed", and every other owner's stack decided "open on
/// another task".
///
/// So one program, unchanged, was diagnosed one way alone and another way beside
/// a footprint-disjoint entry point — and in the second case the note named the
/// other entry point's machine, which is one test's identity appearing in
/// another test's failure report. Both questions are now asked of the performing
/// entry point's own table, so the three cases below are one diagnosis.
#[test]
fn an_unbalanced_exit_is_diagnosed_from_the_entry_points_own_span_table() {
    let program_span = forged(1, "orders");

    // Alone.
    let d = driver();
    let alone = d
        .exit(entry_point(), "orders", program_span.clone())
        .expect_err("a forged span is `E0445`");
    assert_eq!(alone.code, codes::SPAN_UNBALANCED);
    assert!(
        rendered(&alone).contains("no `trace.enter` in this entry point has answered that id"),
        "{}",
        rendered(&alone)
    );

    // Beside an entry point that happens to hold span 1, on a channel that does
    // not conflict with `orders`.
    let d = driver();
    let other = entry_point();
    let _ = d.enter(other, "items", "list_items");
    let beside = d
        .exit(entry_point(), "orders", program_span.clone())
        .expect_err("a forged span is `E0445`");
    assert_eq!(beside.code, codes::SPAN_UNBALANCED);
    assert!(
        !rendered(&beside).contains(&format!("{}", other.machine)),
        "no other entry point's machine appears in this program's failure report: {}",
        rendered(&beside)
    );

    // Beside an entry point that opened and closed span 1, which used to be a
    // third classification from nothing this program did.
    let d = driver();
    let other = entry_point();
    let theirs = d.enter(other, "items", "list_items");
    d.exit(other, "items", theirs).expect("closed");
    let after = d
        .exit(entry_point(), "orders", program_span)
        .expect_err("a forged span is `E0445`");

    assert_eq!(
        rendered(&alone),
        rendered(&beside),
        "one program's `E0445` reads the same whatever another entry point did"
    );
    assert_eq!(rendered(&beside), rendered(&after));
}

/// The classification that *is* this program's own: a span open on another task
/// of the same entry point. That one still names both tasks, because both are
/// this program's and putting one task's timing under another's is exactly the
/// wrong answer §1.3 exists to refuse.
#[test]
fn a_span_open_on_another_task_of_the_same_entry_point_still_names_it() {
    let d = driver();
    let mine = entry_point();
    let theirs = d.enter(mine.in_task(1), "orders", "place_order");
    let refused = d
        .exit(mine.in_task(2), "orders", theirs)
        .expect_err("another task's span is `E0445`");
    assert_eq!(refused.code, codes::SPAN_UNBALANCED);
    assert!(
        rendered(&refused).contains("task @1"),
        "{}",
        rendered(&refused)
    );
}

/// The counters the shutdown banner prints are the run's, not an entry point's,
/// and that is correct for a banner and wrong for anything a test asserts on.
/// Pinned so that nobody builds a per-test assertion on top of them.
#[test]
fn the_run_level_counts_are_a_sum_over_every_entry_point() {
    let d = driver();
    let a = entry_point();
    let b = entry_point();
    let a_span = d.enter(a, "orders", "a");
    let _ = d.enter(b, "items", "b");
    d.exit(a, "orders", a_span).expect("closed");

    let counts = d.trace.counts();
    assert_eq!(counts.spans, 2, "`opened` counts both entry points' spans");
    assert_eq!(
        counts.events, 3,
        "two `enter` records and one `exit` record, from two entry points"
    );
    assert_eq!(d.trace.open_spans(), 1, "`b`'s is still open");
}

// ---------------------------------------------------------------------------
// The configuration snapshot
// ---------------------------------------------------------------------------

fn snapshot(set: &[&str], env: &[(&str, &str)], keys: Vec<Key>) -> Snapshot {
    let set: Vec<String> = set.iter().map(|s| (*s).to_string()).collect();
    let env: Vec<(String, String)> = env
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    let sources = Sources::read_with(&set, &[], &env, &|_| {
        Err(std::io::Error::other("no `--config` file in this test"))
    })
    .expect("the sources read");
    let spec = Spec::new(keys).expect("the schema is well formed");
    Snapshot::resolve(&sources, Some(&spec))
        .expect("the schema resolves")
        .snapshot
}

fn key(name: &str, shape: Shape) -> Key {
    Key {
        name: name.to_string(),
        shape,
        required: false,
        default: None,
    }
}

/// **No finding.** The snapshot is the one of the three states that is
/// structurally incapable of coupling two entry points: it is built before any
/// handler exists and has no mutator, so every reader of one `Arc<Snapshot>`
/// sees the same map for the life of the run whatever anything else does.
///
/// Asserted under real contention rather than by inspection, because "there is
/// no method that changes it" is a claim about the code and this is a claim
/// about the run: eight threads, a hundred reads each, against a snapshot cloned
/// before they started and compared after they finished.
#[test]
fn every_entry_point_reads_one_configuration_and_none_can_move_it() {
    let before = snapshot(
        &["DESK_REGION=eu"],
        &[("DESK_PORT", "8137"), ("DESK_API_KEY", "hunter2")],
        vec![
            key("DESK_REGION", Shape::Text),
            key("DESK_PORT", Shape::Int),
            key("DESK_API_KEY", Shape::Secret),
        ],
    );
    let shared = Arc::new(before.clone());

    std::thread::scope(|scope| {
        for _ in 0..8 {
            let shared = Arc::clone(&shared);
            scope.spawn(move || {
                for _ in 0..100 {
                    assert_eq!(shared.get("DESK_REGION"), Some("eu"));
                    assert_eq!(shared.get("DESK_PORT"), Some("8137"));
                    // The `SSecret` gate: `config.get` answers `None` whatever
                    // the sources hold, on every thread and every read.
                    assert_eq!(shared.get("DESK_API_KEY"), None);
                    assert_eq!(shared.get("NOTHING_SUPPLIED"), None);
                }
            });
        }
    });

    assert_eq!(
        *shared, before,
        "a hundred reads on eight threads moved the snapshot, which would mean `config.read[k]` \
         is not a read and the conflict graph is wrong about every configuration reader"
    );
}

/// The other half: a snapshot is a value, so two of them in one process are two
/// runs' configurations and neither is the other's. `ply test` builds one and
/// hands every worker an `Arc` of it; nothing here can make a second one visible
/// to the first's readers.
#[test]
fn two_snapshots_in_one_process_do_not_see_each_other() {
    let one = snapshot(
        &["DESK_REGION=eu"],
        &[],
        vec![key("DESK_REGION", Shape::Text)],
    );
    let two = snapshot(
        &["DESK_REGION=us"],
        &[],
        vec![key("DESK_REGION", Shape::Text)],
    );
    assert_eq!(one.get("DESK_REGION"), Some("eu"));
    assert_eq!(two.get("DESK_REGION"), Some("us"));
    // Built second, and the first is unchanged — which is the whole of what a
    // test asking "can a configuration change in one test be seen by another"
    // has to check, because there is no other way to make one.
    assert_eq!(one.get("DESK_REGION"), Some("eu"));
    assert!(!Snapshot::unopened().has_spec());
    assert_eq!(Snapshot::unopened().get("DESK_REGION"), None);
}

// ---------------------------------------------------------------------------
// The stop flag
// ---------------------------------------------------------------------------

fn listener_label() -> Resource {
    Resource::Named(Symbol::new("listener"))
}

fn settle(host: &TcpHost, answered: Result<HostAnswer, Diagnostic>) -> Value {
    match answered.expect("the operation was accepted") {
        HostAnswer::Value(value) => value,
        HostAnswer::Pending(pending) => {
            let until = Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(value) = host.poll(&pending).expect("this host's token") {
                    return value;
                }
                assert!(Instant::now() < until, "`{pending}` never resolved");
                let _ = host.park_until(Duration::from_millis(20));
            }
        }
    }
}

fn int(value: &Value) -> i64 {
    match value {
        Value::Int(i) => *i,
        other => panic!("not an Int: {}", other.type_name()),
    }
}

fn until_phase_two(shutdown: &Arc<Shutdown>) {
    let until = Instant::now() + Duration::from_secs(5);
    while !shutdown.stopped_accepting() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        shutdown.stopped_accepting(),
        "the phase machine never reached phase 2"
    );
}

/// **No finding.** The stop flag is per coordinator and a coordinator is per
/// run, so a stop one `Shutdown` was asked for is invisible to everything that
/// did not ask. That is what makes §4.7's answer — withhold `signal` under
/// `ply test` — sufficient rather than merely conventional: even if two runs
/// existed in one process, one's stop would not decide the other's verdicts.
#[test]
fn a_stop_one_run_asked_for_reaches_nothing_that_did_not() {
    let asked = Shutdown::new(Bounds::default());
    let untouched = Shutdown::new(Bounds::default());
    assert!(asked.request(Signal::Terminate));

    assert!(asked.stopping());
    assert!(
        !untouched.stopping(),
        "a stop leaked from the run that asked for it into one that did not"
    );
    assert_eq!(
        untouched.deadline_ms(),
        -1,
        "a run nobody stopped has no deadline"
    );
    assert!(!untouched.drain_expired());
    assert!(untouched.signal().is_none());
    assert!(!untouched.second_requested());
}

/// A `Host` with no coordinator answers `false` to the scheduler's `stopping()`
/// whatever any other coordinator in the process is doing — which is the
/// property `ply test` depends on, since it builds exactly such a `Host` and the
/// park loop and the deadlock check are the two places the answer is read.
#[test]
fn a_host_with_no_coordinator_is_never_stopping() {
    let stopping = Shutdown::new(Bounds::default());
    assert!(stopping.request(Signal::Interrupt));
    until_phase_two(&stopping);

    let hermetic = ply_host::Host::new();
    assert!(
        hermetic.stop().is_none(),
        "a `Host` nobody wired a coordinator to has none"
    );
    let runtime = hermetic.runtime();
    assert!(
        !runtime.stopping(),
        "a run that binds no signal handler observed another run's stop"
    );
    assert!(
        runtime.drain_expired().is_none(),
        "a run with no drain cannot have one expire"
    );
}

/// The registrations are the same two either way, so `ply hosts` lists them for
/// a suite as well as for a service — what differs is whether they are bound.
/// A handler this run withheld and dispatched anyway is Ply's fault and says so
/// rather than answering a flag nobody set.
#[test]
fn a_withheld_signal_handler_refuses_rather_than_answering() {
    let withheld = ply_host::signal::registrations(None);
    assert_eq!(withheld.len(), 2);
    let (declaration, handler) = &withheld[0];
    assert_eq!(declaration.path, "ply_host::signal::stopping");
    let request = HostRequest {
        atom: EffectAtom::new(
            Symbol::new(ply_host::signal::EFFECT),
            Resource::Singleton,
            Mode::Read,
        ),
        op: declaration,
        args: &[],
        span: Span::DUMMY,
        machine: MachineId::next(),
        task: None,
        declared: None,
    };
    let Err(refused) = handler.call(&NoRuntime, &request) else {
        panic!("a withheld handler has no flag to read and must refuse");
    };
    assert_eq!(refused.code, codes::INTERNAL_ERROR);
    assert!(refused.message.contains("withheld"), "{}", refused.message);
}

/// `ply run --host` calls `signal::listen` before it opens the pool, loads the
/// TLS material, binds the registry and verifies the schema, and only then calls
/// `Host::stopping_on`, which is what hands the coordinator the socket table. A
/// signal delivered in that window runs the phase machine with **no**
/// `Accepting` attached.
///
/// What that used to do was worse than either clean answer: `signal.stopping()`
/// answered `true`, so a readiness route shed and a load balancer took the
/// instance out — and the listener stayed open and kept accepting and serving
/// until the drain deadline expired, at which point the run reported `W0608
/// DRAIN_INCOMPLETE` and exited `3`. A shutdown that was going to be clean was
/// reported as one that dropped requests, and requests that should have been
/// refused were served.
///
/// The window is not theoretical: it contains a real postgres connect with its
/// own `--db-connect-ms` timeout, so a `SIGTERM` to an instance that is still
/// coming up — the ordinary shape of a rolling restart or a failed readiness
/// probe — lands in it. `attach_net` now catches up with a phase machine that
/// has already run, so the answer is the same as if the signal had arrived a
/// moment later.
#[test]
fn a_signal_before_the_coordinator_is_wired_still_stops_accept() {
    let shutdown = Shutdown::new(Bounds {
        lead: Duration::ZERO,
        drain: Duration::from_secs(5),
    });
    // The signal arrives after `signal::listen` and before `Host::stopping_on`.
    assert!(shutdown.request(Signal::Terminate));
    until_phase_two(&shutdown);

    // The facilities finish coming up and are wired to the coordinator, exactly
    // as `Hosts::open_stopping` wires them.
    let net = Arc::new(TcpHost::new());
    shutdown.attach_net(Arc::clone(&net) as Arc<dyn Accepting>);
    let listener = int(&settle(&net, net.listen(&listener_label(), 0, Span::DUMMY)));

    assert!(shutdown.stopping(), "the run is stopping");
    assert!(
        shutdown.deadline_ms() >= 0,
        "the drain is already running, so a handler is being told to shed"
    );

    assert_eq!(
        int(&settle(
            &net,
            net.accept(&listener_label(), listener, Span::DUMMY)
        )),
        0,
        "a run that was already stopping when the socket table arrived answers 0, which is what \
         ends a sequential accept loop"
    );
    assert_eq!(
        net.connections_in_flight(),
        0,
        "nothing was handed to the program after the stop"
    );

    // And the catch-up's own account is in the banner's numbers rather than
    // lost: a socket table that already had a listener when it arrived reports
    // the one it closed.
    let shutdown = Shutdown::new(Bounds {
        lead: Duration::ZERO,
        drain: Duration::from_secs(5),
    });
    let late = Arc::new(TcpHost::new());
    let listener = int(&settle(
        &late,
        late.listen(&listener_label(), 0, Span::DUMMY),
    ));
    assert!(shutdown.request(Signal::Terminate));
    until_phase_two(&shutdown);
    shutdown.attach_net(Arc::clone(&late) as Arc<dyn Accepting>);
    assert_eq!(
        int(&settle(
            &late,
            late.accept(&listener_label(), listener, Span::DUMMY)
        )),
        0
    );
    assert_eq!(
        shutdown.at_stop().0,
        1,
        "the listener the catch-up closed is counted, so the banner is not short by it"
    );
}

/// The same window, one step later: a signal delivered *after* the socket table
/// is attached does stop accept, which is what makes the test above a window
/// rather than a total failure.
#[test]
fn a_signal_after_the_coordinator_is_wired_stops_accept() {
    let shutdown = Shutdown::new(Bounds {
        lead: Duration::ZERO,
        drain: Duration::from_secs(5),
    });
    let net = Arc::new(TcpHost::new());
    shutdown.attach_net(Arc::clone(&net) as Arc<dyn Accepting>);
    let listener = int(&settle(&net, net.listen(&listener_label(), 0, Span::DUMMY)));

    assert!(shutdown.request(Signal::Terminate));
    until_phase_two(&shutdown);

    assert_eq!(
        int(&settle(
            &net,
            net.accept(&listener_label(), listener, Span::DUMMY)
        )),
        0,
        "a listener the drain closed answers 0, which is what ends a sequential accept loop"
    );
}

/// `Shutdown::request` hands the phase machine to a thread of its own so the
/// signal reactor stays free to notice a second signal, and phase 2 then tells
/// the socket table to stop accepting, dials every listener until the parked
/// `accept`s have returned, and writes down what it found.
///
/// The first of those is what makes the run stop — `net.accept` answers `0` the
/// instant the socket table's flag is set — and the last is what the shutdown
/// banner reads. So while the write came third, the machine's thread could run
/// the whole drain, the teardown and the banner while the coordinator was still
/// dialling, and the operator was told **`0 listener(s) closed · 0 connection(s)
/// in flight · 0 transaction(s) open`** for a run that had one of each. That was
/// observable on `examples/desk.ply` with no special timing.
///
/// ADR 0015 §6's rule is that every number an operator reads is a fact the run
/// already holds. The write now happens under the state lock and around
/// `stop_accepting`, so a reader of the banner blocks on it rather than racing
/// it; the dialling, which the banner reports nothing about, stays outside.
///
/// Made deterministic here with an `Accepting` whose parked `accept` takes a
/// while to come back, which is the only thing the real wake loop is waiting on.
#[test]
fn the_shutdown_banners_counts_are_written_before_the_run_can_observe_the_stop() {
    use std::sync::atomic::AtomicUsize;

    struct SlowToWake {
        stopped: AtomicUsize,
        parked: AtomicUsize,
    }

    impl Accepting for SlowToWake {
        fn stop_accepting(&self) -> usize {
            // What really happens here: the socket table's flag goes up and
            // `net.accept` answers `0` from this instant. The run is stopping.
            self.stopped.fetch_add(1, Ordering::Release);
            1
        }
        fn listening_at(&self) -> Vec<std::net::SocketAddr> {
            // An address the wake dial can try and never wake anything on, so
            // the loop goes round rather than giving up on having nowhere to
            // dial. Port 1 on the loopback refuses immediately, which keeps the
            // round at the loop's own five-millisecond sleep.
            vec![std::net::SocketAddr::from(([127, 0, 0, 1], 1))]
        }
        fn connections_in_flight(&self) -> usize {
            1
        }
        fn accepts_in_flight(&self) -> usize {
            // Still parked, so the coordinator is inside its wake loop and has
            // long since written the state it found.
            self.parked.load(Ordering::Acquire)
        }
    }

    let net = Arc::new(SlowToWake {
        stopped: AtomicUsize::new(0),
        parked: AtomicUsize::new(1),
    });
    let shutdown = Shutdown::new(Bounds {
        lead: Duration::ZERO,
        drain: Duration::from_secs(30),
    });
    shutdown.attach_net(Arc::clone(&net) as Arc<dyn Accepting>);
    assert!(shutdown.request(Signal::Terminate));

    // Wait for the point the *run* stops: the socket table has been told, so
    // every `net.accept` answers `0` and `serve` is already returning.
    let until = Instant::now() + Duration::from_secs(5);
    while net.stopped.load(Ordering::Acquire) == 0 && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        net.stopped.load(Ordering::Acquire),
        1,
        "phase 2 never told the socket table to stop"
    );

    // This is the banner, printed on the machine's thread after the entry point
    // returned, while the coordinator is still dialling. Every number is the
    // coordinator's own account.
    assert_eq!(
        shutdown.at_stop(),
        (1, 1, 0),
        "the counts are readable from the instant the run could notice the stop"
    );
    assert!(
        shutdown.deadline_ms() >= 0,
        "the drain deadline is armed by the same critical section"
    );

    // And they do not move once the wake loop finishes.
    net.parked.store(0, Ordering::Release);
    let until = Instant::now() + Duration::from_secs(5);
    while net.accepts_in_flight() > 0 && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(shutdown.at_stop(), (1, 1, 0));
}
