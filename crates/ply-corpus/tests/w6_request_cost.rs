//! What one request allocates, exactly, layer by layer.
//!
//! The W6 ladder says where a request's *time* goes. This says where its
//! *allocations* go, which is the same decomposition in a unit that does not
//! change between machines — so a later change is measured against it rather
//! than against a wall clock on somebody else's laptop.
//!
//! The counting allocator is why this is its own test binary: a
//! `#[global_allocator]` is a whole-binary decision.

use ply_eval::Value;
use ply_span::Span;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::path::{Path, PathBuf};

thread_local! {
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static BYTES: Cell<usize> = const { Cell::new(0) };
}

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        let _ = BYTES.try_with(|c| c.set(c.get() + layout.size()));
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn counted<T>(f: impl FnOnce() -> T) -> (T, usize, usize) {
    ALLOCS.with(|c| c.set(0));
    BYTES.with(|c| c.set(0));
    let out = f();
    (out, ALLOCS.with(Cell::get), BYTES.with(Cell::get))
}

/// The repository root this test reads `examples/desk.ply` from.
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives two levels below the repository root")
        .to_path_buf()
}

/// Allocations per request, on the same rungs the ladder times.
///
/// Each mode of the driver is the one below it plus exactly one layer, so a
/// difference is that layer's allocation cost. The whole-request row is the
/// service over `SimNet`, which adds the read loop, the effect performs and the
/// handler-stack walk to the pure pieces above it.
#[test]
fn a_request_allocates_where_the_ladder_says_its_time_goes() {
    let loaded = ply_corpus::w6_run::program(&repo()).expect("the service must compile");
    let bench = loaded.full("w6_bench").expect("the driver declares w6_bench");
    let request = ply_corpus::w6_run::head();

    const N: u32 = 200;
    let mut rows = Vec::new();
    for (mode, name) in [
        (0i64, "loop only"),
        (1, "+ endpoint"),
        (2, "+ framing"),
        (3, "+ routing"),
    ] {
        // A first pass so lazily-built machine state is not charged to the count.
        loaded
            .pure_call(&bench, vec![Value::Int(mode), Value::Int(8)], 1)
            .expect("the driver runs");
        let (_, allocs, bytes) = counted(|| {
            loaded
                .pure_call(
                    &bench,
                    vec![Value::Int(mode), Value::Int(N as i64)],
                    1,
                )
                .expect("the driver runs")
        });
        rows.push((
            name,
            allocs as f64 / N as f64,
            bytes as f64 / N as f64,
        ));
    }

    let script: Vec<Vec<Vec<u8>>> = (0..N).map(|_| vec![request.clone()]).collect();
    loaded
        .over_sim(vec![vec![request.clone()]])
        .expect("the service serves one connection");
    let (_, allocs, bytes) = counted(|| loaded.over_sim(script).expect("the service serves"));
    rows.push((
        "whole request over SimNet",
        allocs as f64 / N as f64,
        bytes as f64 / N as f64,
    ));

    println!("allocations per request, /health, {N} requests per row");
    let mut previous = 0.0;
    for (name, per_request, per_request_bytes) in &rows {
        println!(
            "  {name:<26} {per_request:>10.1} allocs  {per_request_bytes:>10.0} bytes  \
             (+{:.1} over the row above)",
            per_request - previous
        );
        previous = *per_request;
    }

    let whole = rows.last().expect("five rows").1;
    assert!(
        whole > 0.0,
        "a request that allocates nothing is a measurement that did not run"
    );
    // Every rung adds work, so every rung must allocate at least as much as the
    // one below it. A layer that allocated *less* would mean the driver's modes
    // are not nested, and every difference in this table would be meaningless.
    for pair in rows.windows(2) {
        assert!(
            pair[1].1 >= pair[0].1,
            "`{}` allocated {:.1} against `{}`'s {:.1}, so the rungs are not nested",
            pair[1].0,
            pair[1].1,
            pair[0].0,
            pair[0].1
        );
    }
}

/// The twin's store is not on a served request path, and this is why it is kept
/// out of the ladder's `endpoint` rung: `std.db`'s memory engine parses its SQL
/// in Ply on every call, so a `/items` handler over it allocates several times
/// what the same handler costs against postgres.
#[test]
fn the_in_memory_store_is_priced_apart_from_the_endpoint() {
    let loaded = ply_corpus::w6_run::program(&repo()).expect("the service must compile");
    let items = loaded.full("w6_items").expect("the driver declares w6_items");

    const N: u32 = 100;
    let mut rows = Vec::new();
    for (mode, name) in [(0i64, "/items handler"), (1, "the twin's scan alone")] {
        loaded
            .pure_call(&items, vec![Value::Int(mode), Value::Int(4)], 1)
            .expect("the driver runs");
        let (_, allocs, _) = counted(|| {
            loaded
                .pure_call(&items, vec![Value::Int(mode), Value::Int(N as i64)], 1)
                .expect("the driver runs")
        });
        rows.push((name, allocs as f64 / N as f64));
    }
    for (name, per_request) in &rows {
        println!("  {name:<24} {per_request:>10.1} allocations per call");
    }
    let (handler, scan) = (rows[0].1, rows[1].1);
    assert!(
        scan > 0.0 && handler > scan,
        "the handler allocated {handler:.1} and the scan under it {scan:.1}; the scan is inside \
         the handler and cannot cost more"
    );
}

/// Entering the machine is not the cost. W1 said the host boundary was 0.5µs of
/// a 601µs request; this is the same statement about the call itself, in
/// allocations, and it is what keeps `Machine::call` out of every layer above
/// it.
#[test]
fn entering_the_machine_allocates_a_bounded_amount() {
    let loaded = ply_corpus::w6_run::program(&repo()).expect("the service must compile");
    let constant = loaded.full("w6_const").expect("the driver declares w6_const");
    loaded
        .pure_call(&constant, Vec::new(), 8)
        .expect("the driver runs");
    const N: u32 = 1_000;
    let (_, allocs, _) = counted(|| {
        loaded
            .pure_call(&constant, Vec::new(), N)
            .expect("the driver runs")
    });
    let per_call = allocs as f64 / N as f64;
    println!("  Machine::call on a constant: {per_call:.1} allocations");
    assert!(
        per_call < 100.0,
        "one call into the machine allocated {per_call:.1} times, which is no longer a boundary \
         cost but a layer"
    );
}

/// The two engines answer the same value on the same request path, which is
/// what makes the W6 engine substitution a ratio rather than a comparison
/// between two programs.
#[test]
fn both_engines_answer_the_request_path_alike() {
    let loaded = ply_corpus::w6_run::program(&repo()).expect("the service must compile");
    let bench = loaded.full("w6_bench").expect("the driver declares w6_bench");
    let args = vec![Value::Int(3), Value::Int(16)];
    let mut interp = ply_eval::Interp::new(&loaded.program, &loaded.resolved, &loaded.check);
    let tree = interp
        .call(&bench, args.clone(), Span::DUMMY)
        .expect("the tree-walker runs the request path");
    let mut machine = ply_eval::Machine::new(&loaded.program, &loaded.resolved, &loaded.check);
    let control = machine
        .call(&bench, args, Span::DUMMY)
        .expect("the machine runs the request path");
    assert_eq!(
        tree, control,
        "the engines disagree on the request path, so the W6 engine row would be a ratio between \
         two different programs"
    );
}
