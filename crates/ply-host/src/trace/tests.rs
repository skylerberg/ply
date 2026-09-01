use super::*;
use crate::trace::sink::{Kept, Recording};
use ply_core::ty::EffectAtom;
use ply_eval::TaskId;
use ply_eval::host::Pending;
use ply_syntax::ast::Mode;
use std::sync::atomic::AtomicI64;

/// A clock that counts, because the number a span's cost owes is "a discarded event reads the clock zero
/// times" and nothing else can assert it.
#[derive(Default)]
struct Counting {
    reads: AtomicU64,
    at: AtomicI64,
}

impl Clock for Counting {
    fn micros(&self) -> i64 {
        self.reads.fetch_add(1, Ordering::Relaxed);
        // Ascending by one per read, so a duration is the number of reads between two stamps and a
        // golden line has a stamp that does not move.
        self.at.fetch_add(1, Ordering::Relaxed) + 1
    }
}

impl Counting {
    fn reads(&self) -> u64 {
        self.reads.load(Ordering::Relaxed)
    }
}

struct NoRuntime;

impl HostRuntime for NoRuntime {
    fn poll(&self, _: &Pending) -> Result<Option<ply_eval::Value>, Diagnostic> {
        unreachable!("nothing here answers `Pending`")
    }
    fn park(&self) -> Result<(), Diagnostic> {
        unreachable!("nothing here answers `Pending`")
    }
    fn block_on(&self, _: Pending) -> Result<ply_eval::Value, Diagnostic> {
        unreachable!("nothing here answers `Pending`")
    }
}

/// One driver, its sink and its clock, plus a machine to perform as.
struct Fixture {
    trace: Arc<Trace>,
    sink: Arc<Recording>,
    clock: Arc<Counting>,
    machine: MachineId,
}

fn fixture() -> Fixture {
    fixture_at(Level::Debug)
}

fn fixture_at(level: Level) -> Fixture {
    let sink = Arc::new(Recording::new(level));
    let clock = Arc::new(Counting::default());
    Fixture {
        trace: Arc::new(Trace::with_clock(
            Arc::clone(&sink) as Arc<dyn Sink>,
            Arc::clone(&clock) as Arc<dyn Clock>,
        )),
        sink,
        clock,
        machine: MachineId::next(),
    }
}

fn discarding() -> (Arc<Trace>, Arc<Counting>) {
    let clock = Arc::new(Counting::default());
    (
        Arc::new(Trace::with_clock(
            Arc::new(Discard),
            Arc::clone(&clock) as Arc<dyn Clock>,
        )),
        clock,
    )
}

fn ctor(name: &str, args: Vec<ply_eval::Value>) -> ply_eval::Value {
    ply_eval::Value::ctor(format!("{MODULE}.{name}"), args)
}

fn no_fields() -> ply_eval::Value {
    ply_eval::Value::empty_map()
}

fn field(key: &str, value: ply_eval::Value) -> ply_eval::Value {
    ply_eval::Value::map([(ply_eval::Value::str(key), value)])
}

impl Fixture {
    fn perform(&self, op: Op, at: &str, args: Vec<ply_eval::Value>) -> ply_eval::Value {
        self.as_task(None, op, at, args)
            .unwrap_or_else(|d| panic!("{d:?}"))
    }

    fn as_task(
        &self,
        task: Option<TaskId>,
        op: Op,
        at: &str,
        args: Vec<ply_eval::Value>,
    ) -> Result<ply_eval::Value, Diagnostic> {
        perform_on(&self.trace, self.machine, task, op, at, args)
    }

    fn kinds(&self) -> Vec<Kind> {
        self.sink.records().iter().map(|r| r.kind).collect()
    }

    fn records(&self) -> Vec<Kept> {
        self.sink.records()
    }
}

fn perform_on(
    trace: &Arc<Trace>,
    machine: MachineId,
    task: Option<TaskId>,
    op: Op,
    at: &str,
    args: Vec<ply_eval::Value>,
) -> Result<ply_eval::Value, Diagnostic> {
    let handler = Operation {
        op,
        trace: Arc::clone(trace),
    };
    let declaration = op.declaration("ply_host::trace::tests");
    let atom = EffectAtom::new(
        Symbol::new(EFFECT),
        Resource::Named(Symbol::new(at)),
        Mode::Write,
    );
    let request = HostRequest {
        atom,
        op: &declaration,
        args: &args,
        span: Span::DUMMY,
        machine,
        task,
        declared: None,
    };
    match handler.call(&NoRuntime, &request)? {
        HostAnswer::Value(value) => Ok(value),
        HostAnswer::Pending(_) => panic!("`trace` never waits on a peer"),
    }
}

fn enter(f: &Fixture, at: &str, name: &str) -> ply_eval::Value {
    f.perform(Op::Enter, at, vec![ply_eval::Value::str(name), no_fields()])
}

fn exit(f: &Fixture, at: &str, span: ply_eval::Value) -> ply_eval::Value {
    f.perform(Op::Exit, at, vec![span, ctor("Ok", Vec::new())])
}

fn event(f: &Fixture, at: &str, level: &str, name: &str) -> ply_eval::Value {
    f.perform(
        Op::Event,
        at,
        vec![
            ctor(level, Vec::new()),
            ply_eval::Value::str(name),
            no_fields(),
        ],
    )
}

// --- what the row and the listing say -------------------------------------------

/// The resource is a channel and every operation is `Any`, so `bind` expands one registration into
/// one row per channel the program uses.
#[test]
fn every_operation_registers_per_channel_at_most_once_and_not_blocking() {
    for op in Op::ALL {
        let declaration = op.declaration("ply_host::trace::discard");
        assert_eq!(declaration.effect.as_str(), EFFECT);
        assert_eq!(declaration.resource, HostResource::Any);
        assert_eq!(declaration.determinism, Determinism::Nondeterministic);
        assert_eq!(
            declaration.linearity,
            Linearity::AtMostOnce,
            "replaying `{}` writes its record twice, and a duplicated span is a wrong answer",
            op.name()
        );
        assert!(!declaration.blocking, "`{}` waits on nothing", op.name());
    }
}

/// The path a row prints is the sink's, so `--trace off` says so rather than naming a writer the
/// run does not have.
#[test]
fn the_listing_names_the_sink_that_actually_serves_the_run() {
    let discard = registry(Arc::new(Trace::new(Arc::new(Discard))));
    assert!(
        discard
            .ops()
            .all(|op| op.path == "ply_host::trace::discard"),
        "a discarding run must not claim to be writing JSON"
    );
    let json = registry(Arc::new(Trace::new(Arc::new(Json::new(Level::Info)))));
    assert!(json.ops().all(|op| op.path == "ply_host::trace::json"));
    assert_eq!(json.len(), Op::ALL.len());
}

// --- spans ------------------------------------------------------------------------

#[test]
fn enter_answers_a_span_carrying_its_id_and_its_channel() {
    let f = fixture();
    let span = enter(&f, "orders", "place_order");
    let ply_eval::Value::Record(fields) = &span else {
        panic!("a `Span` is a record: {span:?}");
    };
    assert_eq!(fields[&Symbol::new("id")], ply_eval::Value::Int(1));
    assert_eq!(
        fields[&Symbol::new("channel")],
        ply_eval::Value::str("orders")
    );
}

#[test]
fn a_nested_span_records_its_parent_and_an_event_inside_it_records_the_span() {
    let f = fixture();
    let outer = enter(&f, "http", "request");
    let inner = enter(&f, "http", "query");
    event(&f, "http", "Warn", "slow");
    exit(&f, "http", inner);
    exit(&f, "http", outer);

    let records = f.records();
    assert_eq!(
        f.kinds(),
        [
            Kind::Enter,
            Kind::Enter,
            Kind::Event,
            Kind::Exit,
            Kind::Exit
        ]
    );
    let spans: Vec<i64> = records.iter().map(|r| r.span).collect();
    let parents: Vec<i64> = records.iter().map(|r| r.parent).collect();
    assert_eq!(spans, [1, 2, 2, 2, 1]);
    assert_eq!(parents, [0, 1, 1, 1, 0]);
    assert_eq!(records[2].level, Level::Warn, "the event keeps its level");
    assert!(
        records.iter().all(|r| r.channel == "http"),
        "every record is on the channel the call site named"
    );
}

/// The one the orchestrator asked for by name: a span opened in one task must not close in another.
#[test]
fn a_span_opened_in_one_task_does_not_close_in_another() {
    let f = fixture();
    let first = f
        .as_task(
            Some(TaskId(1)),
            Op::Enter,
            "http",
            vec![ply_eval::Value::str("request"), no_fields()],
        )
        .expect("task 1 opens a span");

    let refused = f
        .as_task(
            Some(TaskId(2)),
            Op::Exit,
            "http",
            vec![first.clone(), ctor("Ok", Vec::new())],
        )
        .expect_err("task 2 must not close task 1's span");
    assert_eq!(refused.code, codes::SPAN_UNBALANCED);
    assert!(
        refused.notes.iter().any(|n| n.contains("task @1")),
        "{refused:?}"
    );

    // Nothing was recorded for the refusal, and the span is still task 1's to close.
    assert_eq!(f.kinds(), [Kind::Enter]);
    f.as_task(
        Some(TaskId(1)),
        Op::Exit,
        "http",
        vec![first, ctor("Ok", Vec::new())],
    )
    .expect("task 1 closes its own span");
    assert_eq!(f.kinds(), [Kind::Enter, Kind::Exit]);
}

/// Two tasks interleaving under one channel, checked against the tree the record list itself
/// implies rather than against the driver's own bookkeeping.
#[test]
fn two_tasks_interleaving_produce_correctly_nested_parent_links() {
    let f = fixture();
    let one = |op, args| {
        f.as_task(Some(TaskId(1)), op, "http", args)
            .expect("task 1")
    };
    let two = |op, args| {
        f.as_task(Some(TaskId(2)), op, "http", args)
            .expect("task 2")
    };

    let a = one(Op::Enter, vec![ply_eval::Value::str("a"), no_fields()]);
    let b = two(Op::Enter, vec![ply_eval::Value::str("b"), no_fields()]);
    let a_inner = one(Op::Enter, vec![ply_eval::Value::str("a2"), no_fields()]);
    let b_inner = two(Op::Enter, vec![ply_eval::Value::str("b2"), no_fields()]);
    two(Op::Exit, vec![b_inner, ctor("Ok", Vec::new())]);
    one(Op::Exit, vec![a_inner, ctor("Ok", Vec::new())]);
    two(Op::Exit, vec![b, ctor("Ok", Vec::new())]);
    one(Op::Exit, vec![a, ctor("Ok", Vec::new())]);

    // The reference: each name's `enter`, and what its parent's name is.
    let records = f.records();
    let parent_of = |name: &str| -> String {
        let record = records
            .iter()
            .find(|r| r.name == name && r.kind == Kind::Enter)
            .expect("an enter");
        records
            .iter()
            .find(|r| r.span == record.parent && r.kind == Kind::Enter)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| "-".to_string())
    };
    assert_eq!(parent_of("a"), "-");
    assert_eq!(parent_of("b"), "-", "task 2's root is not inside task 1's");
    assert_eq!(parent_of("a2"), "a");
    assert_eq!(parent_of("b2"), "b");
    assert_eq!(f.trace.open_spans(), 0);
    assert_eq!(f.trace.counts().abandoned, 0);
}

/// The rollback shape: nothing runs the inner `exit`, so the outer one closes both and only the
/// outer carries the outcome the program named.
#[test]
fn closing_an_outer_span_abandons_the_spans_above_it() {
    let f = fixture();
    let outer = enter(&f, "orders", "place_order");
    let _inner = enter(&f, "orders", "reserve");
    f.perform(
        Op::Exit,
        "orders",
        vec![
            outer,
            ctor("Failed", vec![ply_eval::Value::str("rolled back")]),
        ],
    );

    let records = f.records();
    assert_eq!(
        f.kinds(),
        [Kind::Enter, Kind::Enter, Kind::Exit, Kind::Exit]
    );
    assert_eq!(records[2].span, 2, "innermost first");
    assert_eq!(records[2].outcome, Outcome::Abandoned);
    assert_eq!(records[3].span, 1);
    assert_eq!(
        records[3].outcome,
        Outcome::Failed("rolled back".to_string())
    );
    assert_eq!(f.trace.counts().abandoned, 1);
}

/// The fourth exit — the one no handler clause can catch.
#[test]
fn teardown_closes_what_the_program_left_open_and_reports_w0609() {
    let f = fixture();
    enter(&f, "http", "request");
    enter(&f, "db", "query");

    let warning = f
        .trace
        .end_entry_point(f.machine)
        .expect("two spans were open");
    assert_eq!(warning.code, codes::SPAN_ABANDONED);
    assert!(warning.message.contains("`query`"), "{}", warning.message);

    let records = f.records();
    assert_eq!(
        f.kinds(),
        [Kind::Enter, Kind::Enter, Kind::Exit, Kind::Exit]
    );
    assert_eq!(records[2].name, "query", "innermost first");
    assert!(
        records[2..].iter().all(|r| r.outcome == Outcome::Abandoned),
        "a span teardown closed is abandoned, not ok"
    );
    assert_eq!(f.trace.open_spans(), 0);
    assert!(
        f.trace.end_entry_point(f.machine).is_none(),
        "a second teardown has nothing to close"
    );
}

/// A `db.rollback` inside a span is a discarded continuation, so the span's record is the one thing
/// that says what the request was doing when it stopped.
#[test]
fn teardown_writes_before_it_flushes() {
    let f = fixture();
    enter(&f, "orders", "place_order");
    assert_eq!(f.sink.flushes(), 0);
    f.trace.end_entry_point(f.machine);
    assert_eq!(f.kinds(), [Kind::Enter, Kind::Exit]);
    f.trace.flush();
    assert_eq!(f.sink.flushes(), 1);
    assert!(f.trace.counts().flushed);
}

// --- metrics ----------------------------------------------------------------------

#[test]
fn a_metric_is_a_record_on_the_channel_the_call_site_named() {
    let f = fixture();
    f.perform(
        Op::Count,
        "orders",
        vec![
            ply_eval::Value::str("orders_placed"),
            ply_eval::Value::Int(3),
            no_fields(),
        ],
    );
    f.perform(
        Op::Gauge,
        "orders",
        vec![
            ply_eval::Value::str("shelf"),
            ply_eval::Value::Decimal("41.75".parse().expect("a decimal")),
            no_fields(),
        ],
    );
    f.perform(
        Op::Time,
        "orders",
        vec![
            ply_eval::Value::str("place_order"),
            ply_eval::Value::Int(8213),
            no_fields(),
        ],
    );

    let records = f.records();
    assert_eq!(f.kinds(), [Kind::Count, Kind::Gauge, Kind::Time]);
    assert_eq!(records[0].amount, Some(3));
    assert_eq!(
        records[1].value,
        Some("41.75".parse().expect("a decimal")),
        "a gauge is a `Decimal`, so a test can assert on it"
    );
    assert_eq!(records[2].amount, Some(8213));
    assert!(
        records.iter().all(|r| r.level == Level::Info),
        "a metric is the shape of a thing that happened"
    );
}

// --- what a span costs when nothing is collecting ---------------------------------

/// The claim a disabled span makes, in the one form that can be checked here: under `discard` no record is
/// written and the clock is read **zero** times, whatever the operation.
#[test]
fn a_discarded_record_reads_no_clock_and_writes_nothing() {
    let (trace, clock) = discarding();
    let machine = MachineId::next();
    let span = perform_on(
        &trace,
        machine,
        None,
        Op::Enter,
        "orders",
        vec![ply_eval::Value::str("place_order"), no_fields()],
    )
    .expect("a span opens whatever the sink does");

    for _ in 0..64 {
        perform_on(
            &trace,
            machine,
            None,
            Op::Event,
            "orders",
            vec![
                ctor("Error", Vec::new()),
                ply_eval::Value::str("placed"),
                field("sku", ctor("FText", vec![ply_eval::Value::str("BOLT-1")])),
            ],
        )
        .expect("an event is discarded, not refused");
    }
    perform_on(
        &trace,
        machine,
        None,
        Op::Exit,
        "orders",
        vec![span, ctor("Ok", Vec::new())],
    )
    .expect("the span closes");

    assert_eq!(
        clock.reads(),
        0,
        "a discarding sink never stamps, so a disabled span pays no clock read"
    );
    assert_eq!(trace.counts().events, 0);
}

/// And the span bookkeeping is kept anyway, because `E0445` is a statement about the program: a run
/// whose verdict moved with `--trace off` would be a run nobody could debug.
#[test]
fn spans_are_tracked_under_discard_so_a_verdict_does_not_depend_on_the_sink() {
    let (trace, _) = discarding();
    let machine = MachineId::next();
    let span = perform_on(
        &trace,
        machine,
        Some(TaskId(1)),
        Op::Enter,
        "http",
        vec![ply_eval::Value::str("request"), no_fields()],
    )
    .expect("a span opens");
    assert_eq!(trace.open_spans(), 1);

    let refused = perform_on(
        &trace,
        machine,
        Some(TaskId(2)),
        Op::Exit,
        "http",
        vec![span.clone(), ctor("Ok", Vec::new())],
    )
    .expect_err("the other task still may not close it");
    assert_eq!(refused.code, codes::SPAN_UNBALANCED);

    let warning = trace
        .end_entry_point(machine)
        .expect("teardown still reports what was left open");
    assert_eq!(warning.code, codes::SPAN_ABANDONED);
    assert!(
        warning.message.contains("`request`"),
        "the name is kept under `discard` so the warning can use it: {}",
        warning.message
    );
}

/// A level filter saves the record and nothing else, which is the honest claim: the perform and the
/// call site's map are already paid for by the time the sink is asked.
#[test]
fn a_level_filter_drops_the_record_and_reads_no_clock() {
    let f = fixture_at(Level::Warn);
    event(&f, "orders", "Debug", "chatty");
    event(&f, "orders", "Info", "ordinary");
    assert_eq!(f.clock.reads(), 0);
    assert!(f.records().is_empty());

    event(&f, "orders", "Warn", "slow");
    assert_eq!(f.clock.reads(), 1);
    assert_eq!(f.kinds(), [Kind::Event]);

    // A span is `Info`, so a `warn` filter drops it — and the stack still knows about it, which is
    // what keeps `E0445` and `W0609` right.
    let span = enter(&f, "orders", "place_order");
    assert_eq!(f.trace.open_spans(), 1);
    exit(&f, "orders", span);
    assert_eq!(f.kinds(), [Kind::Event], "nothing at `info` was written");
    assert_eq!(f.clock.reads(), 1, "and nothing at `info` was stamped");
}

/// A span's duration is the sink's, computed from the two stamps it took, so a call site never
/// reads a clock and `clock.read` never enters a tracing function's row.
#[test]
fn a_closing_span_carries_the_duration_the_sink_measured() {
    let f = fixture();
    let span = enter(&f, "http", "request");
    event(&f, "http", "Info", "midway");
    exit(&f, "http", span);
    let records = f.records();
    assert_eq!(records[0].ts, 1);
    assert_eq!(records[2].ts, 3);
    assert_eq!(
        records[2].amount,
        Some(2),
        "the exit carries `exit_ts - enter_ts`"
    );
}

// --- the wire format ---------------------------------------------------------------

/// The envelope is fixed and the program's fields are nested under `fields` **always**, so a
/// program cannot forge a level by naming a field.
#[test]
fn a_program_field_named_like_an_envelope_key_does_not_shadow_it() {
    let f = fixture();
    f.perform(
        Op::Event,
        "orders",
        vec![
            ctor("Info", Vec::new()),
            ply_eval::Value::str("placed"),
            ply_eval::Value::map([
                (
                    ply_eval::Value::str("level"),
                    ctor("FText", vec![ply_eval::Value::str("error")]),
                ),
                (
                    ply_eval::Value::str("span"),
                    ctor("FInt", vec![ply_eval::Value::Int(9999)]),
                ),
                (
                    ply_eval::Value::str("ts"),
                    ctor("FInt", vec![ply_eval::Value::Int(0)]),
                ),
            ]),
        ],
    );
    let line = f.sink.lines().pop().expect("one line");
    assert_eq!(
        line,
        r#"{"ts":1,"level":"info","channel":"orders","kind":"event","name":"placed","span":0,"parent":0,"fields":{"level":"error","span":9999,"ts":0}}"#
    );
}

#[test]
fn every_field_shape_renders_and_an_empty_field_set_still_writes_the_object() {
    let f = fixture();
    let render = |value: ply_eval::Value| -> String {
        f.perform(
            Op::Event,
            "orders",
            vec![
                ctor("Info", Vec::new()),
                ply_eval::Value::str("x"),
                field("f", value),
            ],
        );
        let line = f.sink.lines().pop().expect("a line");
        let at = line.find("\"fields\"").expect("the fields object");
        line[at..].to_string()
    };
    assert_eq!(
        render(ctor("FInt", vec![ply_eval::Value::Int(-3)])),
        r#""fields":{"f":-3}}"#
    );
    assert_eq!(
        render(ctor("FBool", vec![ply_eval::Value::Bool(true)])),
        r#""fields":{"f":true}}"#
    );
    assert_eq!(
        render(ctor("FText", vec![ply_eval::Value::str("a\"b\nc")])),
        r#""fields":{"f":"a\"b\nc"}}"#
    );
    // A `Decimal` is a string: its scale is a digit count the value carries, and a JSON number
    // consumer would round it away.
    assert_eq!(
        render(ctor(
            "FDecimal",
            vec![ply_eval::Value::Decimal(
                "41.750".parse().expect("a decimal")
            )]
        )),
        r#""fields":{"f":"41.750"}}"#
    );
    assert_eq!(
        render(ctor("FBytes", vec![ply_eval::Value::bytes([0u8, 255, 16])])),
        r#""fields":{"f":"00ff10"}}"#
    );
    // JSON has no NaN, so a writer that emitted one would produce a document no parser accepts.
    assert_eq!(
        render(ctor("FFloat", vec![ply_eval::Value::Float(f64::NAN)])),
        r#""fields":{"f":"NaN"}}"#
    );
    assert_eq!(
        render(ctor("FFloat", vec![ply_eval::Value::Float(1.5)])),
        r#""fields":{"f":1.5}}"#
    );

    f.perform(
        Op::Event,
        "orders",
        vec![
            ctor("Info", Vec::new()),
            ply_eval::Value::str("x"),
            no_fields(),
        ],
    );
    assert!(
        f.sink
            .lines()
            .pop()
            .expect("a line")
            .ends_with(r#""fields":{}}"#),
        "the object is written even when empty, so a consumer never has to test for it"
    );
}

#[test]
fn a_json_field_is_serialized_by_the_one_writer_this_crate_has() {
    let f = fixture();
    let json = |name: &str, args: Vec<ply_eval::Value>| {
        ply_eval::Value::ctor(format!("std.json.{name}"), args)
    };
    f.perform(
        Op::Event,
        "orders",
        vec![
            ctor("Info", Vec::new()),
            ply_eval::Value::str("x"),
            field(
                "body",
                ctor(
                    "FJson",
                    vec![json(
                        "Object",
                        vec![ply_eval::Value::map([
                            (
                                ply_eval::Value::str("sku"),
                                json("Str", vec![ply_eval::Value::str("BOLT-1")]),
                            ),
                            (
                                ply_eval::Value::str("n"),
                                json(
                                    "Number",
                                    vec![ply_eval::Value::Decimal("2".parse().expect("a decimal"))],
                                ),
                            ),
                        ])],
                    )],
                ),
            ),
        ],
    );
    let line = f.sink.lines().pop().expect("a line");
    assert!(
        line.ends_with(r#""fields":{"body":{"n":2,"sku":"BOLT-1"}}}"#),
        "{line}"
    );
}

#[test]
fn an_exit_carries_its_outcome_and_a_failure_carries_its_reason() {
    let f = fixture();
    let span = enter(&f, "orders", "place_order");
    f.perform(
        Op::Exit,
        "orders",
        vec![span, ctor("Failed", vec![ply_eval::Value::str("23514")])],
    );
    let line = f.sink.lines().pop().expect("a line");
    assert!(
        line.contains(r#""outcome":"failed","reason":"23514""#),
        "{line}"
    );
    // And an `enter` carries none: an outcome on a line that has not happened yet would be a fact
    // the run does not hold.
    assert!(!f.sink.lines()[0].contains("outcome"));
}

/// The `Map` order is the canonical one, so two field sets built in different orders produce the
/// same line and a golden test over it is stable.
#[test]
fn two_field_sets_built_in_different_orders_render_identically() {
    let render = |entries: [(&str, i64); 3]| {
        let f = fixture();
        f.perform(
            Op::Event,
            "orders",
            vec![
                ctor("Info", Vec::new()),
                ply_eval::Value::str("x"),
                ply_eval::Value::map(entries.map(|(k, v)| {
                    (
                        ply_eval::Value::str(k),
                        ctor("FInt", vec![ply_eval::Value::Int(v)]),
                    )
                })),
            ],
        );
        f.sink.lines().pop().expect("a line")
    };
    assert_eq!(
        render([("a", 1), ("b", 2), ("c", 3)]),
        render([("c", 3), ("a", 1), ("b", 2)])
    );
}

// --- shapes inference already checked, refused rather than guessed at --------------

#[test]
fn a_perform_the_front_end_could_not_have_produced_is_plys_fault() {
    let f = fixture();
    let wrong_arity = perform_on(
        &f.trace,
        f.machine,
        None,
        Op::Event,
        "orders",
        vec![ctor("Info", Vec::new())],
    )
    .expect_err("an event takes three arguments");
    assert_eq!(wrong_arity.code, codes::INTERNAL_ERROR);

    let wrong_level = perform_on(
        &f.trace,
        f.machine,
        None,
        Op::Event,
        "orders",
        vec![
            ply_eval::Value::Int(3),
            ply_eval::Value::str("x"),
            no_fields(),
        ],
    )
    .expect_err("a level is a constructor");
    assert_eq!(wrong_level.code, codes::INTERNAL_ERROR);
}
