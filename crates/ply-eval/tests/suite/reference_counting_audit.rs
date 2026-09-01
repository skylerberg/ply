//! The reference-counting pass asked of whole programs rather than of synthetic expressions.

use ply_eval::{Machine, rc};
use ply_span::{SourceId, SourceMap};
use ply_syntax::ast::ModuleName;
use ply_syntax::parse_program;
use ply_syntax::resolve::resolve;

/// Runs every test in `src` on the machine and requires all of them to pass.
#[track_caller]
fn passes(src: &str) -> rc::Stats {
    let mut map = SourceMap::new();
    let id: SourceId = map.add("rc.ply", src.to_string());
    let mut program = match parse_program([(id, ModuleName::from_dotted("rc"), src)]) {
        Ok(p) => p,
        Err(ds) => panic!("the probe must parse: {ds:#?}\n{src}"),
    };
    let resolved = resolve(&mut program).expect("the probe must resolve");
    if let Err(ds) = ply_core::check_program(&program, &resolved) {
        panic!("the probe must check: {ds:#?}\n{src}");
    }

    rc::reset();
    let mut machine = Machine::for_program(&program, &resolved);
    let count = machine.test_count();
    assert!(count > 0, "this probe declares no test\n{src}");
    for i in 0..count {
        if let Err(d) = machine.eval_test(i) {
            panic!(
                "`{}` must pass: [{}] {}\n{src}",
                machine.test_name(i).unwrap_or("?"),
                d.code,
                d.message
            );
        }
    }
    rc::stats()
}

/// The world is an owner no analysis of a scope can see: `xs` and the cell hold one `Arc`, and the
/// binding's last use hands `push` a value it must not rewrite.
#[test]
fn a_list_the_cell_still_holds_is_copied_rather_than_rewritten() {
    passes(
        r#"
test "a list also held by a cell keeps its length" {
  with_cell[s]([1, 2]) { c -> {
    let xs = cell_get(c);
    let ys = push(xs, 3);
    assert_eq(len(ys), 3);
    assert_eq(len(cell_get(c)), 2)
  } }
}
"#,
    );
}

/// A closure is the owner `Live` refuses to let a barrier own past, and the answer proves the
/// refusal reached the update and not only the move.
#[test]
fn a_list_a_closure_captured_is_copied_rather_than_rewritten() {
    passes(
        r#"
test "a captured list keeps its length" {
  let xs = [1, 2];
  let peek = || len(xs);
  let ys = push(xs, 3);
  assert_eq(len(ys), 3);
  assert_eq(peek(), 2)
}
"#,
    );
}

/// Per-task region stacks gives every task its own region stack, and a value two tasks reach is a value
/// neither may rewrite.
#[test]
fn a_list_two_tasks_reach_is_copied_rather_than_rewritten() {
    passes(
        r#"
test "two tasks pushing onto one list each answer three" {
  simulate {
    let xs = [1, 2];
    let a = task.spawn(|| len(push(xs, 3)));
    let b = task.spawn(|| len(push(xs, 4)));
    assert_eq(task.join(a) + task.join(b), 6)
  }
}
"#,
    );
}

/// The `drop` half rather than the `dup` half.
#[test]
fn a_binding_one_resumption_released_is_still_read_by_the_next() {
    passes(
        r#"
effect amb { read flip[coin]() -> Bool }

test "the second resumption still reads what the first released" {
  let xs = [1, 2, 3];
  let total = handle {
    let b = amb.flip[coin]();
    { let n = len(xs); if b { n } else { n * 10 } }
  } with {
    amb.flip[coin]() resume k -> k(true) + k(false),
    return x -> x
  };
  assert_eq(total, 33)
}
"#,
    );
}

/// The same question with the resumption moved outside the block entirely: the continuation is
/// parked in a cell and applied after the `handle` has already answered, so every release the block
/// performed on the way out has run.
#[test]
fn a_continuation_parked_in_a_cell_still_reads_a_released_binding() {
    passes(
        r#"
effect amb { read flip[coin]() -> Bool }
type Saved = Nothing | Just((Bool) -> Int)

test "a parked continuation reads a binding the block released" {
  with_cell[slot](Nothing) { s -> {
    let xs = [1, 2, 3];
    let inner = handle {
      let b = amb.flip[coin]();
      if b { len(xs) } else { 0 }
    } with {
      amb.flip[coin]() resume k -> { cell_set(s, Just(k)); 0 },
      return x -> x
    };
    assert_eq(inner, 0);
    match cell_get(s) {
      Just(k) -> assert_eq(k(true), 3),
      Nothing -> assert(false)
    }
  } }
}
"#,
    );
}

/// A fold whose accumulator nothing else holds is the case the reference-counting pass exists for, and the one the whole
/// scheme is paid for by.
#[test]
fn a_fold_accumulator_nothing_else_holds_is_rewritten_in_place() {
    let stats = passes(
        r#"
test "a fold builds its list without recopying it" {
  assert_eq(len(fold(range(0, 64), [], |acc, x| push(acc, x))), 64)
}
"#,
    );
    assert!(
        stats.updates >= 64,
        "the fold performs one update per element: {stats:?}"
    );
    assert!(
        stats.updates_in_place * 2 > stats.updates,
        "most of a private accumulator's updates must be in place: {stats:?}"
    );
}

/// The cycle guard's honest extent.
#[test]
fn a_cell_holding_a_closure_that_reads_it_is_not_a_cycle() {
    let stats = passes(
        r#"
type Saved = Nothing | Just(() -> Int)

test "a cell holding a closure over itself" {
  with_cell[s](Nothing) { c -> {
    cell_set(c, Just(|| 1));
    match cell_get(c) { Just(f) -> assert_eq(f(), 1), Nothing -> assert(false) }
  } }
}
"#,
    );
    assert_eq!(
        stats.cycles, 0,
        "no `Arc` cycle exists to report: {stats:?}"
    );
}

/// A parameter released at the statement that last reads it — the sequence S3,
/// which is the ownership design P2, whose landing condition was that the case
/// analysis be written rather than assumed. It is written at the seeding site in
/// `code.rs`; these are its six cases, run.
#[test]
fn a_parameter_a_later_construct_still_reaches_is_not_released() {
    // 1. Captured by a closure written after the last direct read.
    passes(
        r#"
fn go(xs: List<Int>) -> Int = { let n = len(xs); let f = || len(xs); n + f() }
test "closure" { assert_eq(go([1, 2, 3]), 6) }
"#,
    );
    // 2. Captured by a handler clause.
    passes(
        r#"
effect ask { read one[k]() -> Int }
fn go(xs: List<Int>) -> Int = {
  let n = len(xs);
  n + handle { ask.one[k]() } with { ask.one[k]() -> len(xs), return x -> x } }
test "handler clause" { assert_eq(go([1, 2, 3]), 6) }
"#,
    );
    // 3. Stored in a cell, then read back out of it.
    passes(
        r#"
fn go(xs: List<Int>) -> Int =
  with_cell[r]([]) { c -> { let n = len(xs); cell_set(c, xs); n + len(cell_get(c)) } }
test "cell" { assert_eq(go([1, 2, 3]), 6) }
"#,
    );
    // 4. Read in a later `match` arm.
    passes(
        r#"
fn go(xs: List<Int>, b: Bool) -> Int = {
  let n = len(xs);
  n + match b { true -> len(xs), false -> 0 } }
test "match arm" { assert_eq(go([1, 2, 3], true), 6) }
"#,
    );
    // 5. Read in the tail, after the statements.
    passes(
        r#"
fn go(xs: List<Int>) -> Int = { let n = len(xs); let m = n * 2; m + len(xs) }
test "tail" { assert_eq(go([1, 2, 3]), 9) }
"#,
    );
    // 6. Shadowed by an inner binder of the same name, which must release the
    //    *binder* and leave the parameter to the reads left of it.
    passes(
        r#"
fn go(xs: List<Int>) -> Int = {
  let n = len(xs);
  let xs = [9, 9];
  let m = len(xs);
  n + m }
test "shadowed" { assert_eq(go([1, 2, 3]), 5) }
"#,
    );
}

/// The half of P2 that is the point of it: threaded as a parameter, an
/// A parameter accumulator whose `push` is in last-argument position is reused.
///
/// This does **not** discriminate the ownership design's P2: it passes with and without it, because the
/// caller's `drop(env)` before the call already delivers the value at one owner. The shape that
/// does is `position_invariance_g1`'s "let binding against parameter" pair, where the append is a
/// statement rather than a last argument.
#[test]
fn an_accumulator_threaded_as_a_parameter_is_rewritten_in_place() {
    let stats = passes(
        r#"
fn grow(n: Int, xs: List<Int>) -> List<Int> =
  if n == 0 { xs } else { { let m = n - 1; grow(m, push(xs, n)) } }
test "grown" { assert_eq(len(grow(50, [])), 50) }
"#,
    );
    assert_eq!(
        (stats.updates, stats.updates_in_place),
        (50, 50),
        "a parameter accumulator must be reused as a `let` one is"
    );
}
