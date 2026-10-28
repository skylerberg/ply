use ply_core::host::MachineId;
use ply_core::{Diagnostic, Span, TaskId, codes};
use ply_host::config::{Entry, Snapshot};
use ply_host::signal::{Accepting, Bounds, Shutdown, ShutdownSignal};
use ply_host::tcp::TcpHost;
use ply_host::trace::sink::Recording;
use ply_host::trace::spans::err_unbalanced;
use ply_host::trace::{Clock, Level, Outcome, OwnedRecord, RecordKind, Sink, Trace};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

/// Ascends by one per read, so golden assertions hold nothing that moves with the wall clock.
#[derive(Default)]
struct Ticking(AtomicI64);

impl Clock for Ticking {
    fn micros(&self) -> i64 {
        self.0.fetch_add(1, Ordering::Relaxed) + 1
    }
}

struct Driver {
    trace: Arc<Trace>,
    sink: Arc<Recording>,
}

fn driver() -> Driver {
    let sink = Arc::new(Recording::new(Level::Debug));
    let trace = Arc::new(Trace::with_clock(
        Arc::clone(&sink) as Arc<dyn Sink>,
        Arc::new(Ticking::default()),
    ));
    Driver { trace, sink }
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
    fn in_task(self, task: u64) -> EntryPoint {
        EntryPoint {
            task: Some(TaskId(task)),
            ..self
        }
    }
}

impl Driver {
    fn enter(&self, who: EntryPoint, channel: &str, name: &str) -> i64 {
        self.trace
            .enter((who.machine, who.task), channel, name, &[])
    }

    /// `E0445`, as the real handler's native fails the run, where `id` names no span `who` holds.
    fn exit(&self, who: EntryPoint, channel: &str, id: i64) -> Result<(), Diagnostic> {
        self.trace
            .exit((who.machine, who.task), channel, id, Outcome::Ok)
            .map_err(|why| err_unbalanced(Span::DUMMY, "`trace.exit`", id, &why))
    }

    fn event(&self, who: EntryPoint, channel: &str, name: &str) {
        self.trace.record(
            (who.machine, who.task),
            RecordKind::Event,
            Level::Info,
            channel,
            name,
            None,
            None,
            &[],
        );
    }

    fn records(&self) -> Vec<OwnedRecord> {
        self.sink.records()
    }
}

fn rendered(diagnostic: &Diagnostic) -> String {
    let mut text = format!("{}: {}", diagnostic.code, diagnostic.message);
    for note in &diagnostic.notes {
        text.push_str("\n  = ");
        text.push_str(note);
    }
    text
}

#[test]
fn two_entry_points_on_disjoint_channels_never_nest_into_each_other() {
    let d = driver();
    let a = entry_point();
    let b = entry_point();

    let outer_a = d.enter(a, "orders", "place_order");
    // `b` opens and closes a whole span while `a`'s is open.
    let outer_b = d.enter(b, "items", "list_items");
    d.event(b, "items", "scanned");
    d.exit(b, "items", outer_b)
        .expect("`b` closes its own span");
    d.event(a, "orders", "priced");
    d.exit(a, "orders", outer_a)
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
        outer_b,
        "`b`'s event landed in the wrong span"
    );
    assert_eq!(
        by_name("priced").span,
        outer_a,
        "`a`'s event landed in the wrong span, so `b`'s exit closed `a`'s span"
    );
    // An `Abandoned` here would mean one entry point's exit swept up the other's.
    let outcomes: Vec<&Outcome> = records
        .iter()
        .filter(|r| r.kind == RecordKind::Exit)
        .map(|r| &r.outcome)
        .collect();
    assert_eq!(outcomes, [&Outcome::Ok, &Outcome::Ok]);
}

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

    // And `b` can still close its own, which it could not if `a`'s teardown had taken it.
    d.exit(b, "items", b_span)
        .expect("`b`'s span survived another entry point's teardown");
    assert_eq!(d.trace.open_spans(), 0);
}

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
    d.exit(one, "orders", held)
        .expect("task one's span is still open");

    let records = d.records();
    let resumed = records
        .iter()
        .find(|r| r.name == "resumed")
        .expect("the resumed event was written");
    assert_eq!(
        resumed.span, held,
        "a continuation resumed after another task's span attached to the wrong span"
    );
    assert_eq!(
        resumed.parent, 0,
        "the resumed event's span is at depth zero, so its parent is 0"
    );
}

#[test]
fn a_span_id_a_program_receives_does_not_move_with_a_disjoint_entry_points_work() {
    // Alone: the first span of this entry point is 1.
    let d = driver();
    let alone = d.enter(entry_point(), "orders", "place_order");

    // Beside a footprint-disjoint entry point that opened two spans on another channel first.
    let d = driver();
    let other = entry_point();
    let _ = d.enter(other, "items", "list_items");
    let _ = d.enter(other, "items", "featured");
    let beside = d.enter(entry_point(), "orders", "place_order");

    assert_eq!(alone, 1);
    assert_eq!(
        beside, 1,
        "the id counter is per entry point, so a disjoint entry point's work is invisible"
    );

    // Within one entry point they still ascend and are never reused.
    let d = driver();
    let mine = entry_point();
    let first = d.enter(mine, "orders", "a");
    let second = d.enter(mine, "orders", "b");
    let third = d.enter(mine.in_task(1), "orders", "c");
    assert_eq!(
        [first, second, third],
        [1, 2, 3],
        "one entry point's ids ascend across its tasks"
    );
}

#[test]
fn an_unbalanced_exit_is_diagnosed_from_the_entry_points_own_span_table() {
    // A `Span` is an ordinary, forgeable record, so a program may name an id it never opened.
    let program_span = 1;

    // Alone.
    let d = driver();
    let alone = d
        .exit(entry_point(), "orders", program_span)
        .expect_err("a forged span is `E0445`");
    assert_eq!(alone.code, codes::SPAN_UNBALANCED);
    assert!(
        rendered(&alone).contains("no `trace.enter` in this entry point has answered that id"),
        "{}",
        rendered(&alone)
    );

    // Beside an entry point holding span 1 on a channel that does not conflict with `orders`.
    let d = driver();
    let other = entry_point();
    let _ = d.enter(other, "items", "list_items");
    let beside = d
        .exit(entry_point(), "orders", program_span)
        .expect_err("a forged span is `E0445`");
    assert_eq!(beside.code, codes::SPAN_UNBALANCED);
    assert!(
        !rendered(&beside).contains(&format!("{}", other.machine)),
        "no other entry point's machine appears in this program's failure report: {}",
        rendered(&beside)
    );

    // Beside an entry point that opened and closed span 1.
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

/// Run-level, not per entry point: right for the shutdown banner, wrong for a test to assert on.
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

fn snapshot(entries: &[(&str, &str, bool)]) -> Snapshot {
    Snapshot::new(
        entries
            .iter()
            .map(|(key, value, secret)| {
                (
                    (*key).to_string(),
                    Entry {
                        value: (*value).to_string(),
                        secret: *secret,
                    },
                )
            })
            .collect(),
        true,
    )
}

#[test]
fn every_entry_point_reads_one_configuration_and_none_can_move_it() {
    let before = snapshot(&[
        ("SERVICE_REGION", "eu", false),
        ("SERVICE_PORT", "8137", false),
        ("SERVICE_API_KEY", "hunter2", true),
    ]);
    let shared = Arc::new(before.clone());

    std::thread::scope(|scope| {
        for _ in 0..8 {
            let shared = Arc::clone(&shared);
            scope.spawn(move || {
                for _ in 0..100 {
                    assert_eq!(shared.get("SERVICE_REGION"), Some("eu"));
                    assert_eq!(shared.get("SERVICE_PORT"), Some("8137"));
                    // The `SSecret` gate: `config.get` answers `None` whatever the sources hold.
                    assert_eq!(shared.get("SERVICE_API_KEY"), None);
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

#[test]
fn two_snapshots_in_one_process_do_not_see_each_other() {
    let one = snapshot(&[("SERVICE_REGION", "eu", false)]);
    let two = snapshot(&[("SERVICE_REGION", "us", false)]);
    assert_eq!(one.get("SERVICE_REGION"), Some("eu"));
    assert_eq!(two.get("SERVICE_REGION"), Some("us"));
    // Built second, and the first is unchanged: the only route between two tests' configurations.
    assert_eq!(one.get("SERVICE_REGION"), Some("eu"));
    assert!(!Snapshot::unopened().has_spec());
    assert_eq!(Snapshot::unopened().get("SERVICE_REGION"), None);
}

const LISTENER: &str = "listener";

fn listen(net: &TcpHost) -> i64 {
    net.listen(LISTENER, 0, Span::DUMMY)
        .expect("a loopback port")
}

/// What a listener's next `accept` answers, its job run here.
fn accepted(net: &TcpHost, listener: i64) -> i64 {
    let job = net
        .accept(LISTENER, listener, Span::DUMMY)
        .expect("the accept is taken");
    ply_host::native::int_of(&job().expect("the accept ran")).expect("a handle")
}

fn until_phase_two(shutdown: &Arc<Shutdown>) {
    let until = Instant::now() + Duration::from_secs(5);
    while !shutdown.stopped_accepting() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        shutdown.stopped_accepting(),
        "the shutdown never reached its stop"
    );
}

#[test]
fn a_stop_one_run_asked_for_reaches_nothing_that_did_not() {
    let asked = Shutdown::new(Bounds::default());
    let untouched = Shutdown::new(Bounds::default());
    assert!(asked.request(ShutdownSignal::Terminate));

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

/// `ply test` builds exactly such a `Host`; the park loop and the deadlock check read this answer.
#[test]
fn a_host_with_no_coordinator_is_never_stopping() {
    let stopping = Shutdown::new(Bounds::default());
    assert!(stopping.request(ShutdownSignal::Interrupt));
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

/// A run that listens calls `signal::listen` well before `Host::stopping_on` wires the socket table.
#[test]
fn a_signal_before_the_coordinator_is_wired_still_stops_accept() {
    let shutdown = Shutdown::new(Bounds {
        lead: Duration::ZERO,
        drain: Duration::from_secs(5),
    });
    // The signal arrives after `signal::listen` and before `Host::stopping_on`.
    assert!(shutdown.request(ShutdownSignal::Terminate));
    until_phase_two(&shutdown);

    // Wired to the coordinator exactly as `Hosts::open_stopping` wires them.
    let net = Arc::new(TcpHost::new());
    shutdown.attach_net(Arc::clone(&net) as Arc<dyn Accepting>);
    let listener = listen(&net);

    assert!(shutdown.stopping(), "the run is stopping");
    assert!(
        shutdown.deadline_ms() >= 0,
        "the drain is already running, so a handler is being told to shed"
    );

    assert_eq!(
        accepted(&net, listener),
        0,
        "a run that was already stopping when the socket table arrived answers 0, which is what \
         ends a sequential accept loop"
    );
    assert_eq!(
        net.connections_in_flight(),
        0,
        "nothing was handed to the program after the stop"
    );

    // A socket table that already had a listener reports the one the catch-up closed.
    let shutdown = Shutdown::new(Bounds {
        lead: Duration::ZERO,
        drain: Duration::from_secs(5),
    });
    let late = Arc::new(TcpHost::new());
    let listener = listen(&late);
    assert!(shutdown.request(ShutdownSignal::Terminate));
    until_phase_two(&shutdown);
    shutdown.attach_net(Arc::clone(&late) as Arc<dyn Accepting>);
    assert_eq!(accepted(&late, listener), 0);
    assert_eq!(
        shutdown.at_stop().0,
        1,
        "the listener the catch-up closed is counted, so the banner is not short by it"
    );
}

#[test]
fn a_signal_after_the_coordinator_is_wired_stops_accept() {
    let shutdown = Shutdown::new(Bounds {
        lead: Duration::ZERO,
        drain: Duration::from_secs(5),
    });
    let net = Arc::new(TcpHost::new());
    shutdown.attach_net(Arc::clone(&net) as Arc<dyn Accepting>);
    let listener = listen(&net);

    assert!(shutdown.request(ShutdownSignal::Terminate));
    until_phase_two(&shutdown);

    assert_eq!(
        accepted(&net, listener),
        0,
        "a listener the drain closed answers 0, which is what ends a sequential accept loop"
    );
}

/// The phase machine runs on its own thread: stop accept, then dial until parked `accept`s return.
#[test]
fn the_shutdown_banners_counts_are_written_before_the_run_can_observe_the_stop() {
    use std::sync::atomic::AtomicUsize;

    struct SlowToWake {
        stopped: AtomicUsize,
        parked: AtomicUsize,
    }

    impl Accepting for SlowToWake {
        fn stop_accepting(&self) -> usize {
            // The socket table's flag goes up and `net.accept` answers `0` from here.
            self.stopped.fetch_add(1, Ordering::Release);
            1
        }
        fn listening_at(&self) -> Vec<std::net::SocketAddr> {
            // Dialable but wakes nothing, so the loop goes round rather than giving up.
            vec![std::net::SocketAddr::from(([127, 0, 0, 1], 1))]
        }
        fn connections_in_flight(&self) -> usize {
            1
        }
        fn accepts_in_flight(&self) -> usize {
            // Still parked, so the coordinator is in its wake loop and has written its state.
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
    assert!(shutdown.request(ShutdownSignal::Terminate));

    // Wait until the run stops: every `net.accept` answers `0`.
    let until = Instant::now() + Duration::from_secs(5);
    while net.stopped.load(Ordering::Acquire) == 0 && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        net.stopped.load(Ordering::Acquire),
        1,
        "the stop never told the socket table to stop"
    );

    // The banner, printed after the entry point returned while the coordinator still dials.
    assert_eq!(
        shutdown.at_stop(),
        (1, 1),
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
    assert_eq!(shutdown.at_stop(), (1, 1));
}
