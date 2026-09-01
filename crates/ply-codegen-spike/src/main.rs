//! ADR 0016 §3's spike, run.

use anyhow::{Result, bail};
use ply_codegen_spike::jit::{Jit, Opts};
use ply_codegen_spike::measure::{ENTRY_FN, GROUP, Harness, Input, InputResult, compare, speedup};
use ply_codegen_spike::program::Loaded;
use ply_codegen_spike::served;
use ply_eval::Value;
use ply_span::Span;
use serde::Serialize;
use std::path::PathBuf;

const READ_LINE: &str = "std.http.read_line";
const CHOSEN: &str = "the innermost loop of the framing layer — one call for the request line, one \
                      per field line, one for the terminator — and the function with the highest \
                      per-request cost whose whole body, with the whole call graph under it \
                      (`line_at`, `line_stops`), is inside ADR 0016 §3.2's fragment";
const PARSE_HEAD: &str = "std.http.parse_head";
const MAX_REQUEST_LINE: i64 = 8192;
const MAX_HEADER_BYTES: i64 = 65536;

/// What a browser sends: thirteen field lines, the length W1's sweep called browser-sized.
const BROWSER_HEAD: &[u8] = b"GET /items HTTP/1.1\r\n\
Host: localhost:8137\r\n\
Connection: keep-alive\r\n\
sec-ch-ua: \"Chromium\";v=\"124\", \"Not-A.Brand\";v=\"99\"\r\n\
sec-ch-ua-mobile: ?0\r\n\
sec-ch-ua-platform: \"macOS\"\r\n\
Upgrade-Insecure-Requests: 1\r\n\
User-Agent: Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36\r\n\
Accept: text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8\r\n\
Sec-Fetch-Site: none\r\n\
Sec-Fetch-Mode: navigate\r\n\
Accept-Encoding: gzip, deflate, br\r\n\
Accept-Language: en-GB,en-US;q=0.9,en;q=0.8\r\n\
\r\n";

fn large_head() -> Vec<u8> {
    let mut head = b"GET /items HTTP/1.1\r\nHost: localhost:8137\r\n".to_vec();
    let mut i = 0;
    while head.len() < 64_000 {
        head.extend_from_slice(format!("x-filler-{i:04}: ").as_bytes());
        head.extend_from_slice(&vec![b'v'; 96]);
        head.extend_from_slice(b"\r\n");
        i += 1;
    }
    head.extend_from_slice(b"\r\n");
    head
}

/// Where every line of a head begins — where `field_lines` calls `read_line` from.
fn line_starts(head: &[u8]) -> Vec<usize> {
    let mut starts = vec![0usize];
    let mut i = 0;
    while i + 1 < head.len() {
        if head[i] == b'\r' && head[i + 1] == b'\n' {
            let next = i + 2;
            if next >= head.len() {
                break;
            }
            starts.push(next);
            i = next;
        } else {
            i += 1;
        }
    }
    starts
}

fn budget_at(starts: &[usize], index: usize) -> i64 {
    if index == 0 {
        MAX_REQUEST_LINE
    } else {
        MAX_HEADER_BYTES - (starts[index] - starts[1]) as i64
    }
}

fn args(buf: &[u8], from: i64, budget: i64) -> Vec<Value> {
    vec![Value::bytes(buf), Value::Int(from), Value::Int(budget)]
}

fn spike_inputs(large: &[u8]) -> Vec<Input> {
    let browser = line_starts(BROWSER_HEAD);
    let large_starts = line_starts(large);
    vec![
        Input {
            name: "served-head:request-line".into(),
            args: args(served::REQUEST, 0, MAX_REQUEST_LINE),
        },
        Input {
            name: "browser-head:request-line".into(),
            args: args(BROWSER_HEAD, 0, MAX_REQUEST_LINE),
        },
        Input {
            name: "browser-head:user-agent".into(),
            args: args(BROWSER_HEAD, browser[7] as i64, budget_at(&browser, 7)),
        },
        Input {
            name: "64k-head:request-line".into(),
            args: args(large, 0, MAX_REQUEST_LINE),
        },
        Input {
            name: "64k-head:last-field".into(),
            args: {
                let i = large_starts.len() - 2;
                args(large, large_starts[i] as i64, budget_at(&large_starts, i))
            },
        },
    ]
}

#[derive(Serialize)]
struct Variant {
    name: String,
    what: String,
    fold_literals: bool,
    compiled: Vec<String>,
    nodes: usize,
    compile_micros: f64,
    interpreter_entry_micros: f64,
    spike_entry_micros: f64,
    inputs: Vec<InputResult>,
    /// The minimum conservative ratio over the inputs, entry costs included on both sides.
    speedup: f64,
    /// The same ratio with each side's entry cost subtracted: what the body is worth when it is
    /// called from inside a request rather than entered.
    body_speedup: f64,
    per_request: PerRequest,
    builtin_calls_per_call: f64,
}

#[derive(Serialize, Clone)]
struct PerRequest {
    head_bytes: usize,
    calls: usize,
    interpreter_micros: f64,
    spike_micros: f64,
}

/// How much of one module the fragment can even reach.
#[derive(Serialize)]
struct Census {
    module: String,
    functions: usize,
    accepted: usize,
    refused_by: Vec<(String, usize)>,
}

#[derive(Serialize)]
struct Served {
    requests: u32,
    micros_per_request: f64,
    requests_per_second: f64,
    head_bytes: usize,
}

#[derive(Serialize)]
struct Projection {
    /// `read_line`'s share of one served request, from the two measurements beside it.
    share_of_request: f64,
    share_of_parse_head: f64,
    kernel_speedup: f64,
    /// Amdahl over the share and the kernel ratio.
    end_to_end: f64,
    /// The limit at an infinitely fast kernel, which no backend can pass.
    ceiling: f64,
}

/// Exactly `ply_corpus::w6::Spike`'s shape, so the ladder run and the spike run can produce their
/// halves of the W6 report independently and a field-by-field merge picks this up.
#[derive(Serialize)]
struct SpikeHalf {
    function: String,
    chosen_because: String,
    nodes: usize,
    compile_micros: f64,
    inputs: Vec<InputResult>,
}

#[derive(Serialize)]
struct Output {
    spike: SpikeHalf,
    function: String,
    chosen_because: String,
    provenance: Provenance,
    variants: Vec<Variant>,
    parse_head_micros: f64,
    parse_head_browser_micros: f64,
    served: Option<Served>,
    served_browser_head: Option<Served>,
    browser_head_per_request: PerRequest,
    projection: Option<Projection>,
    projection_browser_head: Option<Projection>,
    census: Vec<Census>,
    refusals: Vec<String>,
    not_measured: Vec<String>,
}

#[derive(Serialize)]
struct Provenance {
    machine: String,
    profile: String,
    rustc: String,
    cranelift: String,
    iterations: u32,
    repeats: u32,
}

struct Args {
    iterations: u32,
    repeats: u32,
    out: Option<String>,
    /// Where to write the *half* `ply-corpus w6` merges: `{"spike": ..}` and nothing else.
    half: Option<String>,
    connections: u32,
    per_connection: u32,
    served: bool,
}

fn parse_args() -> Result<Args> {
    let mut a = Args {
        iterations: 2_000,
        repeats: 7,
        out: None,
        half: None,
        connections: 40,
        per_connection: 100,
        served: true,
    };
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--iterations" => a.iterations = argv.next().unwrap_or_default().parse()?,
            "--repeats" => a.repeats = argv.next().unwrap_or_default().parse()?,
            "--out" => a.out = argv.next(),
            "--half" => a.half = argv.next(),
            "--connections" => a.connections = argv.next().unwrap_or_default().parse()?,
            "--per-connection" => a.per_connection = argv.next().unwrap_or_default().parse()?,
            "--no-served" => a.served = false,
            other => bail!("unknown argument: {other}"),
        }
    }
    Ok(a)
}

/// Agreement, on every input the spike will be timed on and on every refusal path a peer can
/// choose, against the machine.
fn verify(harness: &mut Harness, large: &[u8]) -> Result<usize> {
    let mut inputs: Vec<Input> = Vec::new();
    for (label, head) in [
        ("served", served::REQUEST.to_vec()),
        ("browser", BROWSER_HEAD.to_vec()),
        ("large", large.to_vec()),
    ] {
        let starts = line_starts(&head);
        for i in 0..starts.len() {
            inputs.push(Input {
                name: format!("{label}@{}", starts[i]),
                args: args(&head, starts[i] as i64, budget_at(&starts, i)),
            });
        }
    }
    for (name, buf, from, budget) in [
        ("empty", b"".to_vec(), 0i64, 8192i64),
        ("past-end", b"GET /x HTTP/1.1\r\n".to_vec(), 99, 8192),
        ("at-end", b"GET /x HTTP/1.1\r\n".to_vec(), 17, 8192),
        (
            "bare-lf",
            b"GET /x HTTP/1.1\nHost: a\r\n\r\n".to_vec(),
            0,
            8192,
        ),
        (
            "bare-cr",
            b"GET /x HTTP/1.1\rHost: a\r\n\r\n".to_vec(),
            0,
            8192,
        ),
        ("nul-byte", b"GET /x\0 HTTP/1.1\r\n".to_vec(), 0, 8192),
        ("del-byte", b"GET /x\x7f HTTP/1.1\r\n".to_vec(), 0, 8192),
        ("htab-inside", b"GET /x\tHTTP/1.1\r\n".to_vec(), 0, 8192),
        ("too-long", vec![b'a'; 200], 0, 16),
        ("no-terminator", b"GET /x HTTP/1.1".to_vec(), 0, 8192),
        ("cr-at-end", b"GET /x HTTP/1.1\r".to_vec(), 0, 8192),
        ("zero-budget", b"\r\nrest".to_vec(), 0, 0),
        ("negative-budget", b"abc\r\n".to_vec(), 0, -1),
        ("exact-budget", b"abcd\r\n".to_vec(), 0, 4),
        ("one-over", b"abcde\r\n".to_vec(), 0, 4),
        ("negative-from", b"abcde\r\n".to_vec(), -3, 8192),
    ] {
        inputs.push(Input {
            name: name.to_string(),
            args: args(&buf, from, budget),
        });
    }

    let mut disagreements = Vec::new();
    for input in &inputs {
        let expected = harness.interpret(READ_LINE, &input.args);
        let actual = harness.compiled_call(READ_LINE, &input.args);
        let same = match (&expected, &actual) {
            (Ok(a), Ok(b)) => ply_eval::values_equal(a, b, Span::DUMMY).unwrap_or(false),
            (Err(_), Err(_)) => true,
            _ => false,
        };
        if !same {
            disagreements.push(format!("{}: the machine and the spike differ", input.name));
        }
    }
    if !disagreements.is_empty() {
        bail!(
            "the spike does not agree with the interpreter on {} of {} inputs:\n  {}",
            disagreements.len(),
            inputs.len(),
            disagreements.join("\n  ")
        );
    }
    Ok(inputs.len())
}

#[allow(clippy::too_many_arguments)]
fn variant(
    name: &str,
    what: &str,
    group: &[&str],
    opts: Opts,
    inputs: &[Input],
    head: &[u8],
    a: &Args,
) -> Result<Variant> {
    let mut harness = Harness::with(group, opts)?;
    let entry_input = [Input {
        name: ENTRY_FN.to_string(),
        args: Vec::new(),
    }];
    let entry = compare(
        &mut harness,
        ENTRY_FN,
        &entry_input,
        a.iterations,
        a.repeats,
    )?;
    let interpreter_entry = entry.results[0].interpreter_best_micros;
    let spike_entry = entry.results[0].spike_best_micros;

    let measured = compare(&mut harness, READ_LINE, inputs, a.iterations, a.repeats)?;
    let k = speedup(&measured.results);
    let body_k = measured
        .results
        .iter()
        .map(|r| {
            (r.interpreter_best_micros - interpreter_entry)
                / (r.spike_worst_micros - spike_entry).max(f64::MIN_POSITIVE)
        })
        .fold(f64::INFINITY, f64::min);

    // Every `read_line` call one request makes, at the offsets and budgets the parser passes, with
    // each side's entry cost subtracted.
    let starts = line_starts(head);
    let mut interpreter_total = 0.0;
    let mut spike_total = 0.0;
    for i in 0..starts.len() {
        let site = [Input {
            name: format!("site@{}", starts[i]),
            args: args(head, starts[i] as i64, budget_at(&starts, i)),
        }];
        let m = compare(&mut harness, READ_LINE, &site, a.iterations, a.repeats)?;
        interpreter_total += m.results[0].interpreter_best_micros - interpreter_entry;
        spike_total += m.results[0].spike_best_micros - spike_entry;
    }

    let builtin_calls_before = harness.bodies.builtin_calls();
    for _ in 0..100 {
        harness.compiled_call(READ_LINE, &args(head, 0, MAX_REQUEST_LINE))?;
    }
    let builtin_calls = harness.bodies.builtin_calls() - builtin_calls_before;

    Ok(Variant {
        name: name.to_string(),
        what: what.to_string(),
        fold_literals: opts.fold_literals,
        compiled: group.iter().map(|s| (*s).to_string()).collect(),
        nodes: group
            .iter()
            .filter_map(|n| harness.compiled().nodes.get(*n))
            .sum(),
        compile_micros: harness.compiled().compile_nanos as f64 / 1000.0,
        interpreter_entry_micros: interpreter_entry,
        spike_entry_micros: spike_entry,
        inputs: measured.results,
        speedup: k,
        body_speedup: body_k,
        per_request: PerRequest {
            head_bytes: head.len(),
            calls: starts.len(),
            interpreter_micros: interpreter_total,
            spike_micros: spike_total,
        },
        builtin_calls_per_call: builtin_calls as f64 / 100.0,
    })
}

/// How much of one module the fragment reaches, offering the module **as one unit**.
fn census(loaded: &'static Loaded, module: &str) -> Census {
    let all = loaded.functions_in(module);
    let functions = all.len();
    let lost = ply_codegen_spike::entry::refusals_over(loaded, &all)
        .unwrap_or_else(|e| panic!("classifying `{module}` failed: {e}"));
    let mut reasons: Vec<(String, usize)> = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for (function, reason) in &lost {
        if seen.contains(&function.as_str()) {
            continue;
        }
        seen.push(function);
        match reasons.iter_mut().find(|(r, _)| r == reason) {
            Some((_, n)) => *n += 1,
            None => reasons.push((reason.clone(), 1)),
        }
    }
    reasons.sort_by(|a, b| b.1.cmp(&a.1));
    Census {
        module: module.to_string(),
        functions,
        accepted: functions - seen.len(),
        refused_by: reasons,
    }
}

fn main() -> Result<()> {
    let a = parse_args()?;
    let large = large_head();
    let inputs = spike_inputs(&large);

    let mut harness = Harness::new(GROUP)?;
    let checked = verify(&mut harness, &large)?;
    println!("agreement: {checked} inputs, against the machine, before anything was timed");
    drop(harness);

    let loaded: &'static Loaded = Box::leak(Box::new(Loaded::std_library()?));
    let mut refusals = Vec::new();
    for name in [
        "std.http.parse_head",
        "std.http.field_lines",
        "std.http.parse_field",
        "std.http.list_field",
        "std.net.send_all",
    ] {
        match Jit::compile(loaded, &[name]) {
            Ok(_) => refusals.push(format!("{name}: compiled")),
            Err(e) => refusals.push(format!("{name}: {e}")),
        }
    }

    let variants = vec![
        variant(
            "group",
            "`read_line` with its whole call graph — `line_at` and `line_stops` — compiled, \
             literals folded into the code object",
            GROUP,
            Opts {
                fold_literals: true,
            },
            &inputs,
            served::REQUEST,
            &a,
        )?,
        variant(
            "group, literals rebuilt",
            "the same, except that every `Bytes`, `String` and nullary-constructor literal is \
             allocated per evaluation as the interpreter allocates it — so the ratio is dispatch \
             alone",
            GROUP,
            Opts {
                fold_literals: false,
            },
            &inputs,
            served::REQUEST,
            &a,
        )?,
        variant(
            "group, browser head",
            "the headline variant again, costed over a browser-sized head — thirteen field lines              rather than two, which is `read_line`'s other regime",
            GROUP,
            Opts {
                fold_literals: true,
            },
            &inputs,
            BROWSER_HEAD,
            &a,
        )?,
    ];

    // `parse_head` on the same head: the layer `read_line` is inside.
    let mut parse_harness = Harness::new(GROUP)?;
    let limits = parse_harness.interpret("std.http.default_limits", &[])?;
    let mut parse_body = [0.0f64; 2];
    for (slot, head) in [served::REQUEST, BROWSER_HEAD].iter().enumerate() {
        let parse_args = vec![Value::bytes(*head), limits.clone()];
        let mut best = f64::INFINITY;
        for _ in 0..a.repeats {
            let started = std::time::Instant::now();
            for _ in 0..a.iterations {
                parse_harness.interpret(PARSE_HEAD, &parse_args)?;
            }
            best = best.min(started.elapsed().as_secs_f64() * 1e6 / f64::from(a.iterations));
        }
        parse_body[slot] = best - variants[0].interpreter_entry_micros;
    }

    let served_result = if a.served {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("the crate sits two levels under the repository root")
            .to_path_buf();
        let binary = root.join("target/release/ply");
        if !binary.exists() {
            bail!(
                "{} does not exist; the served denominator needs the release binary",
                binary.display()
            );
        }
        let out = std::env::temp_dir().join("ply-codegen-spike");
        let project = served::project(&root, &out)?;
        let server = served::Server::start(&binary, &project, 2 * a.connections + 8)?;
        // A warm connection first: the first request a process answers pays for whatever it lowers
        // on the way.
        served::drive(server.port, 1, 20)?;
        let short = served::drive(server.port, a.connections, a.per_connection)?;
        let long = served::drive_with(server.port, a.connections, a.per_connection, BROWSER_HEAD)?;
        drop(server);
        Some((
            Served {
                requests: short.requests,
                micros_per_request: short.micros_per_request,
                requests_per_second: short.requests_per_second,
                head_bytes: short.head_bytes,
            },
            Served {
                requests: long.requests,
                micros_per_request: long.micros_per_request,
                requests_per_second: long.requests_per_second,
                head_bytes: long.head_bytes,
            },
        ))
    } else {
        None
    };

    let project = |per: &PerRequest, served: &Served, parse: f64| {
        let share = per.interpreter_micros / served.micros_per_request;
        let k = per.interpreter_micros / per.spike_micros;
        Projection {
            share_of_request: share,
            share_of_parse_head: per.interpreter_micros / parse,
            kernel_speedup: k,
            end_to_end: 1.0 / ((1.0 - share) + share / k),
            ceiling: 1.0 / (1.0 - share),
        }
    };
    let projection = served_result
        .as_ref()
        .map(|(short, _)| project(&variants[0].per_request, short, parse_body[0]));
    let projection_browser = served_result
        .as_ref()
        .map(|(_, long)| project(&variants[3].per_request, long, parse_body[1]));

    let census = vec![
        census(loaded, "std.http"),
        census(loaded, "std.router"),
        census(loaded, "std.json"),
    ];

    println!();
    for v in &variants {
        println!("== {} — {}", v.name, v.what);
        println!(
            "   {:<28} {:>12} {:>12} {:>8} {:>8}",
            "input", "interp best", "spike worst", "cons.", "body"
        );
        for r in &v.inputs {
            println!(
                "   {:<28} {:>12.3} {:>12.3} {:>8.2} {:>8.2}",
                r.name,
                r.interpreter_best_micros,
                r.spike_worst_micros,
                r.interpreter_best_micros / r.spike_worst_micros,
                (r.interpreter_best_micros - v.interpreter_entry_micros)
                    / (r.spike_worst_micros - v.spike_entry_micros),
            );
        }
        println!(
            "   k {:.2}x (entry included) / {:.2}x (body only); compile {:.0}µs; {} nodes",
            v.speedup, v.body_speedup, v.compile_micros, v.nodes
        );
        println!(
            "   entry: interpreter {:.3}µs, spike {:.3}µs; {} builtin calls per call",
            v.interpreter_entry_micros, v.spike_entry_micros, v.builtin_calls_per_call
        );
        println!(
            "   per request ({} head bytes, {} calls): interpreter {:.3}µs → spike {:.3}µs ({:.2}x)",
            v.per_request.head_bytes,
            v.per_request.calls,
            v.per_request.interpreter_micros,
            v.per_request.spike_micros,
            v.per_request.interpreter_micros / v.per_request.spike_micros
        );
        println!();
    }
    println!(
        "parse_head: {:.3}µs on the 63-byte head, {:.3}µs on the browser head",
        parse_body[0], parse_body[1]
    );
    if let Some((short, long)) = &served_result {
        for s in [short, long] {
            println!(
                "served ({} head bytes): {:.0} req/s, {:.1}µs per request over {} requests",
                s.head_bytes, s.requests_per_second, s.micros_per_request, s.requests
            );
        }
    }
    for (label, p) in [
        ("63-byte head", &projection),
        ("browser head", &projection_browser),
    ] {
        if let Some(p) = p {
            println!(
                "{label}: share of a request {:.1}% · of parse_head {:.1}% · k {:.2}x · end to end {:.3}x · ceiling {:.3}x",
                p.share_of_request * 100.0,
                p.share_of_parse_head * 100.0,
                p.kernel_speedup,
                p.end_to_end,
                p.ceiling
            );
        }
    }
    for c in &census {
        println!(
            "fragment coverage of {}: {} of {} functions; refused by {}",
            c.module,
            c.accepted,
            c.functions,
            c.refused_by
                .iter()
                .map(|(r, n)| format!("{r} ×{n}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    println!();
    for r in &refusals {
        println!("refused: {r}");
    }

    let headline = &variants[0];
    let output = Output {
        spike: SpikeHalf {
            function: READ_LINE.to_string(),
            chosen_because: CHOSEN.to_string(),
            nodes: headline.nodes,
            compile_micros: headline.compile_micros,
            inputs: headline.inputs.clone(),
        },
        function: READ_LINE.to_string(),
        chosen_because: CHOSEN.to_string(),
        provenance: Provenance {
            machine: std::env::var("W6_MACHINE").unwrap_or_else(|_| "aarch64-apple-darwin".into()),
            profile: if cfg!(debug_assertions) {
                "debug".into()
            } else {
                "release".into()
            },
            rustc: "1.94.0".into(),
            cranelift: "0.134.3".into(),
            iterations: a.iterations,
            repeats: a.repeats,
        },
        parse_head_micros: parse_body[0],
        parse_head_browser_micros: parse_body[1],
        browser_head_per_request: variants[3].per_request.clone(),
        variants,
        served: served_result.as_ref().map(|(s, _)| Served {
            requests: s.requests,
            micros_per_request: s.micros_per_request,
            requests_per_second: s.requests_per_second,
            head_bytes: s.head_bytes,
        }),
        served_browser_head: served_result.as_ref().map(|(_, s)| Served {
            requests: s.requests,
            micros_per_request: s.micros_per_request,
            requests_per_second: s.requests_per_second,
            head_bytes: s.head_bytes,
        }),
        projection,
        projection_browser_head: projection_browser,
        census,
        refusals,
        not_measured: vec![
            "effects: no `perform`, no handler-stack walk and no host boundary is inside the \
             compiled fragment, and those are what a request spends its time on above framing"
                .into(),
            "continuations: nothing here captures or resumes one".into(),
            "garbage collection: compiled values live in an arena freed per call, so the spike \
             pays no reference counting the interpreter pays"
                .into(),
            "polymorphism and derived code: a `derive json` encoder is a record of closures, \
             which the fragment refuses"
                .into(),
            "a second engine's cost: the backend audit would have to police a third evaluator, and \
             nothing here measures that"
                .into(),
        ],
    };
    if let Some(path) = a.out {
        std::fs::write(&path, serde_json::to_string_pretty(&output)?)?;
        println!("wrote {path}");
    }
    if let Some(path) = a.half {
        std::fs::write(
            &path,
            format!(
                "{}\n",
                serde_json::to_string_pretty(&serde_json::json!({ "spike": &output.spike }))?
            ),
        )?;
        println!("wrote {path}");
    }
    Ok(())
}
