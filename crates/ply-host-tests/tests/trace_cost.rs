use ply_core::TaskId;
use ply_core::host::MachineId;
use ply_host::trace::{
    Clock, Discard, Field, Json, Level, Outcome, Owner, RecordKind, Sink, Trace,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

thread_local! {
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
}

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn allocations_of<T>(f: impl FnOnce() -> T) -> (T, usize) {
    ALLOCS.with(|c| c.set(0));
    let out = f();
    (out, ALLOCS.with(Cell::get))
}

#[derive(Default)]
struct CountedClock(AtomicU64);

impl Clock for CountedClock {
    fn micros(&self) -> i64 {
        self.0.fetch_add(1, Ordering::Relaxed) as i64 + 1
    }
}

struct Bench {
    trace: Arc<Trace>,
    clock: Arc<CountedClock>,
    owner: Owner,
    fields: Vec<(String, Field)>,
}

fn bench(sink: Arc<dyn Sink>) -> Bench {
    let clock = Arc::new(CountedClock::default());
    Bench {
        trace: Arc::new(Trace::with_clock(
            sink,
            Arc::clone(&clock) as Arc<dyn Clock>,
        )),
        clock,
        owner: (MachineId::next(), Some(TaskId(1))),
        fields: vec![
            ("sku".to_string(), Field::Text("BOLT-1".to_string())),
            ("qty".to_string(), Field::Int(3)),
            ("rush".to_string(), Field::Bool(false)),
        ],
    }
}

impl Bench {
    fn event(&self, level: Level) {
        self.trace.record(
            self.owner,
            RecordKind::Event,
            level,
            "orders",
            "placed",
            None,
            None,
            black_box(&self.fields),
        );
    }

    fn count(&self) {
        self.trace.record(
            self.owner,
            RecordKind::Count,
            Level::Info,
            "orders",
            "orders_placed",
            Some(1),
            None,
            black_box(&self.fields),
        );
    }

    fn span(&self) {
        let id = self
            .trace
            .enter(self.owner, "http", "request", black_box(&self.fields));
        self.trace
            .exit(self.owner, "http", id, Outcome::Ok)
            .expect("it closes");
    }

    fn clock_reads(&self) -> u64 {
        self.clock.0.load(Ordering::Relaxed)
    }
}

const EVENTS: usize = 1_000;

fn steady(times: usize, mut each: impl FnMut()) -> usize {
    for _ in 0..16 {
        each();
    }
    allocations_of(|| {
        for _ in 0..times {
            each();
        }
    })
    .1
}

#[test]
fn an_event_that_the_trace_handler_discards_allocates_no_memory_and_does_not_read_the_time() {
    let b = bench(Arc::new(Discard));
    assert!(!b.trace.wants(Level::Info), "the handler writes no field");
    let allocs = steady(EVENTS, || b.event(Level::Info));
    assert_eq!(
        allocs, 0,
        "a discarded event must cost the run nothing; it cost {allocs} allocations over {EVENTS} events"
    );
    assert_eq!(
        b.clock_reads(),
        0,
        "a discarding sink never stamps, so a disabled event pays no clock read"
    );
    assert_eq!(b.trace.counts().events, 0);
}

#[test]
fn a_discarded_metric_allocates_nothing() {
    let b = bench(Arc::new(Discard));
    let allocs = steady(EVENTS, || b.count());
    assert_eq!(allocs, 0, "a discarded counter cost {allocs} allocations");
}

#[test]
fn a_debug_event_when_the_level_filter_is_warn_allocates_no_memory_and_does_not_read_the_time() {
    let b = bench(Arc::new(Json::new(Level::Warn)));
    assert!(!b.trace.wants(Level::Debug), "the handler writes no field");
    let allocs = steady(EVENTS, || b.event(Level::Debug));
    assert_eq!(
        allocs, 0,
        "a filtered event must not build a record; it cost {allocs} allocations"
    );
    assert_eq!(b.clock_reads(), 0, "a filtered event is not stamped");
}

#[test]
fn a_span_that_the_trace_handler_discards_makes_fewer_allocations_than_a_recorded_span_and_does_not_read_the_time()
 {
    const SPANS: usize = 200;
    let discarded = bench(Arc::new(Discard));
    let discarding = steady(SPANS, || discarded.span());
    let collected = bench(Arc::new(ply_host::trace::sink::Recording::new(
        Level::Debug,
    )));
    let collecting = steady(SPANS, || collected.span());
    assert!(
        discarding < collecting,
        "a disabled span cost {discarding} allocations over {SPANS} spans, a collected one {collecting}"
    );
    assert_eq!(
        discarded.clock_reads(),
        0,
        "a disabled span is never stamped, so `clock` stays out of every tracing function's row"
    );
    assert_eq!(discarded.trace.open_spans(), 0, "every span was closed");
}

#[test]
fn an_event_recorded_by_the_sink_allocates_memory_for_its_record() {
    let sink = Arc::new(ply_host::trace::sink::Recording::new(Level::Debug));
    let b = bench(Arc::clone(&sink) as Arc<dyn Sink>);
    const COLLECTED: usize = 200;
    let allocs = steady(COLLECTED, || b.event(Level::Info));
    assert!(
        allocs > 0,
        "a collected event that allocated nothing would mean nothing was collected"
    );
    assert_eq!(sink.records().len(), COLLECTED + 16);
}
