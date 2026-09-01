//! What a request allocates, attributed to the `Value` it is building.

// `Value::Record` and `Value::Closure` hold an `Arc` over a type that is not `Send`: a `Value` is
// thread-confined by design (`value.rs`'s note on `RcK`), and building one here has to use the same
// `Arc` the enum declares.
#![allow(clippy::arc_with_non_send_sync)]

use ply_eval::{ARGUMENT_VECTOR_CLASSES as ARGV_CLASSES, Closure, ClosureKind, Value};
use ply_span::Symbol;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static INSIDE: Cell<bool> = const { Cell::new(false) };
    static SITES: RefCell<HashMap<Key, Count>> = RefCell::new(HashMap::new());
    /// Code address -> the `ply_*` names at it.
    static NAMES: RefCell<HashMap<usize, Vec<String>>> = RefCell::new(HashMap::new());
    static TOTAL: Cell<usize> = const { Cell::new(0) };
    static BYTES: Cell<usize> = const { Cell::new(0) };
}

/// An allocation's size and the frames that asked for it.
type Key = (usize, String);

#[derive(Clone, Copy, Default)]
struct Count {
    calls: usize,
    bytes: usize,
}

struct Tracing;

unsafe impl GlobalAlloc for Tracing {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let armed = ARMED.try_with(Cell::get).unwrap_or(false);
        if armed && !INSIDE.try_with(Cell::get).unwrap_or(true) {
            let _ = INSIDE.try_with(|c| c.set(true));
            let _ = TOTAL.try_with(|c| c.set(c.get() + 1));
            let _ = BYTES.try_with(|c| c.set(c.get() + layout.size()));
            let key = (layout.size(), frames());
            let _ = SITES.try_with(|s| {
                let mut s = s.borrow_mut();
                let e = s.entry(key).or_default();
                e.calls += 1;
                e.bytes += layout.size();
            });
            let _ = INSIDE.try_with(|c| c.set(false));
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Tracing = Tracing;

/// The nearest three `ply_*` frames.
fn frames() -> String {
    let mut out: Vec<String> = Vec::new();
    backtrace::trace(|frame| {
        out.extend(named(frame));
        out.len() < 3
    });
    out.truncate(3);
    if out.is_empty() {
        "<no ply frame>".to_string()
    } else {
        out.join(" < ")
    }
}

/// The `ply_*` names at one frame, or empty for a frame this file drops.
fn named(frame: &backtrace::Frame) -> Vec<String> {
    let ip = frame.ip() as usize;
    if let Some(hit) = NAMES.with(|c| c.borrow().get(&ip).cloned()) {
        return hit;
    }
    let mut found: Vec<String> = Vec::new();
    backtrace::resolve_frame(frame, |symbol| {
        let Some(name) = symbol.name() else { return };
        let name = name.to_string();
        if !name.starts_with("ply_") || name.contains("r4_value_construction") {
            return;
        }
        let cut = name.rfind("::h").map(|i| &name[..i]).unwrap_or(&name);
        found.push(cut.to_string());
    });
    NAMES.with(|c| c.borrow_mut().insert(ip, found.clone()));
    found
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives two levels below the repository root")
        .to_path_buf()
}

#[derive(Clone)]
struct Window {
    requests: usize,
    total: usize,
    bytes: usize,
    sites: HashMap<Key, Count>,
}

fn capture<T>(requests: usize, f: impl FnOnce() -> T) -> Window {
    SITES.with(|s| s.borrow_mut().clear());
    TOTAL.with(|c| c.set(0));
    BYTES.with(|c| c.set(0));
    ARMED.with(|c| c.set(true));
    let answered = f();
    ARMED.with(|c| c.set(false));
    drop(answered);
    Window {
        requests,
        total: TOTAL.with(Cell::get),
        bytes: BYTES.with(Cell::get),
        sites: SITES.with(|s| s.borrow().clone()),
    }
}

/// A pair of windows over one call, so a per-request slope can be separated from a per-`Machine`
/// intercept.
struct Fit {
    small: Window,
    large: Window,
}

impl Fit {
    fn span(&self) -> f64 {
        (self.large.requests - self.small.requests) as f64
    }

    fn at(w: &Window, k: &Key) -> (f64, f64) {
        let c = w.sites.get(k).copied().unwrap_or_default();
        (c.calls as f64, c.bytes as f64)
    }

    /// Allocations and bytes a further request would add.
    fn slope(&self, k: &Key) -> (f64, f64) {
        let (lc, lb) = Self::at(&self.large, k);
        let (sc, sb) = Self::at(&self.small, k);
        ((lc - sc) / self.span(), (lb - sb) / self.span())
    }

    fn intercept(&self, k: &Key) -> f64 {
        let (sc, _) = Self::at(&self.small, k);
        sc - self.slope(k).0 * self.small.requests as f64
    }

    fn total_slope(&self) -> (f64, f64) {
        (
            (self.large.total - self.small.total) as f64 / self.span(),
            (self.large.bytes - self.small.bytes) as f64 / self.span(),
        )
    }

    fn total_intercept(&self) -> f64 {
        self.small.total as f64 - self.total_slope().0 * self.small.requests as f64
    }

    fn keys(&self) -> Vec<Key> {
        let mut k: Vec<Key> = self
            .large
            .sites
            .keys()
            .chain(self.small.sites.keys())
            .cloned()
            .collect();
        k.sort();
        k.dedup();
        k
    }
}

/// What a bucket says about the allocation: which `Value` it is part of, or that it is not part of
/// one.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Kind {
    /// A `Value` variant's own heap payload.
    Payload,
    /// A `Vec<Value>` that a `Value` is about to be built from, or that a call passes its arguments
    /// in.
    Spine,
    /// Neither: control stack, lowering, interning, formatting, the host.
    Other,
}

/// `(bucket, kind, frame substring, size predicate)`, first match wins.
struct Rule {
    bucket: &'static str,
    kind: Kind,
    frame: &'static str,
    size: fn(usize) -> bool,
}

fn any(_: usize) -> bool {
    true
}
fn arc_header(n: usize) -> bool {
    n == 40
}
fn closure(n: usize) -> bool {
    n == 80
}
fn btree_node(n: usize) -> bool {
    n == 544
}
fn value_vec(n: usize) -> bool {
    n >= size_of::<Value>() && n.is_multiple_of(size_of::<Value>()) && n <= 32 * 64
}

const RULES: &[Rule] = &[
    // `interp::literal` (crates/ply-eval/src/interp.rs) allocates for `Lit::Str` and `Lit::Bytes`
    // and for nothing else.
    Rule {
        bucket: "Value::Str|Bytes — literal, rebuilt per evaluation",
        kind: Kind::Payload,
        frame: "ply_eval::interp::literal",
        size: any,
    },
    // `Value::str` / `Value::bytes` (value.rs:154, :158): one `Arc` each.
    Rule {
        bucket: "Value::Str — computed",
        kind: Kind::Payload,
        frame: "ply_eval::value::Value::str",
        size: any,
    },
    Rule {
        bucket: "Value::Bytes — computed",
        kind: Kind::Payload,
        frame: "ply_eval::value::Value::bytes",
        size: any,
    },
    // `ctor_value` (interp.rs) used to build a fresh `Arc<Closure>` for every mention of a
    // constructor of arity >= 1 and a fresh `Value::Ctor` for every mention of a nullary one; it
    // now shares one value per constructor per thread, so both rows read 0.0 per request.
    Rule {
        bucket: "Value::Ctor — nullary, rebuilt per mention",
        kind: Kind::Payload,
        frame: "ply_eval::value::Value::ctor < ply_eval::interp::ctor_value",
        size: arc_header,
    },
    Rule {
        bucket: "Value::Closure — constructor, rebuilt per mention",
        kind: Kind::Payload,
        frame: "ply_eval::interp::ctor_value",
        size: closure,
    },
    // Every other `Value::ctor` (value.rs:180) — builtins and the host.
    Rule {
        bucket: "Value::Ctor — builtins and host",
        kind: Kind::Payload,
        frame: "ply_eval::value::Value::ctor",
        size: any,
    },
    // `enter_closure`'s `ClosureKind::Ctor` arm (machine.rs) does `Arc::new(args)` inline, so
    // `Machine::apply` is the deepest frame and 40 is `ArcInner<Vec<Value>>`.
    Rule {
        bucket: "Value::Ctor — applied constructor",
        kind: Kind::Payload,
        frame: "ply_eval::machine::Machine::apply",
        size: arc_header,
    },
    // `Value::list` (value.rs:162) — the `Arc`; the `Vec` under it is Spine and was allocated by
    // whoever filled it.
    Rule {
        bucket: "Value::List",
        kind: Kind::Payload,
        frame: "ply_eval::value::Value::list",
        size: any,
    },
    // `Frame::ListItem` and `Frame::RecordField` (frame.rs:259, :300) build the `Arc` inline, so
    // the deepest frame is `frame::dispatch` and 40 bytes is `ArcInner<Vec<Value>>` or
    // `ArcInner<BTreeMap<Symbol, Value>>` — the same size, and this instrument cannot separate
    // them.
    Rule {
        bucket: "Value::List|Record — Arc header built in a frame",
        kind: Kind::Payload,
        frame: "ply_eval::frame",
        size: arc_header,
    },
    // A record's B-tree, built in `Frame::RecordField` (frame.rs:258-259).
    Rule {
        bucket: "Value::Record — B-tree node",
        kind: Kind::Payload,
        frame: "ply_eval::frame",
        size: btree_node,
    },
    Rule {
        bucket: "Value::Map — rpds node",
        kind: Kind::Payload,
        frame: "ply_eval::map",
        size: any,
    },
    // Captured continuations.
    Rule {
        bucket: "Value::Continuation",
        kind: Kind::Payload,
        frame: "ply_eval::cont::Stack::capture",
        size: any,
    },
    // `Frame::AppCallee`'s call to `argv::take` (frame.rs:111), `32 * arity` bytes — one per
    // application that the free list could not serve, which since ADR 0019 §1 is not every
    // application.
    Rule {
        bucket: "Vec<Value> — call arguments",
        kind: Kind::Spine,
        frame: "ply_eval::frame",
        size: value_vec,
    },
    // `Frame::ListItem`'s and the machine's own `Vec<Value>` buffers.
    Rule {
        bucket: "Vec<Value> — list and pattern spines",
        kind: Kind::Spine,
        frame: "ply_eval::machine::Machine::match_pattern",
        size: value_vec,
    },
    Rule {
        bucket: "Vec<Value> — evaluation spines",
        kind: Kind::Spine,
        frame: "ply_eval::machine::Machine::step",
        size: value_vec,
    },
    Rule {
        bucket: "Vec<Value> — builtin spines",
        kind: Kind::Spine,
        frame: "ply_eval::builtins",
        size: value_vec,
    },
    // Not a Value, and named so a reader can see how much of a request is not about values at all.
    Rule {
        bucket: "control stack — frame pool",
        kind: Kind::Other,
        frame: "ply_eval::pool",
        size: any,
    },
    Rule {
        bucket: "environment — scope release",
        kind: Kind::Other,
        frame: "ply_eval::env",
        size: any,
    },
    Rule {
        bucket: "lowering — compile-time, per Machine",
        kind: Kind::Other,
        frame: "ply_eval::code",
        size: any,
    },
    Rule {
        bucket: "reference counting — cell reachability",
        kind: Kind::Other,
        frame: "ply_eval::rc",
        size: any,
    },
    Rule {
        bucket: "name interning",
        kind: Kind::Other,
        frame: "ply_span::Symbol",
        size: any,
    },
    Rule {
        bucket: "host boundary — footprints, sockets, diagnostics",
        kind: Kind::Other,
        frame: "ply_eval::machine::Machine::perform_host",
        size: any,
    },
    Rule {
        bucket: "host boundary — footprints, sockets, diagnostics",
        kind: Kind::Other,
        frame: "ply_host",
        size: any,
    },
    // The same allocations as the two rules above, under the one spelling the three-frame window
    // cannot reach past.
    Rule {
        bucket: "host boundary — footprints, sockets, diagnostics",
        kind: Kind::Other,
        frame: "ply_std::is_reserved",
        size: any,
    },
    Rule {
        bucket: "region arena and scheduler",
        kind: Kind::Other,
        frame: "ply_eval::region",
        size: any,
    },
    Rule {
        bucket: "region arena and scheduler",
        kind: Kind::Other,
        frame: "ply_eval::arena",
        size: any,
    },
    Rule {
        bucket: "region arena and scheduler",
        kind: Kind::Other,
        frame: "ply_eval::sim",
        size: any,
    },
    Rule {
        bucket: "region arena and scheduler",
        kind: Kind::Other,
        frame: "ply_eval::sched",
        size: any,
    },
];

fn classify(key: &Key) -> Option<(&'static str, Kind)> {
    RULES
        .iter()
        .find(|r| key.1.contains(r.frame) && (r.size)(key.0))
        .map(|r| (r.bucket, r.kind))
}

/// How many allocations per request the rule table is allowed to leave unnamed.
const UNATTRIBUTED_CEILING_HEALTH: f64 = 60.0;
const UNATTRIBUTED_CEILING_ROUTING: f64 = 40.0;

/// The window this file's companion, `w6_alloc_sites.rs`, has always ranked at.
const SMALL: usize = 20;

/// The window `w6-alloc` and every published request-path figure use.
const LARGE: usize = 200;

fn script(request: &[u8], requests: usize) -> Vec<Vec<Vec<u8>>> {
    (0..requests).map(|_| vec![request.to_vec()]).collect()
}

fn health(loaded: &ply_corpus::w3::Loaded) -> Fit {
    let request = ply_corpus::w6_run::head();
    loaded
        .over_sim(vec![vec![request.clone()]])
        .expect("the service serves one connection");
    let small = capture(SMALL, || {
        loaded
            .over_sim(script(&request, SMALL))
            .expect("the service serves")
    });
    let large = capture(LARGE, || {
        loaded
            .over_sim(script(&request, LARGE))
            .expect("the service serves")
    });
    Fit { small, large }
}

fn routing(loaded: &ply_corpus::w3::Loaded) -> Fit {
    let bench = loaded
        .full("w6_bench")
        .expect("the driver declares w6_bench");
    let go = |n: usize| {
        loaded
            .pure_call(&bench, vec![Value::Int(3), Value::Int(n as i64)], 1)
            .expect("the driver runs")
    };
    go(4);
    let small = capture(SMALL, || go(SMALL));
    let large = capture(LARGE, || go(LARGE));
    Fit { small, large }
}

struct Row {
    bucket: String,
    kind: Kind,
    calls: f64,
    bytes: f64,
    fixed: f64,
}

fn roll_up(fit: &Fit) -> (Vec<Row>, Vec<(Key, f64)>) {
    let mut buckets: HashMap<(&'static str, Kind), (f64, f64, f64)> = HashMap::new();
    let mut loose: Vec<(Key, f64)> = Vec::new();
    for key in fit.keys() {
        let (calls, bytes) = fit.slope(&key);
        let fixed = fit.intercept(&key);
        match classify(&key) {
            Some((bucket, kind)) => {
                let e = buckets.entry((bucket, kind)).or_insert((0.0, 0.0, 0.0));
                e.0 += calls;
                e.1 += bytes;
                e.2 += fixed;
            }
            None => {
                if calls != 0.0 || fixed != 0.0 {
                    loose.push((key, calls));
                }
            }
        }
    }
    let mut rows: Vec<Row> = buckets
        .into_iter()
        .map(|((bucket, kind), (calls, bytes, fixed))| Row {
            bucket: bucket.to_string(),
            kind,
            calls,
            bytes,
            fixed,
        })
        .collect();
    rows.sort_by(|a, b| b.calls.total_cmp(&a.calls));
    loose.sort_by(|a, b| b.1.total_cmp(&a.1));
    (rows, loose)
}

/// Answers `(share placed, allocations left unnamed)`, both per request.
fn report(name: &str, fit: &Fit) -> (f64, f64) {
    let (slope, byte_slope) = fit.total_slope();
    let (rows, loose) = roll_up(fit);
    println!("\n================ {name}");
    println!(
        "  {SMALL} requests: {:>8.1} allocations each\n  {LARGE} requests: {:>8.1} allocations each\n  \
         fit:            {slope:>8.1} allocations + {:.0} bytes per request, and {:.0} allocations once per Machine",
        fit.small.total as f64 / fit.small.requests as f64,
        fit.large.total as f64 / fit.large.requests as f64,
        byte_slope,
        fit.total_intercept(),
    );

    for kind in [Kind::Payload, Kind::Spine, Kind::Other] {
        let subtotal: f64 = rows
            .iter()
            .filter(|r| r.kind == kind)
            .map(|r| r.calls)
            .sum();
        let label = match kind {
            Kind::Payload => "a Value's own heap payload",
            Kind::Spine => "a Vec<Value>: not a variant, but size_of::<Value>() wide per element",
            Kind::Other => "not a Value",
        };
        println!(
            "\n  -- {label}: {subtotal:.1} per request, {:.1}%",
            100.0 * subtotal / slope
        );
        println!(
            "     {:>8} {:>7} {:>9} {:>10}  bucket",
            "per req", "share", "bytes/req", "per Machine"
        );
        for row in rows.iter().filter(|r| r.kind == kind) {
            println!(
                "     {:>8.1} {:>6.1}% {:>9.0} {:>10.0}  {}",
                row.calls,
                100.0 * row.calls / slope,
                row.bytes,
                row.fixed,
                row.bucket
            );
        }
    }

    let placed: f64 = rows.iter().map(|r| r.calls).sum();
    let residue = slope - placed;
    println!(
        "\n  -- unattributed: {residue:.1} per request, {:.1}%",
        100.0 * residue / slope
    );
    for (key, calls) in loose.iter().take(12) {
        if *calls < 0.5 {
            continue;
        }
        println!("     {calls:>8.1} {:>8}B  {}", key.0, key.1);
    }
    (placed / slope, residue)
}

#[test]
fn a_requests_allocations_are_attributed_to_the_values_they_build() {
    let loaded = ply_corpus::w6_run::program(&repo()).expect("the service must compile");

    let health = health(&loaded);
    let (covered_health, residue_health) =
        report("/health, over SimNet — the served request path", &health);

    let routing = routing(&loaded);
    let (covered_routing, residue_routing) = report(
        "the routing rung, a pure call — the interpreter with no socket in it",
        &routing,
    );

    println!(
        "\n  The two routes disagree on ranking and both are printed: the SimNet path is the \
         only one that pays for framing, the host boundary and the response encode, and the \
         pure call is the only one with no socket in it. A lever is judged on the route it \
         would run on."
    );

    for (name, fit) in [
        ("/health over SimNet", &health),
        ("the routing rung", &routing),
    ] {
        let (slope, _) = fit.total_slope();
        let (rows, _) = roll_up(fit);
        let literal = rows
            .iter()
            .find(|r| r.bucket.starts_with("Value::Str|Bytes — literal"))
            .map(|r| (r.calls, r.fixed))
            .unwrap_or((0.0, 0.0));
        let rebuilt: f64 = rows
            .iter()
            .filter(|r| r.bucket.contains("rebuilt per"))
            .map(|r| r.calls)
            .sum();
        println!(
            "\n  {name}\n    literal Str|Bytes construction: {:.1} of {slope:.1} allocations per \
             request = {:.1}% (and {:.0} once per Machine)\n    every Value rebuilt from a \
             compile-time constant — literals plus constructor mentions: {rebuilt:.1} = {:.1}%",
            literal.0,
            100.0 * literal.0 / slope,
            literal.1,
            100.0 * rebuilt / slope,
        );
        // This read `assert!(literal.0 > 0.0)` and said that a zero here meant either that a
        // literal had started being built once — "the change this file exists to detect" — or that
        // the rule had stopped matching.
        assert_eq!(
            literal.0, 0.0,
            "{name} attributes {:.1} allocations per request to a literal: `NodeKind::Lit` \
             carries the value it denotes and `Machine::eval` clones it, so nothing on this \
             path should reach `interp::literal` at all — either the hoist regressed or the \
             constructor memo is not on this route",
            literal.0
        );
    }

    for (name, residue, ceiling, covered) in [
        (
            "/health over SimNet",
            residue_health,
            UNATTRIBUTED_CEILING_HEALTH,
            covered_health,
        ),
        (
            "the routing rung",
            residue_routing,
            UNATTRIBUTED_CEILING_ROUTING,
            covered_routing,
        ),
    ] {
        assert!(
            residue <= ceiling,
            "on {name} the rule table left {residue:.1} allocations per request unnamed, above \
             the ceiling of {ceiling:.1}: a rule has stopped matching the tree and the buckets \
             above understate whatever it used to hold. This is an absolute count on purpose — \
             it does not move when a lever removes allocations the table already names. The \
             share, {:.1}%, is a diagnostic and is not asserted.",
            100.0 * covered
        );
    }
}

/// A micro-program with one construction per loop body, so a difference against the empty loop is
/// that construction and nothing else.
const MICRO: &str = r#"
type R4Colour = R4Red | R4Green | R4Blue
type R4Boxed = R4Box(Int)

fn r4_id1(a: Int) -> Int = a
fn r4_id3(a: Int, b: Int, c: Int) -> Int = a + b + c
fn r4_rank(c: R4Colour) -> Int = match c { R4Red -> 1, R4Green -> 2, R4Blue -> 3 }
fn r4_unbox(b: R4Boxed) -> Int = match b { R4Box(v) -> v }

fn r4_base(n: Int, acc: Int) -> Int =
  if n <= 0 { acc } else { r4_base(n - 1, acc) }

fn r4_call1(n: Int, acc: Int) -> Int =
  if n <= 0 { acc } else { r4_call1(n - 1, acc + r4_id1(1)) }

fn r4_call3(n: Int, acc: Int) -> Int =
  if n <= 0 { acc } else { r4_call3(n - 1, acc + r4_id3(1, 2, 3)) }

fn r4_str(n: Int, acc: Int) -> Int =
  if n <= 0 { acc } else { r4_str(n - 1, acc + string_len("abcd")) }

fn r4_bytes(n: Int, acc: Int) -> Int =
  if n <= 0 { acc } else { r4_bytes(n - 1, acc + bytes_len(b"abcd")) }

fn r4_nullary(n: Int, acc: Int) -> Int =
  if n <= 0 { acc } else { r4_nullary(n - 1, acc + r4_rank(R4Red)) }

fn r4_applied(n: Int, acc: Int) -> Int =
  if n <= 0 { acc } else { r4_applied(n - 1, acc + r4_unbox(R4Box(1))) }
"#;

/// Iterations the micro-loops are fitted over.
const MICRO_SMALL: usize = 100;
const MICRO_LARGE: usize = 1000;

fn micro(loaded: &ply_corpus::w3::Loaded, name: &str) -> Fit {
    let full = loaded.full(name).unwrap_or_else(|e| panic!("{name}: {e}"));
    let go = |n: usize| {
        loaded
            .pure_call(&full, vec![Value::Int(n as i64), Value::Int(0)], 1)
            .unwrap_or_else(|e| panic!("`{name}` raised: {e}"))
    };
    go(4);
    let small = capture(MICRO_SMALL, || go(MICRO_SMALL));
    let large = capture(MICRO_LARGE, || go(MICRO_LARGE));
    Fit { small, large }
}

/// One construction's cost: the loop with it, minus the same loop without it, per iteration, at the
/// sizes where the difference lands.
fn added(base: &Fit, probe: &Fit) -> Vec<(usize, String, f64)> {
    let mut keys: Vec<Key> = base.keys();
    keys.extend(probe.keys());
    keys.sort();
    keys.dedup();
    let mut rows: Vec<(usize, String, f64)> = keys
        .into_iter()
        .map(|k| {
            let d = probe.slope(&k).0 - base.slope(&k).0;
            (k.0, k.1, d)
        })
        .filter(|r| r.2 > 0.5)
        .collect();
    rows.sort_by(|a, b| b.2.total_cmp(&a.2));
    rows
}

fn micro_program() -> ply_corpus::w3::Loaded {
    ply_corpus::w3::Loaded::parse(MICRO).expect("the micro program must compile")
}

/// Whether an application takes its argument vector from the allocator.
#[test]
fn a_warm_ply_call_takes_its_argument_vector_from_the_free_list() {
    let loaded = micro_program();
    let base = micro(&loaded, "r4_base");
    for (name, arity) in [("r4_call1", 1usize), ("r4_call3", 3)] {
        let probe = micro(&loaded, name);
        let rows = added(&base, &probe);
        println!("\n{name}: adding one {arity}-argument call to the loop body adds");
        for (size, site, per) in &rows {
            println!("   {per:>+6.2} per iteration  {size:>5}B  {site}");
        }
        let want = arity * size_of::<Value>();
        if let Some((size, site, per)) = rows
            .iter()
            .find(|(size, site, _)| *size == want && is_argument_vector(site))
        {
            panic!(
                "one {arity}-argument call added {per:.2} allocations of {size} bytes per \
                 iteration under `{site}`: the free list is not serving arity {arity}, and the \
                 178.0 allocations per request ADR 0019 §1 took off the /health path are back"
            );
        }
    }

    // The control, and the correction.
    let control = added(&base, &micro(&loaded, "r4_str"));
    println!("\nr4_str: adding one 1-argument builtin call to the loop body adds");
    for (size, site, per) in &control {
        println!("   {per:>+6.2} per iteration  {size:>5}B  {site}");
    }
    let want = size_of::<Value>();
    let hit = control
        .iter()
        .find(|(size, site, _)| *size == want && is_argument_vector(site));
    let (_, _, per) = hit.unwrap_or_else(|| {
        panic!(
            "the control loop added no {want}-byte allocation under `ply_eval::argv::take`, so \
             the two zeros above mean nothing: either `builtins::call` now returns the buffer it \
             was handed — which would be the change ADR 0019 §1's arithmetic assumes had already \
             happened, and is worth reporting — or this instrument has stopped seeing an argument \
             vector at all"
        )
    });
    assert!(
        (per - 1.0).abs() < 0.05,
        "a 1-argument builtin call added {per:.2} argument vectors per iteration, not 1: the \
         residue the free list cannot reach is no longer one buffer per builtin application and \
         the split printed by \
         `the_argument_vectors_the_free_list_does_not_take_are_the_ones_no_callee_gives_back` is \
         about a different population"
    );
}

/// The frame chain an argument vector is allocated under, in either profile.
fn is_argument_vector(site: &str) -> bool {
    site.contains("ply_eval::argv") || site.contains("ply_eval::frame")
}

/// Whether evaluating a `Str` or a `Bytes` literal reaches the allocator.
#[test]
fn a_literal_value_is_built_once_at_lowering_rather_than_per_evaluation() {
    let loaded = micro_program();
    let base = micro(&loaded, "r4_base");
    for name in ["r4_str", "r4_bytes"] {
        let probe = micro(&loaded, name);
        let rows = added(&base, &probe);
        println!("\n{name}: adding one four-byte literal to the loop body adds");
        for (size, site, per) in &rows {
            println!("   {per:>+6.2} per iteration  {size:>5}B  {site}");
        }
        if let Some((size, site, per)) = rows
            .iter()
            .find(|(_, site, _)| site.contains("ply_eval::interp::literal"))
        {
            panic!(
                "`{name}` added {per:.2} allocations of {size} bytes per iteration under \
                 `{site}`: a literal is being rebuilt per evaluation again, and the 65.0 \
                 allocations per request ADR 0019 §2 took off the /health path are back"
            );
        }
    }

    // The control.
    let control = added(&base, &micro(&loaded, "r4_applied"));
    let hit = control
        .iter()
        .find(|(size, site, _)| *size == 40 && site.contains("ply_eval::machine::Machine::apply"));
    let (_, _, per) = hit.unwrap_or_else(|| {
        panic!(
            "the control loop added no 40-byte allocation under `Machine::apply`, so the two \
             zeros above mean nothing: either `enter_closure`'s `ClosureKind::Ctor` arm stopped \
             wrapping its arguments in an `Arc`, or this instrument has stopped seeing this \
             path at all"
        )
    });
    assert!(
        (per - 1.0).abs() < 0.05,
        "the control allocation moved to {per:.2} per iteration from 1.00; the zeros above rest \
         on it and it is measuring something else now"
    );

    // The literals are still evaluated and still mean what they said: both loops sum
    // `string_len("abcd")` / `bytes_len(b"abcd")`, so a shared value that had drifted to another
    // literal would not answer 4 per iteration.
    for name in ["r4_str", "r4_bytes"] {
        let full = loaded.full(name).unwrap_or_else(|e| panic!("{name}: {e}"));
        let (_, answered) = loaded
            .pure_call(
                &full,
                vec![Value::Int(MICRO_SMALL as i64), Value::Int(0)],
                1,
            )
            .unwrap_or_else(|e| panic!("`{name}` raised: {e}"));
        assert_eq!(
            answered,
            Value::Int(4 * MICRO_SMALL as i64),
            "`{name}` answered {answered} rather than four bytes per iteration: the shared \
             literal is no longer the literal that was written"
        );
    }
}

/// `interp::literal` cannot allocate for an `Int`, a `Bool` or a `Float`, so the premise that
/// primitives are boxed is false in the tree as it stands.
#[test]
fn an_int_a_bool_and_a_float_are_inline_and_allocate_nothing() {
    let mut log: Vec<(&str, usize)> = Vec::new();
    for (label, build) in [
        ("Value::Int", (|| Value::Int(7)) as fn() -> Value),
        ("Value::Bool", || Value::Bool(true)),
        ("Value::Float", || Value::Float(1.5)),
        ("Value::Unit", || Value::Unit),
        ("Value::Decimal", || {
            Value::Decimal(ply_eval::Decimal::new(15, 1))
        }),
    ] {
        let w = capture(1, build);
        log.push((label, w.total));
    }
    for (label, allocations) in &log {
        println!("  {label:<16} {allocations} allocations");
    }
    assert!(
        log.iter().all(|(_, n)| *n == 0),
        "a primitive `Value` allocated: {log:?} — the premise that `Int`, `Bool` and `Float` \
         are inline enum variants no longer holds and every figure in this file is about a \
         different representation"
    );
    assert_eq!(
        size_of::<Value>(),
        32,
        "size_of::<Value>() moved; every `Vec<Value>` size in the rule table is scaled by it"
    );
    assert_eq!(
        size_of::<Option<Value>>(),
        size_of::<Value>(),
        "`Option<Value>` is no longer niche-optimized, so the enum has spare room it did not have"
    );
}

/// Whether a mention of a constructor reaches the allocator.
#[test]
fn a_nullary_constructor_is_built_once_rather_than_on_every_mention() {
    let loaded = micro_program();
    let base = micro(&loaded, "r4_base");
    let probe = micro(&loaded, "r4_nullary");
    let rows = added(&base, &probe);
    println!("\nr4_nullary: adding one mention of a nullary constructor to the loop body adds");
    for (size, site, per) in &rows {
        println!("   {per:>+6.2} per iteration  {size:>5}B  {site}");
    }
    if let Some((size, site, per)) = rows.iter().find(|(_, site, _)| is_ctor_mention(site)) {
        panic!(
            "one mention of a nullary constructor added {per:.2} allocations of {size} bytes \
             per iteration under `{site}`: `interp::ctor_value` is building a fresh value per \
             mention again, and the 21.0 allocations per request ADR 0019 §2 took off the \
             /health path are back"
        );
    }

    // The mention is still evaluated and still means `R4Red`: `r4_rank` answers 1 for it and 2 or 3
    // for the other two arms, so a shared value that had drifted to another constructor would not
    // sum to the iteration count.
    let full = loaded.full("r4_nullary").expect("r4_nullary must resolve");
    let (_, answered) = loaded
        .pure_call(
            &full,
            vec![Value::Int(MICRO_SMALL as i64), Value::Int(0)],
            1,
        )
        .expect("`r4_nullary` raised");
    assert_eq!(
        answered,
        Value::Int(MICRO_SMALL as i64),
        "the shared `R4Red` no longer matches the arm a fresh one matched"
    );
}

/// The same for a constructor of arity >= 1, whose mention evaluates to an `Arc<Closure>` rather
/// than to a `Value::Ctor`.
#[test]
fn a_constructor_of_arity_one_or_more_is_built_once_rather_than_per_mention() {
    let loaded = micro_program();
    let base = micro(&loaded, "r4_base");
    let probe = micro(&loaded, "r4_applied");
    let rows = added(&base, &probe);
    println!("\nr4_applied: adding one application of a 1-argument constructor to the loop adds");
    for (size, site, per) in &rows {
        println!("   {per:>+6.2} per iteration  {size:>5}B  {site}");
    }
    // Only `ctor_value` here, not [`is_ctor_mention`]'s second spelling: the applied constructor
    // below builds its `Value::Ctor` inline in `enter_closure` rather than through `Value::ctor`,
    // and matching that spelling would fail this test on its own control.
    if let Some((size, site, per)) = rows
        .iter()
        .find(|(_, site, _)| site.contains("ply_eval::interp::ctor_value"))
    {
        panic!(
            "one mention of `R4Box` added {per:.2} allocations of {size} bytes per iteration \
             under `{site}`: the constructor closure is being rebuilt per mention again, which \
             is the 24.0 allocations per request ADR 0019 §2 removed"
        );
    }
    let control = rows
        .iter()
        .find(|(size, site, _)| *size == 40 && site.contains("ply_eval::machine::Machine::apply"));
    let (_, _, per) = control.unwrap_or_else(|| {
        panic!(
            "applying `R4Box` added no 40-byte allocation under `Machine::apply`, so this loop \
             is not a control for anything: either `enter_closure`'s `ClosureKind::Ctor` arm \
             stopped wrapping the arguments in an `Arc`, or the instrument has stopped seeing \
             this path at all — and in the second case the zero above means nothing"
        )
    });
    assert!(
        (per - 1.0).abs() < 0.05,
        "the applied constructor's `Arc<Vec<Value>>` moved to {per:.2} per iteration from 1.00; \
         the control this test rests on is measuring something else now"
    );
}

/// Either spelling of the frame chain a rebuilt constructor mention allocates under.
fn is_ctor_mention(site: &str) -> bool {
    site.contains("ply_eval::interp::ctor_value") || site.contains("ply_eval::value::Value::ctor")
}

/// How many allocations each variant costs and at what size — where the rule table's sizes come
/// from, and the answer to "`Ctor` is two allocations, what would `Arc<[Value]>` save".
#[test]
fn the_shape_of_every_value_variant_is_measured() {
    fn shape<T>(label: &str, f: impl FnOnce() -> T) -> (String, Vec<usize>) {
        let mut sizes: Vec<usize> = Vec::new();
        let w = capture(1, f);
        let mut keys: Vec<Key> = w.sites.keys().cloned().collect();
        keys.sort();
        for k in keys {
            for _ in 0..w.sites[&k].calls {
                sizes.push(k.0);
            }
        }
        println!("  {label:<46} {} allocation(s) {sizes:?}", sizes.len());
        (label.to_string(), sizes)
    }

    println!("\n-- what one Value costs to build --");
    shape("Value::str(\"ok\")", || Value::str("ok"));
    shape("Value::bytes(4 bytes)", || Value::bytes([1u8, 2, 3, 4]));
    let list = shape("Value::list(4 elements)", || {
        Value::list((0..4).map(Value::Int).collect())
    });
    // The names are interned outside the window: `Symbol::new` is an `Arc<str>` of its own and
    // would be counted as the constructor's cost.
    let some = Symbol::new("Some");
    let none = Symbol::new("None");
    let x = Symbol::new("x");
    // Two halves, priced separately because they are made at different sites: the machine fills the
    // argument `Vec` in `Frame::AppArgs` and `Value::ctor` only wraps it.
    let spine = shape("the Vec<Value> a 1-argument call fills", || {
        let v: Vec<Value> = vec![Value::Int(1)];
        v
    });
    let args = vec![Value::Int(1)];
    let ctor = shape("Value::ctor(Symbol, that owned Vec)", || {
        Value::ctor(some, args)
    });
    shape("Value::ctor(Symbol, 0 arguments)", || {
        Value::ctor(none, Vec::new())
    });
    let record = shape("Value::Record(1 field)", || {
        Value::Record(Arc::new(ply_eval::Fields::from_iter([(x, Value::Int(1))])))
    });
    shape("Value::map(4 entries)", || {
        Value::map((0..4).map(|i| (Value::Int(i), Value::Int(i))))
    });
    let k = Symbol::new("K");
    shape("Value::Closure", || {
        Value::Closure(Arc::new(Closure {
            name: None,
            kind: ClosureKind::Ctor { name: k, arity: 1 },
        }))
    });
    shape("Value::Secret(Int)", || {
        Value::Secret(Arc::new(Value::Int(1)))
    });
    let built = Value::list((0..4).map(Value::Int).collect());
    let cloned = shape("clone of a Value::List (a refcount bump)", || built.clone());

    println!("\n-- what the 32 bytes are spent on --");
    for (label, bytes) in [
        ("Symbol            (Ctor.name)", size_of::<Symbol>()),
        (
            "Arc<Vec<Value>>   (Ctor.args)",
            size_of::<Arc<Vec<Value>>>(),
        ),
        ("Arc<str>          (Str)", size_of::<Arc<str>>()),
        ("Decimal", size_of::<ply_eval::Decimal>()),
        ("Map               (rpds RBT)", size_of::<ply_eval::Map>()),
        ("Arc<Closure>", size_of::<Arc<Closure>>()),
    ] {
        println!("  {label:<34} {bytes}");
    }

    // `Ctor` is the only variant whose payload needs three words.
    #[allow(dead_code)]
    enum Boxed {
        Int(i64),
        Decimal(ply_eval::Decimal),
        Str(Arc<str>),
        Bytes(Arc<[u8]>),
        List(Arc<Vec<Value>>),
        Map(ply_eval::Map),
        Record(Arc<BTreeMap<Symbol, Value>>),
        Ctor(Arc<(Symbol, Vec<Value>)>),
        Closure(Arc<Closure>),
        Secret(Arc<Value>),
    }
    #[allow(dead_code)]
    enum Sliced {
        Int(i64),
        Decimal(ply_eval::Decimal),
        Str(Arc<str>),
        Bytes(Arc<[u8]>),
        List(Arc<Vec<Value>>),
        Map(ply_eval::Map),
        Record(Arc<BTreeMap<Symbol, Value>>),
        Ctor { name: Symbol, args: Arc<[Value]> },
        Closure(Arc<Closure>),
        Secret(Arc<Value>),
    }
    println!(
        "\n  Value today                                      {}",
        size_of::<Value>()
    );
    println!(
        "  with Ctor behind one Arc<(Symbol, Vec<Value>)>   {}",
        size_of::<Boxed>()
    );
    println!(
        "  with Ctor.args as Arc<[Value]>                   {}",
        size_of::<Sliced>()
    );

    println!("\n-- Arc<[Value]> against Arc<Vec<Value>> --");
    let from_vec = shape("Arc<[Value]>::from(an owned Vec)", || {
        Arc::<[Value]>::from((0..2).map(Value::Int).collect::<Vec<_>>())
    });
    let collected = shape("Arc<[Value]> collected from an exact-size iterator", || {
        (0..2).map(Value::Int).collect::<Arc<[Value]>>()
    });
    let arc_vec = shape("Arc::new(an owned Vec<Value>)", || {
        Arc::new((0..2).map(Value::Int).collect::<Vec<Value>>())
    });

    assert_eq!(
        (spine.1.len(), ctor.1.len()),
        (1, 1),
        "a one-argument `Value::Ctor` no longer costs one allocation for the argument `Vec` and \
         one for the `Arc` around it: it cost {:?} and {:?}, and every Ctor figure in this file \
         assumes the pair",
        spine.1,
        ctor.1
    );
    assert!(
        cloned.1.is_empty(),
        "cloning a `Value::List` allocated {:?}: the claim that a shared literal would cost a \
         refcount bump rather than an allocation rests on this being empty",
        cloned.1
    );
    assert_eq!(
        record.1,
        vec![40, 48],
        "a one-field record no longer costs a 40-byte `Arc` and one 48-byte field vector: it \
         cost {:?}. It was a 544-byte B-tree node until `Fields` replaced the map, and the \
         `Value::Record` bucket's bytes are read against whichever this is",
        record.1
    );
    assert_eq!(
        list.1.len(),
        2,
        "`Value::list` of four elements cost {} allocations, not 2",
        list.1.len()
    );
    assert_eq!(
        from_vec.1.len(),
        arc_vec.1.len(),
        "`Arc<[Value]>::from(Vec)` and `Arc::new(Vec)` no longer cost the same: the claim that \
         moving `Ctor.args` to `Arc<[Value]>` saves nothing unless the argument vector stops \
         being built first would have to be re-taken"
    );
    assert_eq!(
        collected.1.len(),
        1,
        "collecting an exact-size iterator into `Arc<[Value]>` cost {} allocations, not 1: the \
         one-allocation path a `Ctor` rewrite would have to take does not exist",
        collected.1.len()
    );
    assert_eq!(
        size_of::<Sliced>(),
        40,
        "an `Arc<[Value]>` in `Ctor` no longer widens `Value` to 40 bytes; the cost side of that \
         trade has moved"
    );
}

/// A third window, so the slope can be checked against a second slope rather than assumed.
const THIRD: usize = 400;

/// What is left of the argument-vector line once the free list has taken what it can, split by the
/// reason each survivor survived.
#[test]
fn the_argument_vectors_the_free_list_does_not_take_are_the_ones_no_callee_gives_back() {
    let loaded = ply_corpus::w6_run::program(&repo()).expect("the service must compile");
    let fit = health(&loaded);
    let (slope, _) = fit.total_slope();

    let mut by_arity: std::collections::BTreeMap<usize, (f64, f64)> =
        std::collections::BTreeMap::new();
    for key in fit.keys() {
        if classify(&key) != Some(("Vec<Value> — call arguments", Kind::Spine)) {
            continue;
        }
        let (calls, bytes) = fit.slope(&key);
        let e = by_arity
            .entry(key.0 / size_of::<Value>())
            .or_insert((0.0, 0.0));
        e.0 += calls;
        e.1 += bytes;
    }

    let total: f64 = by_arity.values().map(|(c, _)| c).sum();
    println!("\n-- /health: the argument vectors the free list did not serve, by arity --");
    println!(
        "   {:>6} {:>8} {:>7} {:>9}  share of what is left",
        "arity", "buffer", "per req", "bytes/req"
    );
    let mut cumulative = 0.0;
    for (arity, (calls, bytes)) in &by_arity {
        cumulative += calls;
        if *calls < 0.05 {
            continue;
        }
        println!(
            "   {arity:>6} {:>7}B {calls:>7.1} {bytes:>9.0}  {:>5.1}%   cumulative {:>5.1}%",
            arity * size_of::<Value>(),
            100.0 * calls / total,
            100.0 * cumulative / total
        );
    }
    println!(
        "   {total:>22.1} argument vectors per request = {:.1}% of the request's {slope:.1} allocations",
        100.0 * total / slope
    );

    // The only callee that keeps the buffer.
    let (rows, _) = roll_up(&fit);
    let retained = rows
        .iter()
        .find(|r| r.bucket == "Value::Ctor — applied constructor")
        .map(|r| r.calls)
        .unwrap_or(0.0);
    let pooled: f64 = by_arity
        .iter()
        .filter(|(arity, _)| **arity <= ARGV_CLASSES)
        .map(|(_, (c, _))| c)
        .sum();
    let wide = total - pooled;
    let unreturned = pooled - retained;
    println!(
        "\n   retained as `Ctor.args`      {retained:>7.1}  the buffer becomes the value; there is \
         nothing to give back\n   \
         wider than the free list     {wide:>7.1}  arity above {ARGV_CLASSES}, left to the \
         allocator by construction\n   \
         freed but never given back  {unreturned:>7.1}  = {:.1}% of the request: a callee that is \
         not `enter_code`, and `builtins::call`\n   {:>29}  takes its `Vec<Value>` by value, so \
         the buffer is freed where it cannot be handed back",
        100.0 * unreturned / slope,
        ""
    );

    let slots: f64 = by_arity
        .iter()
        .map(|(arity, (calls, _))| *arity as f64 * calls)
        .sum();
    println!(
        "\n-- what a narrower Value would move --\n   {slots:.1} Value-wide slots per request in \
         argument-vector *allocations* = {:.0} bytes at {}B each.\n   Narrowing Value to 24B (its \
         floor, reached only by boxing Ctor's name and args together) would save {:.0} bytes per \
         request of that and zero allocations.\n   This understates it: a recycled buffer is \
         allocated once and filled many times, so the slots a request touches are unchanged and \
         only the slots it allocates are counted here. ADR 0019 §4's figure was taken before the \
         free list existed and is the count of slots touched.",
        slots * size_of::<Value>() as f64,
        size_of::<Value>(),
        slots * 8.0
    );

    assert!(
        total > 0.0,
        "no argument vector was attributed at all: either every callee now gives its buffer back \
         — which would close the `builtins::call` residue this test exists to price, and should \
         be reported rather than asserted away — or the rule stopped matching"
    );
    assert!(
        wide > 0.0,
        "every argument vector this request allocates is inside the free list's {ARGV_CLASSES} \
         capacity classes ({by_arity:?}): either the class count moved and this file did not move \
         with it, or the service stopped making an application wider than {ARGV_CLASSES} \
         arguments"
    );
    // A retained buffer is by construction one of the pooled classes' survivors: it was taken from
    // the list and never came back.
    assert!(
        retained <= pooled + 0.05,
        "the `Value::Ctor — applied constructor` line is {retained:.1} per request but only \
         {pooled:.1} argument vectors of arity 1-{ARGV_CLASSES} were allocated: a retained buffer \
         is one of those by construction, so the two classifiers are counting different things \
         and the split printed above is not a split of one population"
    );
}

/// Every figure in this file is a slope through two points, and two points always fit a line.
#[test]
fn the_per_request_slope_is_the_same_between_the_second_and_third_window() {
    let loaded = ply_corpus::w6_run::program(&repo()).expect("the service must compile");
    let request = ply_corpus::w6_run::head();
    loaded
        .over_sim(vec![vec![request.clone()]])
        .expect("the service serves one connection");
    let go = |n: usize| {
        capture(n, || {
            loaded
                .over_sim(script(&request, n))
                .expect("the service serves")
        })
    };
    let a = go(SMALL);
    let b = go(LARGE);
    let c = go(THIRD);

    let (ls, lb) = Fit {
        small: a,
        large: b.clone(),
    }
    .total_slope();
    let (hs, hb) = Fit { small: b, large: c }.total_slope();
    println!(
        "\n-- is it a slope? --\n  {SMALL} -> {LARGE}:  {ls:>8.1} allocations and {lb:>9.0} bytes per request\n  \
         {LARGE} -> {THIRD}: {hs:>8.1} allocations and {hb:>9.0} bytes per request\n  \
         allocations differ by {:.1}%, bytes by {:.1}%",
        100.0 * (hs - ls).abs() / ls,
        100.0 * (hb - lb).abs() / lb,
    );
    println!(
        "  The allocation slope holds and the byte slope does not, which is \
         `CONTRIBUTING.md` §\"Things known to be broken\" item 8 reproduced here: something on \
         this path is superlinear in total bytes. Every bytes/request column in this file is \
         therefore comparable only against another figure taken at the same pair of windows, \
         and no conclusion in it rests on one."
    );
    assert!(
        (hs - ls).abs() / ls < 0.05,
        "the allocation slope moved from {ls:.1} to {hs:.1} between windows — more than 5% — so \
         a request's cost is not linear in the number of requests and every per-request share in \
         this file is a share of a denominator that depends on the window"
    );
}
