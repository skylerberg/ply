//! What one request allocates, exactly, layer by layer.

use crate::counting::charge;
use ply_eval::Value;
use std::path::{Path, PathBuf};

/// The repository root this test reads `examples/desk.ply` from.
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives two levels below the repository root")
        .to_path_buf()
}

/// Allocations per request, on the same rungs the ladder times.
#[test]
fn a_request_allocates_where_the_ladder_says_its_time_goes() {
    let loaded = ply_corpus::w6_run::program(&repo()).expect("the service must compile");
    let bench = loaded
        .full("w6_bench")
        .expect("the driver declares w6_bench");
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
        let (_, allocs, bytes) = charge(|| {
            loaded
                .pure_call(&bench, vec![Value::Int(mode), Value::Int(N as i64)], 1)
                .expect("the driver runs")
        });
        rows.push((name, allocs as f64 / N as f64, bytes as f64 / N as f64));
    }

    let script: Vec<Vec<Vec<u8>>> = (0..N).map(|_| vec![request.clone()]).collect();
    loaded
        .over_sim(vec![vec![request.clone()]])
        .expect("the service serves one connection");
    let (_, allocs, bytes) = charge(|| loaded.over_sim(script).expect("the service serves"));
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
    // Every rung adds work, so every rung must allocate at least as much as the one below it.
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

/// The twin's store is not on a served request path, and this is why it is kept out of the ladder's
/// `endpoint` rung: `std.db`'s memory engine parses its SQL in Ply on every call, so a `/items`
/// handler over it allocates several times what the same handler costs against postgres.
#[test]
fn the_in_memory_store_is_priced_apart_from_the_endpoint() {
    let loaded = ply_corpus::w6_run::program(&repo()).expect("the service must compile");
    let items = loaded
        .full("w6_items")
        .expect("the driver declares w6_items");

    const N: u32 = 100;
    let mut rows = Vec::new();
    for (mode, name) in [(0i64, "/items handler"), (1, "the twin's scan alone")] {
        loaded
            .pure_call(&items, vec![Value::Int(mode), Value::Int(4)], 1)
            .expect("the driver runs");
        let (_, allocs, _) = charge(|| {
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

/// Entering the machine is not the cost.
#[test]
fn entering_the_machine_allocates_a_bounded_amount() {
    let loaded = ply_corpus::w6_run::program(&repo()).expect("the service must compile");
    let constant = loaded
        .full("w6_const")
        .expect("the driver declares w6_const");
    loaded
        .pure_call(&constant, Vec::new(), 8)
        .expect("the driver runs");
    const N: u32 = 1_000;
    let (_, allocs, _) = charge(|| {
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
