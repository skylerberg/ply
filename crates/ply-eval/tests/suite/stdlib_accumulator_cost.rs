//! Whether the standard library's own accumulators copy, counted rather than timed.

use ply_eval::{Machine, TaskRegions, rc};
use ply_span::{SourceMap, Symbol};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::parse_program;
use ply_syntax::resolve::{Resolved, resolve};

/// A probe that encodes a string of `k` characters, against `std.json` as it ships —
/// `ply_std::source`, not a copy of the shape.
fn json_probe(k: usize, byte: &str, encoded_width: usize) -> String {
    format!(
        r#"
import std.json

fn subject(k: Int) -> String =
  string_of_bytes(bytes_concat_all(map(range(0, k), |_i: Int| b"{byte}")))

test "enc" {{
  assert_eq(string_len(json::encode_string(subject({k}), json::string_json())), {len})
}}
"#,
        byte = byte,
        k = k,
        len = 2 + encoded_width * k,
    )
}

/// `std.router`'s `numbered`, driven through the public `well_formed`.
fn router_probe(n: usize) -> String {
    format!(
        r#"
import std.http
import std.router

fn table(n: Int) -> List<router::Route<Int>> =
  map(range(0, n), |i: Int|
    ({{method: http::Get,
      path: [router::Literal("a"), router::Literal(int_to_string(i))],
      endpoint: i}}))

test "wf" {{
  assert_eq(len(router::well_formed(table({n}))), 0)
}}
"#
    )
}

/// `std.trace`'s `append`, driven through the public `event_step`.
fn trace_probe(n: usize) -> String {
    format!(
        r#"
import std.trace

fn fill(n: Int, s: trace::Sink) -> trace::Sink =
  if n == 0 {{
    s
  }} else {{
    fill(n - 1, trace::event_step(s, "c", trace::Info, "e", map_new()))
  }}

test "tr" {{
  assert_eq(len(trace::drain(fill({n}, trace::sink()))), {n})
}}
"#
    )
}

/// The probe, plus `roots` and everything they import, transitively — against the modules as they
/// ship (`ply_std::source`), never a copy of the shape.
fn load(src: &str, roots: &[&str]) -> (Program, Resolved) {
    let mut map = SourceMap::new();
    let probe_id = map.add("probe.ply", src.to_string());
    let mut sources: Vec<(ply_span::SourceId, ModuleName, &'static str)> = Vec::new();
    let mut queue: Vec<ModuleName> = roots.iter().copied().map(ModuleName::from_dotted).collect();
    while let Some(name) = queue.pop() {
        if sources.iter().any(|(_, n, _)| *n == name) {
            continue;
        }
        let text = ply_std::source(&name).expect("this module ships with the compiler");
        let id = map.add(ply_std::pseudo_path(&name), text.to_string());
        let module =
            ply_syntax::parse_module(id, name.clone(), text).expect("a shipped module parses");
        queue.extend(
            module
                .imports
                .iter()
                .map(|i| i.module_name())
                .filter(ply_std::is_std),
        );
        sources.push((id, name, text));
    }

    let mut program = parse_program(
        std::iter::once((probe_id, ModuleName::from_dotted("probe"), src))
            .chain(sources.iter().map(|(id, n, t)| (*id, n.clone(), *t))),
    )
    .expect("the probe and the modules it imports must parse");
    assert!(
        ply_derive::expand_program(&mut program).is_empty(),
        "derive expansion must not diagnose"
    );
    let resolved = resolve(&mut program).expect("the probe must resolve");
    (program, resolved)
}

/// The pushes one run performed, and how many of them copied the accumulator rather than rewriting
/// it.
struct Counted {
    updates: u64,
    copies: u64,
}

/// Counters are per thread and cumulative, so this takes them either side of the one test rather
/// than resetting them — a reset would discard whatever a neighbouring test on this thread had
/// counted.
fn count(program: &Program, resolved: &Resolved) -> Counted {
    // By module and ordinal, never by index into the whole program: `std.json` ships 37 tests of
    // its own, so `eval_test(0)` counts one of those and `test_count()` is 38.
    let probe = Symbol::from("probe");
    let before = rc::stats();
    let outcome = {
        let mut machine = Machine::for_program(program, resolved).with_max_calls(200_000);
        machine.set_regions(TaskRegions::new());
        machine.eval_test_in(&probe, 0)
    };
    let after = rc::stats();

    // The signature failure here is a green count over a program that never ran: a probe that fails
    // to run performs no pushes and would sail past a `copies <= 8` bound.
    outcome.unwrap_or_else(|d| {
        panic!("the probe did not run, so its counters measure nothing: {d:#?}")
    });

    Counted {
        updates: after.updates - before.updates,
        copies: (after.updates - before.updates)
            - (after.updates_in_place - before.updates_in_place),
    }
}

fn encode_on(k: usize, byte: &str, encoded_width: usize) -> Counted {
    let (program, resolved) = load(&json_probe(k, byte, encoded_width), &["std.json"]);
    count(&program, &resolved)
}

fn route_on(n: usize) -> Counted {
    let (program, resolved) = load(&router_probe(n), &["std.http", "std.router"]);
    count(&program, &resolved)
}

fn trace_on(n: usize) -> Counted {
    let (program, resolved) = load(&trace_probe(n), &["std.trace"]);
    count(&program, &resolved)
}

/// Every escape in the subject, so `k` is the number of escapes.
const ESCAPE: &str = "\\\"";

/// The sizes the `json` claim is read at.
const SIZES: [usize; 6] = [1_000, 2_000, 4_000, 8_000, 16_000, 32_000];

/// The sizes the `router` and `trace` claims are read at — the ones the accumulator fixes item 3 reports the
/// survey's before-counts at.
const TABLE_SIZES: [usize; 3] = [200, 400, 800];

/// Slack for pushes that are not the accumulator's — a probe's own scaffolding and whatever the
/// callee does around it.
const SLACK: u64 = 8;

/// **`std.json`'s string serializer must not copy its accumulator per escape.**
#[test]
fn encoding_a_string_of_escapes_copies_the_accumulator_a_constant_number_of_times() {
    println!("{:>8} {:>10} {:>10}", "k", "updates", "copies");
    let mut failures = Vec::new();
    for k in SIZES {
        let counted = encode_on(k, ESCAPE, 2);
        println!("{:>8} {:>10} {:>10}", k, counted.updates, counted.copies);

        // Proves the encode reached the accumulator loop rather than answering early: one push per
        // escape is the shape, whatever the copying does.
        assert!(
            counted.updates >= k as u64,
            "k = {k} performed only {} pushes, so this is not measuring `escape_runs`",
            counted.updates
        );
        if counted.copies > SLACK {
            failures.push(format!("k = {k}: {} copies", counted.copies));
        }
    }
    // The module's headline property, which the fix had to leave alone: "a string with no escapes
    // costs one pass and one copy" (`json.ply`, above `escape_runs`).
    for k in [1_000usize, 32_000] {
        let clean = encode_on(k, "a", 1);
        println!(
            "{:>8} {:>10} {:>10}  (no escapes)",
            k, clean.updates, clean.copies
        );
        assert_eq!(
            clean.updates, 1,
            "a string of {k} characters that need no escaping performed {} pushes; the whole \
             point of the native scan is that it costs one piece and one copy however long it is",
            clean.updates
        );
        assert_eq!(clean.copies, 0, "and that one push must not copy");
    }

    assert!(
        failures.is_empty(),
        "`std.json`'s `escape_runs` copied the whole accumulator per escape — {} — against a \
         budget of {SLACK}, a constant. A growing container must be built in the last \
         sub-expression of its enclosing node (`spikes/ply-lexer/GAPS.md` §1); nesting the \
         `push` at argument 0 of 2 makes a served response that echoes attacker-influenced \
         text quadratic in the escapes the client chose.",
        failures.join(", "),
    );
}

/// **The other two sites the survey fixed, gated at last.**
#[test]
fn the_route_table_and_the_trace_sink_do_not_copy_their_accumulators_either() {
    println!("{:>8} {:>10} {:>10}  site", "n", "updates", "copies");
    let mut failures = Vec::new();
    for (site, run) in [
        ("std.router `numbered`", route_on as fn(usize) -> Counted),
        ("std.trace `append`", trace_on),
    ] {
        for n in TABLE_SIZES {
            let counted = run(n);
            println!(
                "{:>8} {:>10} {:>10}  {site}",
                n, counted.updates, counted.copies
            );

            // Same guard as the `json` bound above, and for the same reason: a probe that answered
            // early would perform no pushes and sail past a constant.
            assert!(
                counted.updates >= n as u64,
                "n = {n} performed only {} pushes, so this is not measuring {site}",
                counted.updates
            );
            if counted.copies > SLACK {
                failures.push(format!("{site} at n = {n}: {} copies", counted.copies));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "a standard-library accumulator copied itself whole per element — {} — against a budget \
         of {SLACK}, a constant. `numbered`'s `out` and `append`'s `records` must each stay the \
         **last** field of their record literal (`spikes/ply-lexer/GAPS.md` §1): written first, \
         the pending frame carries the scope, the list is at two owners, and `push` copies. \
         `numbered` is on every `conflicts`/`well_formed` assertion a service makes about its \
         table; `append` is on the path of every record a served request traces.",
        failures.join(", "),
    );
}
