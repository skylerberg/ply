//! The list index on real source, through parse, resolve, check and the evaluator.

use crate::fixture::Compiled;

/// Every `test` in the source, all of which must pass.
/// Every position of a three-element list, plus the two derivations the builtin is meant to make
/// expressible — `head` and `last`, which ADR 0027 §3 declines to ship as names of their own.
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
    Compiled::ran(source);
}

/// Absent, not clamped, and not counted from the end.
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
    Compiled::ran(source);
}

/// The element type is a parameter, so a record, an ADT value and a nested list all have to come
/// back whole.
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
    Compiled::ran(source);
}

/// The builtin is pure and takes no callback, so a call publishes exactly the row of the
/// expressions it was given — which for `list_at` is nothing of its own.
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
    let c = Compiled::ran(source);
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
/// `crates/ply-std/ply/json.ply` and `db.ply` — each of which ships an `nth` of its own — free to
/// keep theirs, and is the reason ADR 0027 §6 declines the bare name `nth`.
#[test]
fn a_module_may_shadow_the_name() {
    let source = r#"
fn list_at<a>(xs: List<a>, i: Int) -> Int = 0 - 1

test "the module's own definition wins" {
  assert_eq(list_at([1, 2, 3], 0), -1)
}
"#;
    Compiled::ran(source);
}

/// The index is not a callback builtin, so it cannot suspend, so a peek inside a loop costs the
/// loop nothing.
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
    Compiled::ran(source);
}
