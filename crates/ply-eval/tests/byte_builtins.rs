//! The byte builtins on real source, through parse, resolve, check and both
//! engines.
//!
//! Unit tests in `ply-eval` drive `builtins::call` directly, which is where the
//! edge cases belong. This is the other half: the same builtins reached the way
//! a program reaches them, with `Option` coming from the prelude and the two
//! engines run against each other — `--engine both` on a corpus using every new
//! builtin is what ADR 0012's required test 40 asks for, and a `bytes_position`
//! whose frame the machine handled differently from the tree-walker's host loop
//! would be caught here rather than in a user's cache.

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
    let program = match ply_syntax::parse_program(inputs) {
        Ok(p) => p,
        Err(d) => panic!("did not parse: {d:#?}"),
    };
    let resolved = match resolve(&program) {
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
fn every_byte_builtin_behaves_as_the_contract_states() {
    run_both(
        r#"
fn head() -> Bytes = b"GET /orders?id=7 HTTP/1.1\r\nHost: x\r\n\r\nbody"

test "index_of finds the first occurrence and says so with an Option" {
  assert_eq(bytes_index_of(head(), b"\r\n\r\n"), Some(34));
  assert_eq(bytes_index_of(head(), b"nowhere"), None);
  assert_eq(bytes_index_of(head(), b""), Some(0));
  assert_eq(bytes_index_of(b"", b""), Some(0));
  assert_eq(bytes_index_of(b"", b"a"), None)
}

test "index_of_from resumes a search and answers an absolute index" {
  assert_eq(bytes_index_of_from(head(), b"\r\n", 0), Some(25));
  assert_eq(bytes_index_of_from(head(), b"\r\n", 26), Some(34));
  assert_eq(bytes_index_of_from(head(), b"\r\n", 38), None);
  assert_eq(bytes_index_of_from(head(), b"", bytes_len(head())), Some(42))
}

test "index_of_byte takes a byte and finds it" {
  assert_eq(bytes_index_of_byte(head(), 63), Some(11));
  assert_eq(bytes_index_of_byte(head(), 255), None)
}

test "starts_with and ends_with exit at the first mismatch" {
  assert(bytes_starts_with(head(), b"GET "));
  assert(bytes_starts_with(head(), b""));
  assert(!bytes_starts_with(head(), b"POST"));
  assert(bytes_ends_with(head(), b"body"));
  assert(!bytes_ends_with(head(), b"HEAD"))
}

test "split keeps the empty pieces a join needs" {
  assert_eq(bytes_split(b"a,b,c", b","), [b"a", b"b", b"c"]);
  assert_eq(bytes_split(b"", b","), [b""]);
  assert_eq(bytes_split(b",a,", b","), [b"", b"a", b""]);
  assert_eq(bytes_split(b"abc", b","), [b"abc"])
}

test "a scan stops off its class and scan_until stops on it" {
  assert_eq(bytes_scan_until(head(), 0, b" ", 25), 3);
  assert_eq(bytes_scan(head(), 4, b"/ordes?=i", 25), 15);
  assert_eq(bytes_scan(head(), 15, b"0123456789", 25), 16)
}

// The budget is the whole reason `max` is an argument: a caller tells a class
// ending from a bound running out by comparing against `from + max`, and
// neither answer is a sentinel that a careless program hands to `bytes_slice`.
test "a scan that spends its budget answers the budget" {
  assert_eq(bytes_scan_until(head(), 0, b"y", 4), 4);
  assert_eq(bytes_scan_until(head(), 0, b"y", 100), 41);
  assert_eq(bytes_scan_until(head(), 0, b"z", 100), bytes_len(head()))
}

test "position is the escape hatch and stops at the first byte it accepts" {
  assert_eq(bytes_position(head(), 0, |b| b == 63), Some(11));
  assert_eq(bytes_position(head(), 12, |b| b == 63), None);
  assert_eq(bytes_position(b"", 0, |b| true), None)
}

// A predicate that performs is why this is a frame rather than host recursion,
// and the counter proves the search stopped rather than ran to the end.
test "a position predicate may perform, and the search still exits early" {
  with_cell[calls](0) { calls -> {
    let at = handle {
      bytes_position(head(), 0, |b| {
        cell_set(calls, cell_get(calls) + 1);
        b == 32
      })
    } with {
      return x -> x,
    };
    assert_eq(at, Some(3));
    assert_eq(cell_get(calls), 4)
  } }
}
"#,
    );
}

/// The claim the milestone rests on, checked rather than asserted: the native
/// searches answer what W1's folds answered. The folds are kept here verbatim
/// so the comparison is against the code that was replaced, not against a
/// paraphrase of it.
#[test]
fn the_builtins_agree_with_the_folds_they_replaced() {
    run_both(
        r#"
fn fold_index_of(hay: Bytes, byte: Int, from: Int) -> Int =
  fold(range(from, bytes_len(hay)), -1, |found, i|
    if found >= 0 { found } else if bytes_at(hay, i) == byte { i } else { found })

fn fold_head_end(head: Bytes) -> Int =
  fold(range(0, bytes_len(head) - 3), -1, |found, i|
    if found >= 0 {
      found
    } else if bytes_at(head, i) == 13 && bytes_at(head, i + 1) == 10
           && bytes_at(head, i + 2) == 13 && bytes_at(head, i + 3) == 10 {
      i + 4
    } else {
      found
    })

fn fold_all_upper(b: Bytes) -> Bool =
  fold(range(0, bytes_len(b)), true, |ok, i|
    ok && bytes_at(b, i) >= 65 && bytes_at(b, i) <= 90)

// The same question the fold answered, in the sentinel the fold used, so the
// two are comparable. A caller that wanted an `Option` would write
// `bytes_index_of_byte` and never see a `-1` at all.
fn native_index_of(hay: Bytes, set: Bytes, from: Int) -> Int =
  if from > bytes_len(hay) {
    -1
  } else {
    let at = bytes_scan_until(hay, from, set, bytes_len(hay));
    if at >= bytes_len(hay) { -1 } else { at }
  }

fn native_head_end(head: Bytes) -> Int =
  match bytes_index_of(head, b"\r\n\r\n") {
    Some(at) -> at + 4,
    None -> -1,
  }

fn native_all_upper(b: Bytes) -> Bool =
  bytes_scan(b, 0, b"ABCDEFGHIJKLMNOPQRSTUVWXYZ", bytes_len(b)) == bytes_len(b)

fn heads() -> List<Bytes> = [
  b"",
  b"\r\n\r\n",
  b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n",
  b"POST /orders HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello",
  b"GET / HTTP/1.1\r\nHost: x\r\n",
  b"GET / HTTP/1.1\r\n\r\nGET / HTTP/1.1\r\n\r\n",
  b"\r\n",
  b"\r",
  b"GET /\xff\xfe HTTP/1.1\r\n\r\n",
]

fn methods() -> List<Bytes> = [b"", b"GET", b"get", b"GEt", b"P0ST", b"OPTIONS"]

test "the native searches answer what the folds answered" {
  assert_eq(
    map(heads(), |h| native_head_end(h)),
    map(heads(), |h| fold_head_end(h)));
  assert_eq(
    map(heads(), |h| native_index_of(h, b"\n", 0)),
    map(heads(), |h| fold_index_of(h, 10, 0)));
  assert_eq(
    map(heads(), |h| native_index_of(h, b" ", 4)),
    map(heads(), |h| fold_index_of(h, 32, 4)));
  assert_eq(
    map(methods(), |m| native_all_upper(m)),
    map(methods(), |m| fold_all_upper(m)))
}
"#,
    );
}

/// Out of range is refused rather than clamped, following `bytes_slice`. A
/// clamp turns an off-by-one into a shorter answer that every later assertion
/// agrees with, which is the silent-wrong-answer shape this project refuses.
#[test]
fn a_position_outside_the_buffer_is_a_diagnostic_under_both_engines() {
    let source = r#"
fn past_the_end() -> Int = bytes_scan(b"abc", 9, b"a", 1)
fn negative_budget() -> Int = bytes_scan(b"abc", 0, b"a", -1)
fn not_a_byte() -> Option<Int> = bytes_index_of_byte(b"abc", 256)
fn empty_separator() -> List<Bytes> = bytes_split(b"abc", b"")

test "the source checks" { assert(true) }
"#;
    let c = compile(source);
    for name in [
        "m.past_the_end",
        "m.negative_budget",
        "m.not_a_byte",
        "m.empty_separator",
    ] {
        let mut interp = Interp::new(&c.program, &c.resolved, &c.check);
        let walked = interp
            .call(name, Vec::new(), ply_span::Span::DUMMY)
            .expect_err(name);
        let mut machine = Machine::new(&c.program, &c.resolved, &c.check);
        let stepped = machine
            .call(name, Vec::new(), ply_span::Span::DUMMY)
            .expect_err(name);
        assert_eq!(walked.code, ply_span::codes::RUNTIME_ERROR, "{name}");
        assert_eq!(
            walked.message, stepped.message,
            "the two engines phrase `{name}` differently, which `--engine both` reports as a \
             divergence on correct code"
        );
    }
}
