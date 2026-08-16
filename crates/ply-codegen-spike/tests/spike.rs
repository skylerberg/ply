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
    let entry = harness.compiled.entry(READ_LINE).expect("compiled");
    let expected = harness.machine.call(READ_LINE, input.args.clone(), Span::DUMMY);
    let actual = harness.compiled_call(entry, &input.args);
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

#[test]
fn a_call_the_spike_did_not_compile_goes_through_the_machine_and_still_agrees() {
    let mut harness = Harness::with(&[READ_LINE], Opts::default()).expect("compiles");
    harness.ctx.machine_calls = 0;
    for input in inputs() {
        assert!(
            agree_on(&mut harness, &input),
            "the trampolined form disagreed on `{}`",
            input.name
        );
    }
    assert!(
        harness.ctx.machine_calls > 0,
        "the trampoline was never taken, so this measured nothing"
    );
}

#[test]
fn the_fragment_refuses_what_it_cannot_compile_and_names_it() {
    let loaded: &'static Loaded = Box::leak(Box::new(Loaded::std_library().expect("the stdlib")));
    for (name, expected) in [
        ("std.http.parse_head", "field access"),
        ("std.http.list_field", "list literal"),
        ("std.net.send_from", "perform"),
        ("std.http.contains_string", "lambda"),
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
