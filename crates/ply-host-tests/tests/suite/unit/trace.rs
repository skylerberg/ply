mod spans;

use ply_core::host::MachineId;
use ply_core::{Span, TaskId, codes};
use ply_host::native::{int, parts};
use ply_host::trace::sink::{OwnedRecord, Recording};
use ply_host::trace::spans::err_unbalanced;
use ply_host::trace::*;
use std::sync::Arc;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
struct Counting {
    reads: AtomicU64,
    at: AtomicI64,
}

impl Clock for Counting {
    fn micros(&self) -> i64 {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.at.fetch_add(1, Ordering::Relaxed) + 1
    }
}

impl Counting {
    fn reads(&self) -> u64 {
        self.reads.load(Ordering::Relaxed)
    }
}

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

fn field(key: &str, value: Field) -> Vec<(String, Field)> {
    vec![(key.to_string(), value)]
}

impl Fixture {
    fn owner(&self, task: Option<TaskId>) -> Owner {
        (self.machine, task)
    }

    fn enter_as(&self, task: Option<TaskId>, at: &str, name: &str) -> i64 {
        self.trace.enter(self.owner(task), at, name, &[])
    }

    fn exit_as(
        &self,
        task: Option<TaskId>,
        at: &str,
        id: i64,
        outcome: Outcome,
    ) -> Result<(), Unbalanced> {
        self.trace.exit(self.owner(task), at, id, outcome)
    }

    fn event_with(&self, at: &str, level: Level, name: &str, fields: &[(String, Field)]) {
        self.trace.record(
            self.owner(None),
            RecordKind::Event,
            level,
            at,
            name,
            None,
            None,
            fields,
        );
    }

    fn kinds(&self) -> Vec<RecordKind> {
        self.sink.records().iter().map(|r| r.kind).collect()
    }

    fn records(&self) -> Vec<OwnedRecord> {
        self.sink.records()
    }
}

fn enter(f: &Fixture, at: &str, name: &str) -> i64 {
    f.enter_as(None, at, name)
}

fn exit(f: &Fixture, at: &str, id: i64) {
    f.exit_as(None, at, id, Outcome::Ok)
        .expect("the span is open");
}

fn event(f: &Fixture, at: &str, level: Level, name: &str) {
    f.event_with(at, level, name, &[]);
}

#[test]
fn a_nested_span_records_its_parent_and_an_event_inside_it_records_the_span() {
    let f = fixture();
    let outer = enter(&f, "http", "request");
    let inner = enter(&f, "http", "query");
    assert_eq!((outer, inner), (1, 2), "ids ascend from one");
    event(&f, "http", Level::Warn, "slow");
    exit(&f, "http", inner);
    exit(&f, "http", outer);

    let records = f.records();
    assert_eq!(
        f.kinds(),
        [
            RecordKind::Enter,
            RecordKind::Enter,
            RecordKind::Event,
            RecordKind::Exit,
            RecordKind::Exit
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

#[test]
fn a_span_opened_in_one_task_does_not_close_in_another() {
    let f = fixture();
    let first = f.enter_as(Some(TaskId(1)), "http", "request");

    let reason = f
        .exit_as(Some(TaskId(2)), "http", first, Outcome::Ok)
        .expect_err("task 2 must not close task 1's span");
    let rejected = err_unbalanced(Span::DUMMY, "`trace.exit`", first, &reason);
    assert_eq!(rejected.code, codes::SPAN_UNBALANCED);
    assert!(
        rejected.notes.iter().any(|n| n.contains("task @1")),
        "{rejected:?}"
    );

    assert_eq!(f.kinds(), [RecordKind::Enter]);
    f.exit_as(Some(TaskId(1)), "http", first, Outcome::Ok)
        .expect("task 1 closes its own span");
    assert_eq!(f.kinds(), [RecordKind::Enter, RecordKind::Exit]);
}

#[test]
fn closing_a_span_with_a_different_trace_channel_than_it_was_opened_with_is_rejected() {
    let f = fixture();
    let id = enter(&f, "http", "request");
    let reason = f
        .exit_as(None, "db", id, Outcome::Ok)
        .expect_err("the channel is part of the span");
    assert!(matches!(reason, Unbalanced::OtherChannel(_)), "{reason:?}");
    exit(&f, "http", id);
}

#[test]
fn two_tasks_interleaving_produce_correctly_nested_parent_links() {
    let f = fixture();
    let one = Some(TaskId(1));
    let two = Some(TaskId(2));
    let close = |task, id| {
        f.exit_as(task, "http", id, Outcome::Ok)
            .expect("the task closes its own span")
    };

    let a = f.enter_as(one, "http", "a");
    let b = f.enter_as(two, "http", "b");
    let a_inner = f.enter_as(one, "http", "a2");
    let b_inner = f.enter_as(two, "http", "b2");
    close(two, b_inner);
    close(one, a_inner);
    close(two, b);
    close(one, a);

    let records = f.records();
    let parent_of = |name: &str| -> String {
        let record = records
            .iter()
            .find(|r| r.name == name && r.kind == RecordKind::Enter)
            .expect("an enter");
        records
            .iter()
            .find(|r| r.span == record.parent && r.kind == RecordKind::Enter)
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

#[test]
fn closing_an_outer_span_writes_each_open_inner_span_as_ended_without_a_close() {
    let f = fixture();
    let outer = enter(&f, "orders", "place_order");
    let _inner = enter(&f, "orders", "reserve");
    f.exit_as(
        None,
        "orders",
        outer,
        Outcome::Failed("rolled back".to_string()),
    )
    .expect("the outer span is open");

    let records = f.records();
    assert_eq!(
        f.kinds(),
        [
            RecordKind::Enter,
            RecordKind::Enter,
            RecordKind::Exit,
            RecordKind::Exit
        ]
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
        [
            RecordKind::Enter,
            RecordKind::Enter,
            RecordKind::Exit,
            RecordKind::Exit
        ]
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

#[test]
fn when_a_task_ends_the_spans_it_left_open_are_written_with_an_abandoned_status() {
    let f = fixture();
    let task = TaskId(3);
    f.enter_as(Some(task), "http", "request");
    f.enter_as(Some(task), "db", "query");
    enter(&f, "http", "serve");

    f.trace.end_task(f.machine, task);
    let records = f.records();
    assert_eq!(
        f.kinds(),
        [
            RecordKind::Enter,
            RecordKind::Enter,
            RecordKind::Enter,
            RecordKind::Exit,
            RecordKind::Exit
        ]
    );
    assert_eq!(records[3].name, "query", "innermost first");
    assert_eq!(records[4].name, "request");
    assert!(records[3..].iter().all(|r| r.outcome == Outcome::Abandoned));
    assert_eq!(
        f.trace.open_spans(),
        1,
        "the entry point's own span stays open"
    );
    assert_eq!(
        f.trace.span_owners(),
        1,
        "the retired task's stack went with it"
    );
}

#[test]
fn a_span_left_open_by_a_task_that_has_ended_produces_warning_w0609_when_the_entry_point_ends() {
    let f = fixture();
    f.enter_as(Some(TaskId(3)), "db", "query");
    f.trace.end_task(f.machine, TaskId(3));
    assert_eq!(f.trace.open_spans(), 0);

    let warning = f
        .trace
        .end_entry_point(f.machine)
        .expect("the retired task's span is reported");
    assert_eq!(warning.code, codes::SPAN_ABANDONED);
    assert!(
        warning.message.contains(
            "1 span was still open when its task or the entry point ended: `query` on `db`"
        ),
        "{}",
        warning.message
    );
    assert_eq!(
        f.kinds(),
        [RecordKind::Enter, RecordKind::Exit],
        "written once, at retirement"
    );
    assert!(
        f.trace.end_entry_point(f.machine).is_none(),
        "reported once"
    );
}

#[test]
fn warning_w0609_counts_spans_left_open_by_ended_tasks_and_by_the_entry_point() {
    let f = fixture();
    let outer = enter(&f, "orders", "place_order");
    enter(&f, "orders", "reserve");
    exit(&f, "orders", outer);
    f.enter_as(Some(TaskId(3)), "db", "query");
    f.trace.end_task(f.machine, TaskId(3));
    enter(&f, "http", "request");

    let first = f
        .trace
        .end_entry_point(f.machine)
        .expect("two were left open");
    assert!(
        first.message.starts_with("2 spans were still open"),
        "{}",
        first.message
    );
    assert_eq!(f.trace.left_open(), 2);
    assert_eq!(
        f.trace.counts().abandoned,
        3,
        "the span closed through is `Abandoned` all the same"
    );

    enter(&f, "http", "request");
    let second = f
        .trace
        .end_entry_point(f.machine)
        .expect("one was left open");
    assert!(
        second.message.starts_with("1 span was still open"),
        "{}",
        second.message
    );
    assert_eq!(f.trace.left_open(), 3, "each entry point's count adds up");
}

#[test]
fn ending_the_entry_point_writes_the_closing_records_without_flushing_the_output_and_a_later_explicit_flush_does()
 {
    let f = fixture();
    enter(&f, "orders", "place_order");
    assert_eq!(f.sink.flushes(), 0);
    f.trace.end_entry_point(f.machine);
    assert_eq!(f.kinds(), [RecordKind::Enter, RecordKind::Exit]);
    f.trace.flush();
    assert_eq!(f.sink.flushes(), 1);
    assert!(f.trace.counts().flushed);
}

#[test]
fn the_count_gauge_and_time_metric_calls_each_write_a_trace_record_to_the_channel_the_caller_passes_in()
 {
    let f = fixture();
    let owner = f.owner(None);
    let gauge: ply_core::Decimal = "41.75".parse().expect("a decimal");
    let metric = |kind, name, amount, value| {
        f.trace
            .record(owner, kind, Level::Info, "orders", name, amount, value, &[])
    };
    metric(RecordKind::Count, "orders_placed", Some(3), None);
    metric(RecordKind::Gauge, "shelf", None, Some(gauge));
    metric(RecordKind::Time, "place_order", Some(8213), None);

    let records = f.records();
    assert_eq!(
        f.kinds(),
        [RecordKind::Count, RecordKind::Gauge, RecordKind::Time]
    );
    assert!(records.iter().all(|r| r.channel == "orders"));
    assert_eq!(records[0].amount, Some(3));
    assert_eq!(
        records[1].value,
        Some(gauge),
        "a gauge is a `Decimal`, so a test can assert on it"
    );
    assert_eq!(records[2].amount, Some(8213));
}

#[test]
fn a_discarded_record_reads_no_clock_and_writes_nothing() {
    let (trace, clock) = discarding();
    let owner = (MachineId::next(), None);
    let id = trace.enter(owner, "orders", "place_order", &[]);
    for _ in 0..64 {
        trace.record(
            owner,
            RecordKind::Event,
            Level::Error,
            "orders",
            "placed",
            None,
            None,
            &field("sku", Field::Text("BOLT-1".to_string())),
        );
    }
    trace
        .exit(owner, "orders", id, Outcome::Ok)
        .expect("the span closes");

    assert_eq!(
        clock.reads(),
        0,
        "a discarding sink never stamps, so a disabled span pays no clock read"
    );
    assert_eq!(trace.counts().events, 0);
    assert!(
        !trace.wants(Level::Error),
        "the handler need not write a field"
    );
}

#[test]
fn spans_are_still_tracked_when_trace_records_are_dropped_so_span_errors_and_warnings_are_the_same_whatever_output_is_used()
 {
    let (trace, _) = discarding();
    let machine = MachineId::next();
    let id = trace.enter((machine, Some(TaskId(1))), "http", "request", &[]);
    assert_eq!(trace.open_spans(), 1);

    let reason = trace
        .exit((machine, Some(TaskId(2))), "http", id, Outcome::Ok)
        .expect_err("the other task still may not close it");
    assert!(matches!(reason, Unbalanced::OtherOwner(_)), "{reason:?}");

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

#[test]
fn a_level_filter_drops_the_record_and_reads_no_clock() {
    let f = fixture_at(Level::Warn);
    assert!(!f.trace.wants(Level::Info));
    event(&f, "orders", Level::Debug, "chatty");
    event(&f, "orders", Level::Info, "ordinary");
    assert_eq!(f.clock.reads(), 0);
    assert!(f.records().is_empty());

    event(&f, "orders", Level::Warn, "slow");
    assert_eq!(f.clock.reads(), 1);
    assert_eq!(f.kinds(), [RecordKind::Event]);

    let span = enter(&f, "orders", "place_order");
    assert_eq!(f.trace.open_spans(), 1);
    exit(&f, "orders", span);
    assert_eq!(
        f.kinds(),
        [RecordKind::Event],
        "nothing at `info` was written"
    );
    assert_eq!(f.clock.reads(), 1, "and nothing at `info` was stamped");
}

#[test]
fn a_closing_span_carries_the_duration_the_sink_measured() {
    let f = fixture();
    let span = enter(&f, "http", "request");
    event(&f, "http", Level::Info, "midway");
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

#[test]
fn a_program_field_named_level_span_or_ts_does_not_overwrite_the_trace_line_own_field_with_that_name()
 {
    let f = fixture();
    f.event_with(
        "orders",
        Level::Info,
        "placed",
        &[
            ("level".to_string(), Field::Text("error".to_string())),
            ("span".to_string(), Field::Int(9999)),
            ("ts".to_string(), Field::Int(0)),
        ],
    );
    let line = f.sink.lines().pop().expect("one line");
    assert_eq!(
        line,
        r#"{"ts":1,"level":"info","channel":"orders","kind":"event","name":"placed","span":0,"parent":0,"fields":{"level":"error","span":9999,"ts":0}}"#
    );
}

#[test]
fn every_kind_of_trace_field_value_renders_as_json_and_an_event_with_no_fields_still_writes_an_empty_fields_object()
 {
    let f = fixture();
    let render = |value: Field| -> String {
        f.event_with("orders", Level::Info, "x", &field("f", value));
        let line = f.sink.lines().pop().expect("a line");
        let at = line.find("\"fields\"").expect("the fields object");
        line[at..].to_string()
    };
    assert_eq!(render(Field::Int(-3)), r#""fields":{"f":-3}}"#);
    assert_eq!(render(Field::Bool(true)), r#""fields":{"f":true}}"#);
    assert_eq!(
        render(Field::Text("a\"b\nc".to_string())),
        r#""fields":{"f":"a\"b\nc"}}"#
    );
    assert_eq!(
        render(Field::Decimal("41.750".parse().expect("a decimal"))),
        r#""fields":{"f":"41.750"}}"#
    );
    assert_eq!(
        render(Field::Bytes(vec![0, 255, 16])),
        r#""fields":{"f":"00ff10"}}"#
    );
    assert_eq!(render(Field::Float(f64::NAN)), r#""fields":{"f":"NaN"}}"#);
    assert_eq!(render(Field::Float(1.5)), r#""fields":{"f":1.5}}"#);
    assert_eq!(
        render(Field::Json(r#"{"n":2,"sku":"BOLT-1"}"#.to_string())),
        r#""fields":{"f":{"n":2,"sku":"BOLT-1"}}}"#
    );

    event(&f, "orders", Level::Info, "x");
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
fn an_exit_carries_its_outcome_and_a_failure_carries_its_reason() {
    let f = fixture();
    let span = enter(&f, "orders", "place_order");
    f.exit_as(None, "orders", span, Outcome::Failed("23514".to_string()))
        .expect("the span is open");
    let line = f.sink.lines().pop().expect("a line");
    assert!(
        line.contains(r#""outcome":"failed","reason":"23514""#),
        "{line}"
    );
    assert!(!f.sink.lines()[0].contains("outcome"));
}

#[test]
fn fields_read_each_kind_as_the_real_handler_writes_it() {
    let written = parts(&[
        b"n",
        &[&[0u8][..], &int(-3)].concat(),
        b"on",
        &[1, 1],
        b"sku",
        b"\x02BOLT-1",
        b"x",
        &[&[3u8][..], &int(ply_core::float::bits(1.5) as i64)].concat(),
        b"shelf",
        b"\x0441.750",
        b"raw",
        &[5, 0, 255],
        b"body",
        b"\x06{\"n\":2}",
    ]);
    let read = fields_read(&written).expect("every field reads");
    let named = |name: &str| {
        read.iter()
            .find(|(n, _)| n == name)
            .map(|(_, field)| field.clone())
            .expect("the field is there")
    };
    assert_eq!(read.len(), 7);
    assert_eq!(named("n"), Field::Int(-3));
    assert_eq!(named("on"), Field::Bool(true));
    assert_eq!(named("sku"), Field::Text("BOLT-1".to_string()));
    assert_eq!(named("x"), Field::Float(1.5));
    assert_eq!(
        named("shelf"),
        Field::Decimal("41.750".parse().expect("a decimal"))
    );
    assert_eq!(named("raw"), Field::Bytes(vec![0, 255]));
    assert_eq!(named("body"), Field::Json("{\"n\":2}".to_string()));
    assert_eq!(fields_read(&[]), Some(Vec::new()));
}

#[test]
fn fields_read_rejects_bytes_the_real_handler_never_writes() {
    let unpaired = parts(&[b"n"]);
    assert_eq!(fields_read(&unpaired), None, "a name with no field");
    let unknown = parts(&[b"n", b"\x09"]);
    assert_eq!(fields_read(&unknown), None, "a kind no field has");
    let flag = parts(&[b"on", &[1, 2]]);
    assert_eq!(fields_read(&flag), None, "a flag is 0 or 1");
    let decimal = parts(&[b"shelf", b"\x04forty"]);
    assert_eq!(fields_read(&decimal), None, "a decimal's text is a decimal");
    assert_eq!(fields_read(&[0, 0, 0, 9, 1]), None, "a part cut short");
}
