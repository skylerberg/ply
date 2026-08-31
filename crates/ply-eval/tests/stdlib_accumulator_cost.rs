//! Whether the standard library's own accumulators copy, counted rather than
//! timed — **and on which of the two engines they do not**.
//!
//! ADR 0020 §4.1 found `std.json`'s string serializer quadratic in the number of
//! escapes in one string — client-influenced input, in shipped code — and
//! `spikes/ply-lexer/GAPS.md` §1 gives the rule it breaks: a growing container
//! must be built in the **last sub-expression of its enclosing node**, or the
//! pending frame carries the scope, the accumulator is at two owners, and `push`
//! takes its copying branch (`builtins.rs:456-473`). A survey with this counter
//! found the same shape at three sites and all three were fixed: `json.ply`'s
//! `escape_runs`, `router.ply`'s `numbered`, `trace.ply`'s `append`.
//!
//! **The rule, and therefore the fix, is the machine engine's.** The
//! tree-walker runs no reference counting at all: `Interp::lookup` answers every
//! `Var` with `v.clone()`, `Own` exists only on `code::Node` — which lowering
//! produces and the tree-walker has no lowering step — and neither
//! `Env::take_unique` nor `rc::carry` has a call site in `interp.rs`. So the
//! accumulator is at two owners at *every* `push` whatever position it is
//! written in, `Arc::get_mut` fails, and all three sites stay quadratic there.
//! That is not a suspicion; it is
//! `all_three_fixes_are_the_machine_engines_only` below, which pins one copy
//! per element on the tree-walker so that the day somebody makes it reuse, this
//! file fails and names the documents to correct.
//!
//! The property is stated as a **count, not a duration**. `rc::stats` is
//! deterministic — two runs of one program agree to the digit whatever the
//! machine is doing — so this belongs in the parallel shards rather than in
//! `.github/ci-shards.sh`'s `DEFERRED` table, which exists for tests whose
//! assertion reads a wall clock.
//!
//! **This raises a shipped bound and says so.** `escape_runs` holds one frame
//! per escape against `limit::DEFAULT_MAX_CALLS` (10,000), so `ply test` cannot
//! encode a string of more than 9,993 escapes at all. The two largest sizes here
//! run with `with_max_calls` raised to 200,000, which is what lets the count be
//! read well past the depth the clock can reach. It buys reach, not headroom:
//! the shipped ceiling is unchanged and no shipped program gets these sizes.

use ply_eval::{Interp, Machine, TaskRegions, rc};
use ply_span::{SourceMap, Symbol};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::parse_program;
use ply_syntax::resolve::{Resolved, resolve};

/// A probe that encodes a string of `k` characters, against `std.json` as it
/// ships — `ply_std::source`, not a copy of the shape.
///
/// `byte` is what every character of the subject is: `\"` when the point is to
/// make every position an escape, an ordinary letter when the point is that
/// none of them is. The encoded length is asserted inside the probe, so a
/// probe that silently encodes something else fails rather than being counted.
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
///
/// Nothing but `numbered` performs a `push` here: the table is `map` over
/// `range` and each pattern is a list literal, both of which allocate their
/// answer whole, and `well_formed`'s fold calls `concat_faults` with the empty
/// fault list of a well-formed table. The probe asserts the table *is* well
/// formed, so one that started reporting faults would fail rather than be
/// counted with the pushes those faults cost. Confirmed by the count itself:
/// exactly `n` pushes for `n` routes, no slack used.
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
///
/// The sink is the **last** argument of `fill` on purpose. Written
/// `fill(event_step(s, ..), n - 1)` this probe would be quadratic for a reason
/// that is the probe's and not `append`'s — `GAPS.md` §1 is about the caller as
/// much as the callee — and the count would be measuring this file.
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

/// The probe, plus `roots` and everything they import, transitively — against
/// the modules as they ship (`ply_std::source`), never a copy of the shape.
///
/// Transitively, because `std.router` imports `std.http` which imports
/// `std.net`, and a module left out is an unresolved name rather than a
/// different measurement.
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

/// The pushes one run performed, and how many of them copied the accumulator
/// rather than rewriting it.
struct Counted {
    updates: u64,
    copies: u64,
}

/// One accumulator site: what to call it, how to drive it at a size, and the
/// sizes to drive it at.
type Site = (&'static str, fn(usize, Engine) -> Counted, &'static [usize]);

/// Which evaluator the counters are taken on.
///
/// Ply ships two and `--engine both` is the audit that catches one drifting from
/// the other, so a cost claim naming neither is a claim about half the product.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Engine {
    Machine,
    Treewalk,
}

/// Counters are per thread and cumulative, so this takes them either side of the
/// one test rather than resetting them — a reset would discard whatever a
/// neighbouring test on this thread had counted.
fn count(program: &Program, resolved: &Resolved, engine: Engine) -> Counted {
    // By module and ordinal, never by index into the whole program: `std.json`
    // ships 37 tests of its own, so `eval_test(0)` counts one of those and
    // `test_count()` is 38. The first shape of this test did exactly that.
    let probe = Symbol::from("probe");
    let before = rc::stats();
    let outcome = match engine {
        Engine::Machine => {
            let mut machine = Machine::for_program(program, resolved).with_max_calls(200_000);
            machine.set_regions(TaskRegions::new());
            machine.eval_test_in(&probe, 0)
        }
        Engine::Treewalk => {
            let mut interp = Interp::for_program(program, resolved).with_max_calls(200_000);
            interp.set_regions(TaskRegions::new());
            interp.eval_test_in(&probe, 0)
        }
    };
    let after = rc::stats();

    // The signature failure here is a green count over a program that never ran:
    // a probe that fails to run performs no pushes and would sail past a
    // `copies <= 8` bound. The run has to have succeeded for the count to mean
    // anything.
    outcome.unwrap_or_else(|d| {
        panic!("the probe did not run on {engine:?}, so its counters measure nothing: {d:#?}")
    });

    Counted {
        updates: after.updates - before.updates,
        copies: (after.updates - before.updates)
            - (after.updates_in_place - before.updates_in_place),
    }
}

fn encode_on(k: usize, byte: &str, encoded_width: usize, engine: Engine) -> Counted {
    let (program, resolved) = load(&json_probe(k, byte, encoded_width), &["std.json"]);
    count(&program, &resolved, engine)
}

fn route_on(n: usize, engine: Engine) -> Counted {
    let (program, resolved) = load(&router_probe(n), &["std.http", "std.router"]);
    count(&program, &resolved, engine)
}

fn trace_on(n: usize, engine: Engine) -> Counted {
    let (program, resolved) = load(&trace_probe(n), &["std.trace"]);
    count(&program, &resolved, engine)
}

/// Every escape in the subject, so `k` is the number of escapes.
const ESCAPE: &str = "\\\"";

/// The sizes the `json` claim is read at. The last two are past the shipped call
/// ceiling and are reachable only because `count` raises it.
const SIZES: [usize; 6] = [1_000, 2_000, 4_000, 8_000, 16_000, 32_000];

/// The sizes the `router` and `trace` claims are read at — the ones ADR 0020 §7
/// item 3 reports the survey's before-counts at.
const TABLE_SIZES: [usize; 3] = [200, 400, 800];

/// Slack for pushes that are not the accumulator's — a probe's own scaffolding
/// and whatever the callee does around it. A **constant**, deliberately: the
/// defect makes this number a function of the size, so any constant refuses it.
/// All three probes in fact use none of it.
const SLACK: u64 = 8;

/// **`std.json`'s string serializer must not copy its accumulator per escape.**
///
/// The bound is a constant against a `k` that grows 32-fold, so the quadratic
/// this was written against — one whole-accumulator copy per escape, ADR 0020
/// §7 item 3 — fails it at every size by three orders of magnitude, and no
/// threshold-tuning makes the two outcomes close.
///
/// **On the machine engine.** The tree-walker still copies once per escape and
/// `all_three_fixes_are_the_machine_engines_only` is where that is pinned.
#[test]
fn encoding_a_string_of_escapes_copies_the_accumulator_a_constant_number_of_times() {
    println!("{:>8} {:>10} {:>10}", "k", "updates", "copies");
    let mut failures = Vec::new();
    for k in SIZES {
        let counted = encode_on(k, ESCAPE, 2, Engine::Machine);
        println!("{:>8} {:>10} {:>10}", k, counted.updates, counted.copies);

        // Proves the encode reached the accumulator loop rather than answering
        // early: one push per escape is the shape, whatever the copying does.
        assert!(
            counted.updates >= k as u64,
            "k = {k} performed only {} pushes, so this is not measuring `escape_runs`",
            counted.updates
        );
        if counted.copies > SLACK {
            failures.push(format!("k = {k}: {} copies", counted.copies));
        }
    }
    // The module's headline property, which the fix had to leave alone: "a string
    // with no escapes costs one pass and one copy" (`json.ply`, above
    // `escape_runs`). One `push`, whatever the length — a fix that bought the
    // linear shape by splitting every clean string into pieces would show up
    // here as a count that grows.
    for k in [1_000usize, 32_000] {
        let clean = encode_on(k, "a", 1, Engine::Machine);
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
///
/// ADR 0020 §7 item 3 records that the same counter found the same shape in
/// `router.ply`'s `numbered` (the growing field first of two, on the build-time
/// table check) and `trace.ply`'s `append` (first of three, on a serving path),
/// and that both were fixed — but nothing asserted it afterwards. The nearest
/// test, `ply-corpus`'s `what_the_route_table_costs_to_rebuild`, prints a
/// microsecond figure and asserts nothing at all, so "it passed" was never
/// evidence about either.
///
/// One bound per site, each armed against a revert of **its own** literal.
#[test]
fn the_route_table_and_the_trace_sink_do_not_copy_their_accumulators_either() {
    println!("{:>8} {:>10} {:>10}  site", "n", "updates", "copies");
    let mut failures = Vec::new();
    for (site, run) in [
        (
            "std.router `numbered`",
            route_on as fn(usize, Engine) -> Counted,
        ),
        ("std.trace `append`", trace_on),
    ] {
        for n in TABLE_SIZES {
            let counted = run(n, Engine::Machine);
            println!(
                "{:>8} {:>10} {:>10}  {site}",
                n, counted.updates, counted.copies
            );

            // Same guard as the `json` bound above, and for the same reason: a
            // probe that answered early would perform no pushes and sail past a
            // constant.
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

/// **The three fixes hold on the machine engine and on that engine only, and
/// this is what says so.**
///
/// Ply ships two evaluators and `--engine both` exists to catch one drifting
/// from the other. It compares *answers* — diagnostics, footprints, cell state
/// — so a divergence in **cost** passes it silently, and that is exactly what
/// these three fixes are. Measured here rather than argued: on the tree-walker
/// every `push` copies, so `copies == updates` at all three sites, at every
/// size, after the fix as before it.
///
/// The mechanism, read off `crates/ply-eval/`:
///
/// - the machine stamps each `Var` with [`ply_eval::Own`] during lowering
///   (`code.rs`, `lower_node`) from `rc::Live`'s backward walk, and moves the
///   value out of the scope at an `Own::Owned` occurrence (`machine.rs`, the
///   `NodeKind::Var` arm, `Env::take_unique`);
/// - a frame that will not read its scope again carries `Env::empty()` instead
///   (`rc::carry`, eight call sites in `frame.rs`, `machine.rs`, `handler.rs`);
/// - the tree-walker does **neither**. It evaluates the AST, which has no `own`
///   field because only lowering produces one; `Interp::lookup` answers
///   `Slot::Live(v) => Ok(v.clone())` unconditionally; `interp.rs` contains no
///   `take_unique` and no `carry`; and `Interp::eval` holds its scope by shared
///   reference, so the caller's bindings are live for the whole of every
///   subexpression by construction.
///
/// So the accumulator is at two owners at every `push` there — the binding plus
/// the argument clone — `Arc::get_mut` fails, and position cannot help.
///
/// **This test is a disclosure with an assertion under it.** It fails if the
/// tree-walker ever starts reusing, which is good news and still a failure,
/// because the six documents its message names would then all be wrong. A
/// disclosure nothing asserts is a claim waiting to go stale, which is the
/// defect class this repository spends most of its review budget on.
#[test]
fn all_three_fixes_are_the_machine_engines_only() {
    println!(
        "{:>22} {:>8} {:>10} {:>10}",
        "site", "n", "updates", "copies"
    );
    let mut reusing = Vec::new();
    let sites: [Site; 3] = [
        (
            "std.json `escape_runs`",
            |k, e| encode_on(k, ESCAPE, 2, e),
            &[1_000, 2_000, 4_000, 8_000],
        ),
        ("std.router `numbered`", route_on, &TABLE_SIZES),
        ("std.trace `append`", trace_on, &TABLE_SIZES),
    ];
    for (site, run, sizes) in sites {
        for &n in sizes {
            let counted = run(n, Engine::Treewalk);
            println!(
                "{:>22} {:>8} {:>10} {:>10}",
                site, n, counted.updates, counted.copies
            );
            assert!(
                counted.updates >= n as u64,
                "{site} at n = {n} performed only {} pushes on the tree-walker, so this is \
                 measuring something else",
                counted.updates
            );
            if counted.copies + SLACK < n as u64 {
                reusing.push(format!("{site} at n = {n}: {} copies", counted.copies));
            }
        }
    }
    assert!(
        reusing.is_empty(),
        "the tree-walker reused an accumulator it has never reused — {} — which is a real \
         improvement and makes six documents wrong. Correct each, in place and quoting the \
         withdrawn text: this file's module comment; `json.ply` above `escape_runs`; \
         `trace.ply` above `append`; `router.ply` above `numbered`; ADR 0020 §7 item 3; and \
         `spikes/ply-lexer/GAPS.md` §1 — every one of which states that the linear shape is \
         the machine engine's alone.",
        reusing.join(", "),
    );
}
