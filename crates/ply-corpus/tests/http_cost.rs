//! What one head costs `std.http.parse_head`, and what the cost is a function of.
//!
//! ADR 0013 §4 states the hazard plainly: W2 removed the property that a
//! request's cost is proportional to its bytes, and a parser that re-scans its
//! buffer, or that folds over the whole head per field, restores it quietly.
//! This is the harness that would notice.
//!
//! **Two sweeps, because there are two questions and one number cannot answer
//! both.** Growing a head by lengthening the *value* of one field gives the
//! parser no more fields to parse, so the time must be flat — that is the W2
//! property, and it is the one that would break if a scan started at zero on
//! every read or if a field's value were folded over per byte. Growing a head by
//! adding *fields* gives the parser more to do, so the time must rise, and it
//! must rise linearly rather than quadratically — a parser whose per-field work
//! crossed the whole buffer would be O(fields²) and is the shape this catches.
//!
//! The numbers are asserted as *ratios*, never as absolute microseconds: a
//! threshold in microseconds is a test that fails on a loaded CI machine and
//! teaches people to ignore it.

use ply_core::CheckOutput;
use ply_eval::{Machine, Value};
use ply_span::Span;
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::{Resolved, resolve};
use std::time::{Duration, Instant};

/// The caller `parse_head` is measured through: one call, one `Bytes` argument,
/// and an answer small enough that building it is not what is being timed.
const BENCH: &str = "
import std.http (parse_head, default_limits, Parsed, Refused, Incomplete)

pub fn bench(raw: Bytes) -> Int =
  match parse_head(raw, default_limits()) {
    Parsed(h) -> h.consumed,
    Refused(r) -> 0 - r.status,
    Incomplete -> -1,
  }
";

struct Bench {
    program: Program,
    resolved: Resolved,
    check: CheckOutput,
    name: String,
}

impl Bench {
    fn open() -> Bench {
        let mut sources: Vec<(&str, String)> = ply_std::sources()
            .map(|(name, src)| (name, src.to_string()))
            .collect();
        sources.push(("m", BENCH.to_string()));
        let inputs: Vec<_> = sources
            .iter()
            .enumerate()
            .map(|(i, (name, src))| {
                (
                    ply_span::SourceId(i as u32),
                    ModuleName::from_dotted(name),
                    src.as_str(),
                )
            })
            .collect();
        let mut program = ply_syntax::parse_program(inputs).expect("the shipped modules parse");
        let diags = ply_derive::expand_program(&mut program);
        assert!(diags.is_empty(), "{diags:#?}");
        let resolved = resolve(&program).expect("the shipped modules resolve");
        let check = match ply_core::check_program(&program, &resolved) {
            Ok(check) => check,
            Err(d) => {
                let errors: Vec<_> = d.iter().filter(|d| d.code.starts_with("E0") && d.severity == ply_span::Severity::Error).collect();
                panic!("they check: {errors:#?}")
            }
        };
        Bench {
            program,
            resolved,
            check,
            name: "m.bench".to_string(),
        }
    }

    /// The best of a few runs rather than the mean: the fastest run is the one
    /// with the least interference from everything else on the machine, and a
    /// mean over a noisy box measures the box.
    fn cost(&self, head: &[u8], calls: u32) -> Duration {
        let mut best: Option<Duration> = None;
        for _ in 0..5 {
            let mut machine = Machine::new(&self.program, &self.resolved, &self.check);
            let arg = Value::bytes(head);
            let started = Instant::now();
            for _ in 0..calls {
                let answer = machine
                    .call(&self.name, vec![arg.clone()], Span::DUMMY)
                    .unwrap_or_else(|d| panic!("`parse_head` raised: {}", d.message));
                assert!(
                    matches!(&answer, Value::Int(n) if *n > 0),
                    "the head did not parse: {answer}"
                );
            }
            let taken = started.elapsed();
            if best.is_none_or(|b| taken < b) {
                best = Some(taken);
            }
        }
        best.expect("at least one run")
    }

    fn micros_per_call(&self, head: &[u8], calls: u32) -> f64 {
        self.cost(head, calls).as_secs_f64() * 1e6 / calls as f64
    }
}

/// One field whose value is `pad` bytes long. The parser sees the same number of
/// fields whatever `pad` is, so this is head length with no extra work in it.
fn long_value(pad: usize) -> Vec<u8> {
    let mut head = b"GET /hello HTTP/1.1\r\nHost: x\r\nX-Pad: ".to_vec();
    head.extend(std::iter::repeat_n(b'a', pad));
    head.extend_from_slice(b"\r\n\r\n");
    head
}

/// `fields` filler fields, each short. This is the axis the cost is allowed to
/// be proportional to.
fn many_fields(fields: usize) -> Vec<u8> {
    let mut head = b"GET /hello HTTP/1.1\r\nHost: x\r\n".to_vec();
    for i in 0..fields {
        head.extend_from_slice(format!("X-Pad-{i:03}: 0123456789abcdef\r\n").as_bytes());
    }
    head.extend_from_slice(b"\r\n");
    head
}

/// The W2 property, re-run against the module that replaced the endpoint's
/// hand-written parser. 8 KB of field the parser never reads must cost what 0
/// bytes of it cost, to within the noise of an unoptimized interpreter.
#[test]
#[ignore = "timing; run with `cargo test -p ply-corpus --test http_cost -- --ignored --nocapture`"]
fn the_cost_of_a_head_is_flat_in_the_length_of_a_field_it_does_not_read() {
    let bench = Bench::open();
    let calls = 2_000;
    let base = bench.micros_per_call(&long_value(0), calls);
    let mut worst = 1.0f64;
    println!("\n  field-value sweep — one field, growing value");
    println!("  {:>8}  {:>10}  {:>8}", "bytes", "us/req", "x base");
    for pad in [0usize, 64, 256, 1024, 4096, 8192] {
        let head = long_value(pad);
        let per = bench.micros_per_call(&head, calls);
        println!(
            "  {:>8}  {:>10.2}  {:>8.2}",
            head.len(),
            per,
            per / base
        );
        worst = worst.max(per / base);
    }
    // A parser that scanned the buffer per field, or re-scanned from zero, would
    // be tens of times this. The bound is loose because the claim is the shape.
    assert!(
        worst < 3.0,
        "8 KB of unread field cost {worst:.2}x what none did; the cost has become a function of bytes"
    );
}

/// The other axis, and the one a smuggling-safe parser is allowed to pay for:
/// cost rises with the number of fields, and rises *linearly*.
#[test]
#[ignore = "timing; run with `cargo test -p ply-corpus --test http_cost -- --ignored --nocapture`"]
fn the_cost_of_a_head_is_linear_in_the_number_of_fields() {
    let bench = Bench::open();
    let calls = 2_000;
    println!("\n  field-count sweep — short fields, growing count");
    println!(
        "  {:>7}  {:>8}  {:>10}  {:>12}",
        "fields", "bytes", "us/req", "us/field"
    );
    let mut per_field = Vec::new();
    for fields in [1usize, 2, 4, 8, 16, 32, 64] {
        let head = many_fields(fields);
        let per = bench.micros_per_call(&head, calls);
        println!(
            "  {:>7}  {:>8}  {:>10.2}  {:>12.3}",
            fields,
            head.len(),
            per,
            per / fields as f64
        );
        per_field.push(per / fields as f64);
    }
    // Quadratic per-field work would make the last entry many times the first.
    let first = per_field[0];
    let last = per_field[per_field.len() - 1];
    assert!(
        last < first,
        "per-field cost rose from {first:.3} to {last:.3} us; the per-field work is not constant"
    );
}

// ------------------------------------------------- a whole request, end to end

/// The service the end-to-end rung serves: `std.http`'s loop, a handler that
/// touches the request, and nothing else. Comparable to `ply-corpus serve`'s
/// `host-sim` rung, which is the same shape over the W2 example's parser.
const SERVICE: &str = r#"
import std.net (net)
import std.http (Request, Response, serve_connection, text_response, default_limits, method_name)

fn app(req: Request) -> Response = text_response(200, method_name(req.method) ++ " " ++ req.path)

pub fn bench_serve(count: Int) -> Int / {net.write[listener], net.write[conn]} = {
  let l = net.listen[listener](0);
  let served = accept_loop(l, count);
  net.close[listener](l);
  served
}

fn accept_loop(l: Int, count: Int) -> Int / {net.write[listener], net.write[conn]} =
  if count <= 0 {
    0
  } else {
    serve_connection(net.accept[listener](l), default_limits(), app);
    1 + accept_loop(l, count - 1)
  }
"#;

/// One request per connection, answered over the simulated network — every layer
/// of `std.http` and the host boundary, and no syscall. What this measures that
/// the sweeps above do not is the whole path: the head parse, the framing
/// decision, the (empty) body read, the response encode and the write.
#[test]
#[ignore = "timing; run with `cargo test -p ply-corpus --test http_cost -- --ignored --nocapture`"]
fn a_whole_request_through_the_host_boundary() {
    use ply_host::tcp::{Net, SimNet};
    use std::sync::Arc;

    let request = b"GET /hello HTTP/1.1\r\nHost: 127.0.0.1\r\nUser-Agent: ply-bench\r\n\r\n";
    let requests = 500u32;

    let mut sources: Vec<(&str, String)> = ply_std::sources()
        .map(|(name, src)| (name, src.to_string()))
        .collect();
    sources.push(("m", SERVICE.to_string()));
    let inputs: Vec<_> = sources
        .iter()
        .enumerate()
        .map(|(i, (name, src))| {
            (
                ply_span::SourceId(i as u32),
                ModuleName::from_dotted(name),
                src.as_str(),
            )
        })
        .collect();
    let mut program = ply_syntax::parse_program(inputs).expect("it parses");
    assert!(ply_derive::expand_program(&mut program).is_empty());
    let resolved = resolve(&program).expect("it resolves");
    let check = ply_core::check_program(&program, &resolved)
        .unwrap_or_else(|d| panic!("{:#?}", d.iter().take(3).collect::<Vec<_>>()));

    let mut best = f64::MAX;
    for _ in 0..3 {
        let script: Vec<Vec<Vec<u8>>> = (0..requests).map(|_| vec![request.to_vec()]).collect();
        let net: Arc<dyn Net> = Arc::new(SimNet::new(script));
        let binding = ply_host::tcp::registry(net)
            .bind(&check)
            .expect("the declaration and the registration agree");
        let mut machine = Machine::new(&program, &resolved, &check);
        machine.set_host_binding(Arc::new(binding));
        if let Some(declared) = check
            .defs
            .values()
            .find(|d| d.simple_name.as_str() == "bench_serve")
            .map(|d| d.footprint.clone())
        {
            machine.set_declared_footprint(declared);
        }
        let started = Instant::now();
        let served = machine
            .call(
                "m.bench_serve",
                vec![Value::Int(requests as i64)],
                Span::DUMMY,
            )
            .unwrap_or_else(|d| panic!("the service raised: {}", d.message));
        let taken = started.elapsed().as_secs_f64() * 1e6 / requests as f64;
        assert!(
            matches!(&served, Value::Int(n) if *n == requests as i64),
            "served {served}"
        );
        best = best.min(taken);
    }
    println!("\n  whole request over the simulated network (no syscall)");
    println!("  {:>10.1} us/req   {:>8.0} req/s", best, 1e6 / best);
}
