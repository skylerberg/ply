//! `W0611`, one node kind at a time.
//!
//! `crates/ply-eval/tests/field_order_oracle.rs` is the load-bearing test: it
//! checks the pass against `rc::Stats` on the definitions ADR 0020 measured.
//! This file checks the thing that test cannot — that each *kind* of enclosing
//! node is modelled, rather than the four the spike files happen to use.
//!
//! Every row here that says "fires" or "silent" about a shape the oracle does
//! not cover was checked against the counter before it was written down. In
//! particular the block-statement row: `Frame::BlockStep` carries
//! `scope.release(dead)` rather than the whole scope, so it was not obvious
//! that a `let`'s value is a trap. `{ let one = push(acc, i); push(one, i) }`
//! over 200 steps measures 400 updates and 200 in place — one copy per step, at
//! the statement, and the tail's push in place. GAPS.md §1 column 4 says the
//! same thing with a clock.

use ply_span::SourceMap;
use ply_syntax::ast::ModuleName;
use ply_syntax::parse_program;
use ply_syntax::resolve::resolve;
use std::path::Path;

/// The simple names of the definitions `W0611` fires in, with one firing per
/// entry, so a definition that fires twice appears twice.
fn firings(source: &str) -> Vec<String> {
    let mut map = SourceMap::new();
    let name = ModuleName::from_relative_path(Path::new("t.ply")).expect("a module name");
    let id = map.add("t.ply", source.to_string());
    let mut program = parse_program(vec![(id, name, source)])
        .unwrap_or_else(|d| panic!("the fixture does not parse: {d:?}"));
    assert!(ply_derive::expand_program(&mut program).is_empty());
    let resolved =
        resolve(&program).unwrap_or_else(|d| panic!("the fixture does not resolve: {d:?}"));
    ply_core::fieldorder::firings(&program, &resolved)
        .into_iter()
        .map(|f| f.simple.as_str().to_string())
        .collect()
}

fn fires_in(source: &str, name: &str) -> usize {
    firings(source).iter().filter(|f| *f == name).count()
}

const PRELUDE: &str = "type S = { a: Int, xs: List<Int> }\n\
                       fn sink(a: List<Int>, b: Int) -> List<Int> = a\n\
                       fn sink3(a: Int, b: List<Int>, c: Int) -> List<Int> = b\n";

fn probe(body: &str) -> String {
    format!("{PRELUDE}fn f(xs: List<Int>, i: Int) -> {body}\n")
}

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
    assert_eq!(
        fires_in(&probe("List<List<Int>> = [xs, push(xs, i)]"), "f"),
        0
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
#[test]
fn an_if_condition_fires_and_a_branch_inherits() {
    assert_eq!(
        fires_in(
            &probe("Int = if len(push(xs, i)) > 0 { 1 } else { 0 }"),
            "f"
        ),
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
        "argument 1 of 2 is the last one"
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
