//! Whether the standard library's own accumulators copy, counted rather than timed.
//!
//! ADR 0020 §4.1 found `std.json`'s string serializer quadratic in the number of
//! escapes in one string — client-influenced input, in shipped code — and
//! `spikes/ply-lexer/GAPS.md` §1 gives the rule it breaks: a growing container
//! must be built in the **last sub-expression of its enclosing node**, or the
//! pending frame carries the scope, the accumulator is at two owners, and `push`
//! takes its copying branch (`builtins.rs:456-473`).
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

use ply_eval::{Machine, TaskRegions, rc};
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
fn probe(k: usize, byte: &str, encoded_width: usize) -> String {
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

fn load(src: &str) -> (Program, Resolved) {
    let mut map = SourceMap::new();
    let probe_id = map.add("probe.ply", src.to_string());
    let json = ModuleName::from_dotted("std.json");
    let json_src = ply_std::source(&json).expect("std.json ships with this compiler");
    let json_id = map.add(ply_std::pseudo_path(&json), json_src.to_string());

    let mut program = parse_program([
        (probe_id, ModuleName::from_dotted("probe"), src),
        (json_id, json, json_src),
    ])
    .expect("the probe and std.json must parse");
    assert!(
        ply_derive::expand_program(&mut program).is_empty(),
        "derive expansion must not diagnose"
    );
    let resolved = resolve(&program).expect("the probe must resolve");
    (program, resolved)
}

/// The pushes one encode performed, and how many of them copied the accumulator
/// rather than rewriting it.
struct Counted {
    updates: u64,
    copies: u64,
}

/// Counters are per thread and cumulative, so this takes them either side of the
/// one test rather than resetting them — a reset would discard whatever a
/// neighbouring test on this thread had counted.
fn encode(k: usize, byte: &str, encoded_width: usize) -> Counted {
    let src = probe(k, byte, encoded_width);
    let (program, resolved) = load(&src);
    let mut machine = Machine::for_program(&program, &resolved).with_max_calls(200_000);
    machine.set_regions(TaskRegions::new());

    // By module and ordinal, never by index into the whole program: `std.json`
    // ships 37 tests of its own, so `eval_test(0)` counts one of those and
    // `test_count()` is 38. The first shape of this test did exactly that.
    let probe_module = Symbol::from("probe");
    let before = rc::stats();
    let outcome = machine.eval_test_in(&probe_module, 0);
    let after = rc::stats();

    // The signature failure here is a green count over a program that never ran:
    // a probe that fails to encode performs no pushes and would sail past a
    // `copies <= 8` bound. The encode has to have succeeded for the count to
    // mean anything.
    outcome.unwrap_or_else(|d| {
        panic!("the probe at k = {k} did not encode, so its counters measure nothing: {d:#?}")
    });

    Counted {
        updates: after.updates - before.updates,
        copies: (after.updates - before.updates)
            - (after.updates_in_place - before.updates_in_place),
    }
}

/// The sizes the claim is read at. The last two are past the shipped call
/// ceiling and are reachable only because `encode` raises it.
const SIZES: [usize; 6] = [1_000, 2_000, 4_000, 8_000, 16_000, 32_000];

/// Slack for pushes that are not `escape_runs`' — the probe's own scaffolding
/// and whatever the codec does around the string. A **constant**, deliberately:
/// the defect makes this number a function of `k`, so any constant refuses it.
const SLACK: u64 = 8;

/// **`std.json`'s string serializer must not copy its accumulator per escape.**
///
/// The bound is a constant against a `k` that grows 32-fold, so the quadratic
/// this was written against — one whole-accumulator copy per escape, ADR 0020
/// §7 item 3 — fails it at every size by three orders of magnitude, and no
/// threshold-tuning makes the two outcomes close.
#[test]
fn encoding_a_string_of_escapes_copies_the_accumulator_a_constant_number_of_times() {
    println!("{:>8} {:>10} {:>10}", "k", "updates", "copies");
    let mut failures = Vec::new();
    for k in SIZES {
        let counted = encode(k, "\\\"", 2);
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
        let clean = encode(k, "a", 1);
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
