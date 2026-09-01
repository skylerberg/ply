//! `{..b, f: e}` reuses the base record's cells when nothing else holds it — ADR 0034's decision
//! 3, drop-reuse for record literals — and is built as written otherwise. The programs here pin
//! the semantics the reuse must not move: a shared base stays what it was, and a literal that
//! copies only some of a record's fields is that smaller record.

use crate::fixture::Compiled;
use ply_eval::rc;

fn passes(source: &str, test: &str) -> rc::Stats {
    let compiled = Compiled::new(source);
    let index = compiled.index_of(test);
    let before = rc::stats();
    if let Err(d) = compiled.machine().eval_test(index) {
        panic!("{test:?} must pass: {d:#?}");
    }
    let after = rc::stats();
    rc::Stats {
        updates: after.updates - before.updates,
        updates_in_place: after.updates_in_place - before.updates_in_place,
        ..after
    }
}

#[test]
fn an_update_of_a_shared_base_leaves_the_base_as_it_was() {
    passes(
        r#"
type S = {k: Int, out: List<Int>}

test "two updates of one base" {
  let s: S = {k: 1, out: [1]};
  let t = {..s, k: 2};
  let u = {..s, out: push(s.out, 2)};
  assert_eq(s.k, 1);
  assert_eq(s.out, [1]);
  assert_eq(t.k, 2);
  assert_eq(t.out, [1]);
  assert_eq(u.k, 1);
  assert_eq(u.out, [1, 2])
}
"#,
        "two updates of one base",
    );
}

/// The reuse is keyed on the literal naming exactly the base's fields, which the expansion of
/// `{..b, ..}` always does and a hand-written literal need not.
#[test]
fn a_literal_copying_some_of_a_records_fields_is_that_smaller_record() {
    passes(
        r#"
type B = {a: Int, c: Int, d: Int}

test "two of three fields, one rewritten" {
  let b: B = {a: 1, c: 2, d: 3};
  let x = {a: b.a, c: 5};
  assert_eq(x, {a: 1, c: 5});
  assert_eq(b, {a: 1, c: 2, d: 3})
}
"#,
        "two of three fields, one rewritten",
    );
}

/// The accumulator shape the fifth gate pair is about, threaded through an update: the written
/// field's `push` takes the old list out of the record, and the base is then reused in place, so
/// every round rewrites rather than copies.
#[test]
fn a_threaded_update_appends_in_place_every_round() {
    let stats = passes(
        r#"
type S = {k: Int, out: List<Int>}

fn go(i: Int, s: S) -> S =
  if i == 64 { s } else { go(i + 1, {..s, k: s.k + 1, out: push(s.out, i)}) }

test "sixty-four rounds" {
  let s = go(0, {k: 0, out: []});
  assert_eq(s.k, 64);
  assert_eq(len(s.out), 64)
}
"#,
        "sixty-four rounds",
    );
    assert_eq!(
        (stats.updates, stats.updates_in_place),
        (64, 64),
        "every append found the list at one owner: {stats:?}"
    );
}
