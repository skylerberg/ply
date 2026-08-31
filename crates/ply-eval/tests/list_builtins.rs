//! The list index on real source, through parse, resolve, check and both
//! engines.
//!
//! Follows `map_builtins.rs`. Both engines are run over every `test` because
//! `builtins::call` is one definition they share and this file is the run that
//! stands beside that construction argument — see ADR 0027 §5 for what that
//! argument is and what it is not.
//!
//! # What each test was seen to fail against
//!
//! House rule: a gate nobody has watched fail is not a gate. Every corruption
//! below was applied, the named test was watched to go red, and the corruption
//! was reverted.
//!
//! | corruption | test that went red |
//! | --- | --- |
//! | `at`: `xs.get(i)` → `xs.get(i.saturating_sub(1))` | `an_index_inside_the_list_answers_that_element` |
//! | `at`: `usize::try_from(i).ok()` → `Some(i.max(0) as usize)` | `an_index_outside_the_list_is_absent_rather_than_clamped` |
//! | `ListAt`: `option(at(xs, i).cloned())` → `option(None)` | `an_index_inside_the_list_answers_that_element` |
//! | omit `Builtin::ListAt` from `Builtin::all()` | `builtins::tests::builtin_all_is_complete_and_lists_each_name_once` |
//! | give `list_at`'s **prelude scheme** a third parameter | `ply-core`'s `list_builtins::a_third_argument_to_list_at_is_refused_by_the_scheme` |
//! | give `ListAt` **`Builtin::arity()`** `(2, 3)` | **nothing — recorded as a one-directional hole, not as a gate** |
//! | give `ListAt` **`Builtin::arity()`** `(1, 1)` | every test in this file that calls `list_at`, at run time |
//! | add `ListAt` to `higher_order()` | `region_kind::tests::the_callback_builtins_are_the_six_this_module_knows` and `builtins::tests::exactly_the_callback_builtins_are_higher_order` |
//! | `costs.rs`: drop `Builtin::ListAt`'s arm, falling through to `_ => Owner::Fresh` | `ownership_checker_armed::an_append_onto_an_indexed_element_is_flagged_and_the_counters_confirm_it` — but only after that test's program was rewritten; see the note below |
//! | `costs.rs`: keep the arm, give it `map_get`'s cause and a reason that does not name `list_at` | the same test's `reason.contains("list_at")` assertion |
//!
//! **Two of those rows are not the tests this change expected to name, and the
//! difference is the finding.** Both were predicted against tests that turned
//! out to be vacuous over the corruption:
//!
//! - Omitting a variant from `Builtin::all()` left
//!   `every_builtin_is_reachable_by_the_name_it_reports` **green**, because that
//!   test *iterates* `all()`: a variant missing from the list is never named, so
//!   it is never checked — and the same is true of every other table driven by
//!   `all()`. Nothing checked that `all()` was complete.
//!   `builtin_all_is_complete_and_lists_each_name_once` was written for this and
//!   the corruption was then seen to go red against it.
//! - Giving `ListAt` an arity of `(2, 3)` left
//!   `every_builtin_checks_its_argument_count` **green**, because that test
//!   asserts the *declared* arity is enforced, not that it is right: `(2, 3)`
//!   still refuses one argument and still refuses four. It also leaves the
//!   `ply-core` test green, because that test corrupts the **scheme** and the
//!   scheme is a different table.
//!
//!   ~~`Builtin::arity()` is a backstop for a call that never met inference,
//!   and **no test in this tree gates its value**; what a well-typed program
//!   meets is the scheme, and that is what `ply-core` pins.~~ **Half of that is
//!   wrong and the review that found it ran the other corruption.**
//!   `builtins::call` reads `b.arity()` on *every* call, well-typed or not
//!   (`builtins.rs:558`; `region_kind.rs:1086` and `value.rs:169` read it too),
//!   so giving `ListAt` `(1, 1)` is not a silent hole at all — it reddens five
//!   of the six tests in this file, at run time, under the tree-walker. The
//!   hole is **one-directional**: an arity *wider* than the truth is
//!   unreachable from a well-typed program and therefore invisible, and an
//!   arity *narrower* than the truth is caught by any test that calls the
//!   builtin. That is the shape of the drift already in the table — `assert`
//!   and `range` are both `(1, 2)` over schemes of 1 and 2 arguments, i.e.
//!   both too wide — and it is why the hole has cost nothing so far.
//!
//!   A general "arity agrees with the scheme" test cannot simply be written,
//!   and the reason is worth knowing: `assert` is `(1, 2)` and `range` is
//!   `(1, 2)` in `Builtin::arity()`, but their schemes take 1 and 2 arguments
//!   respectively — so `assert(c, "msg")` and `range(5)` are **`E0202`** and
//!   the second leg of each arity is unreachable from any well-typed program.
//!   Two hand-maintained tables have already drifted apart twice, in the
//!   direction that costs nothing, and nothing notices.
//!
//! - **The ownership row above was itself vacuous as first shipped, and a
//!   review caught it.** `COPIES_VIA_LIST_AT` originally wrote the append as
//!   `len(push(row, i)) + touch(...)`, which the *position* rule flags on its
//!   own: the verdict was `Copies` with the reason *"the scope binding `row` is
//!   still held by an enclosing frame"* both with `result_owner`'s `ListAt` arm
//!   and without it, so deleting the arm left the test **green**. The program
//!   now puts the `push` in a helper's tail, where the position rule has
//!   nothing to say and the verdict can only come from `result_owner`, and the
//!   test additionally asserts that the reason **names `list_at`** — because a
//!   right answer reached by the wrong rule is what this row claimed to have
//!   watched fail and had not. Both corruptions were then seen to go red.
//!
//! One mutation is **equivalent** and is recorded as equivalent rather than as
//! a hole: replacing `usize::try_from(i).ok()` with `Some(i as usize)` leaves
//! `-1` mapping to `2^64 - 1`, which `Vec::get` refuses on any list that fits
//! in memory. A test written to kill it would be asserting the spelling of that
//! line rather than a fact about the language, which is the assertion-growth
//! `spikes/ply-parser/GAPS.md` §2 warns against.

use ply_core::{CheckOutput, check_program};
use ply_eval::{Interp, Machine};
use ply_span::SourceId;
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::{Resolved, resolve};

struct Compiled {
    program: Program,
    resolved: Resolved,
    check: CheckOutput,
}

fn compile(source: &str) -> Compiled {
    let inputs = vec![(SourceId(0), ModuleName::from_dotted("m"), source)];
    let mut program = match ply_syntax::parse_program(inputs) {
        Ok(p) => p,
        Err(d) => panic!("did not parse: {d:#?}"),
    };
    let resolved = match resolve(&mut program) {
        Ok(r) => r,
        Err(d) => panic!("did not resolve: {d:#?}"),
    };
    let check = match check_program(&program, &resolved) {
        Ok(c) => c,
        Err(d) => panic!("did not typecheck: {d:#?}"),
    };
    Compiled {
        program,
        resolved,
        check,
    }
}

/// Every `test` in the source, under both engines, which must agree.
fn run_both(source: &str) -> Compiled {
    let c = compile(source);
    assert!(!c.check.tests.is_empty(), "the source declares no test");
    let mut interp = Interp::new(&c.program, &c.resolved, &c.check);
    for (i, t) in c.check.tests.iter().enumerate() {
        if let Err(d) = interp.eval_test(i) {
            panic!("`{}` failed under the tree-walker: {d:#?}", t.name);
        }
    }
    let mut machine = Machine::new(&c.program, &c.resolved, &c.check);
    for (i, t) in c.check.tests.iter().enumerate() {
        if let Err(d) = machine.eval_test(i) {
            panic!("`{}` failed under the machine: {d:#?}", t.name);
        }
    }
    c
}

/// Every position of a three-element list, plus the two derivations the builtin
/// is meant to make expressible — `head` and `last`, which ADR 0027 §3 declines
/// to ship as names of their own.
#[test]
fn an_index_inside_the_list_answers_that_element() {
    let source = r#"
fn xs() -> List<Int> = [10, 20, 30]

fn head(ys: List<Int>) -> Option<Int> = list_at(ys, 0)
fn last(ys: List<Int>) -> Option<Int> = list_at(ys, len(ys) - 1)

test "every position of a three-element list" {
  assert_eq(list_at(xs(), 0), Some(10));
  assert_eq(list_at(xs(), 1), Some(20));
  assert_eq(list_at(xs(), 2), Some(30))
}

test "head and last are the index, not builtins of their own" {
  assert_eq(head(xs()), Some(10));
  assert_eq(last(xs()), Some(30));
  assert_eq(head([]), None);
  assert_eq(last([]), None)
}
"#;
    run_both(source);
}

/// Absent, not clamped, and not counted from the end.
///
/// `-1` is the reading a Python author brings and the one this builtin
/// deliberately refuses, so it is asserted rather than left to the GUIDE. The
/// two extremes are here because `usize::try_from` is what stands between them
/// and a wrong answer.
#[test]
fn an_index_outside_the_list_is_absent_rather_than_clamped() {
    let source = r#"
fn xs() -> List<Int> = [10, 20, 30]

test "at the end and past it" {
  assert_eq(list_at(xs(), 3), None);
  assert_eq(list_at(xs(), 4), None)
}

test "a negative index is absent, not the last element" {
  assert_eq(list_at(xs(), -1), None);
  assert_eq(list_at(xs(), -3), None)
}

test "the extremes of Int" {
  assert_eq(list_at(xs(), 9223372036854775807), None);
  assert_eq(list_at(xs(), 0 - 9223372036854775807 - 1), None)
}

test "the empty list has no element anywhere" {
  assert_eq(list_at([], 0), None);
  assert_eq(list_at([], 1), None);
  assert_eq(list_at([], -1), None)
}
"#;
    run_both(source);
}

/// The element type is a parameter, so a record, an ADT value and a nested list
/// all have to come back whole. A builtin that answered a shallow copy would be
/// caught here rather than by a program that read one back.
#[test]
fn the_element_comes_back_whole_whatever_it_is() {
    let source = r#"
type Row = { id: Int, name: String }
type Shape = Circle(Int) | Rect(Int, Int) | Point

fn rows() -> List<Row> = [{id: 1, name: "a"}, {id: 2, name: "b"}]
fn shapes() -> List<Shape> = [Circle(3), Rect(2, 5), Point]
fn nested() -> List<List<Int>> = [[1, 2], [], [3]]

fn unwrap_int(o: Option<Int>) -> Int = match o { Some(v) -> v, None -> 0 - 1 }

test "a record element" {
  assert_eq(list_at(rows(), 1), Some({id: 2, name: "b"}))
}

test "an ADT element, payload and payload-free" {
  assert_eq(list_at(shapes(), 0), Some(Circle(3)));
  assert_eq(list_at(shapes(), 1), Some(Rect(2, 5)));
  assert_eq(list_at(shapes(), 2), Some(Point))
}

test "a list element, which is also what makes list_at nest" {
  assert_eq(list_at(nested(), 0), Some([1, 2]));
  assert_eq(list_at(nested(), 1), Some([]));
  assert_eq(
    unwrap_int(match list_at(nested(), 0) { Some(inner) -> list_at(inner, 1), None -> None }),
    2
  )
}
"#;
    run_both(source);
}

/// The builtin is pure and takes no callback, so a call publishes exactly the
/// row of the expressions it was given — which for `list_at` is nothing of its
/// own.
#[test]
fn the_index_performs_nothing_of_its_own() {
    let source = r#"
effect log {
  write note[out](line: String) -> Unit
}

fn quiet(xs: List<Int>) -> Option<Int> = list_at(xs, 0)

fn loud(xs: List<Int>) -> Option<Int> / {log.write[out]} =
  list_at(xs, { log.note[out]("asked"); 0 })

test "a pure peek and an effectful index expression" {
  handle {
    assert_eq(quiet([10, 20]), Some(10));
    assert_eq(loud([10, 20]), Some(10))
  } with {
    log.note[out](line) -> (),
  }
}
"#;
    let c = run_both(source);
    assert_eq!(
        c.check.defs[&ply_span::Symbol::new("m.quiet")]
            .footprint
            .to_string(),
        "{}"
    );
    assert_eq!(
        c.check.defs[&ply_span::Symbol::new("m.loud")]
            .footprint
            .to_string(),
        "{m.log.write[out]}",
        "an argument's row is the call's, which is the half of strictness a peek can observe"
    );
}

/// A user definition of the name shadows the builtin, which is what keeps
/// `crates/ply-std/ply/json.ply` and `db.ply` — each of which ships an `nth` of
/// its own — free to keep theirs, and is the reason ADR 0027 §6 declines the
/// bare name `nth`.
#[test]
fn a_module_may_shadow_the_name() {
    let source = r#"
fn list_at<a>(xs: List<a>, i: Int) -> Int = 0 - 1

test "the module's own definition wins" {
  assert_eq(list_at([1, 2, 3], 0), -1)
}
"#;
    run_both(source);
}

/// The index is not a callback builtin, so it cannot suspend, so a peek inside
/// a loop costs the loop nothing. Ten thousand peeks under a `fold` is well
/// past the 10,000-nested-call budget if any one of them nested.
///
/// This is also the shape the parser spike could not write at all: the peek it
/// needed was a `Map<Int, Token>` descent because the list had no index.
#[test]
fn ten_thousand_peeks_nest_no_deeper_than_one() {
    let source = r#"
fn at_or_zero(xs: List<Int>, i: Int) -> Int =
  match list_at(xs, i) { Some(v) -> v, None -> 0 }

fn sum_by_index(xs: List<Int>) -> Int =
  fold(range(0, len(xs)), 0, |acc, i| acc + at_or_zero(xs, i))

test "a peek per element over ten thousand elements" {
  assert_eq(sum_by_index(range(0, 10000)), 49995000)
}
"#;
    run_both(source);
}
