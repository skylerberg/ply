//! The equivalence audit: the machine against the tree-walker, on programs
//! written to make them disagree.
//!
//! Auditing the source that happens to exist finds the disagreements somebody
//! already wrote down. These cases are adversarial instead: the shapes where the
//! tree-walker's handler *stack* and the machine's segment *capture* could
//! plausibly encode one language two ways.
//!
//! A disagreement is a failure and never a warning, because the result cache
//! records whichever engine ran first and never recomputes it.

use ply_eval::differential::compare_tests;
use ply_eval::{Fixture, Interp, Machine};
use ply_span::{SourceId, SourceMap};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::parse_program;
use ply_syntax::resolve::{Resolved, resolve};

fn load(src: &str) -> (Program, Resolved) {
    let mut map = SourceMap::new();
    let id: SourceId = map.add("audit.ply", src.to_string());
    let program = match parse_program([(id, ModuleName::from_dotted("audit"), src)]) {
        Ok(p) => p,
        Err(ds) => panic!("the audit program must parse: {ds:#?}\n{src}"),
    };
    let resolved = resolve(&program).expect("the audit program must resolve");
    (program, resolved)
}

/// Returns how many tests were compared, and refuses a case that compared
/// nothing: an all-skipped run and a clean run must not look alike.
#[track_caller]
fn agree(src: &str) -> usize {
    let (program, resolved) = load(src);
    let mut treewalk = Interp::for_program(&program, &resolved);
    let mut machine = Machine::for_program(&program, &resolved);
    let report = compare_tests(&mut treewalk, &mut machine, &Fixture::empty());
    assert!(report.is_clean(), "{report}\n--- program ---\n{src}");
    assert!(
        report.compared > 0 || report.machine_only > 0,
        "this case declares no test\n{src}"
    );
    // Agreement on a subject whose footprint was never compared is agreement on
    // two axes of three, and the missing one is the only one that sees a
    // `perform` with no effect on the value or the world.
    assert_eq!(
        report.footprints_compared, report.compared,
        "an engine stopped reporting what it performed\n{report}"
    );
    report.compared
}

/// A program broken in some unrelated way would "agree" on the resulting
/// failure and prove nothing about the shape it was written for, so passing is
/// asserted separately.
#[track_caller]
fn agree_and_pass(src: &str) {
    let compared = agree(src);
    let (program, resolved) = load(src);
    let mut machine = Machine::for_program(&program, &resolved);
    for i in 0..compared {
        if let Err(d) = machine.eval_test(i) {
            panic!(
                "test {i} (`{}`) was expected to pass: [{}] {}\n{src}",
                machine.test_name(i).unwrap_or("?"),
                d.code,
                d.message
            );
        }
    }
}

/// Auditing a diagnostic is worthless once the program stops producing it, so
/// the code is pinned as well as compared.
#[track_caller]
fn agree_and_fail(src: &str, code: &str) {
    agree(src);
    let (program, resolved) = load(src);
    let mut machine = Machine::for_program(&program, &resolved);
    let mut treewalk = Interp::for_program(&program, &resolved);
    for i in 0..machine.test_count() {
        let m = machine.eval_test(i).expect_err("this case must fail");
        let t = treewalk.eval_test(i).expect_err("this case must fail");
        assert_eq!(m.code, code, "machine: {}", m.message);
        assert_eq!(t.code, code, "treewalk: {}", t.message);
    }
}

#[test]
fn literals_variables_and_operators() {
    agree_and_pass(
        r#"
type Colour = Red | Green | Blue

fn shadowed(x: Int) -> Int = { let x = x + 1; let x = x * 2; x }

test "literals" {
  assert_eq(1, 1);
  assert_eq(true, true);
  assert_eq("a\nb", "a\nb");
  assert_eq((), ());
  assert_eq(1_000_000, 1000000)
}

test "unary" {
  assert_eq(-(3), 0 - 3);
  assert_eq(!true, false);
  assert_eq(- -5, 5);
  assert_eq(!!false, false)
}

test "arithmetic and precedence" {
  assert_eq(1 + 2 * 3, 7);
  assert_eq((1 + 2) * 3, 9);
  assert_eq(7 / 2, 3);
  assert_eq(-7 / 2, -3);
  assert_eq(7 % 3, 1);
  assert_eq(-7 % 3, -1);
  assert_eq(7 % -3, 1)
}

test "comparison and equality" {
  assert(1 < 2); assert(2 <= 2); assert(3 > 2); assert(3 >= 3);
  assert("a" < "b"); assert("ab" > "aa");
  assert(1 != 2); assert([1, 2] == [1, 2]); assert([1] != [2])
}

test "logical short circuit" {
  assert(true || panic_if_evaluated());
  assert(!(false && panic_if_evaluated()))
}

fn panic_if_evaluated() -> Bool = { panic("the short circuit did not happen"); true }

test "string concat" {
  assert_eq("a" ++ "b" ++ "c", "abc");
  assert_eq(string_concat("x", int_to_string(12)), "x12")
}

test "let shadowing in a block" { assert_eq(shadowed(1), 4) }

test "constructors and equality" {
  assert(Red == Red);
  assert(Red != Green);
  assert_eq(len([Red, Green, Blue]), 3)
}
"#,
    );
}

#[test]
fn records_lists_fields_and_indexing() {
    agree_and_pass(
        r#"
test "records" {
  let r = { b: 2, a: 1 };
  assert_eq(r.a, 1);
  assert_eq(r.b, 2);
  assert_eq(r, { a: 1, b: 2 })
}

test "nested records" {
  let r = { outer: { inner: 7 } };
  assert_eq(r.outer.inner, 7)
}

test "lists" {
  assert_eq(len([]), 0);
  assert_eq(len([1, 2, 3]), 3);
  assert_eq(push([1], 2), [1, 2]);
  assert_eq(range(3), [0, 1, 2]);
  assert_eq(range(2, 5), [2, 3, 4]);
  assert_eq(range(5, 2), [])
}

test "lists of records of lists" {
  let xs = [{ v: [1, 2] }, { v: [] }];
  assert_eq(len(xs), 2);
  assert_eq(len(xs) + 0, 2)
}
"#,
    );
}

#[test]
fn if_match_and_lambda_in_every_combination() {
    agree_and_pass(
        r#"
type Tree = Leaf | Node(Int, Int)

fn depth_of(t: Tree) -> Int = match t { Leaf -> 0, Node(a, b) -> a + b }

test "if in every position" {
  assert_eq(if true { 1 } else { 2 }, 1);
  assert_eq(if false { 1 } else { if true { 2 } else { 3 } }, 2);
  assert_eq((if true { 1 } else { 2 }) + (if false { 10 } else { 20 }), 21)
}

test "match on literals" {
  let f = |n| match n { 0 -> "zero", 1 -> "one", _ -> "many" };
  assert_eq(f(0), "zero");
  assert_eq(f(1), "one");
  assert_eq(f(9), "many")
}

test "match on constructors" {
  assert_eq(depth_of(Leaf), 0);
  assert_eq(depth_of(Node(2, 3)), 5)
}

test "nested patterns" {
  let f = |xs| match xs {
    [] -> 0,
    [x] -> x,
    [x, y, ..rest] -> x + y + len(rest)
  };
  assert_eq(f([]), 0);
  assert_eq(f([7]), 7);
  assert_eq(f([1, 2, 3, 4]), 5)
}

test "record patterns" {
  let f = |r| match r { { a: 1, b: b } -> b, { a: a, b: _ } -> a };
  assert_eq(f({ a: 1, b: 9 }), 9);
  assert_eq(f({ a: 5, b: 9 }), 5)
}

test "guards choose the first arm whose guard holds" {
  let f = |n| match n {
    x if x > 10 -> "big",
    x if x > 5 -> "medium",
    _ -> "small"
  };
  assert_eq(f(20), "big");
  assert_eq(f(7), "medium");
  assert_eq(f(1), "small")
}

test "a redundant arm after a catch-all is never reached" {
  let f = |n| match n { _ -> 1, 0 -> 2 };
  assert_eq(f(0), 1)
}

test "a failing guard falls through to the next arm binding the same name" {
  let f = |n| match n { x if x > 100 -> x, x -> x * 2 };
  assert_eq(f(3), 6);
  assert_eq(f(200), 200)
}

test "lambdas and closures" {
  let add = |a, b| a + b;
  let inc = |n| add(n, 1);
  assert_eq(inc(41), 42);
  let make = |k| |n| n + k;
  assert_eq(make(10)(5), 15)
}
"#,
    );
}

#[test]
fn integer_overflow_in_every_operator() {
    agree_and_fail(
        r#"
test "add" { assert_eq(9223372036854775807 + 1, 0) }
test "sub" { assert_eq(0 - 9223372036854775807 - 2, 0) }
test "mul" { assert_eq(9223372036854775807 * 2, 0) }
test "neg" { assert_eq(-(0 - 9223372036854775807 - 1), 0) }
test "div" { assert_eq((0 - 9223372036854775807 - 1) / (0 - 1), 0) }
"#,
        "E0502",
    );
}

#[test]
fn division_and_remainder_by_zero() {
    agree_and_fail(
        r#"
test "div" { assert_eq(1 / 0, 0) }
test "rem" { assert_eq(1 % 0, 0) }
"#,
        "E0502",
    );
}

#[test]
fn every_builtins_failure_mode() {
    // One program per code: a case that changed its mind about which failure it
    // produces must fail rather than quietly audit something else.
    agree_and_fail(
        r#"
test "assert" { assert(false) }
test "assert with a message" { assert(1 == 2, "one is not two") }
test "assert_eq on ints" { assert_eq(1, 2) }
test "assert_eq on lists" { assert_eq([1, 2, 3], [1, 9, 3]) }
test "assert_eq on records" { assert_eq({ a: 1 }, { a: 2 }) }
test "assert_eq on strings" { assert_eq("abc", "abd") }
"#,
        "E0501",
    );

    agree_and_fail(
        r#"
test "panic" { panic("boom") }
test "len of an int" { assert_eq(len(1), 0) }
test "push onto an int" { assert_eq(push(1, 2), []) }
test "int_to_string of a list" { assert_eq(int_to_string([1]), "") }
test "string_concat of ints" { assert_eq(string_concat(1, 2), "") }
test "a huge range" { assert_eq(len(range(20000000)), 0) }
test "map over an int" { assert_eq(map(1, |x| x), []) }
test "filter with a non-boolean predicate" { assert_eq(filter([1], |x| x), []) }
test "a non-boolean if condition" { assert_eq(if 1 { 1 } else { 2 }, 1) }
test "a non-boolean guard" { assert_eq(match 1 { x if x -> 1, _ -> 2 }, 1) }
test "comparing two functions" { assert(|x| x == |y| y) }
"#,
        "E0502",
    );

    agree_and_fail(
        r#"
fn one(a: Int) -> Int = a
test "too many arguments" { assert_eq(one(1, 2), 1) }
test "too few arguments" { assert_eq(one(), 1) }
test "a builtin with the wrong arity" { assert_eq(len([1], [2]), 1) }
"#,
        "E0202",
    );

    agree_and_fail(
        r#"
test "calling an int" { assert_eq((1)(2), 1) }
"#,
        "E0204",
    );

    agree_and_fail(
        r#"
test "no arm matches" { assert_eq(match 3 { 1 -> 1, 2 -> 2 }, 3) }
test "a let pattern that cannot match" { let [a] = [1, 2]; assert_eq(a, 1) }
"#,
        "E0205",
    );

    agree_and_fail(
        r#"
test "no such field" { assert_eq({ a: 1 }.b, 1) }
"#,
        "E0101",
    );

    agree_and_fail(
        r#"
test "field access on a non-record" { assert_eq((1).a, 1) }
"#,
        "E0502",
    );
}

/// Order is incidental until an effect can see it, and then it is semantics.
/// Each case performs once per position and asserts the sequence the handler
/// recorded.
#[test]
fn evaluation_order_is_observable_and_identical() {
    agree_and_pass(
        r#"
effect log { write note[trace](n: Int) -> Int }

fn traced(body: () -> Int / {log.write[trace]}, c: Cell<List<Int>>) -> List<Int> = {
  handle { body(); () } with {
    log.note[trace](n) -> { cell_set(c, push(cell_get(c), n)); n }
  };
  cell_get(c)
}

test "application evaluates the callee before its arguments, left to right" {
  with_cell[t]([]) { c ->
    assert_eq(
      traced(|| {
        let f = |a, b, d| a + b + d;
        (if log.note[trace](0) == 0 { f } else { f })(
          log.note[trace](1), log.note[trace](2), log.note[trace](3))
      }, c),
      [0, 1, 2, 3])
  }
}

test "binary operators evaluate left then right" {
  with_cell[t]([]) { c ->
    assert_eq(traced(|| log.note[trace](1) + log.note[trace](2), c), [1, 2])
  }
}

test "a list evaluates its elements left to right" {
  with_cell[t]([]) { c ->
    assert_eq(
      traced(|| { let xs = [log.note[trace](1), log.note[trace](2), log.note[trace](3)]; len(xs) }, c),
      [1, 2, 3])
  }
}

test "a record evaluates its fields in source order, not name order" {
  with_cell[t]([]) { c ->
    assert_eq(
      traced(|| { let r = { z: log.note[trace](1), a: log.note[trace](2) }; r.a }, c),
      [1, 2])
  }
}

test "a perform evaluates its own arguments left to right" {
  with_cell[t]([]) { c ->
    assert_eq(
      traced(|| log.note[trace](log.note[trace](1) + log.note[trace](2)), c),
      [1, 2, 3])
  }
}

test "block statements run in order and the tail runs last" {
  with_cell[t]([]) { c ->
    assert_eq(
      traced(|| { log.note[trace](1); let x = log.note[trace](2); x + log.note[trace](3) }, c),
      [1, 2, 3])
  }
}

test "with_cell evaluates its initial value before the body" {
  with_cell[t]([]) { c ->
    assert_eq(
      traced(|| with_cell[inner](log.note[trace](1)) { d -> cell_get(d) + log.note[trace](2) }, c),
      [1, 2])
  }
}

test "an if evaluates the condition and exactly one branch" {
  with_cell[t]([]) { c ->
    assert_eq(
      traced(|| if log.note[trace](1) == 1 { log.note[trace](2) } else { log.note[trace](99) }, c),
      [1, 2])
  }
}

test "a match evaluates the scrutinee, then guards in arm order" {
  with_cell[t]([]) { c ->
    assert_eq(
      traced(|| match log.note[trace](0) {
        x if log.note[trace](1) == 99 -> x,
        x if log.note[trace](2) == 2 -> x,
        _ -> 0
      }, c),
      [0, 1, 2])
  }
}

test "a field access evaluates its base once" {
  with_cell[t]([]) { c ->
    assert_eq(traced(|| { let r = { a: log.note[trace](1) }; r.a + r.a }, c), [1])
  }
}

test "the short-circuited operand never runs" {
  with_cell[t]([]) { c ->
    assert_eq(traced(|| { if false && log.note[trace](9) == 9 { 1 } else { 0 } }, c), [])
  }
}
"#,
    );
}

#[test]
fn handlers_in_every_arrangement() {
    agree_and_pass(
        r#"
effect state { read get[s]() -> Int  write put[s](v: Int) -> Int }
effect other { read ask[o]() -> Int }

test "a tail-resumptive clause returns to the perform site" {
  assert_eq(handle { state.get[s]() + 1 } with { state.get[s]() -> 41 }, 42)
}

test "a return clause transforms the body's value" {
  assert_eq(
    handle { state.get[s]() } with { state.get[s]() -> 1, return x -> x * 10 },
    10)
}

test "a return clause runs even when nothing was performed" {
  assert_eq(handle { 5 } with { state.get[s]() -> 1, return x -> x + 1 }, 6)
}

test "the innermost handler wins" {
  assert_eq(
    handle { handle { state.get[s]() } with { state.get[s]() -> 1 } } with { state.get[s]() -> 2 },
    1)
}

test "an outer handler answers what the inner one does not name" {
  assert_eq(
    handle { handle { other.ask[o]() } with { state.get[s]() -> 1 } } with { other.ask[o]() -> 7 },
    7)
}

test "a clause that performs the operation it handles reaches the next handler out" {
  assert_eq(
    handle {
      handle { state.get[s]() } with { state.get[s]() -> state.get[s]() + 1 }
    } with { state.get[s]() -> 10 },
    11)
}

test "a handler installed inside a clause body delimits only that body" {
  assert_eq(
    handle {
      handle { state.get[s]() } with {
        state.get[s]() -> handle { other.ask[o]() } with { other.ask[o]() -> 3 }
      }
    } with { state.get[s]() -> 0 },
    3)
}

test "a clause sees the environment where the handler was written" {
  let k = 100;
  assert_eq(handle { state.get[s]() } with { state.get[s]() -> k }, 100)
}

test "a clause receives its operation's arguments" {
  assert_eq(handle { state.put[s](7) } with { state.put[s](v) -> v * 2 }, 14)
}

test "one handler with several clauses dispatches on the operation" {
  assert_eq(
    handle { state.put[s](3) + state.get[s]() } with {
      state.get[s]() -> 1,
      state.put[s](v) -> v
    },
    4)
}

test "a handler's body may be a call into another definition" {
  assert_eq(handle { indirect() } with { state.get[s]() -> 5 }, 5)
}

fn indirect() -> Int / {state.read[s]} = deeper()
fn deeper() -> Int / {state.read[s]} = state.get[s]()

test "a return clause may itself perform, reaching outward" {
  assert_eq(
    handle {
      handle { 1 } with { state.get[s]() -> 0, return x -> x + other.ask[o]() }
    } with { other.ask[o]() -> 41 },
    42)
}

test "a handler over a body that performs nothing is transparent" {
  assert_eq(handle { 3 } with { state.get[s]() -> 0 }, 3)
}
"#,
    );
}

#[test]
fn a_resource_label_selects_the_clause() {
    agree_and_pass(
        r#"
effect db { read get[r](k: Int) -> Int }

test "two resources of one operation reach two clauses" {
  assert_eq(
    handle { db.get[users](1) * 10 + db.get[orders](2) } with {
      db.get[users](k) -> k,
      db.get[orders](k) -> k
    },
    12)
}

test "the first matching clause wins when two name one resource" {
  assert_eq(
    handle { db.get[users](1) } with { db.get[users](k) -> 1, db.get[users](k) -> 2 },
    1)
}
"#,
    );
}

#[test]
fn unhandled_and_misdeclared_operations() {
    agree_and_fail(
        r#"
effect state { read get[s]() -> Int }
test "no handler at all" { assert_eq(state.get[s](), 1) }
test "a handler for another resource does not catch it" {
  assert_eq(handle { state.get[s]() } with { state.get[other]() -> 1 }, 1)
}
test "the handler is gone once its body returned" {
  handle { 1 } with { state.get[s]() -> 1 };
  assert_eq(state.get[s](), 1)
}
"#,
        "E0303",
    );
}

#[test]
fn effects_performed_inside_higher_order_builtins() {
    agree_and_pass(
        r#"
effect state { read get[s]() -> Int  write put[s](v: Int) -> Int }

test "map over a performing closure" {
  assert_eq(
    handle { map([1, 2, 3], |x| x + state.get[s]()) } with { state.get[s]() -> 10 },
    [11, 12, 13])
}

test "filter with a performing predicate" {
  assert_eq(
    handle { filter([1, 2, 3, 4], |x| x % state.get[s]() == 0) } with { state.get[s]() -> 2 },
    [2, 4])
}

test "fold with a performing function" {
  assert_eq(
    handle { fold([1, 2, 3], 0, |acc, x| acc + x * state.get[s]()) } with { state.get[s]() -> 2 },
    12)
}

test "a handler whose clause writes a cell is seen by later elements" {
  with_cell[s](0) { c -> {
    let ys = handle { map([1, 2, 3], |x| x * state.get[s]()) } with {
      state.get[s]() -> { cell_set(c, cell_get(c) + 1); cell_get(c) }
    };
    assert_eq(ys, [1, 4, 9]);
    assert_eq(cell_get(c), 3)
  } }
}

test "a handler installed inside the callback does not leak to the next element" {
  assert_eq(
    map([1, 2], |x| handle { state.get[s]() } with { state.get[s]() -> x }),
    [1, 2])
}

test "nested higher-order builtins each perform" {
  assert_eq(
    handle {
      map([1, 2], |x| fold(range(3), x, |acc, y| acc + y * state.get[s]()))
    } with { state.get[s]() -> 1 },
    [4, 5])
}

test "a failing assertion inside a callback keeps its structured failure" {
  let seen = map([1, 2], |x| x);
  assert_eq(seen, [1, 2])
}
"#,
    );
}

#[test]
fn cells_closures_and_the_world() {
    agree_and_pass(
        r#"
effect state { read get[s]() -> Int }

test "a cell read and written in a region" {
  with_cell[s](1) { c -> {
    cell_set(c, cell_get(c) + 41);
    assert_eq(cell_get(c), 42)
  } }
}

test "nested regions allocate distinct cells" {
  with_cell[a](1) { x -> {
    with_cell[b](2) { y -> {
      cell_set(x, 10);
      assert_eq(cell_get(x), 10);
      assert_eq(cell_get(y), 2)
    } }
  } }
}

test "a closure captures the cell, not its value" {
  with_cell[s](0) { c -> {
    let bump = || cell_set(c, cell_get(c) + 1);
    bump(); bump(); bump();
    assert_eq(cell_get(c), 3)
  } }
}

test "a closure escaping the lambda that made it still reaches the cell" {
  with_cell[s](0) { c -> {
    let make = |n| || cell_set(c, cell_get(c) + n);
    let by_two = make(2);
    by_two(); by_two();
    assert_eq(cell_get(c), 4)
  } }
}

test "a cell stored inside an enclosing cell outlives its region" {
  with_cell[outer](0) { keeper -> {
    with_cell[inner](5) { c -> cell_set(keeper, cell_get(c)) };
    assert_eq(cell_get(keeper), 5)
  } }
}

test "a cell in a loop allocates one entry per iteration" {
  assert_eq(fold(range(4), 0, |acc, i| with_cell[s](i) { c -> acc + cell_get(c) }), 6)
}

test "two handlers backed by two cells do not interfere" {
  with_cell[a](0) { x -> {
    with_cell[b](0) { y -> {
      handle {
        handle { state.get[s]() } with { state.get[s]() -> { cell_set(y, 2); 0 } }
      } with { state.get[s]() -> { cell_set(x, 1); 0 } };
      assert_eq(cell_get(x), 0);
      assert_eq(cell_get(y), 2)
    } }
  } }
}
"#,
    );
}

#[test]
fn recursion_well_inside_the_bound() {
    agree_and_pass(
        r#"
fn count(n: Int) -> Int = if n == 0 { 0 } else { 1 + count(n - 1) }
fn loop_(n: Int, acc: Int) -> Int = if n == 0 { acc } else { loop_(n - 1, acc + n) }
fn even_(n: Int) -> Bool = if n == 0 { true } else { odd_(n - 1) }
fn odd_(n: Int) -> Bool = if n == 0 { false } else { even_(n - 1) }

test "non-tail recursion" { assert_eq(count(500), 500) }
test "tail recursion in an if branch" { assert_eq(loop_(500, 0), 125250) }
test "mutual recursion" { assert(even_(500)); assert(!odd_(500)) }

fn last_of(xs: List<Int>) -> Int = match xs { [x] -> x, [_, ..r] -> last_of(r), [] -> 0 }
test "tail recursion in a match arm" { assert_eq(last_of(range(500)), 499) }

fn sum_block(n: Int, acc: Int) -> Int = { let next = acc + n; if n == 0 { next } else { sum_block(n - 1, next) } }
test "tail recursion in a block tail" { assert_eq(sum_block(500, 0), 125250) }

effect state { read get[s]() -> Int }
fn drain(n: Int, acc: Int) -> Int / {state.read[s]} =
  if n == 0 { acc } else { drain(n - 1, acc + state.get[s]()) }
test "tail recursion under a handler" {
  assert_eq(handle { drain(200, 0) } with { state.get[s]() -> 1 }, 200)
}

fn tail_in_a_clause(n: Int) -> Int / {state.read[s]} =
  handle { state.get[s]() } with { state.get[s]() -> if n == 0 { 0 } else { tail_in_a_clause(n - 1) } }
test "tail recursion in a clause body" { assert_eq(tail_in_a_clause(100), 0) }
"#,
    );
}

#[test]
fn a_resume_binder_is_refused_by_one_engine_and_never_compared() {
    let src = r#"
effect amb { read flip[coin]() -> Bool }

test "two resumptions" {
  with_cell[trace](0) { c -> {
    let total = handle {
      let b = amb.flip[coin]();
      cell_set(c, cell_get(c) + 1);
      if b { 10 } else { 20 }
    } with {
      amb.flip[coin]() resume k -> k(true) + k(false),
      return x -> x
    };
    assert_eq(total, 30);
    assert_eq(cell_get(c), 2)
  } }
}
"#;
    let (program, resolved) = load(src);
    let mut treewalk = Interp::for_program(&program, &resolved);
    let mut machine = Machine::for_program(&program, &resolved);
    let report = compare_tests(&mut treewalk, &mut machine, &Fixture::empty());
    assert!(report.is_clean(), "{report}");
    assert_eq!(report.compared, 0, "{report}");
    assert_eq!(report.machine_only, 1, "{report}");
}

#[test]
fn a_refusal_is_reported_per_test_that_reaches_the_clause() {
    let src = r#"
effect amb { read flip[coin]() -> Bool }

fn unreached() -> Int = handle { if amb.flip[coin]() { 1 } else { 2 } } with {
  amb.flip[coin]() resume k -> k(true) + k(false)
}

test "never calls it" { assert_eq(1, 1) }
test "calls it" { assert_eq(unreached(), 3) }
"#;
    let (program, resolved) = load(src);
    let mut treewalk = Interp::for_program(&program, &resolved);
    let mut machine = Machine::for_program(&program, &resolved);
    let report = compare_tests(&mut treewalk, &mut machine, &Fixture::empty());
    assert!(report.is_clean(), "{report}");
    assert_eq!(report.compared + report.machine_only, 2, "{report}");
}

#[test]
fn pairs_of_forms_nested_in_each_other() {
    agree_and_pass(
        r#"
effect state { read get[s]() -> Int }
type Box = Empty | Full(Int)

fn h(body: () -> Int / {state.read[s]}) -> Int = handle { body() } with { state.get[s]() -> 3 }

test "handle inside match inside lambda inside list" {
  let fs = [|n| match n { 0 -> h(|| state.get[s]()), _ -> n }];
  assert_eq(map(fs, |f| f(0)), [3])
}

test "with_cell inside a match arm inside a handler" {
  assert_eq(
    handle {
      match state.get[s]() { 3 -> with_cell[s](1) { c -> cell_get(c) + state.get[s]() }, _ -> 0 }
    } with { state.get[s]() -> 3 },
    4)
}

test "a record of lambdas performing under a handler" {
  assert_eq(h(|| { let r = { f: || state.get[s]() }; (r.f)() }), 3)
}

test "a constructor argument that performs" {
  assert_eq(h(|| match Full(state.get[s]()) { Full(n) -> n, Empty -> 0 }), 3)
}

test "a guard that performs" {
  assert_eq(h(|| match 1 { x if state.get[s]() == 3 -> x, _ -> 0 }), 1)
}

test "a field access whose base performs" {
  assert_eq(h(|| { let r = { v: state.get[s]() }; r.v }), 3)
}

test "a with_cell whose initial value performs" {
  assert_eq(h(|| with_cell[s](state.get[s]()) { c -> cell_get(c) }), 3)
}

test "a handler whose body is a with_cell whose body is a handler" {
  assert_eq(
    handle {
      with_cell[s](1) { c ->
        handle { cell_get(c) + state.get[s]() } with { state.get[s]() -> 10 }
      }
    } with { state.get[s]() -> 100 },
    11)
}

test "a lambda defined inside a clause body and called outside it" {
  with_cell[s](0) { c -> {
    let f = handle { state.get[s]() ; |n| n + 1 } with { state.get[s]() -> { cell_set(c, 1); 0 } };
    assert_eq(f(1), 2);
    assert_eq(cell_get(c), 1)
  } }
}

test "operators over calls over performs" {
  assert_eq(h(|| (state.get[s]() * 2) - (state.get[s]() - 1)), 4)
}
"#,
    );
}

/// The whole equivalence of the two clause forms, checked across the engines
/// that can each run one of them: ADR 0005 §1.3 says `op(x̄) -> e` *is*
/// `op(x̄) resume k -> k(e)`, so the tree-walker running the first and the
/// machine running the second must agree on value, diagnostic and world.
fn tail_and_general_forms_agree(tail: &str, general: &str) {
    // The comparison includes every label's span, so the clause bodies are
    // padded to start at the same byte offset in both programs. Without that a
    // diagnostic raised anywhere after the clause head would "diverge" purely
    // because `resume k -> k(` is longer than `->`.
    assert_eq!(tail.len(), general.len(), "the two programs must line up");
    let (a, ra) = load(tail);
    let (b, rb) = load(general);
    let mut treewalk = Interp::for_program(&a, &ra);
    let mut machine = Machine::for_program(&b, &rb);
    let report = compare_tests(&mut treewalk, &mut machine, &Fixture::empty());
    assert!(
        report.is_clean(),
        "{report}\n--- tail ---{tail}\n--- general ---{general}"
    );
    assert!(report.compared > 0, "{report}");
}

#[test]
fn the_two_clause_forms_are_one_semantics() {
    let program = |clause: &str| {
        format!(
            r#"
effect state {{ read get[s]() -> Int  write put[s](v: Int) -> Int }}
effect other {{ read ask[o]() -> Int }}

test "answers the perform site" {{
  with_cell[s](0) {{ c -> {{
    assert_eq(handle {{ state.get[s]() + 1 }} with {{ {clause} }}, 42);
    assert_eq(cell_get(c), 7)
  }} }}
}}

test "answers it twice in one body" {{
  with_cell[s](0) {{ c -> assert_eq(handle {{ state.get[s]() + state.get[s]() }} with {{ {clause} }}, 82) }}
}}

test "under a return clause" {{
  with_cell[s](0) {{ c -> assert_eq(handle {{ state.get[s]() }} with {{ {clause}, return x -> x * 2 }}, 82) }}
}}

test "the resumed computation reads the write the clause made" {{
  with_cell[s](0) {{ c -> assert_eq(handle {{ state.get[s]() + cell_get(c) }} with {{ {clause} }}, 48) }}
}}

test "inside a higher-order builtin" {{
  with_cell[s](0) {{ c -> assert_eq(handle {{ map([1, 2], |x| x + state.get[s]()) }} with {{ {clause} }}, [42, 43]) }}
}}

test "under an unrelated outer handler" {{
  with_cell[s](0) {{ c ->
    assert_eq(handle {{ handle {{ state.get[s]() }} with {{ {clause} }} }} with {{ other.ask[o]() -> 1 }}, 41) }}
}}

test "the residual computation may fail" {{
  with_cell[s](0) {{ c -> assert_eq(handle {{ state.get[s]() / 0 }} with {{ {clause} }}, 0) }}
}}

test "a hundred performs answered by one clause" {{
  with_cell[s](0) {{ c -> assert_eq(handle {{ depth(100) }} with {{ {clause} }}, 4100) }}
}}

fn depth(n: Int) -> Int / {{state.read[s]}} =
  if n == 0 {{ 0 }} else {{ state.get[s]() + depth(n - 1) }}
"#
        )
    };

    // The clause writes a cell and only then produces its answer, so world
    // threading, the stack the clause runs on, and where its value goes are all
    // observable in one expression.
    tail_and_general_forms_agree(
        &program("state.get[s]() ->            { cell_set(c, 7); 41 } "),
        &program("state.get[s]() resume k -> k({ cell_set(c, 7); 41 })"),
    );
}

#[test]
fn the_two_clause_forms_agree_when_the_clause_performs_the_operation_it_handles() {
    let program = |clause: &str| {
        format!(
            r#"
effect state {{ read get[s]() -> Int }}

test "the clause reaches the next handler out" {{
  assert_eq(
    handle {{ handle {{ state.get[s]() }} with {{ {clause} }} }} with {{ state.get[s]() -> 100 }},
    41)
}}

test "two nested clauses each reach one further out" {{
  assert_eq(
    handle {{ handle {{ handle {{ state.get[s]() }} with {{ {clause} }} }} with {{ {clause} }} }}
      with {{ state.get[s]() -> 159 }},
    41)
}}

test "the clause runs under whatever handler the perform site had" {{
  assert_eq(
    handle {{ map([1, 2], |x| handle {{ state.get[s]() }} with {{ {clause} }}) }}
      with {{ state.get[s]() -> 100 }},
    [41, 41])
}}
"#
        )
    };
    tail_and_general_forms_agree(
        &program("state.get[s]() ->            state.get[s]() - 59 "),
        &program("state.get[s]() resume k -> k(state.get[s]() - 59)"),
    );
}

/// A `Cell` reached through a continuation resumed after its `with_cell`
/// returned is ADR 0005 §2's monotone world, and it is a *success*. Only the
/// machine can express it, so the audit here is against the ADR rather than
/// against the other engine.
#[test]
fn a_continuation_escaping_its_region_still_reads_the_cell() {
    let src = r#"
effect amb { read flip[coin]() -> Bool }

fn escaped() -> Int = {
  with_cell[hold](0) { h -> {
    with_cell[s](7) { c -> {
      handle { cell_get(c) + amb.flip[coin]() } with {
        amb.flip[coin]() resume k -> cell_set(h, k),
        return x -> x
      }
    } };
    (cell_get(h))(1)
  } }
}

test "resumed outside the region" { assert_eq(escaped(), 8) }
"#;
    let (program, resolved) = load(src);
    let mut machine = Machine::for_program(&program, &resolved);
    machine
        .eval_test(0)
        .expect("the world is monotone, so the cell is still there");
}

#[test]
fn multi_shot_shapes_the_tree_walker_cannot_express() {
    let src = r#"
effect amb { read flip[coin]() -> Bool }
effect state { read get[s]() -> Int }

test "two resumptions leave the trace cell at two" {
  with_cell[trace](0) { c -> {
    let total = handle {
      let b = amb.flip[coin]();
      cell_set(c, cell_get(c) + 1);
      if b { 10 } else { 20 }
    } with { amb.flip[coin]() resume k -> k(true) + k(false), return x -> x };
    assert_eq(total, 30);
    assert_eq(cell_get(c), 2)
  } }
}

test "save and restore around each resumption gives both branches one start" {
  with_cell[trace](0) { c -> {
    let total = handle {
      cell_set(c, cell_get(c) + 1);
      if amb.flip[coin]() { 10 } else { 20 }
    } with {
      amb.flip[coin]() resume k -> {
        let before = cell_get(c);
        let a = k(true);
        cell_set(c, before);
        let b = k(false);
        a + b
      },
      return x -> x
    };
    assert_eq(total, 30);
    assert_eq(cell_get(c), 1)
  } }
}

test "zero resumptions see the writes before the perform and none after" {
  with_cell[log](0) { c -> {
    let v = handle {
      cell_set(c, 1);
      let b = amb.flip[coin]();
      cell_set(c, 2);
      if b { 10 } else { 20 }
    } with { amb.flip[coin]() resume k -> cell_get(c), return x -> x };
    assert_eq(v, 1);
    assert_eq(cell_get(c), 1)
  } }
}

test "a continuation captured inside a map callback produces two lists" {
  with_cell[out](0) { c -> {
    let total = handle {
      let ys = map([1, 2, 3], |x| if x == 2 { if amb.flip[coin]() { 100 } else { 200 } } else { x });
      cell_set(c, cell_get(c) + 1);
      len(ys) * 1000 + fold(ys, 0, |a, b| a + b)
    } with { amb.flip[coin]() resume k -> k(true) + k(false), return x -> x };
    assert_eq(total, 3104 + 3204);
    assert_eq(cell_get(c), 2)
  } }
}

test "a general clause performing what it handles reaches the next handler out" {
  assert_eq(
    handle {
      handle { state.get[s]() } with { state.get[s]() resume k -> k(state.get[s]() + 1) }
    } with { state.get[s]() -> 10 },
    11)
}

test "a continuation applied to the wrong argument count" {
  assert_eq(
    handle { amb.flip[coin]() } with { amb.flip[coin]() resume k -> k(), return x -> 0 },
    0)
}
"#;
    let (program, resolved) = load(src);
    let mut machine = Machine::for_program(&program, &resolved);
    for i in 0..5 {
        machine.eval_test(i).unwrap_or_else(|d| {
            panic!(
                "test {i} (`{}`): [{}] {}",
                machine.test_name(i).unwrap_or("?"),
                d.code,
                d.message
            )
        });
    }
    let d = machine.eval_test(5).expect_err("k takes exactly one value");
    assert_eq!(d.code, "E0202", "{}", d.message);
}

/// The world after a *failing* test is part of what the harness compares, and
/// it is the half a verdict-only check would miss: one engine abandoning a
/// write the other made is a divergence even though both reported the same
/// failure.
#[test]
fn the_world_a_failure_leaves_behind_is_identical() {
    agree(
        r#"
effect state { read get[s]() -> Int }

test "a failure after a write" {
  with_cell[s](0) { c -> {
    cell_set(c, 1);
    assert_eq(cell_get(c), 2)
  } }
}

test "a failure inside a clause body after a write" {
  with_cell[s](0) { c -> {
    handle { state.get[s]() } with { state.get[s]() -> { cell_set(c, 9); panic("stop") } }
  } }
}

test "a failure inside a map callback after a write" {
  with_cell[s](0) { c -> {
    map([1, 2, 3], |x| { cell_set(c, x); assert(x < 2); x })
  } }
}

test "a failure between two regions" {
  with_cell[a](1) { x -> {
    with_cell[b](2) { y -> { cell_set(y, 3); panic("stop") } }
  } }
}

test "a region whose initial value fails allocates nothing" {
  with_cell[a](1) { x -> {
    with_cell[b](1 / 0) { y -> cell_get(y) }
  } }
}
"#,
    );
}

/// A seeded generator over the whole expression grammar, because the shapes a
/// human writes down are the shapes a human already thought about. Each program
/// stores its result in a cell, so a wrong *value* is caught by the world
/// comparison even when neither engine reports anything.
mod fuzz {
    pub struct Rng(pub u64);

    impl Rng {
        pub fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }

        pub fn below(&mut self, n: u64) -> usize {
            (self.next() % n) as usize
        }
    }

    pub struct Gen {
        pub rng: Rng,
        vars: Vec<String>,
        cells: Vec<String>,
        uid: usize,
    }

    impl Gen {
        pub fn new(seed: u64) -> Gen {
            Gen {
                rng: Rng(seed | 1),
                vars: Vec::new(),
                cells: Vec::new(),
                uid: 0,
            }
        }

        fn fresh(&mut self, prefix: &str) -> String {
            self.uid += 1;
            format!("{prefix}{}", self.uid)
        }

        fn scoped<T>(&mut self, vars: &[&str], cells: &[&str], f: impl FnOnce(&mut Gen) -> T) -> T {
            let (v, c) = (self.vars.len(), self.cells.len());
            self.vars.extend(vars.iter().map(|x| x.to_string()));
            self.cells.extend(cells.iter().map(|x| x.to_string()));
            let out = f(self);
            self.vars.truncate(v);
            self.cells.truncate(c);
            out
        }

        pub fn int(&mut self, depth: usize) -> String {
            let leaves = 4;
            let choice = if depth == 0 {
                self.rng.below(leaves as u64)
            } else {
                self.rng.below(28)
            };
            match choice {
                0 => format!("{}", self.rng.below(9)),
                1 if !self.vars.is_empty() => {
                    let i = self.rng.below(self.vars.len() as u64);
                    self.vars[i].clone()
                }
                1 => format!("{}", self.rng.below(9)),
                2 if !self.cells.is_empty() => {
                    let i = self.rng.below(self.cells.len() as u64);
                    format!("cell_get({})", self.cells[i])
                }
                2 => "0".to_string(),
                3 => "len([1, 2, 3])".to_string(),
                4..=8 => {
                    let op = ["+", "-", "*", "/", "%"][choice - 4];
                    let a = self.int(depth - 1);
                    let b = self.int(depth - 1);
                    format!("({a} {op} {b})")
                }
                9 => {
                    let c = self.boolean(depth - 1);
                    let a = self.int(depth - 1);
                    let b = self.int(depth - 1);
                    format!("(if {c} {{ {a} }} else {{ {b} }})")
                }
                10 => {
                    let v = self.fresh("v");
                    let value = self.int(depth - 1);
                    let body = self.scoped(&[&v], &[], |g| g.int(depth - 1));
                    format!("({{ let {v} = {value}; {body} }})")
                }
                11 => {
                    let v = self.fresh("p");
                    let arg = self.int(depth - 1);
                    let body = self.scoped(&[&v], &[], |g| g.int(depth - 1));
                    format!("((|{v}| {body})({arg}))")
                }
                12 => {
                    let scrutinee = self.int(depth - 1);
                    let v = self.fresh("m");
                    let guard = self.scoped(&[&v], &[], |g| g.boolean(depth - 1));
                    let hit = self.scoped(&[&v], &[], |g| g.int(depth - 1));
                    let zero = self.int(depth - 1);
                    let other = self.int(depth - 1);
                    format!(
                        "(match {scrutinee} {{ 0 -> {zero}, {v} if {guard} -> {hit}, _ -> {other} }})"
                    )
                }
                13 => {
                    let c = self.fresh("c");
                    let r = self.fresh("r");
                    let init = self.int(depth - 1);
                    let body = self.scoped(&[], &[&c], |g| g.int(depth - 1));
                    format!("(with_cell[{r}]({init}) {{ {c} -> {body} }})")
                }
                14 if !self.cells.is_empty() => {
                    let i = self.rng.below(self.cells.len() as u64);
                    let cell = self.cells[i].clone();
                    let value = self.int(depth - 1);
                    let after = self.int(depth - 1);
                    format!("({{ cell_set({cell}, {value}); {after} }})")
                }
                14 => self.int(depth - 1),
                15 => {
                    let body = self.int(depth - 1);
                    let answer = self.int(depth - 1);
                    format!("(handle {{ {body} }} with {{ st.get[a]() -> {answer} }})")
                }
                16 => {
                    let arg = self.int(depth - 1);
                    let v = self.fresh("w");
                    let answer = self.scoped(&[&v], &[], |g| g.int(depth - 1));
                    let body = format!("st.put[b]({arg})");
                    format!("(handle {{ {body} }} with {{ st.put[b]({v}) -> {answer} }})")
                }
                17 => {
                    let a = self.int(depth - 1);
                    let b = self.int(depth - 1);
                    let y = self.fresh("y");
                    let f = self.scoped(&[&y], &[], |g| g.int(depth - 1));
                    format!("len(map([{a}, {b}], |{y}| {f}))")
                }
                18 => {
                    let seed = self.int(depth - 1);
                    let acc = self.fresh("acc");
                    let y = self.fresh("y");
                    let step = self.scoped(&[&acc, &y], &[], |g| g.int(depth - 1));
                    format!("fold(range(3), {seed}, |{acc}, {y}| {step})")
                }
                19 => {
                    let body = self.int(depth - 1);
                    let answer = self.int(depth - 1);
                    let v = self.fresh("q");
                    let ret = self.scoped(&[&v], &[], |g| g.int(depth - 1));
                    format!(
                        "(handle {{ {body} }} with {{ st.get[a]() -> {answer}, return {v} -> {ret} }})"
                    )
                }
                20 => {
                    let a = self.int(depth - 1);
                    let b = self.int(depth - 1);
                    format!("(({{ f: {a}, g: {b} }}).g)")
                }
                21 => {
                    let a = self.int(depth - 1);
                    let b = self.int(depth - 1);
                    let y = self.fresh("y");
                    let pred = self.scoped(&[&y], &[], |g| g.boolean(depth - 1));
                    format!("len(filter([{a}, {b}], |{y}| {pred}))")
                }
                22 => {
                    let a = self.int(depth - 1);
                    let b = self.int(depth - 1);
                    format!("len(push([{a}], {b}))")
                }
                23 => {
                    let a = self.int(depth - 1);
                    let b = self.int(depth - 1);
                    let other = self.int(depth - 1);
                    format!("(match [{a}, {b}] {{ [0, x] -> x, [x, _] -> x, _ -> {other} }})")
                }
                24 => "st.get[a]()".to_string(),
                25 => {
                    let body = self.int(depth - 1);
                    let answer = self.int(depth - 1);
                    format!("(handle {{ {body} }} with {{ st.get[z]() -> {answer} }})")
                }
                26 => {
                    let a = self.int(depth - 1);
                    format!("len(int_to_string({a}))")
                }
                27 => {
                    let a = self.int(depth - 1);
                    let z = self.fresh("z");
                    let inner = self.scoped(&[&z], &[], |g| g.int(depth - 1));
                    let f = self.fresh("f");
                    format!("((|{f}| {f}({a}))(|{z}| {inner}))")
                }
                _ => "1".to_string(),
            }
        }

        pub fn boolean(&mut self, depth: usize) -> String {
            match if depth == 0 {
                self.rng.below(3)
            } else {
                self.rng.below(7)
            } {
                0 => "true".to_string(),
                1 => "false".to_string(),
                2 => {
                    let a = self.int(depth.saturating_sub(1));
                    let b = self.int(depth.saturating_sub(1));
                    format!("({a} == {b})")
                }
                3 => {
                    let a = self.int(depth - 1);
                    let b = self.int(depth - 1);
                    format!("({a} < {b})")
                }
                4 => {
                    let a = self.boolean(depth - 1);
                    format!("(!{a})")
                }
                5 => {
                    let a = self.boolean(depth - 1);
                    let b = self.boolean(depth - 1);
                    format!("({a} && {b})")
                }
                _ => {
                    let a = self.boolean(depth - 1);
                    let b = self.boolean(depth - 1);
                    format!("({a} || {b})")
                }
            }
        }
    }

    /// One program of `tests` tests. An unhandled `st` reaching the top is a
    /// diagnostic both engines must produce identically, so it is left in.
    pub fn program(seed: u64, tests: usize, depth: usize) -> String {
        let mut g = Gen::new(seed);
        let mut src =
            String::from("effect st { read get[a]() -> Int  write put[b](v: Int) -> Int }\n");
        for i in 0..tests {
            let body = g.int(depth);
            src.push_str(&format!(
                "test \"t{i}\" {{ with_cell[out](0) {{ o -> cell_set(o, {body}) }} }}\n"
            ));
        }
        src
    }

    /// One program written twice: the outer handler's clause in the derived
    /// tail-resumptive form and in the primitive `resume` form. The tail form's
    /// head is padded so the clause body starts at the same byte offset in
    /// both, which keeps every label span comparable.
    pub fn clause_pair(seed: u64, tests: usize, depth: usize) -> (String, String) {
        let mut g = Gen::new(seed);
        let head = "effect st { read get[a]() -> Int  write put[b](v: Int) -> Int }\n";
        let (mut tail, mut general) = (head.to_string(), head.to_string());
        for i in 0..tests {
            let body = g.int(depth);
            let clause_body = g.int(depth);
            let one = |clause: &str| {
                format!(
                    "test \"t{i}\" {{ with_cell[out](0) {{ o -> cell_set(o, (handle {{ {body} }} with {{ {clause} }})) }} }}\n"
                )
            };
            tail.push_str(&one(&format!("st.get[a]() ->            {clause_body} ")));
            general.push_str(&one(&format!("st.get[a]() resume k -> k({clause_body})")));
        }
        (tail, general)
    }
}

#[test]
fn generated_programs_agree_on_value_diagnostic_and_world() {
    const SEEDS: u64 = 2000;
    const DEPTH: usize = 5;
    let mut compared = 0;
    let mut failures = 0;
    for seed in 1..=SEEDS {
        let src = fuzz::program(seed, 4, DEPTH);
        let (program, resolved) = load(&src);
        let mut treewalk = Interp::for_program(&program, &resolved);
        let mut machine = Machine::for_program(&program, &resolved);
        let report = compare_tests(&mut treewalk, &mut machine, &Fixture::empty());
        assert!(report.is_clean(), "seed {seed}\n{report}\n{src}");
        compared += report.compared;

        let mut engine = Machine::for_program(&program, &resolved);
        failures += (0..engine.test_count())
            .filter(|i| engine.eval_test(*i).is_err())
            .count();
    }
    assert_eq!(compared as u64, SEEDS * 4);
    // Agreement is only evidence if some of the corpus actually failed: a run
    // in which everything passed would say nothing about diagnostic equality.
    assert!(failures > 50, "only {failures} of {compared} failed");
}

fn load_modules(files: &[(&str, &str)]) -> (Program, Resolved) {
    let mut map = SourceMap::new();
    let inputs: Vec<(SourceId, ModuleName, &str)> = files
        .iter()
        .map(|(name, src)| {
            let id = map.add(format!("{name}.ply"), src.to_string());
            (id, ModuleName::from_dotted(*name), *src)
        })
        .collect();
    let program = match parse_program(inputs) {
        Ok(p) => p,
        Err(ds) => panic!("the audit program must parse: {ds:#?}"),
    };
    let resolved = resolve(&program).expect("the audit program must resolve");
    (program, resolved)
}

/// A bare name means what it meant where it was *written*, and the two engines
/// carry that differently: the tree-walker swaps a `module` field in and out
/// around every call and clause body, the machine puts it on every frame.
#[test]
fn names_resolve_identically_across_modules() {
    let files = &[
        (
            "core",
            r#"
pub effect db { read get[users](k: Int) -> Int }
pub type Shape = Round | Square(Int)
pub fn size(s: Shape) -> Int = match s { Round -> 1, Square(n) -> n }
pub fn fetch(k: Int) -> Int / {db.read[users]} = db.get[users](k)
pub fn handler_here(k: Int) -> Int = handle { fetch(k) } with { db.get[users](x) -> x * 2 }
"#,
        ),
        (
            "app",
            r#"
import core

type Shape = Round | Square(Int)
fn size(s: Shape) -> Int = match s { Round -> 100, Square(n) -> n + 100 }

test "each module's own constructors win" {
  assert_eq(size(Round), 100);
  assert_eq(core::size(core::Round), 1);
  assert_eq(size(Square(1)), 101);
  assert_eq(core::size(core::Square(1)), 1)
}

test "a handler in this module discharges an effect declared in another" {
  assert_eq(handle { core::fetch(3) } with { core::db.get[users](x) -> x + 10 }, 13)
}

test "a handler in the other module discharges it" {
  assert_eq(core::handler_here(3), 6)
}

test "the clause body resolves where the handler was written" {
  assert_eq(handle { core::fetch(1) } with { core::db.get[users](x) -> size(Round) }, 100)
}

test "a lambda written here and applied there" {
  assert_eq(map([1, 2], |x| size(Square(x))), [101, 102])
}

test "a clause for another resource does not catch it" {
  assert_eq(handle { core::fetch(1) } with { core::db.get[orders](x) -> x }, 1)
}
"#,
        ),
    ];
    let (program, resolved) = load_modules(files);
    let mut treewalk = Interp::for_program(&program, &resolved);
    let mut machine = Machine::for_program(&program, &resolved);
    let report = compare_tests(&mut treewalk, &mut machine, &Fixture::empty());
    assert!(report.is_clean(), "{report}");
    assert_eq!(report.compared, 6, "{report}");
}

/// `eval_test_in` is the entry point the incremental front end uses, and it is
/// a different lookup from `eval_test` on both engines.
#[test]
fn addressing_a_test_by_module_and_ordinal_agrees() {
    let files = &[
        (
            "a",
            "test \"first\" { assert_eq(1, 1) }\ntest \"second\" { assert_eq(1, 2) }\n",
        ),
        ("b", "test \"first\" { assert_eq(2, 2) }\n"),
    ];
    let (program, resolved) = load_modules(files);
    let mut treewalk = Interp::for_program(&program, &resolved);
    let mut machine = Machine::for_program(&program, &resolved);

    for (module, ordinal) in [("a", 0), ("a", 1), ("b", 0), ("b", 4), ("missing", 0)] {
        let key = ply_span::Symbol::new(module);
        let l = treewalk.eval_test_in(&key, ordinal);
        let r = machine.eval_test_in(&key, ordinal);
        let subject = format!("{module}#{ordinal}");
        assert!(
            ply_eval::compare_outcomes(&treewalk, &machine, &subject, None, &l, &r).is_none(),
            "{subject}: {l:?} vs {r:?}"
        );
    }
}

/// `ply run`'s path: a named entry point called with arguments, whose answer is
/// a value rather than a verdict.
#[test]
fn calling_a_definition_by_name_agrees_on_the_value_and_the_world() {
    let src = r#"
effect state { read get[s]() -> Int }

fn main() -> Int = handle { with_cell[s](2) { c -> {
  cell_set(c, cell_get(c) * state.get[s]());
  cell_get(c)
} } } with { state.get[s]() -> 21 }

fn boom() -> Int = 1 / 0
fn takes(a: Int, b: Int) -> Int = a - b
"#;
    let (program, resolved) = load(src);
    let mut treewalk = Interp::for_program(&program, &resolved);
    let mut machine = Machine::for_program(&program, &resolved);

    for (name, args) in [
        ("audit.main", vec![]),
        ("audit.boom", vec![]),
        (
            "audit.takes",
            vec![ply_eval::Value::Int(9), ply_eval::Value::Int(4)],
        ),
        ("audit.takes", vec![ply_eval::Value::Int(9)]),
        ("audit.nope", vec![]),
    ] {
        let l = treewalk.call(name, args.clone(), ply_span::Span::DUMMY);
        let r = machine.call(name, args, ply_span::Span::DUMMY);
        assert!(
            ply_eval::compare_answers(&treewalk, &machine, name, &l, &r).is_none(),
            "{name}: {l:?} vs {r:?}"
        );
    }
}

/// A fixture built once and opened per test is the milestone's mechanism, and
/// it has to mean the same thing on both engines: every test sees the seed and
/// no test sees another's writes.
#[test]
fn a_seeded_fixture_opens_identically_on_both_engines() {
    let src = r#"
test "writes over its own region" { with_cell[extra](0) { c -> cell_set(c, 1) } }
test "writes over its own region again" { with_cell[extra](0) { c -> cell_set(c, 2) } }
"#;
    let (program, resolved) = load(src);
    let seeded = Fixture::build(|r| ply_eval::Value::Cell(r.alloc_cell(ply_eval::Value::Int(7))));
    let id = seeded
        .handle()
        .as_cell(ply_span::Span::DUMMY, "the fixture handle")
        .expect("a cell");

    let mut treewalk = Interp::for_program(&program, &resolved);
    let mut machine = Machine::for_program(&program, &resolved);
    let report = compare_tests(&mut treewalk, &mut machine, &seeded);
    assert!(report.is_clean(), "{report}");
    assert_eq!(report.compared, 2, "{report}");
    assert_eq!(treewalk.cells().get(id).unwrap().render(), "7");
    assert_eq!(machine.cells().get(id).unwrap().render(), "7");
}

/// ADR 0005 §4.1: an operation is performed once, at the `perform`, and every
/// resumption receives that one value — which is why multi-shot introduces no
/// nondeterminism and E0412 needs no change.
#[test]
fn a_nondet_operation_resumed_twice_delivers_one_value() {
    let src = r#"
nondet effect clock { read now() -> Int }

fn read_once(n: Cell<Int>) -> Int = { cell_set(n, cell_get(n) + 1); cell_get(n) }

test/nondet "both resumptions see one reading" {
  with_cell[reads](0) { n -> {
    let total = handle { clock.now() * 10 } with {
      clock.now() resume k -> k(read_once(n)) + k(read_once(n)),
      return x -> x
    };
    assert_eq(cell_get(n), 2);
    assert_eq(total, 30)
  } }
}
"#;
    let (program, resolved) = load(src);
    Machine::for_program(&program, &resolved)
        .eval_test(0)
        .expect("each resumption receives the value its own call produced");
}

/// The two clause forms, generatively. ADR 0005 §1.3 makes `op(x̄) -> e` a
/// derived form of `op(x̄) resume k -> k(e)`, so a random clause body under a
/// random handled computation is the sharpest available check on the machine's
/// `Frame::Resume`: one engine runs the derived form, the other the primitive.
#[test]
fn the_two_clause_forms_agree_on_generated_programs() {
    const SEEDS: u64 = 2000;
    let mut compared = 0;
    let mut failures = 0;
    for seed in 1..=SEEDS {
        let (tail, general) = fuzz::clause_pair(seed, 4, 4);
        assert_eq!(tail.len(), general.len(), "{tail}\n{general}");
        let (a, ra) = load(&tail);
        let (b, rb) = load(&general);
        let mut treewalk = Interp::for_program(&a, &ra);
        let mut machine = Machine::for_program(&b, &rb);
        let report = compare_tests(&mut treewalk, &mut machine, &Fixture::empty());
        assert!(
            report.is_clean(),
            "seed {seed}\n{report}\n--- tail ---\n{tail}\n--- general ---\n{general}"
        );
        compared += report.compared;

        let mut engine = Machine::for_program(&b, &rb);
        failures += (0..engine.test_count())
            .filter(|i| engine.eval_test(*i).is_err())
            .count();
    }
    assert_eq!(compared as u64, SEEDS * 4);
    assert!(failures > 50, "only {failures} of {compared} failed");
}

/// Both engines bound the same thing at the same number and say so in the same
/// words. This was a divergence: the tree-walker bounded *nested calls* at
/// 10,000 while the machine bounded *pending frames* at 1,000,000, so every
/// program recursing between the two budgets failed `--engine both` — and one
/// past both still diverged, because the messages and notes differed.
///
/// The budget is now `DEFAULT_MAX_CALLS` on both sides, counted as pending
/// calls: the tree-walker's own nesting, the machine's `Frame::Call`s. Both
/// halves of the original finding are asserted, one program each.
///
/// > **Still narrow, no longer a gap (2026-08-24).** An R5 review narrowed this
/// > doc in place, correctly: *"Both bodies pend two frames per level, so both
/// > reach `DEFAULT_MAX_CALLS` first and the frame bound is never in play … So
/// > what this test holds is: **the two engines agree on the recursion bound for
/// > bodies pending fewer than 100 frames per call.** Nothing in the suite arms
/// > the rest, and nothing here is changed to assert the divergence … carries it
/// > as open."* The two programs below still pend two frames a level and this
/// > test still holds only what R5 said it holds. What changed is the rest:
/// > there is no frame bound to reach any more, and
/// > `the_two_engines_and_a_backend_agree_however_many_frames_a_body_pends`
/// > below arms the ratio R5 measured, at the scale it measured it.
#[test]
fn the_two_engines_agree_on_the_recursion_bound() {
    let between = r#"
fn count(n: Int) -> Int = if n == 0 { 0 } else { 1 + count(n - 1) }
test "past the shared budget" { assert_eq(count(20000), 20000) }
"#;
    agree_and_fail(between, ply_span::codes::RUNTIME_ERROR);

    let past_both = r#"
fn forever(n: Int) -> Int = 1 + forever(n + 1)
test "no base case at all" { assert_eq(forever(0), 0) }
"#;
    agree_and_fail(past_both, ply_span::codes::RUNTIME_ERROR);

    let (program, resolved) = load(past_both);
    let d = Machine::for_program(&program, &resolved)
        .eval_test(0)
        .expect_err("a recursion with no base case is bounded");
    assert!(
        d.message.contains(&format!(
            "recursion limit of {} nested calls exceeded",
            ply_eval::DEFAULT_MAX_CALLS
        )),
        "{}",
        d.message
    );
}

/// A backend that is honest about its budget: it runs the body on its own
/// engine, capped at exactly the `budget` the seam handed it, so it provably
/// cannot outrun the bound the machine is holding the program to.
///
/// This is the shape R5's review used to reproduce the seam defect inside this
/// crate, kept because it is the only thing here that exercises the accept path
/// under a budget that matters.
mod honest {
    use ply_eval::{Compiled, Machine, Value};
    use ply_span::{Span, Symbol};
    use ply_syntax::ast::Program;
    use ply_syntax::resolve::{Resolved, resolve};
    use std::cell::Cell;

    pub struct Budgeted {
        program: *const Program,
        copy: &'static Program,
        resolved: &'static Resolved,
        entries: Cell<u64>,
    }

    impl Budgeted {
        /// The program is leaked because a backend may not borrow one — see the
        /// `compiled` field on `Machine`.
        pub fn over(program: &Program) -> Budgeted {
            let copy: &'static Program = Box::leak(Box::new(program.clone()));
            let resolved: &'static Resolved =
                Box::leak(Box::new(resolve(copy).expect("it resolved once already")));
            Budgeted {
                program: std::ptr::from_ref(program),
                copy,
                resolved,
                entries: Cell::new(0),
            }
        }

        pub fn entries(&self) -> u64 {
            self.entries.get()
        }
    }

    impl Compiled for Budgeted {
        fn describes(&self, program: &Program) -> bool {
            std::ptr::eq(self.program, std::ptr::from_ref(program))
        }

        fn enter(&self, name: &Symbol, args: &[Value], budget: usize) -> Option<Value> {
            let mut inner = Machine::for_program(self.copy, self.resolved).with_max_calls(budget);
            match inner.call(name.as_str(), args.to_vec(), Span::DUMMY) {
                Ok(v @ (Value::Int(_) | Value::Bool(_))) => {
                    self.entries.set(self.entries.get() + 1);
                    Some(v)
                }
                _ => None,
            }
        }
    }
}

/// `hog(n) = k * n`, spelled so that descending to the recursive call leaves `k`
/// binary operands pending — one machine frame each, one native tree-walker
/// level each. `k + 1` frames a call, times `depth` calls.
fn hog(k: usize, depth: usize) -> String {
    let plus = vec!["+ 1"; k].join(" ");
    format!(
        "fn hog(n: Int) -> Int = if n == 0 {{ 0 }} else {{ hog(n - 1) {plus} }}\n\
         test \"a recursion whose body pends {k} frames a level\" {{ assert_eq(hog({depth}), {}) }}\n",
        k * depth
    )
}

/// How many frames a body pends per call changes nothing about the answer, on
/// any of the three ways a program can be run here.
///
/// The bound is nested calls, `DEFAULT_MAX_CALLS`. The tree-walker counts its
/// own nesting, the machine counts the `Frame::Call`s on its stack, and a
/// backend is handed the remainder as `budget` — one number, three engines. The
/// operands pending around those calls bound nothing: on the tree-walker they
/// are native stack, on the machine they are heap cells, and a natively
/// compiled body has neither.
///
/// > **This is the test the catalogue said did not exist (2026-08-24).**
/// > `CONTRIBUTING.md` §"Things known to be broken" item 10 read *"nothing in
/// > the suite arms the true bound"*, and item 9 recorded the same ratio
/// > reaching the compiled seam: *"`machine alone: Err("recursion limit of
/// > 1000000 pending frames exceeded")` / `machine + spike: Ok(1350000)`"*. Both
/// > are closed by removing the machine's default frame ceiling — see
/// > `Machine::with_max_frames` — and this is what would notice if it came back.
/// > The ceiling sat at 1,000,000 and a body pending `k` frames a call crossed
/// > it at `depth × (k + 1) > 1_000_000`; R5 measured the crossover at depth
/// > 9,990, k = 90 passing and k = 100 raising, and both are asserted below.
///
/// **This is the memory-heaviest test in the crate and that is inherent.** The
/// tree-walker spends kilobytes of native stack per pending level in a debug
/// build, so the smallest program that crosses a ceiling of 1,000,000 frames
/// costs it gigabytes of peak RSS. There is no cheaper witness: the machine's
/// frame stack is the tree-walker's native stack reified, one frame per level,
/// so any program pending a million machine frames nests a million native
/// levels. So the tree-walker runs **once** here, over the smallest crossing
/// program, and every other leg is a machine — which measured **4,243 MiB**
/// peak for the whole test, `/usr/bin/time -l`, debug, 2026-08-24.
///
/// The per-level figures behind that, peak RSS in one process each, three of
/// them from one run of a three-point series: 1,529 MiB at 304,000 levels,
/// 3,054 MiB at 608,000, 5,036 MiB at 1,003,200 — 5,274, 5,267 and 5,264 bytes
/// a level, flat — and 5,365 MiB at 1,350,000, which is 4,167 bytes a level and
/// so does not sit on that line. Five KiB a level is the number to plan with,
/// not a constant to quote.
#[test]
fn the_two_engines_and_a_backend_agree_however_many_frames_a_body_pends() {
    // 6,700 * 151 = 1,011,700 pending frames: the least this can cost and still
    // cross where the machine used to stop, against 6,701 nested calls.
    let crossing = hog(150, 6_700);
    let (program, resolved) = load(&crossing);
    let check = ply_core::check_program(&program, &resolved).expect("the witness type-checks");

    // Strategy 1 against strategy 2: the comparison `--engine both` makes, and
    // the one that printed "this is a defect in Ply". The machine on the right
    // has to be the *plain* one. Comparing the tree-walker against a machine
    // with a backend attached passes even with the ceiling restored, because the
    // backend answers at the first level shallow enough to fit and the machine
    // takes it — that masking is item 9 itself, and it is asserted below rather
    // than being allowed to stand in for this.
    let mut plain = Machine::new(&program, &resolved, &check);
    let mut treewalk = Interp::new(&program, &resolved, &check);
    let report = compare_tests(&mut treewalk, &mut plain, &Fixture::empty());
    assert!(report.is_clean(), "{report}\n{crossing}");
    assert_eq!(report.compared, 1, "{report}");
    assert_eq!(report.footprints_compared, report.compared, "{report}");

    // The value rather than merely agreement: two engines that both raised would
    // "agree" and prove nothing.
    let mut plain = Machine::new(&program, &resolved, &check);
    plain
        .eval_test(0)
        .unwrap_or_else(|d| panic!("the machine alone raised: [{}] {}", d.code, d.message));

    // Strategy 2 against strategy 3. `budget` is now the whole of what the
    // machine holds the program to, so there is nothing left for a backend to be
    // on the wrong side of.
    let backend = std::rc::Rc::new(honest::Budgeted::over(&program));
    let mut entered = Machine::new(&program, &resolved, &check);
    entered.set_compiled(backend.clone());
    let mut plain = Machine::new(&program, &resolved, &check);
    let report = compare_tests(&mut plain, &mut entered, &Fixture::empty());
    assert!(report.is_clean(), "{report}\n{crossing}");
    assert_eq!(report.compared, 1, "{report}");
    assert!(
        backend.entries() > 0,
        "the backend was never entered, so this compared the machine with itself"
    );

    // Proof that the three legs above are not vacuous, without needing a source
    // edit to get it. A test that passes over a program too small to have
    // reached the old ceiling would prove nothing at all, so: hand this same
    // program to this same machine with a ceiling of exactly the 1,000,000 that
    // used to be the default, and it must still be too big for it.
    let ceilinged = Machine::for_program(&program, &resolved)
        .with_max_frames(1_000_000)
        .eval_test(0)
        .expect_err("6,700 * 151 pending frames must not fit under a ceiling of 1,000,000");
    assert!(
        ceilinged
            .message
            .contains("ceiling of 1000000 pending frames"),
        "the witness stopped being big enough to reach the old default: {}",
        ceilinged.message
    );

    // And the exact crossover R5 measured, at the depth it measured it: k = 90
    // passed and k = 100 raised. Machines only from here — the tree-walker has
    // answered for this shape above and costs gigabytes each time it is asked.
    for k in [90, 100, 150] {
        let src = hog(k, 9_990);
        let (p, r) = load(&src);
        let out = Machine::for_program(&p, &r).eval_test(0);
        assert!(
            out.is_ok(),
            "k = {k} at depth 9,990 still raises: [{}] {}",
            out.as_ref().unwrap_err().code,
            out.as_ref().unwrap_err().message
        );
    }

    // The one number that does bound it still bites, and bites identically on
    // both engines for a body pending 150 frames a call. Held to a budget of 50
    // rather than the default 10,000 on purpose: at the default the tree-walker
    // would nest 1,510,000 levels to reach the conclusion it reaches here in
    // 7,550.
    let past = hog(150, 20_000);
    let (p, r) = load(&past);
    let m = Machine::for_program(&p, &r)
        .with_max_calls(50)
        .eval_test(0)
        .expect_err("20,000 calls do not fit in 50");
    let t = Interp::for_program(&p, &r)
        .with_max_calls(50)
        .eval_test(0)
        .expect_err("20,000 calls do not fit in 50");
    assert_eq!(m.code, t.code);
    assert_eq!(
        m.message, t.message,
        "the two engines phrase it differently"
    );
    assert!(
        m.message
            .contains("recursion limit of 50 nested calls exceeded"),
        "{}",
        m.message
    );
}

/// A frame ceiling is one engine's resource guard, so a machine carrying one
/// offers nothing to a backend: a native body pends no frames and could not
/// honour it, and an answer only one of the three strategies can give is the
/// thing this seam exists to make structurally impossible.
#[test]
fn a_machine_asked_for_a_frame_ceiling_offers_nothing_to_a_backend() {
    let src = hog(4, 20);
    let (program, resolved) = load(&src);
    let check = ply_core::check_program(&program, &resolved).expect("the witness type-checks");

    let offered = std::rc::Rc::new(honest::Budgeted::over(&program));
    let mut open = Machine::new(&program, &resolved, &check);
    open.set_compiled(offered.clone());
    open.eval_test(0).expect("no ceiling, so this is entered");
    assert!(offered.entries() > 0, "the control never reached the seam");

    // Far above what this program needs, so nothing here is about running out:
    // the ceiling's mere presence is what withdraws the offer.
    let refused = std::rc::Rc::new(honest::Budgeted::over(&program));
    let mut capped = Machine::new(&program, &resolved, &check).with_max_frames(1_000_000);
    capped.set_compiled(refused.clone());
    capped.eval_test(0).expect("the ceiling is far above this");
    assert_eq!(
        refused.entries(),
        0,
        "a machine holding a bound it cannot hand over entered compiled code anyway"
    );
}

/// A clause body runs on the stack *below* its own handler, so the calls the
/// handled body made since the handler was installed are not pending while the
/// clause runs. The machine gets that from `capture` for free; the tree-walker
/// has to hold them aside deliberately, and if it did not, one budget would be
/// spent twice over and the two engines would part company at the boundary
/// where handlers and recursion meet.
///
/// Twenty-one calls each side of the `perform`, under a budget of thirty: it
/// fits only if the two halves are not summed.
#[test]
fn a_handler_clause_is_charged_from_below_its_own_handler_on_both_engines() {
    let src = r#"
effect ask { read now[r]() -> Int }
fn down(n: Int) -> Int = if n == 0 { ask.now[r]() } else { down(n - 1) }
fn deep(n: Int) -> Int = if n == 0 { 0 } else { deep(n - 1) }
test "recursion on both sides of a perform" {
  assert_eq(handle { down(20) } with { ask.now[r]() -> deep(20) }, 0)
}
"#;
    let (program, resolved) = load(src);
    Machine::for_program(&program, &resolved)
        .with_max_calls(30)
        .eval_test(0)
        .expect("the machine's capture holds the body's calls aside");
    Interp::for_program(&program, &resolved)
        .with_max_calls(30)
        .eval_test(0)
        .expect("the tree-walker holds them aside the same way");

    // And they are put back: the value returns to the perform site inside the
    // body, whose own calls are pending again.
    for budget in [21, 25] {
        let m = Machine::for_program(&program, &resolved)
            .with_max_calls(budget)
            .eval_test(0);
        let t = Interp::for_program(&program, &resolved)
            .with_max_calls(budget)
            .eval_test(0);
        assert_eq!(
            m.as_ref().err().map(|d| &d.message),
            t.as_ref().err().map(|d| &d.message),
            "budget {budget}"
        );
    }
}

/// The finding with the worse failure mode: a tail-recursive runaway was a
/// diagnostic on the tree-walker in milliseconds and an unbounded loop on the
/// machine, so `--engine both` hung after the authoritative engine had already
/// answered.
///
/// A tail call now costs a `Frame::Call` like any other, so the shared call
/// budget bounds it. The bounds are small here so the property is asserted
/// without running the non-terminating program.
#[test]
fn a_tail_recursive_runaway_is_bounded_on_both_engines() {
    let src = r#"
fn spin(n: Int) -> Int = if n == 0 { 0 } else { spin(n - 1) }
test "ten thousand tail calls" { assert_eq(spin(10000), 0) }
"#;
    let (program, resolved) = load(src);

    let machine = Machine::for_program(&program, &resolved)
        .with_max_calls(64)
        .eval_test(0)
        .expect_err("a tail call is charged like any other call");
    let treewalk = Interp::for_program(&program, &resolved)
        .with_max_calls(64)
        .eval_test(0)
        .expect_err("the tree-walker charges every call, tail or not");

    assert_eq!(machine.message, treewalk.message);
    assert_eq!(machine.notes, treewalk.notes);
    assert!(machine.message.contains("recursion limit"), "{machine:?}");

    // And the runaway itself, which used to be the hang.
    let runaway = r#"
fn spin(n: Int) -> Int = spin(n + 1)
test "no base case in tail position" { assert_eq(spin(0), 0) }
"#;
    agree_and_fail(runaway, ply_span::codes::RUNTIME_ERROR);
}

/// The audit's third axis. ADR 0005 §6 says `--engine both` compares the
/// verdict, the observed footprint *and* the final world; the footprint half
/// was dead code, because neither engine overrode `observed_footprint` and
/// `footprints_compared` was zero on every corpus.
///
/// It is the only axis that sees a `perform` whose result is discarded and
/// whose clause is pure — one that changes no value and no cell.
#[test]
fn both_engines_report_the_footprint_they_observed() {
    let src = r#"
effect state { read get[s]() -> Int }
test "performs one atom" {
  assert_eq(handle { state.get[s]() } with { state.get[s]() -> 1 }, 1)
}
"#;
    let (program, resolved) = load(src);
    let mut treewalk = Interp::for_program(&program, &resolved);
    let mut machine = Machine::for_program(&program, &resolved);
    let report = compare_tests(&mut treewalk, &mut machine, &Fixture::empty());
    assert!(report.is_clean(), "{report}");
    assert_eq!(report.compared, 1, "{report}");
    assert_eq!(report.footprints_compared, 1, "{report}");

    assert_eq!(
        treewalk.trace().footprint().to_string(),
        "{audit.state.read[s]}"
    );
    assert_eq!(
        machine.trace().footprint().to_string(),
        "{audit.state.read[s]}"
    );
    assert_eq!(treewalk.trace().performs(), 1);
    assert_eq!(machine.trace().performs(), 1);
}

/// The axis has to bite, or reporting it is worse than not having it: an
/// engine that performed a different atom, or the same atom a different number
/// of times, must fail the audit even when the value and the world agree.
#[test]
fn a_footprint_divergence_fails_the_audit_on_its_own() {
    let src = r#"
effect state { read get[s]() -> Int  write put[s](v: Int) -> Unit }
test "the result is discarded and the clauses are pure" {
  handle { state.get[s](); () } with { state.get[s]() -> 1, state.put[s](v) -> () }
}
"#;
    let (program, resolved) = load(src);
    let mut left = Interp::for_program(&program, &resolved);
    let mut right = Machine::for_program(&program, &resolved);
    let (l, r) = (left.eval_test(0), right.eval_test(0));
    assert!(l.is_ok() && r.is_ok(), "{l:?} {r:?}");
    assert!(
        ply_eval::compare_outcomes(&left, &right, "t", Some(0), &l, &r).is_none(),
        "the two engines agree on this program"
    );

    // The same test, run twice on one engine: two performs against one, with an
    // identical value and an identical world.
    let twice = r#"
effect state { read get[s]() -> Int }
test "performs twice" {
  handle { state.get[s](); state.get[s](); () } with { state.get[s]() -> 1 }
}
"#;
    let (twice_program, twice_resolved) = load(twice);
    let mut once = Interp::for_program(&program, &resolved);
    let mut doubled = Machine::for_program(&twice_program, &twice_resolved);
    let (a, b) = (once.eval_test(0), doubled.eval_test(0));
    let d = ply_eval::compare_outcomes(&once, &doubled, "t", Some(0), &a, &b)
        .expect("one perform and two are not the same execution");
    assert_eq!(d.detail, ply_eval::Detail::Footprint);
    assert!(d.left.contains("performed 1 time"), "{}", d.left);
    assert!(d.right.contains("performed 2 times"), "{}", d.right);
}

/// `ply check` reports a refused clause before anything runs and the
/// tree-walker reports it on reaching the `handle`. Both are E0504 and both
/// must name the same clause, or a consumer cannot tell one report from two.
///
/// They differ today in the *effect name*: the static walk prints the name as
/// written, the runtime refusal prints the program-wide one. Same code, same
/// span, two spellings of one clause.
#[test]
fn a_refused_clause_is_reported_at_the_same_place_by_both_paths() {
    let src = r#"
effect amb { read flip[coin]() -> Bool }
test "t" {
  handle { if amb.flip[coin]() { 1 } else { 2 } } with { amb.flip[coin]() resume k -> k(true) }
}
"#;
    let (program, resolved) = load(src);
    let at_runtime = Interp::for_program(&program, &resolved)
        .eval_test(0)
        .expect_err("the tree-walker refuses the clause");
    let statically = ply_eval::machine_only_clauses(&program);

    assert_eq!(statically.len(), 1);
    assert_eq!(at_runtime.code, "E0504");
    assert_eq!(statically[0].code, "E0504");
    assert_eq!(at_runtime.primary_span(), statically[0].primary_span());
    assert!(at_runtime.message.ends_with("binds a continuation"));
    assert!(statically[0].message.ends_with("binds a continuation"));
}

// --- `iterate`, the one loop that is depth 1 however long it runs ------------

/// The program both halves of the depth claim are made about: a loop of
/// `n` steps, written once over `iterate` and once as the tail recursion it
/// replaces, so the only difference between the two legs is the driver.
fn loop_and_recursion(n: i64) -> (String, String) {
    let driven = format!(
        "fn step(s: {{ i: Int, acc: Int }}) -> Iter<{{ i: Int, acc: Int }}, Int> =\n  \
         if s.i >= {n} {{ Stop(s.acc) }} else {{ Continue({{i: s.i + 1, acc: s.acc + s.i}}) }}\n\
         test \"a loop of {n} steps\" {{ assert_eq(iterate({{i: 0, acc: 0}}, {}, step), {}) }}\n",
        n + 1,
        n * (n - 1) / 2,
    );
    let recursive = format!(
        "fn walk(i: Int, acc: Int) -> Int = if i >= {n} {{ acc }} else {{ walk(i + 1, acc + i) }}\n\
         test \"a loop of {n} steps\" {{ assert_eq(walk(0, 0), {}) }}\n",
        n * (n - 1) / 2,
    );
    (driven, recursive)
}

/// **The claim the builtin exists for.** `iterate` rides the `Step::Apply`
/// protocol, so the machine pushes one `Frame::IterateStep` and pops it again
/// every round and the tree-walker keeps the loop on the host stack in
/// `Interp::call_builtin`. Neither nests. A budget three orders of magnitude
/// *below* `DEFAULT_MAX_CALLS` therefore runs a loop fifty times *above* it, on
/// both engines, to the same answer.
///
/// The recursive leg is what makes that non-vacuous: the identical loop written
/// as the tail recursion `iterate` replaces must raise at the same cap, or the
/// cap is not measuring depth and the first half proves nothing.
#[test]
fn an_iterate_of_five_hundred_thousand_steps_is_depth_one_on_both_engines() {
    let (driven, recursive) = loop_and_recursion(500_000);

    // The arming leg first, so a cap that turned out to bound nothing fails
    // here rather than passing silently above.
    let (p, r) = load(&recursive);
    for (engine, d) in [
        (
            "machine",
            Machine::for_program(&p, &r).with_max_calls(8).eval_test(0),
        ),
        (
            "treewalk",
            Interp::for_program(&p, &r).with_max_calls(8).eval_test(0),
        ),
    ] {
        let d = d.expect_err("500,000 nested calls do not fit in 8");
        assert!(
            d.message
                .contains("recursion limit of 8 nested calls exceeded"),
            "{engine}: {}",
            d.message
        );
    }

    let (p, r) = load(&driven);
    Machine::for_program(&p, &r)
        .with_max_calls(8)
        .eval_test(0)
        .expect("the machine pushes one frame per round and pops it again");
    Interp::for_program(&p, &r)
        .with_max_calls(8)
        .eval_test(0)
        .expect("the tree-walker drives the same protocol on its host stack");
}

/// The frame count, which the call count does not imply: a machine may be asked
/// for a ceiling on its own pending frames, and a driver that accumulated one
/// frame per round would pass the test above and fail this one.
#[test]
fn an_iterate_of_any_length_fits_under_a_frame_ceiling_that_bounds_nothing_else() {
    let (driven, recursive) = loop_and_recursion(500_000);

    let (p, r) = load(&recursive);
    let d = Machine::for_program(&p, &r)
        .with_max_frames(8)
        .eval_test(0)
        .expect_err("the recursion pends a frame a level and 500,000 do not fit in 8");
    assert!(
        d.message.contains("ceiling of 8 pending frames"),
        "the control must run out of frames, not of something else: {}",
        d.message
    );

    let (p, r) = load(&driven);
    Machine::for_program(&p, &r)
        .with_max_frames(8)
        .eval_test(0)
        .expect("the loop is one frame however many times it goes round");
}

/// A runaway is a diagnostic and not a hang — the property ADR 0005 §7.1
/// removed tail-call elision to keep, and the reason `iterate`'s budget is an
/// argument rather than a constant. There is no per-test timeout anywhere in
/// `ply-test` or `ply-cli`, so an unbounded loop here would hang the suite.
#[test]
fn an_iterate_whose_step_never_stops_is_a_diagnostic_on_both_engines() {
    agree_and_fail(
        r#"
fn never(s: Int) -> Iter<Int, Int> = Continue(s + 1)
test "no Stop" { assert_eq(iterate(0, 5000, never), 0) }
"#,
        ply_span::codes::RUNTIME_ERROR,
    );

    let (p, r) = load(
        r#"
fn never(s: Int) -> Iter<Int, Int> = Continue(s + 1)
test "no Stop" { assert_eq(iterate(0, 5000, never), 0) }
"#,
    );
    let m = Machine::for_program(&p, &r).eval_test(0).unwrap_err();
    let t = Interp::for_program(&p, &r).eval_test(0).unwrap_err();
    assert_eq!(
        m.message, t.message,
        "the two engines phrase it differently"
    );
    assert!(
        m.message
            .contains("`iterate` took its budget of 5000 steps"),
        "{}",
        m.message
    );
    // Not phrased as a recursion limit: nothing nested, and four consumers
    // classify on that string. See `limit::err_iterate_budget`.
    assert!(!m.message.contains("recursion limit"), "{}", m.message);
}

/// A budget that is not a count of steps is refused before the loop starts,
/// identically on both engines.
#[test]
fn an_iterate_budget_below_one_is_refused_by_both_engines() {
    for budget in ["0", "-3"] {
        let src = format!(
            "fn step(s: Int) -> Iter<Int, Int> = Stop(s)\n\
             test \"budget {budget}\" {{ assert_eq(iterate(0, {budget}, step), 0) }}\n"
        );
        agree_and_fail(&src, ply_span::codes::RUNTIME_ERROR);
        let (p, r) = load(&src);
        let m = Machine::for_program(&p, &r).eval_test(0).unwrap_err();
        assert!(
            m.message
                .contains(&format!("`iterate` was given a budget of {budget}")),
            "{}",
            m.message
        );
    }
}

/// The surface `iterate` newly reaches. `cont.rs`'s `MapStep` records that a
/// builtin's callback runs across a frame the tree-walker cannot re-enter, and
/// `iterate`'s step is user code with an open row — so an effect performed
/// inside one is the case that reaches the two engines' handler machinery from
/// a position nothing put it in before. Asserted rather than inferred from the
/// `fold` precedent.
#[test]
fn an_effect_performed_inside_an_iterate_step_agrees_on_both_engines() {
    agree_and_pass(
        r#"
effect tally { write bump[c](n: Int) -> Int }

fn step(s: { i: Int, acc: Int }) -> Iter<{ i: Int, acc: Int }, Int> / {tally.write[c]} =
  if s.i >= 40 {
    Stop(s.acc)
  } else {
    Continue({i: s.i + 1, acc: tally.bump[c](s.acc)})
  }

test "a perform inside an iterate step" {
  assert_eq(handle { iterate({i: 0, acc: 0}, 41, step) }
            with { tally.bump[c](n) -> n + 2 },
            80)
}
"#,
    );
}

/// A record update runs on both engines, and it runs on the *same* path the
/// longhand does — because expansion in the parser makes it the same tree.
///
/// That is the claim worth auditing here. The alternative design, a
/// `RecordUpdate` node each engine evaluates for itself, is two implementations
/// of one construct and two chances to disagree: ADR 0001 rejected `.` for
/// qualified references on exactly that ground. Both engines carry an
/// `unreachable!` for the node instead, guarded by
/// `no_record_update_survives_parse_module_anywhere_in_the_tree`.
#[test]
fn a_record_update_agrees_with_its_longhand_on_both_engines() {
    agree_and_pass(
        r#"
type L = {a: Int, b: Int, c: Int}
type W = {lim: L, n: Int}

fn sugar(s: L) -> L = {..s, b: 20}
fn longhand(s: L) -> L = {a: s.a, c: s.c, b: 20}
fn projected(w: W) -> L = {..w.lim, a: 7}
fn identity(s: L) -> L = {..s}

test "the sugar and the longhand compute one value" {
  let s = {a: 1, b: 2, c: 3};
  assert_eq(sugar(s), longhand(s));
  assert_eq(sugar(s).b, 20);
  assert_eq(sugar(s).a, 1);
  assert_eq(sugar(s).c, 3);
  assert_eq(identity(s), s);
  assert_eq(projected({lim: s, n: 9}), {a: 7, b: 2, c: 3})
}
"#,
    );
}

/// A replacement value may perform, and it performs exactly once — the base is
/// copied field-wise, so an effect in a *written* field is not duplicated by the
/// twelve copies beside it.
#[test]
fn a_replacement_value_performs_once_on_both_engines() {
    agree_and_pass(
        r#"
type L = {a: Int, b: Int, c: Int}
effect counter { write bump[n]() -> Int }

fn go(s: L) -> L = {..s, b: counter.bump[n]()}

test "one bump, not one per field" {
  with_cell[t](0) { c ->
    {
      let s = {a: 1, b: 2, c: 3};
      let r = handle {
        go(s)
      } with {
        counter.bump[n]() -> { cell_set(c, cell_get(c) + 1); 99 }
      };
      // Counted, not assumed: the earlier form of this test handled `bump`
      // without tallying it and pinned neither `r.b` nor the field set, so a
      // replacement value performed twice passed it.
      assert_eq(cell_get(c), 1);
      assert_eq(r, {a: 1, b: 99, c: 3})
    }
  }
}
"#,
    );
}
