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

/// A clock nobody may read without it being visible.
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
    /// Three fields, which is what an endpoint's event actually carries, read once outside the
    /// measurement as the native reads them.
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

/// Warm first: a first call through `dyn` may fault in a page; the steady state is what is
/// measured.
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
fn a_discarded_event_allocates_nothing_and_reads_no_clock() {
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
fn a_debug_event_under_a_warn_filter_costs_what_a_discarded_one_costs() {
    let b = bench(Arc::new(Json::new(Level::Warn)));
    assert!(!b.trace.wants(Level::Debug), "the handler writes no field");
    let allocs = steady(EVENTS, || b.event(Level::Debug));
    assert_eq!(
        allocs, 0,
        "a filtered event must not build a record; it cost {allocs} allocations"
    );
    assert_eq!(b.clock_reads(), 0, "a filtered event is not stamped");
}

/// It costs the stack entry its `exit` is checked against, which is not the sink's.
#[test]
fn a_discarded_span_costs_less_than_a_collected_one_and_reads_no_clock() {
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

/// The collected cost, for the discarded ones to be read against.
#[test]
fn a_collected_event_pays_for_the_record_it_asked_for() {
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
