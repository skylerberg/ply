//! What a trace operation costs when nothing is collecting, exactly.

use ply_core::ty::{EffectAtom, Resource};
use ply_eval::host::{
    HostAnswer, HostHandler, HostOp, HostRegistry, HostRequest, HostRuntime, MachineId, Pending,
};
use ply_eval::{TaskId, Value};
use ply_host::trace::{Clock, Discard, Json, Level, Op, Sink, Trace};
use ply_span::{Diagnostic, Span, Symbol};
use ply_syntax::ast::Mode;
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

/// The same handlers a bound run calls, over the same declarations `ply hosts` prints.
struct Bench {
    handlers: Vec<(HostOp, Arc<dyn HostHandler>)>,
    /// The resolved atom, built **once**.
    atom: EffectAtom,
    trace: Arc<Trace>,
    clock: Arc<CountedClock>,
    machine: MachineId,
}

fn bench(sink: Arc<dyn Sink>, channel: &str) -> Bench {
    let clock = Arc::new(CountedClock::default());
    let path = sink.path();
    let trace = Arc::new(Trace::with_clock(
        sink,
        Arc::clone(&clock) as Arc<dyn Clock>,
    ));
    let mut registry = HostRegistry::new();
    ply_host::trace::register(&mut registry, Arc::clone(&trace));
    let declarations: Vec<HostOp> = registry.ops().cloned().collect();
    assert!(
        declarations.iter().all(|op| op.path == path),
        "the listing names the sink that serves the run"
    );
    Bench {
        handlers: Op::ALL
            .into_iter()
            .zip(declarations)
            .map(|(op, declaration)| {
                (
                    declaration,
                    ply_host::trace::handler(op, Arc::clone(&trace)),
                )
            })
            .collect(),
        atom: EffectAtom::new(
            Symbol::new(ply_host::trace::EFFECT),
            Resource::Named(Symbol::new(channel)),
            Mode::Write,
        ),
        trace,
        clock,
        machine: MachineId::next(),
    }
}

impl Bench {
    fn at(&self, op: Op) -> &(HostOp, Arc<dyn HostHandler>) {
        let index = Op::ALL
            .iter()
            .position(|candidate| *candidate == op)
            .expect("every operation is registered");
        &self.handlers[index]
    }

    fn call(&self, op: Op, args: &[Value]) -> Result<Value, Diagnostic> {
        let (declaration, handler) = self.at(op);
        let request = HostRequest {
            atom: self.atom.clone(),
            op: declaration,
            args,
            span: Span::DUMMY,
            machine: self.machine,
            task: Some(TaskId(1)),
            declared: None,
        };
        match handler.call(&NoRuntime, &request)? {
            HostAnswer::Value(value) => Ok(value),
            HostAnswer::Pending(_) => panic!("`trace` never waits on a peer"),
        }
    }

    fn perform(&self, op: Op, args: &[Value]) -> Result<(), Diagnostic> {
        black_box(self.call(op, args)?);
        Ok(())
    }
}

fn ctor(name: &str, args: Vec<Value>) -> Value {
    Value::ctor(format!("std.trace.{name}"), args)
}

/// Three fields, which is what an endpoint's event actually carries.
fn fields() -> Value {
    Value::map([
        (Value::str("sku"), ctor("FText", vec![Value::str("BOLT-1")])),
        (Value::str("qty"), ctor("FInt", vec![Value::Int(3)])),
        (Value::str("rush"), ctor("FBool", vec![Value::Bool(false)])),
    ])
}

const EVENTS: usize = 1_000;

/// The headline number: a discarded event allocates nothing at all in the handler.
#[test]
fn a_discarded_event_allocates_nothing_and_reads_no_clock() {
    let b = bench(Arc::new(Discard), "orders");
    let level = ctor("Info", Vec::new());
    let name = Value::str("placed");
    let fields = fields();
    let args = [level, name, fields];

    // Warm: the first call through a `dyn` handler may fault in a page, and what is being measured
    // is the steady state a request path is in.
    for _ in 0..16 {
        b.perform(Op::Event, &args).expect("discarded");
    }

    let (_, allocs) = allocations_of(|| {
        for _ in 0..EVENTS {
            b.perform(Op::Event, &args).expect("discarded");
        }
    });

    assert_eq!(
        allocs, 0,
        "a discarded event must cost the handler nothing; it cost {allocs} allocations over {EVENTS} events"
    );
    assert_eq!(
        b.clock.0.load(Ordering::Relaxed),
        0,
        "a discarding sink never stamps, so a disabled event pays no clock read"
    );
    assert_eq!(b.trace.counts().events, 0);
}

/// The same for a metric, because a counter incremented per request is the operation a service
/// performs most.
#[test]
fn a_discarded_metric_allocates_nothing() {
    let b = bench(Arc::new(Discard), "orders");
    let args = [Value::str("orders_placed"), Value::Int(1), fields()];
    for _ in 0..16 {
        b.perform(Op::Count, &args).expect("discarded");
    }
    let (_, allocs) = allocations_of(|| {
        for _ in 0..EVENTS {
            b.perform(Op::Count, &args).expect("discarded");
        }
    });
    assert_eq!(allocs, 0, "a discarded counter cost {allocs} allocations");
}

/// A level filter is the sink's, so it saves everything a discard saves — and **not** the perform
/// or the call site's map, which is the honest half of the claim and is why this asserts the clock
/// rather than the wall clock.
#[test]
fn a_debug_event_under_a_warn_filter_costs_what_a_discarded_one_costs() {
    let b = bench(Arc::new(Json::new(Level::Warn)), "orders");
    let args = [ctor("Debug", Vec::new()), Value::str("chatty"), fields()];
    for _ in 0..16 {
        b.perform(Op::Event, &args).expect("filtered");
    }
    let (_, allocs) = allocations_of(|| {
        for _ in 0..EVENTS {
            b.perform(Op::Event, &args).expect("filtered");
        }
    });
    assert_eq!(
        allocs, 0,
        "a filtered event must not build a record; it cost {allocs} allocations"
    );
    assert_eq!(
        b.clock.0.load(Ordering::Relaxed),
        0,
        "a filtered event is not stamped"
    );
}

/// A span under `discard` is not free, and this is the number rather than a reassurance: the stack
/// entry and the `Span` record the operation's *type* requires are what it costs, and neither is
/// the sink's to avoid.
#[test]
fn a_discarded_span_costs_only_the_stack_entry_and_the_span_it_must_answer() {
    let b = bench(Arc::new(Discard), "http");
    let name = Value::str("request");
    let fields = fields();
    let enter = [name, fields];
    let ok = ctor("Ok", Vec::new());

    let cycle = |b: &Bench| {
        let span = b
            .call(Op::Enter, &enter)
            .expect("a span opens whatever the sink does");
        b.perform(Op::Exit, &[span, ok.clone()]).expect("it closes");
    };

    for _ in 0..16 {
        cycle(&b);
    }
    const SPANS: usize = 200;
    let (_, allocs) = allocations_of(|| {
        for _ in 0..SPANS {
            cycle(&b);
        }
    });
    let per_span = allocs as f64 / SPANS as f64;
    println!("  discarded span    {per_span:.1} allocations · 0 clock reads");
    assert!(
        per_span <= 8.0,
        "a disabled span costs {per_span:.2} allocations, which is more than the `Span` record and its stack entry"
    );
    assert_eq!(
        b.clock.0.load(Ordering::Relaxed),
        0,
        "a disabled span is never stamped, so `clock` stays out of every tracing function's row"
    );
    assert_eq!(b.trace.open_spans(), 0, "every span was closed");
}

/// The other end of the ratio, so the disabled number has something to be read against: a
/// *collected* event decodes its name and every field, which is what a run pays when it has asked
/// for the records.
#[test]
fn a_collected_event_pays_for_the_record_it_asked_for() {
    let sink = Arc::new(ply_host::trace::sink::Recording::new(Level::Debug));
    let b = bench(Arc::clone(&sink) as Arc<dyn Sink>, "orders");
    let args = [ctor("Info", Vec::new()), Value::str("placed"), fields()];
    for _ in 0..16 {
        b.perform(Op::Event, &args).expect("collected");
    }
    const COLLECTED: usize = 200;
    let (_, allocs) = allocations_of(|| {
        for _ in 0..COLLECTED {
            b.perform(Op::Event, &args).expect("collected");
        }
    });
    let per_event = allocs as f64 / COLLECTED as f64;
    assert!(
        per_event > 0.0,
        "a collected event that allocated nothing would mean nothing was collected"
    );
    assert_eq!(sink.records().len(), COLLECTED + 16);
    // The number this file exists to publish.
    println!("  discarded event   0 allocations · 0 clock reads");
    println!("  collected event   {per_event:.1} allocations · 1 clock read");
}
