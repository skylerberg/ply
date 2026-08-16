//! What remembering a nullary pure definition is worth on the desk, and what
//! it costs in meaning: nothing.
//!
//! The control is a **source substitution**, not a flag: the same service with
//! every nullary definition of its own given a dead parameter, which is the
//! narrowest edit that puts a definition outside the rule without changing
//! what it computes. Both variants are parsed once and driven alternately in
//! this process, so a run of the two is one measurement rather than two — the
//! only shape that survives a machine somebody else is also building on.
//!
//! The assertion is the point and the timing is the reason: byte-for-byte
//! identical responses on every route the twin can answer, and the ratio
//! printed beside them.

use anyhow::Result;
use ply_corpus::{w3, w6_run};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("the workspace root is two directories above this crate")
        .to_path_buf()
}

/// The requests the twin answers without a credential: a constant route, a
/// routing miss, a method that is refused, and the one that reads the store.
fn script() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("/health", w3::request("GET", "/health", None, false, 0, 0)),
        ("/ready", w3::request("GET", "/ready", None, false, 0, 0)),
        ("/items", w3::request("GET", "/items", None, false, 0, 0)),
        (
            "/items/featured",
            w3::request("GET", "/items/featured", None, false, 0, 0),
        ),
        (
            "/items/bolt",
            w3::request("GET", "/items/bolt", None, false, 0, 0),
        ),
        ("/orders", w3::request("GET", "/orders", None, false, 0, 0)),
        (
            "/orders/1",
            w3::request("GET", "/orders/1", None, false, 0, 0),
        ),
        (
            "/docs/orders/placing",
            w3::request("GET", "/docs/orders/placing", None, false, 0, 0),
        ),
        (
            "/nowhere",
            w3::request("GET", "/nowhere", None, false, 0, 0),
        ),
        (
            "/orders put",
            w3::request("PUT", "/orders", None, false, 0, 0),
        ),
        ("* options", w3::request("OPTIONS", "*", None, false, 0, 0)),
    ]
}

fn variants() -> Result<(w3::Loaded, w3::Loaded)> {
    let service = w3::Service::open(&repo())?;
    let source = service.source(w3::Variant::Sequential)?;
    // The ladder's own rewrite, so the control this test asserts on and the
    // control `w6-ladder` prices the lever against are one program.
    let control = w6_run::without_constants(&source);
    assert_ne!(source, control, "the rewrite found nothing to disable");
    Ok((w3::Loaded::parse(&source)?, w3::Loaded::parse(&control)?))
}

#[test]
fn remembering_a_constant_changes_no_byte_of_any_response() {
    let (memoized, control) = variants().expect("both variants load");
    for (what, request) in script() {
        let a = memoized
            .response_over_sim(&request)
            .unwrap_or_else(|e| panic!("`{what}` raised on the shipped service: {e}"));
        let b = control
            .response_over_sim(&request)
            .unwrap_or_else(|e| panic!("`{what}` raised on the control: {e}"));
        assert_eq!(
            String::from_utf8_lossy(&a),
            String::from_utf8_lossy(&b),
            "`{what}` answered differently once its constants were remembered"
        );
    }
}

/// Alternating, best-of, in one process. Printed rather than asserted on a
/// threshold: a ratio is a measurement and a machine under load is entitled to
/// produce a worse one without failing a build.
#[test]
fn what_remembering_the_constants_is_worth_per_request() {
    let (memoized, control) = variants().expect("both variants load");
    // `limits().max_keep_alive` is 100, so a connection carrying more requests
    // than that would be closed part way and the divisor would be a fiction.
    // Thirty-two is what the ladder's served rows reuse a connection for.
    let per_conn = 32usize;
    let connections = 16usize;
    let requests = per_conn * connections;
    let rounds = 7;

    for (what, request) in [
        ("/health", w3::request("GET", "/health", None, false, 0, 0)),
        ("/items", w3::request("GET", "/items", None, false, 0, 0)),
    ] {
        let run = |service: &w3::Loaded| -> Duration {
            let script = (0..connections)
                .map(|_| (0..per_conn).map(|_| request.clone()).collect())
                .collect();
            service.over_sim(script).expect("the twin serves").0
        };
        let mut best_memo = Duration::MAX;
        let mut best_control = Duration::MAX;
        for _ in 0..rounds {
            best_memo = best_memo.min(run(&memoized));
            best_control = best_control.min(run(&control));
        }
        let per = |d: Duration| d.as_secs_f64() * 1e6 / requests as f64;
        let (a, b) = (per(best_memo), per(best_control));
        println!(
            "{what}: {b:.1}us/request without the memo, {a:.1}us with it — {:.2}x, \
             {:.0} req/s against {:.0} req/s (best of {rounds} x {requests})",
            b / a,
            1e6 / a,
            1e6 / b,
        );
    }
}

/// The rung the ladder reads routing off, and the definition this began with.
#[test]
fn what_the_route_table_costs_to_rebuild() {
    let loaded = w6_run::program(&repo()).expect("the ladder's driver loads");
    let bench = loaded.full("w6_bench").expect("the driver is present");
    let iterations = 2000u32;
    let mode = |m: i64| -> f64 {
        let mut best = f64::MAX;
        for _ in 0..7 {
            let taken = loaded
                .pure_call(
                    &bench,
                    vec![
                        ply_eval::Value::Int(m),
                        ply_eval::Value::Int(iterations as i64),
                    ],
                    1,
                )
                .expect("the driver runs")
                .0;
            best = best.min(taken.as_secs_f64() * 1e6 / iterations as f64);
        }
        best
    };
    let empty = mode(0);
    let table = mode(5);
    let routed = mode(3);
    let hoisted = mode(4);
    println!(
        "table(): {:.2}us per build over the empty loop; routing rung {routed:.1}us against \
         {hoisted:.1}us with the table hoisted — {:.2}us for the rebuild",
        table - empty,
        routed - hoisted,
    );
}
