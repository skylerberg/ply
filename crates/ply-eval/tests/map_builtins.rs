//! The `Map` builtins on real source, through parse, resolve, check and both
//! engines.
//!
//! The engines are run against each other deliberately: `--engine both`
//! reporting `E0503` on correct code is one of the four failures a
//! non-canonical iteration order would produce, so a map that iterated
//! differently under the machine than under the tree-walker would be caught
//! here rather than in a user's cache.

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

#[test]
fn the_builtins_behave_as_the_contract_states() {
    let source = r#"
fn base() -> Map<String, Int> =
  map_insert(map_insert(map_insert(map_new(), "b", 2), "a", 1), "c", 3)

test "construction, insert and size" {
  assert_eq(map_len(map_new()), 0);
  assert_eq(map_len(base()), 3);
  assert_eq(map_len(map_insert(base(), "a", 99)), 3)
}

test "keys, values and entries all iterate ascending" {
  assert_eq(map_keys(base()), ["a", "b", "c"]);
  assert_eq(map_values(base()), [1, 2, 3]);
  assert_eq(
    map_entries(base()),
    [{key: "a", value: 1}, {key: "b", value: 2}, {key: "c", value: 3}]
  )
}

test "entries round-trip through a map" {
  assert_eq(map_of_entries(map_entries(base())), base())
}

test "a later entry wins, matching a fold of map_insert" {
  assert_eq(
    map_of_entries([{key: "a", value: 1}, {key: "a", value: 9}]),
    map_insert(map_new(), "a", 9)
  )
}

test "lookup answers an Option" {
  assert_eq(map_get(base(), "a"), Some(1));
  assert_eq(map_get(base(), "z"), None);
  assert(map_contains(base(), "b"));
  assert(!map_contains(base(), "z"))
}

test "removing an absent key is a no-op" {
  assert_eq(map_remove(base(), "z"), base());
  assert_eq(map_len(map_remove(base(), "a")), 2);
  assert_eq(map_keys(map_remove(base(), "b")), ["a", "c"])
}

test "the right side wins a shared key in a merge" {
  let left = map_insert(map_insert(map_new(), "a", 1), "b", 2);
  let right = map_insert(map_insert(map_new(), "b", 20), "c", 30);
  assert_eq(map_keys(map_merge(left, right)), ["a", "b", "c"]);
  assert_eq(map_values(map_merge(left, right)), [1, 20, 30]);
  assert_eq(map_values(map_merge(right, left)), [1, 2, 30])
}

test "fold visits entries in ascending key order" {
  assert_eq(map_fold(base(), "", |acc, k, v| acc ++ k ++ int_to_string(v)), "a1b2c3");
  assert_eq(map_fold(base(), 0, |acc, k, v| acc + v), 6)
}

test "insertion order does not change the value" {
  let forward = map_insert(map_insert(map_new(), "x", 1), "y", 2);
  let backward = map_insert(map_insert(map_new(), "y", 2), "x", 1);
  assert_eq(forward, backward)
}

test "a map nests, as a key and as a value" {
  let inner = map_insert(map_new(), 1, 1);
  let outer = map_insert(map_new(), inner, "in");
  assert_eq(map_get(outer, map_insert(map_new(), 1, 1)), Some("in"))
}
"#;
    run_both(source);
}

/// Pattern matching. There is no map *pattern* in W2 — a literal would be sugar
/// over `map_of_entries` and buys no semantics — so what has to work is that a
/// map flows through the patterns the language does have: the `Option` a lookup
/// answers, the `{key, value}` record an entry is, and a binder over a map
/// itself.
#[test]
fn a_map_flows_through_the_patterns_the_language_has() {
    let source = r#"
fn base() -> Map<String, Int> = map_insert(map_insert(map_new(), "a", 1), "b", 2)

fn lookup(m: Map<String, Int>, k: String) -> Int =
  match map_get(m, k) {
    Some(v) -> v,
    None -> 0,
  }

fn first_key(m: Map<String, Int>) -> String =
  match map_entries(m) {
    [{key: k, value: _}, ..rest] -> k,
    _ -> "",
  }

fn size(m: Map<String, Int>) -> Int = match m { other -> map_len(other) }

test "an Option from a lookup destructures" {
  assert_eq(lookup(base(), "a"), 1);
  assert_eq(lookup(base(), "z"), 0)
}

test "an entry is a record and destructures as one" {
  assert_eq(first_key(base()), "a");
  assert_eq(first_key(map_new()), "")
}

test "a map binds to a variable pattern" {
  assert_eq(size(base()), 2)
}
"#;
    run_both(source);
}

/// `map_fold` calls user code, so its loop is a frame rather than host
/// recursion. A fold over ten thousand entries must therefore neither overflow
/// the host stack nor lose an entry.
#[test]
fn a_long_fold_runs_on_frames_rather_than_on_the_host_stack() {
    let source = r#"
fn build(n: Int) -> Map<Int, Int> =
  fold(range(0, n), map_new(), |m, i| map_insert(m, i, i))

test "a ten-thousand entry fold is exact" {
  assert_eq(map_fold(build(10000), 0, |acc, k, v| acc + v), 49995000);
  assert_eq(map_len(build(10000)), 10000)
}
"#;
    run_both(source);
}

/// A `map_fold` whose function performs an effect threads its row, which is the
/// one thing about `map_fold` that is not pure.
#[test]
fn a_fold_threads_its_functions_row() {
    let source = r#"
effect log {
  write note[out](line: String) -> Unit
}

fn shout(m: Map<String, Int>) -> Int / {log.write[out]} =
  map_fold(m, 0, |acc, k, v| { log.note[out](k); acc + v })

test "the atom the folded function performs is in the row" {
  handle {
    assert_eq(shout(map_insert(map_insert(map_new(), "a", 1), "b", 2)), 3)
  } with {
    log.note[out](line) -> (),
  }
}
"#;
    let c = run_both(source);
    assert_eq!(
        c.check.defs[&ply_span::Symbol::new("m.shout")]
            .footprint
            .to_string(),
        "{m.log.write[out]}"
    );
}
