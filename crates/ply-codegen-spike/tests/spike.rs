//! What has to hold before the spike's number means anything.
//!
//! The rules are ADR 0016 §3.4 and §7.11–14: three inputs at least, agreement
//! with **both** evaluators on every one of them, a loud refusal for anything
//! outside the fragment, and a reported ratio that is the weakest input's.

use ply_codegen_spike::jit::{Jit, Opts};
use ply_codegen_spike::measure::{
    GROUP, Harness, Input, InputResult, agrees_with_treewalk, compare, speedup,
};
use ply_codegen_spike::program::Loaded;
use ply_eval::{Value, values_equal};
use ply_span::Span;

const READ_LINE: &str = "std.http.read_line";

fn args(buf: &[u8], from: i64, budget: i64) -> Vec<Value> {
    vec![Value::bytes(buf), Value::Int(from), Value::Int(budget)]
}

/// Heads, offsets and budgets that reach every arm of `read_line` and
/// `line_at`, including the ones only a hostile peer produces.
fn inputs() -> Vec<Input> {
    let mut out = Vec::new();
    let head = b"GET /items HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: */*\r\n\r\n".to_vec();
    for from in [0i64, 21, 38, 51] {
        out.push(Input {
            name: format!("head@{from}"),
            args: args(&head, from, 8192),
        });
    }
    for (name, buf, from, budget) in [
        ("empty", b"".to_vec(), 0i64, 8192i64),
        ("past-end", b"GET /x HTTP/1.1\r\n".to_vec(), 99, 8192),
        ("at-end", b"GET /x HTTP/1.1\r\n".to_vec(), 17, 8192),
        ("bare-lf", b"GET /x HTTP/1.1\nHost: a\r\n\r\n".to_vec(), 0, 8192),
        ("bare-cr", b"GET /x HTTP/1.1\rHost: a\r\n\r\n".to_vec(), 0, 8192),
        ("nul", b"GET /x\0 HTTP/1.1\r\n".to_vec(), 0, 8192),
        ("del", b"GET /x\x7f HTTP/1.1\r\n".to_vec(), 0, 8192),
        ("too-long", vec![b'a'; 200], 0, 16),
        ("unterminated", b"GET /x HTTP/1.1".to_vec(), 0, 8192),
        ("cr-at-end", b"GET /x HTTP/1.1\r".to_vec(), 0, 8192),
        ("zero-budget", b"\r\nrest".to_vec(), 0, 0),
        ("negative-budget", b"abc\r\n".to_vec(), 0, -1),
        ("exact-budget", b"abcd\r\n".to_vec(), 0, 4),
        ("one-over", b"abcde\r\n".to_vec(), 0, 4),
        ("negative-from", b"abcde\r\n".to_vec(), -3, 8192),
        ("huge-budget", b"abcde\r\n".to_vec(), 0, i64::MAX - 1),
    ] {
        out.push(Input {
            name: name.to_string(),
            args: args(&buf, from, budget),
        });
    }
    out
}

fn agree_on(harness: &mut Harness, input: &Input) -> bool {
    let expected = harness
        .machine
        .call(READ_LINE, input.args.clone(), Span::DUMMY);
    let actual = harness.compiled_call(READ_LINE, &input.args);
    match (expected, actual) {
        (Ok(a), Ok(b)) => values_equal(&a, &b, Span::DUMMY).unwrap_or(false),
        (Err(_), Err(_)) => true,
        _ => false,
    }
}

#[test]
fn the_compiled_function_answers_what_the_machine_answers() {
    let mut harness = Harness::new(GROUP).expect("the group compiles");
    let inputs = inputs();
    assert!(inputs.len() >= 3, "a ratio over fewer inputs is one input's constant");
    for input in &inputs {
        assert!(agree_on(&mut harness, input), "disagreed on `{}`", input.name);
    }
}

#[test]
fn and_what_the_tree_walker_answers() {
    let mut harness = Harness::new(GROUP).expect("the group compiles");
    for input in inputs() {
        let expected = harness
            .machine
            .call(READ_LINE, input.args.clone(), Span::DUMMY);
        if expected.is_err() {
            continue;
        }
        assert!(
            agrees_with_treewalk(&mut harness, READ_LINE, &input).expect("the tree-walker ran"),
            "the tree-walker and the spike differ on `{}`",
            input.name
        );
    }
}

#[test]
fn folding_a_literal_into_the_code_object_changes_no_answer() {
    let mut folded = Harness::with(
        GROUP,
        Opts {
            fold_literals: true,
        },
    )
    .expect("compiles");
    let mut rebuilt = Harness::with(
        GROUP,
        Opts {
            fold_literals: false,
        },
    )
    .expect("compiles");
    for input in inputs() {
        assert_eq!(
            agree_on(&mut folded, &input),
            agree_on(&mut rebuilt, &input),
            "the two literal strategies differ on `{}`",
            input.name
        );
        assert!(agree_on(&mut rebuilt, &input));
    }
}

/// **Corrected in R5.** This test used to read
/// `a_call_the_spike_did_not_compile_goes_through_the_machine_and_still_agrees`,
/// compiled `read_line` alone, and asserted that `harness.ctx.machine_calls > 0`
/// — that the trampoline ADR 0016 §3.2 allows was taken and still agreed. There
/// is no trampoline now.
///
/// It was a whole `Machine::call` entry point on a second, privately held
/// machine: `escape::check`, `reset()`, `close_regions`, `end_entry_point`. That
/// was invisible while the only way into compiled code was at the top of a pure
/// integer kernel. Once the interpreter can *enter* compiled code, the same
/// helper is a route out of a live machine's frame into a different machine's
/// `reset()` — the caller's handler stack, trail, region generations and
/// footprint discarded in silence. So a call to a function outside the unit
/// refuses the unit, and the compiled set is closed under calls.
#[test]
fn a_unit_missing_a_callee_is_refused_and_names_the_call() {
    let loaded: &'static Loaded = Box::leak(Box::new(Loaded::std_library().expect("the stdlib")));
    let refusal = Jit::compile(loaded, &[READ_LINE]);
    let message = match refusal {
        Ok(_) => panic!(
            "`{READ_LINE}` was compiled without `line_at` and `line_stops`, so it must be \
             reaching them some other way — which is the trampoline this milestone removed"
        ),
        Err(e) => e.to_string(),
    };
    assert!(
        message.contains("which is not in this compiled unit"),
        "the unit was refused for something other than the missing callee: {message}"
    );
    assert!(
        message.contains("std.http.line_stops") || message.contains("std.http.line_at"),
        "the refusal does not name the function that was missing: {message}"
    );
    // And the whole group still compiles, so the refusal is about closure rather
    // than about `read_line`.
    Jit::compile(loaded, GROUP).expect("the closed group compiles");
}

/// > **Corrected in R5.** `std.http.list_field` was listed here as refused for a
/// > "list literal". It is now refused earlier, for `fold` — a builtin that calls
/// > user code.
/// >
/// > That is not a cosmetic reordering. `fold`, `map`, `filter`, `map_fold` and
/// > `bytes_position` used to compile clean and then raise at run time out of
/// > `rt_builtin`, because `builtins::call` answers `Step::Apply` for them and a
/// > native frame has no machine to run the callback on. A census counting such
/// > a function as compiled was counting one that could not run — the same
/// > defect ADR 0019 §5 item 4 records for `Float`. They are refused at compile
/// > time now, so the refusal fires before the list literal does.
/// >
/// > `std.http.contains_string` moved for the same reason, and it was this
/// > list's only lambda. The lambda refusal is still armed — see
/// > [`a_lambda_is_refused_by_name`], which reaches it through a Ply function
/// > rather than through a builtin.
#[test]
fn the_fragment_refuses_what_it_cannot_compile_and_names_it() {
    let loaded: &'static Loaded = Box::leak(Box::new(Loaded::std_library().expect("the stdlib")));
    for (name, expected) in [
        ("std.http.parse_head", "field access"),
        ("std.http.list_field", "a builtin that calls user code"),
        ("std.net.send_from", "perform"),
        ("std.http.contains_string", "a builtin that calls user code"),
        ("std.http.header", "pattern"),
    ] {
        let refusal = Jit::compile(loaded, &[name]);
        let message = match refusal {
            Ok(_) => panic!("`{name}` is outside the fragment and was compiled anyway"),
            Err(e) => e.to_string(),
        };
        assert!(
            message.contains(expected),
            "`{name}` was refused as `{message}`, which does not name `{expected}`"
        );
    }
}

#[test]
fn a_function_inside_the_fragment_is_compiled_rather_than_refused() {
    let loaded: &'static Loaded = Box::leak(Box::new(Loaded::std_library().expect("the stdlib")));
    let compiled = Jit::compile(loaded, GROUP).expect("the group is inside the fragment");
    assert!(compiled.entry(READ_LINE).is_some());
    assert!(
        ply_codegen_spike::entry::enterable(loaded, &[READ_LINE.to_string()]).is_empty(),
        "`read_line` takes `Bytes` and answers a constructor, so the machine must never be \
         offered it whatever the fragment compiled"
    );
    assert_eq!(compiled.arity(READ_LINE), Some(3));
    assert!(compiled.nodes[READ_LINE] > 0);
    assert!(
        compiled.compile_nanos > 0,
        "compile time is a column, not an assumption"
    );
}

#[test]
fn the_reported_speedup_is_the_weakest_inputs() {
    let results = vec![
        InputResult {
            name: "fast".into(),
            interpreter_best_micros: 10.0,
            interpreter_worst_micros: 11.0,
            spike_best_micros: 1.0,
            spike_worst_micros: 1.0,
            agreed: true,
        },
        InputResult {
            name: "slow".into(),
            interpreter_best_micros: 10.0,
            interpreter_worst_micros: 11.0,
            spike_best_micros: 4.0,
            spike_worst_micros: 5.0,
            agreed: true,
        },
    ];
    assert!((speedup(&results) - 2.0).abs() < 1e-9);
}

#[test]
fn a_measured_input_carries_four_times_and_an_agreement() {
    let mut harness = Harness::new(GROUP).expect("compiles");
    let inputs = vec![Input {
        name: "one".into(),
        args: args(b"GET /x HTTP/1.1\r\n\r\n", 0, 8192),
    }];
    let measured = compare(&mut harness, READ_LINE, &inputs, 50, 3).expect("ran");
    let r = &measured.results[0];
    assert!(r.agreed);
    assert!(r.interpreter_best_micros <= r.interpreter_worst_micros);
    assert!(r.spike_best_micros <= r.spike_worst_micros);
    assert!(r.spike_best_micros > 0.0);
}

/// The lambda refusal, reached where a higher-order *builtin* does not shadow it.
///
/// `Fx::app` resolves the callee before it lowers the arguments, so a lambda
/// handed to `fold` is refused as `fold` and the lambda is never reached. Handed
/// to a Ply function it is the first thing outside the fragment, which is what
/// this probe arranges. Kept because `the_fragment_refuses_what_it_cannot_compile_and_names_it`
/// used to cover it and no longer can.
#[test]
fn a_lambda_is_refused_by_name() {
    let dir = std::env::temp_dir().join(format!("ply-lambda-probe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    std::fs::write(
        dir.join("lam.ply"),
        "pub fn twice(f: (Int) -> Int, x: Int) -> Int = f(f(x))\n\
         pub fn go(x: Int) -> Int = twice(|n: Int| n + 1, x)\n",
    )
    .expect("the probe source");
    let loaded: &'static Loaded = Box::leak(Box::new(
        Loaded::project(&dir).expect("the probe program loads"),
    ));
    let message = match Jit::compile(loaded, &["lam.go"]) {
        Ok(_) => panic!("a lambda was compiled"),
        Err(e) => e.to_string(),
    };
    assert!(
        message.contains("lambda"),
        "`lam.go` was refused as `{message}`, which does not name the lambda"
    );
    std::fs::remove_dir_all(&dir).ok();
}
