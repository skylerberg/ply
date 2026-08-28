//! `W0611`, one row of `children()`'s table at a time.
//!
//! `crates/ply-eval/tests/field_order_oracle.rs` is the load-bearing test: it
//! checks the pass against `rc::Stats` on programs that are actually run. This
//! file checks the thing that test cannot — that each *kind* of enclosing node
//! is modelled, rather than the four the spike files happen to use.
//!
//! **Every test here isolates one row.** That is not a style note; it is the
//! defect the round-1 version of this file shipped. Nine of twelve rows had no
//! test that could fail: an adversarial review corrupted each slot on its own
//! and both suites stayed green under If-cond, Match-scrutinee, Match-guard,
//! Perform-args, Handle-body, Handle-clause, WithCell-init, Field-base and
//! Unary-operand. The sharpest instance was
//! `an_if_condition_fires_and_a_branch_inherits`, whose probe was
//!
//! > `if len(push(xs, i)) > 0 { 1 } else { 0 }`
//!
//! — which fires through the `Binary` row, not the `If` row, and so passed with
//! the `If` row wrong. Its replacement calls a `Bool`-returning function so that
//! the condition is the only carried node in the expression. A probe here earns
//! its place by going red when its own row is corrupted and staying green when
//! any other row is.

use ply_span::SourceMap;
use ply_syntax::ast::ModuleName;
use ply_syntax::parse_program;
use ply_syntax::resolve::resolve;
use std::path::Path;

fn load(source: &str) -> (ply_syntax::ast::Program, ply_syntax::resolve::Resolved) {
    let mut map = SourceMap::new();
    let name = ModuleName::from_relative_path(Path::new("t.ply")).expect("a module name");
    let id = map.add("t.ply", source.to_string());
    let mut program = parse_program(vec![(id, name, source)])
        .unwrap_or_else(|d| panic!("the fixture does not parse: {d:?}"));
    assert!(ply_derive::expand_program(&mut program).is_empty());
    let resolved =
        resolve(&program).unwrap_or_else(|d| panic!("the fixture does not resolve: {d:?}"));
    (program, resolved)
}

/// The simple names of the definitions `W0611` fires in, with one firing per
/// entry, so a definition that fires twice appears twice.
fn firings(source: &str) -> Vec<String> {
    let (program, resolved) = load(source);
    ply_core::fieldorder::firings(&program, &resolved)
        .into_iter()
        .map(|f| f.simple.as_str().to_string())
        .collect()
}

fn fires_in(source: &str, name: &str) -> usize {
    firings(source).iter().filter(|f| *f == name).count()
}

/// Which of the two routes each firing took, read off its primary label.
///
/// The two are different claims about the machine — one about a frame holding
/// the *scope*, one about a frame holding an already-computed *value* — and a
/// row that only changes which of them fires is invisible to a count.
fn routes(source: &str) -> Vec<Route> {
    let (program, resolved) = load(source);
    ply_core::fieldorder::check(&program, &resolved)
        .iter()
        .map(|d| {
            let label = d
                .labels
                .iter()
                .find(|l| l.primary)
                .map(|l| l.message.as_str())
                .unwrap_or_default();
            if label.starts_with("an earlier sub-expression") {
                Route::Alias
            } else {
                Route::Carried
            }
        })
        .collect()
}

#[derive(PartialEq, Eq, Debug)]
enum Route {
    /// A pending frame is holding the scope.
    Carried,
    /// A pending frame is holding a value that contains the list.
    Alias,
}

const PRELUDE: &str = "\
type S = { a: Int, xs: List<Int> }
type W = { xs: List<Int> }
effect ev {
  read op(a: Int, b: List<Int>) -> Int
  read op2(a: List<Int>, b: Int) -> Int
}
fn sink(a: List<Int>, b: Int) -> List<Int> = a
fn sink3(a: Int, b: List<Int>, c: Int) -> List<Int> = b
fn sinki(a: Int, b: Int) -> Int = a
fn pred(a: List<Int>) -> Bool = true
fn wrap(a: List<Int>) -> W = { xs: a }
fn constf(a: List<Int>) -> (Int) -> Int = |z: Int| z
";

fn probe(body: &str) -> String {
    format!("{PRELUDE}fn f(xs: List<Int>, i: Int) -> {body}\n")
}

// --- the positional route, one node kind at a time --------------------------

#[test]
fn a_record_field_that_is_not_last_fires_and_the_last_one_does_not() {
    assert_eq!(
        fires_in(&probe("S = { xs: push(xs, i), a: i }"), "f"),
        1,
        "a `push` in the first of two fields is carried"
    );
    assert_eq!(
        fires_in(&probe("S = { a: i, xs: push(xs, i) }"), "f"),
        0,
        "a `push` in the last field is not"
    );
}

/// A record literal is evaluated in the order it is *written*, not in the order
/// a `BTreeMap` would put the names in. Measured before it was asserted: with
/// `type S = { a: Int, b: List<Int> }`, `{ b: push(s.b, i), a: 0 }` reads
/// `in_place` 0.0000 at n = 200 and n = 400, which is the source order's
/// answer; the alphabetical order would put the `push` last beside a constant
/// and read 1.0.
#[test]
fn a_record_literal_is_evaluated_in_the_order_it_is_written() {
    const SOURCE: &str = "\
type T = { a: Int, b: List<Int> }
fn f(s: T, i: Int) -> T = { b: push(s.b, i), a: 0 }
";
    assert_eq!(fires_in(SOURCE, "f"), 1);
}

#[test]
fn a_call_argument_that_is_not_last_fires_and_the_last_one_does_not() {
    assert_eq!(fires_in(&probe("List<Int> = sink(push(xs, i), i)"), "f"), 1);
    assert_eq!(
        fires_in(&probe("List<Int> = sink3(i, push(xs, i), i)"), "f"),
        1,
        "argument 1 of 3 is still followed by one"
    );
    assert_eq!(
        fires_in(&probe("List<Int> = sink3(i, i, i) "), "f"),
        0,
        "no `push` at all"
    );
}

/// The callee expression of a call with arguments is carried too —
/// `machine.rs:1035` passes `carry(&env, !args.is_empty())`. Isolated by
/// putting the `push` in the callee position and nowhere else: the inner call
/// `constf(push(xs, i))` puts it last among *its* arguments, so only the outer
/// call's treatment of its callee can fire.
#[test]
fn the_callee_of_a_call_with_arguments_is_carried() {
    assert_eq!(fires_in(&probe("Int = constf(push(xs, i))(0)"), "f"), 1);
    assert_eq!(
        fires_in(&probe("(Int) -> Int = constf(push(xs, i))"), "f"),
        0,
        "with no outer call there is no argument list to carry the scope past"
    );
}

/// ADR 0020 §5.2's sharpest prediction: `carry` is `if remaining {
/// env.clone() }` and never asks what the remaining sub-expression reads, so a
/// literal constant after the call is enough. A lint that consulted free
/// variables would be silent here and would be wrong.
#[test]
fn a_constant_after_the_push_is_enough_to_fire() {
    assert_eq!(fires_in(&probe("List<Int> = sink(push(xs, i), 0)"), "f"), 1);
}

#[test]
fn a_list_item_that_is_not_last_fires() {
    assert_eq!(
        fires_in(&probe("List<List<Int>> = [push(xs, i), xs]"), "f"),
        1
    );
    // > **Corrected 2026-08-28. This assertion used to be `0`, with the
    // > comment that the last item is not carried:** the position is right and
    // > the conclusion was wrong. `Frame::ListItem::done` is holding the first
    // > item's value — `xs` itself — while the second runs, so the `push`
    // > copies anyway. Measured through `last_of([xs, push(xs, i)])` folded 200
    // > and 400 times: `in_place` **0.0000** at both sizes.
    assert_eq!(
        fires_in(&probe("List<List<Int>> = [xs, push(xs, i)]"), "f"),
        1,
        "the last item, with the first one still holding the same list"
    );
    assert_eq!(
        fires_in(&probe("List<List<Int>> = [[], push(xs, i)]"), "f"),
        0,
        "the last item, with nothing before it that holds anything"
    );
}

/// Measured before it was asserted: 400 updates and 200 in place over 200
/// steps, one copy at the statement and the tail's `push` in place.
#[test]
fn a_push_in_a_block_statement_fires_and_the_tail_does_not() {
    let source = probe("List<Int> = { let one = push(xs, i); push(one, i) }");
    assert_eq!(
        fires_in(&source, "f"),
        1,
        "the statement's `push` is carried by `Frame::BlockStep`; the tail's is not"
    );
}

/// `Frame::BinaryRhs` holds `env.clone()` while the left operand runs.
#[test]
fn the_left_operand_of_a_binary_fires_and_the_right_does_not() {
    assert_eq!(fires_in(&probe("Int = len(push(xs, i)) + i"), "f"), 1);
    assert_eq!(fires_in(&probe("Int = i + len(push(xs, i))"), "f"), 0);
}

/// `Frame::If` holds `env.clone()` while the condition runs; the branches
/// inherit the `if`'s own position.
///
/// The probe calls a `Bool`-returning function rather than comparing, because
/// a comparison would fire through the `Binary` row and pass with this row
/// wrong. That is exactly how the round-1 version of this test passed.
#[test]
fn an_if_condition_fires_and_a_branch_inherits() {
    assert_eq!(
        fires_in(&probe("Int = if pred(push(xs, i)) { 1 } else { 0 }"), "f"),
        1
    );
    assert_eq!(
        fires_in(
            &probe("List<Int> = if i > 0 { push(xs, i) } else { xs }"),
            "f"
        ),
        0,
        "a branch is where the `if` is, and the `if` is the whole body"
    );
    assert_eq!(
        fires_in(
            &probe("List<Int> = sink(if i > 0 { push(xs, i) } else { xs }, i)"),
            "f"
        ),
        1,
        "the same branch, under a call that carries the scope past it"
    );
}

/// `Frame::MatchArms` holds `env.clone()` while the scrutinee runs. The probe
/// puts the `push` last inside `len`, so only the scrutinee's own slot can
/// fire.
#[test]
fn a_match_scrutinee_fires_and_an_arm_body_inherits() {
    assert_eq!(
        fires_in(
            &probe("Int = match len(push(xs, i)) { 0 -> 0, _ -> 1 }"),
            "f"
        ),
        1
    );
    assert_eq!(
        fires_in(&probe("List<Int> = match i { _ -> push(xs, i) }"), "f"),
        0,
        "an arm body is where the `match` is, and the `match` is the whole body"
    );
}

/// `Frame::MatchGuard` is pushed with the scrutinee and the enclosing scope
/// while a guard runs.
#[test]
fn a_match_guard_is_carried() {
    assert_eq!(
        fires_in(
            &probe("Int = match i { z if pred(push(xs, i)) -> 1, _ -> 0 }"),
            "f"
        ),
        1
    );
}

/// The other half of the guard row: `Frame::MatchGuard` holds the *scrutinee's
/// value* too, so a `push` in a guard onto the list being matched copies for a
/// second, independent reason. Only the route it is attributed to changes, so
/// this is checked on the label rather than on a count.
#[test]
fn a_match_guard_holds_the_scrutinee_as_well_as_the_scope() {
    assert_eq!(
        routes(&probe(
            "Int = match xs { z if pred(push(xs, i)) -> 1, _ -> 0 }"
        )),
        vec![Route::Alias]
    );
    assert_eq!(
        routes(&probe(
            "Int = match i { z if pred(push(xs, i)) -> 1, _ -> 0 }"
        )),
        vec![Route::Carried],
        "the same guard over a scrutinee that holds nothing"
    );
}

/// `handler.rs:208` carries the scope past every `perform` argument but the
/// last, exactly as a call does.
#[test]
fn a_perform_argument_that_is_not_last_fires() {
    assert_eq!(
        fires_in(&probe("Int / {ev.read} = ev.op2(push(xs, i), i)"), "f"),
        1
    );
    assert_eq!(
        fires_in(&probe("Int / {ev.read} = ev.op(i, push(xs, i))"), "f"),
        0
    );
}

/// A `handle` body runs with the scope in the `Prompt`, which outlives it.
#[test]
fn a_handle_body_is_carried() {
    assert_eq!(
        fires_in(
            &probe(
                "Int = handle { len(push(xs, i)) } with { ev.op(a, b) -> 0, ev.op2(a, b) -> 0 }"
            ),
            "f"
        ),
        1
    );
}

/// A clause body and a `return` clause body are entered with a scope built for
/// them, so each is its own root. The `handle` is put in a carried slot so that
/// inheriting and restarting give different answers — with the `handle` as the
/// whole body they agree, and the probe would pass with the row wrong.
#[test]
fn a_handler_clause_body_is_its_own_root() {
    assert_eq!(
        fires_in(
            &probe("Int = sinki(handle { 0 } with { ev.op(a, b) -> len(push(xs, i)) }, i)"),
            "f"
        ),
        0
    );
    assert_eq!(
        fires_in(
            &probe("Int = sinki(handle { 0 } with { return v -> len(push(xs, i)) }, i)"),
            "f"
        ),
        0,
        "the `return` clause, which `leave_handle` evaluates under `prompt.env.bind(..)`"
    );
}

/// `enter_with_cell` pushes `Frame::WithCellBody` with `env.clone()` while the
/// initial value runs; the body inherits.
#[test]
fn a_with_cell_initial_value_is_carried_and_the_body_is_not() {
    assert_eq!(
        fires_in(&probe("Int = with_cell[cel](push(xs, i)) { c -> 0 }"), "f"),
        1
    );
    assert_eq!(
        fires_in(
            &probe("List<Int> = with_cell[cel](0) { c -> push(xs, i) }"),
            "f"
        ),
        0
    );
}

/// `open_cell` moves the initial value into the arena, where the cell owns it
/// for the whole body — so the body holds it exactly as a record's later field
/// holds an earlier one.
#[test]
fn a_with_cell_body_holds_the_initial_value() {
    assert_eq!(
        fires_in(
            &probe("List<Int> = with_cell[cel](xs) { c -> push(xs, i) }"),
            "f"
        ),
        1
    );
}

/// `Frame::Unary` and `Frame::FieldAccess` carry neither a scope nor a value,
/// so their operand is where the node is. Both probes assert silence, which is
/// the direction a wrong row would break.
#[test]
fn a_unary_operand_and_a_field_base_are_where_the_node_is() {
    assert_eq!(fires_in(&probe("Int = -len(push(xs, i))"), "f"), 0);
    assert_eq!(fires_in(&probe("List<Int> = wrap(push(xs, i)).xs"), "f"), 0);
    assert_eq!(
        fires_in(&probe("Int = sinki(-len(push(xs, i)), i)"), "f"),
        1,
        "the same operand, under a call that carries the scope past it"
    );
}

/// No frame on the region or simulation paths holds a scope either.
#[test]
fn a_region_body_and_a_simulation_body_are_where_the_node_is() {
    assert_eq!(
        fires_in(&probe("List<Int> = with_region[reg] { push(xs, i) }"), "f"),
        0
    );
    assert_eq!(
        fires_in(&probe("List<Int> = simulate { push(xs, i) }"), "f"),
        0
    );
    assert_eq!(
        fires_in(
            &probe("List<Int> = sink(with_region[reg] { push(xs, i) }, i)"),
            "f"
        ),
        1,
        "the same body, under a call that carries the scope past it"
    );
}

/// The cumulative rule. A `push` last inside its own node still copies when
/// anything up the chain carried the scope.
#[test]
fn last_inside_an_inner_node_is_not_enough_if_an_outer_one_carried() {
    assert_eq!(
        fires_in(&probe("List<Int> = sink(sink3(i, push(xs, i), i), i)"), "f"),
        1
    );
    assert_eq!(fires_in(&probe("List<Int> = sink3(i, i, i)"), "f"), 0);
}

// --- the alias route --------------------------------------------------------

/// The route the round-1 pass had no model of at all, and the reason its
/// "over-approximation" claim was withdrawn. Each shape here measured
/// `in_place` **0.0000** at n = 200 and n = 400 while the pass was silent.
#[test]
fn an_earlier_sibling_still_holding_the_list_fires_from_the_last_position() {
    const RECORD: &str = "\
type T = { keep: List<Int>, toks: List<Int> }
fn f(s: T, i: Int) -> T = { keep: s.toks, toks: push(s.toks, i) }
";
    assert_eq!(fires_in(RECORD, "f"), 1, "the last field, aliased");
    assert_eq!(routes(RECORD), vec![Route::Alias]);

    const ARGUMENT: &str = "\
fn snd(a: List<Int>, b: List<Int>) -> List<Int> = b
fn f(s: List<Int>, i: Int) -> List<Int> = snd(s, push(s, i))
";
    assert_eq!(fires_in(ARGUMENT, "f"), 1, "the last argument, aliased");

    const ITEM: &str = "\
fn f(xs: List<Int>, i: Int) -> List<List<Int>> = [xs, push(xs, i)]
";
    assert_eq!(fires_in(ITEM, "f"), 1, "the last item, aliased");
}

/// The retention test is a **definite** alias test, so a sibling that mentions
/// the root without keeping the list is not a firing. Both of these measured
/// `in_place` 0.9950 at n = 200 and 0.9975 at n = 400.
#[test]
fn a_sibling_that_only_mentions_the_root_is_not_a_firing() {
    const LENGTH: &str = "\
type T = { n: Int, toks: List<Int> }
fn f(s: T, i: Int) -> T = { n: len(s.toks), toks: push(s.toks, i) }
";
    assert_eq!(fires_in(LENGTH, "f"), 0);

    const OTHER_FIELD: &str = "\
type T = { pos: Int, toks: List<Int> }
fn f(s: T, i: Int) -> T = { pos: s.pos, toks: push(s.toks, i) }
";
    assert_eq!(fires_in(OTHER_FIELD, "f"), 0);
}

/// A closure captures the scope it was built in, so a lambda *as a sibling's
/// value* keeps everything. A lambda that is merely passed to a call does not:
/// it is gone by the time the next field runs. Measured both ways — 0.0000 for
/// the first, 0.9950 for the second.
#[test]
fn a_lambda_as_a_siblings_value_keeps_the_scope_and_one_inside_a_call_does_not() {
    const FIELD: &str = "\
type T = { f: (Int) -> Int, toks: List<Int> }
fn f(s: T, i: Int) -> T = { f: |z: Int| z, toks: push(s.toks, i) }
";
    assert_eq!(fires_in(FIELD, "f"), 1);

    const INSIDE_A_CALL: &str = "\
type T = { n: Int, toks: List<Int> }
fn mk(g: (Int) -> Int) -> Int = g(1)
fn f(s: T, i: Int) -> T = { n: mk(|z: Int| z), toks: push(s.toks, i) }
";
    assert_eq!(fires_in(INSIDE_A_CALL, "f"), 0);
}

/// A literal's value contains its elements', so a sibling that wraps the list
/// in a `[..]` keeps it exactly as a bare mention would. Measured `in_place`
/// **0.0000** at n = 200 and n = 400.
#[test]
fn a_sibling_that_wraps_the_list_in_a_literal_still_keeps_it() {
    const SOURCE: &str = "\
type T = { keep: List<List<Int>>, toks: List<Int> }
fn f(s: T, i: Int) -> T = { keep: [s.toks], toks: push(s.toks, i) }
";
    assert_eq!(fires_in(SOURCE, "f"), 1);
    assert_eq!(routes(SOURCE), vec![Route::Alias]);
}

/// A growing call in the last position, with an earlier sibling holding what it
/// grows. The retained place and the argument need not be the same place: `keep`
/// keeps `s.toks` and `node` is handed `s`, and the list is the same one.
#[test]
fn a_growing_call_in_the_last_position_fires_when_a_sibling_holds_its_argument() {
    const SOURCE: &str = "\
type T = { pos: Int, toks: List<Int> }
fn node(s: T, i: Int) -> T = { pos: i, toks: push(s.toks, i) }
fn keep(a: List<Int>, b: T) -> T = b
fn f(s: T, i: Int) -> T = keep(s.toks, node(s, i))
";
    assert_eq!(fires_in(SOURCE, "f"), 1);
    assert_eq!(routes(SOURCE), vec![Route::Alias]);
}

/// `Frame::BinaryApply` holds the left operand's value while the right one
/// runs, and this probe is **deliberately ill-typed** to reach it. No `BinOp`
/// answers a `List` — `Concat` is `String -> String` — so no program that type
/// checks can put a list there today. The row is kept because the frame really
/// does hold the value, and this is what would catch a list-valued operator
/// being added without the row being revisited.
#[test]
fn a_binary_right_operand_is_held_by_its_left() {
    assert_eq!(
        routes(&probe("Int = xs + len(push(xs, i))")),
        vec![Route::Alias]
    );
    assert_eq!(
        fires_in(&probe("Int = i + len(push(xs, i))"), "f"),
        0,
        "a left operand that holds nothing"
    );
}

// --- what does not fire -----------------------------------------------------

/// A freshly built list has one owner whatever frame holds the scope, so
/// `Arc::get_mut` succeeds and firing would be a false positive.
#[test]
fn a_push_onto_a_list_this_expression_just_built_is_not_a_firing() {
    assert_eq!(fires_in(&probe("List<Int> = sink(push([], i), i)"), "f"), 0);
    assert_eq!(
        fires_in(&probe("List<Int> = sink(push(push([], i), i), i)"), "f"),
        0,
        "the builder idiom: both pushes are onto something fresh"
    );
}

/// The false positive that refuted the round-1 summary. `small`'s only `push`
/// is onto a list `small` built, which its caller cannot reach, so calling it
/// in a carried position copies nothing — measured `in_place` **1.0000** at
/// n = 200 and n = 400 while `W0611` fired.
#[test]
fn a_call_whose_push_is_onto_something_the_caller_cannot_reach_is_not_a_firing() {
    const FRESH: &str = "\
fn keepr(a: Int, b: List<Int>) -> List<Int> = b
fn small(i: Int) -> Int = len(push([], i))
fn f(s: List<Int>, i: Int) -> List<Int> = keepr(small(i), push(s, i))
";
    assert_eq!(fires_in(FRESH, "f"), 0);
    assert_eq!(fires_in(FRESH, "small"), 0);

    // The same shape one level deeper: the accumulator is a lambda's parameter
    // seeded with `[]` inside `build`'s own body. Also measured at 1.0000.
    const ACCUMULATOR: &str = "\
fn keepi(a: Int, b: Int) -> Int = a
fn build(k: Int) -> List<Int> = fold(range(0, k), [], |a: List<Int>, j: Int| push(a, j))
fn f(a: Int, i: Int) -> Int = keepi(len(build(10)), i)
";
    assert_eq!(fires_in(ACCUMULATOR, "f"), 0);
    assert_eq!(fires_in(ACCUMULATOR, "build"), 0);
}

/// A lambda parameter that shadows a `fn` parameter is not that parameter. The
/// `push` here is onto the *lambda's* `xs`, seeded with `[]` inside `build`, so
/// nothing the caller handed over is copied — measured `in_place` **1.0000** at
/// n = 200 and n = 400. Reading the name instead of the binding would mark
/// `build`'s first parameter as grown and fire in `f`.
#[test]
fn a_lambda_parameter_shadowing_a_definitions_parameter_is_not_it() {
    const SOURCE: &str = "\
fn keepi(a: Int, b: Int) -> Int = a
fn build(xs: List<Int>) -> List<Int> = fold(range(0, 10), [], |xs: List<Int>, j: Int| push(xs, j))
fn f(a: Int, i: Int) -> Int = keepi(len(build([])), i)
";
    assert_eq!(fires_in(SOURCE, "build"), 0);
    assert_eq!(fires_in(SOURCE, "f"), 0);
}

/// The interprocedural half. `grow` is written correctly by the stated rule and
/// is still made quadratic by where its caller puts the call, which is the
/// whole reason a local lint cannot do this.
#[test]
fn a_call_to_a_growing_definition_fires_on_position_in_the_caller() {
    const SOURCE: &str = "\
fn sink(a: List<Int>, b: Int) -> List<Int> = a
fn sink_snd(b: Int, a: List<Int>) -> List<Int> = a
fn grow(xs: List<Int>, i: Int) -> List<Int> = push(xs, i)
fn quiet(xs: List<Int>, i: Int) -> List<Int> = xs
fn early(xs: List<Int>, i: Int) -> List<Int> = sink(grow(xs, i), i)
fn late(xs: List<Int>, i: Int) -> List<Int> = sink_snd(i, grow(xs, i))
fn tail(xs: List<Int>, i: Int) -> List<Int> = grow(xs, i)
fn harmless(xs: List<Int>, i: Int) -> List<Int> = sink(quiet(xs, i), i)
";
    assert_eq!(fires_in(SOURCE, "early"), 1);
    assert_eq!(
        fires_in(SOURCE, "late"),
        0,
        "argument 1 of 2 is the last one, and what the earlier one holds is an \
         `Int` rather than the list `grow` pushes onto"
    );
    assert_eq!(fires_in(SOURCE, "tail"), 0);
    assert_eq!(
        fires_in(SOURCE, "harmless"),
        0,
        "`quiet` returns its argument untouched, so its position costs nothing"
    );
    assert_eq!(
        fires_in(SOURCE, "grow"),
        0,
        "`grow` itself is written correctly"
    );
}

/// The summary is per parameter, not per definition. `grow2` pushes onto its
/// second parameter only, so a sibling holding what went to the *first* is not
/// a firing and a sibling holding what went to the second is.
#[test]
fn the_growth_summary_names_which_parameter_it_grows() {
    const SOURCE: &str = "\
fn grow2(a: List<Int>, b: List<Int>) -> List<Int> = push(b, 1)
fn keep(x: List<Int>, y: List<Int>) -> List<Int> = y
fn held_first(p: List<Int>, q: List<Int>) -> List<Int> = keep(p, grow2(p, q))
fn held_second(p: List<Int>, q: List<Int>) -> List<Int> = keep(q, grow2(p, q))
";
    assert_eq!(
        fires_in(SOURCE, "held_first"),
        0,
        "`p` is held, and `grow2` does not push onto `p`"
    );
    assert_eq!(fires_in(SOURCE, "held_second"), 1);
}

/// The summary reaches through a chain and around a cycle. `sccs` puts a
/// mutually recursive group in one component, which is iterated to a fixpoint.
#[test]
fn the_growth_summary_is_transitive_and_survives_recursion() {
    const CHAIN: &str = "\
fn sink(a: List<Int>, b: Int) -> List<Int> = a
fn base(xs: List<Int>, i: Int) -> List<Int> = push(xs, i)
fn middle(xs: List<Int>, i: Int) -> List<Int> = base(xs, i)
fn outer(xs: List<Int>, i: Int) -> List<Int> = middle(xs, i)
fn caller(xs: List<Int>, i: Int) -> List<Int> = sink(outer(xs, i), i)
";
    assert_eq!(
        fires_in(CHAIN, "caller"),
        1,
        "the `push` is three calls away and the summary has to reach it"
    );

    const MUTUAL: &str = "\
fn sink(a: List<Int>, b: Int) -> List<Int> = a
fn ping(xs: List<Int>, i: Int) -> List<Int> =
  if i == 0 { push(xs, i) } else { pong(xs, i - 1) }
fn pong(xs: List<Int>, i: Int) -> List<Int> =
  if i == 0 { xs } else { ping(xs, i - 1) }
fn caller(xs: List<Int>, i: Int) -> List<Int> = sink(pong(xs, i), i)
";
    assert_eq!(
        fires_in(MUTUAL, "caller"),
        1,
        "`pong` grows only through its mutually recursive partner"
    );
}

/// The edge is drawn on a mention rather than on a place, because what a
/// definition hands a growing callee may be built around the caller's list
/// rather than be the list.
#[test]
fn the_summary_follows_a_container_through_a_wrapper() {
    const SOURCE: &str = "\
type Box = { xs: List<Int> }
fn sink(a: List<Int>, b: Int) -> List<Int> = a
fn wrap(a: List<Int>) -> Box = { xs: a }
fn inner(b: Box, i: Int) -> List<Int> = push(b.xs, i)
fn outer(xs: List<Int>, i: Int) -> List<Int> = inner(wrap(xs), i)
fn caller(xs: List<Int>, i: Int) -> List<Int> = sink(outer(xs, i), i)
";
    assert_eq!(fires_in(SOURCE, "caller"), 1);
}

/// A local binding shadows a definition, and `Resolved::lookup` is documented as
/// only being asked about names local lookup already missed.
#[test]
fn a_local_named_like_a_definition_is_not_mistaken_for_it() {
    const SOURCE: &str = "\
fn sink(a: Int, b: Int) -> Int = a
fn grow(xs: List<Int>, i: Int) -> List<Int> = push(xs, i)
fn f(i: Int) -> Int = {
  let grow = |a: Int| a + 1;
  sink(grow(i), i)
}
";
    assert_eq!(
        fires_in(SOURCE, "f"),
        0,
        "`grow` here is a local closure, not the growing definition"
    );
}

/// A lambda body is entered with a scope built for it, so its own analysis
/// starts over rather than inheriting the position of the expression the lambda
/// literal sits in.
#[test]
fn a_lambda_body_is_its_own_root() {
    const SOURCE: &str = "\
fn sink(a: (List<Int>) -> List<Int>, b: Int) -> Int = 0
fn f(xs: List<Int>, i: Int) -> Int =
  sink(|a: List<Int>| push(a, i), i)
";
    assert_eq!(
        fires_in(SOURCE, "f"),
        0,
        "the lambda literal is in a carried slot, but its body is not evaluated there"
    );
}

/// A `let` that names a place is followed through, so the alias route sees
/// `ys` and `s.toks` as one list.
#[test]
fn a_let_that_names_a_place_is_followed_through() {
    const SOURCE: &str = "\
type T = { keep: List<Int>, toks: List<Int> }
fn f(s: T, i: Int) -> T = {
  let ys = s.toks;
  { keep: ys, toks: push(s.toks, i) }
}
";
    assert_eq!(fires_in(SOURCE, "f"), 1);
}

/// `check_where` narrows the **reporting** and not the summary, and the
/// difference is the whole reason the pass is interprocedural: a project's
/// caller and a library's callee are one question. Filtering the callee's
/// module out must not turn it into "does not grow".
#[test]
fn narrowing_the_report_does_not_narrow_the_summary() {
    let mut map = SourceMap::new();
    let lib_name = ModuleName::from_relative_path(Path::new("lib.ply")).expect("a module name");
    let app_name = ModuleName::from_relative_path(Path::new("app.ply")).expect("a module name");
    const LIB: &str = "pub fn grow(xs: List<Int>, i: Int) -> List<Int> = push(xs, i)\n";
    const APP: &str = "\
import lib (grow)
fn sink(a: List<Int>, b: Int) -> List<Int> = a
pub fn caller(xs: List<Int>, i: Int) -> List<Int> = sink(grow(xs, i), i)
";
    let lib_id = map.add("lib.ply", LIB.to_string());
    let app_id = map.add("app.ply", APP.to_string());
    let mut program = parse_program(vec![
        (lib_id, lib_name.clone(), LIB),
        (app_id, app_name, APP),
    ])
    .unwrap_or_else(|d| panic!("the fixture does not parse: {d:?}"));
    assert!(ply_derive::expand_program(&mut program).is_empty());
    let resolved =
        resolve(&program).unwrap_or_else(|d| panic!("the fixture does not resolve: {d:?}"));

    let everything = ply_core::fieldorder::check(&program, &resolved);
    assert_eq!(everything.len(), 1, "one firing, in `caller`");

    let app_only = ply_core::fieldorder::check_where(&program, &resolved, |m| *m != lib_name);
    assert_eq!(
        app_only.len(),
        1,
        "the firing is in `app`, and it depends on `lib`'s summary; narrowing the \
         report to `app` must not lose it"
    );
}

/// The one place the pass still chooses a direction on purpose, asserted so
/// that the choice is visible: the container is a call's result, the pass
/// cannot say it is fresh, and it prefers the firing. Measured `in_place`
/// **1.0000** at n = 200 and n = 400 — a false positive, and the module
/// comment's table says so.
#[test]
fn a_push_onto_a_calls_result_is_a_known_false_positive() {
    const SOURCE: &str = "\
fn mk(i: Int) -> List<Int> = [i]
fn sink(a: List<Int>, b: Int) -> Int = len(a)
fn f(a: Int, i: Int) -> Int = sink(push(mk(i), i), i)
";
    assert_eq!(fires_in(SOURCE, "f"), 1);
    assert_eq!(
        fires_in(&probe("List<Int> = sink(push([], i), i)"), "f"),
        0,
        "the same position with a container the pass *can* see is fresh"
    );
}

/// A second named limit, and one the standard library is already living with:
/// `map_get` clones the value out of the tree, so a `push` onto a list read out
/// of a `Map` is at two owners however the expression is written. Measured
/// `in_place` **0.0000** at n = 200 and n = 400 on the shape
/// `std.http`'s `add_field` is written in.
///
/// The pass is silent because the scrutinee is a call and `Names::place`
/// answers `None` for it — the same gap as
/// `an_alias_through_a_call_is_a_known_miss`, reached from the other side.
#[test]
fn a_push_onto_a_list_read_out_of_a_map_is_a_known_miss() {
    const SOURCE: &str = "\
fn f(m: Map<Int, List<Int>>, i: Int) -> Map<Int, List<Int>> =
  match map_get(m, 0) {
    None -> m,
    Some(vs) -> map_insert(m, 0, push(vs, i)),
  }
";
    assert_eq!(fires_in(SOURCE, "f"), 0);
}

/// The named limit, asserted so that the gap is armed rather than invisible: an
/// earlier sibling that goes through a call is not a firing, and this shape
/// measures `in_place` **0.0000** at n = 200 and n = 400.
///
/// It is here as a **miss**. If a later change closes it, this test goes red
/// and the module comment's table has to move with it.
#[test]
fn an_alias_through_a_call_is_a_known_miss() {
    const SOURCE: &str = "\
type T = { n: List<Int>, toks: List<Int> }
fn id(x: List<Int>) -> List<Int> = x
fn f(s: T, i: Int) -> T = { n: id(s.toks), toks: push(s.toks, i) }
";
    assert_eq!(
        fires_in(SOURCE, "f"),
        0,
        "deciding that `id` answers its argument needs an interprocedural value \
         analysis this pass does not have"
    );
}
