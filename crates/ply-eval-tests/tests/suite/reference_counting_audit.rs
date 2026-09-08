//! The reference-counting pass asked of whole programs rather than of synthetic expressions.

use ply_eval::{Machine, rc};
use ply_span::{SourceId, SourceMap, codes};
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


/// Runs every test in `src` and answers the diagnostic the first failing one raised.
#[track_caller]
fn fails(src: &str) -> ply_span::Diagnostic {
    let mut map = SourceMap::new();
    let id: SourceId = map.add("rc.ply", src.to_string());
    let mut program = match parse_program([(id, ModuleName::from_dotted("rc"), src)]) {
        Ok(p) => p,
        Err(ds) => panic!("the probe must parse: {ds:#?}\n{src}"),
    };
    let resolved = resolve(&mut program).expect("the probe must resolve");
    let mut machine = Machine::for_program(&program, &resolved);
    for i in 0..machine.test_count() {
        if let Err(d) = machine.eval_test(i) {
            return d;
        }
    }
    panic!("every test passed, and one was expected to fail\n{src}");
}


/// The read-during-update hole ADR 0024 named — a perform between the take and the set exposes the
/// emptied slot — is closed by refusing the read loudly rather than answering the placeholder.
#[test]
fn a_read_of_a_cell_during_its_update_is_refused() {
    let d = fails(
        r#"
effect peek {
  read now[k]() -> Int
}

test "the handler reads the cell the update holds" {
  with_cell[r]([1, 2]) { c -> {
    handle {
      cell_update(c, |xs| push(xs, peek.now[k]()))
    } with {
      peek.now[k]() -> len(cell_get(c)),
      return x -> x
    }
  } }
}
"#,
    );
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    assert!(
        d.message.contains("cell_update"),
        "the refusal names the update holding the contents: {}",
        d.message
    );
}

#[test]
fn a_nested_update_of_the_same_cell_is_refused() {
    let d = fails(
        r#"
test "an update inside its own update" {
  with_cell[r](1) { c -> cell_update(c, |n| { cell_update(c, |m| m + 1); n + 1 }) }
}
"#,
    );
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    assert!(d.message.contains("cell_update"), "{}", d.message);
}


