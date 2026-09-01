//! The bit surface on real source, through parse, resolve, check and both engines.

use ply_core::{CheckOutput, check_program};
use ply_eval::Machine;
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

fn run(source: &str) -> Compiled {
    let c = compile(source);
    assert!(!c.check.tests.is_empty(), "the source declares no test");
    let mut machine = Machine::new(&c.program, &c.resolved, &c.check);
    for (i, t) in c.check.tests.iter().enumerate() {
        if let Err(d) = machine.eval_test(i) {
            panic!("`{}` failed: {d:#?}", t.name);
        }
    }
    c
}

/// `name` raises, with the code a program-level failure carries.
#[track_caller]
fn refused(c: &Compiled, name: &str) -> String {
    let mut machine = Machine::new(&c.program, &c.resolved, &c.check);
    let raised = machine
        .call(name, Vec::new(), ply_span::Span::DUMMY)
        .expect_err(name);
    assert_eq!(raised.code, ply_span::codes::RUNTIME_ERROR, "{name}");
    raised.message
}

/// `~`, `&`, `|` and `^` over the two's-complement pattern, with the identities that only hold if
/// the pattern is what is being read.
#[test]
fn every_bit_operator_answers_the_two_s_complement_pattern() {
    run(r#"
test "and, or and xor at small values" {
  assert_eq(12 & 10, 8);
  assert_eq(12 | 10, 14);
  assert_eq(12 ^ 10, 6);
  assert_eq(0 & 255, 0);
  assert_eq(0 | 255, 255)
}

test "not is every bit flipped, so ~x is -x - 1" {
  assert_eq(~0, 0 - 1);
  assert_eq(~5, 0 - 6);
  assert_eq(~(0 - 1), 0);
  assert_eq(~~7, 7)
}

// De Morgan holds bit for bit, which it cannot if either operator is reading something other than
// the pattern.
test "de morgan over the whole word" {
  assert_eq(~(12 & 10), ~12 | ~10);
  assert_eq(~(12 | 10), ~12 & ~10);
  assert_eq(12 ^ 10, (12 | 10) & ~(12 & 10))
}

test "the sign bit is an ordinary bit" {
  assert_eq((0 - 1) & 255, 255);
  assert(((0 - 1) | 0) < 0);
  assert_eq((0 - 1) ^ (0 - 1), 0)
}

test "the operators are not their logical namesakes: both sides are evaluated" {
  with_cell[n](0) { n -> {
    let r = (0 & { cell_set(n, cell_get(n) + 1); 5 });
    assert_eq(r, 0);
    assert_eq(cell_get(n), 1)
  } }
}
"#);
}

/// The deliberate exception to checked arithmetic, and the only one.
#[test]
fn a_left_shift_discards_what_leaves_and_does_not_raise() {
    run(r#"
fn min_int() -> Int = 0 - 9223372036854775807 - 1

test "1 << 63 is the sign bit and raises nothing" {
  assert(1 << 63 < 0);
  assert_eq(1 << 63, min_int());
  assert_eq((1 << 63) >>> 63, 1);
  assert_eq((1 << 63) >> 63, 0 - 1)
}

// `*` raises on exactly this input, which is the whole content of "deliberate exception".
test "a shift that overflows is not a multiplication that overflows" {
  assert_eq(4611686018427387904 << 1, min_int());
  assert_eq(9223372036854775807 << 1, 0 - 2);
  assert_eq(1 << 62, 4611686018427387904)
}

test "shifting a negative left keeps discarding" {
  assert_eq((0 - 1) << 63, min_int());
  assert_eq((0 - 1) << 1, 0 - 2)
}

test "a count of zero is a shift of nothing" {
  assert_eq(7 << 0, 7);
  assert_eq(7 >> 0, 7);
  assert_eq((0 - 7) >>> 0, 0 - 7)
}
"#);
}

/// Both shifts are in the language because `Int` is signed, and the pair only earns its keep if
/// they differ.
#[test]
fn an_arithmetic_shift_and_a_logical_shift_differ_on_a_negative() {
    run(r#"
test "arithmetic propagates the sign" {
  assert_eq(-8 >> 1, 0 - 4);
  assert_eq(-8 >> 3, 0 - 1);
  assert_eq(-1 >> 63, 0 - 1);
  assert_eq(8 >> 1, 4)
}

test "logical fills with zeros over the same 64 bits" {
  assert_eq(-8 >>> 1, 9223372036854775804);
  assert(-8 >>> 1 > 0);
  assert_eq(-1 >>> 63, 1);
  assert_eq(-1 >>> 1, 9223372036854775807);
  assert_eq(8 >>> 1, 4)
}

// The two agree on every non-negative value and on no negative one, which is the whole of the
// difference.
test "they agree exactly where the sign bit is clear" {
  assert_eq(map(range(0, 20), |x| x >> 2), map(range(0, 20), |x| x >>> 2));
  assert(-4 >> 2 != -4 >>> 2)
}
"#);
}

/// The count is refused rather than masked.
#[test]
fn a_shift_count_outside_the_word_raises_on_both_engines() {
    let c = compile(
        r#"
fn at_minus_one() -> Int = 1 << -1
fn at_sixty_four() -> Int = 1 << 64
fn arithmetic_at_sixty_four() -> Int = 1 >> 64
fn logical_at_minus_one() -> Int = 1 >>> -1
fn at_sixty_three() -> Int = 1 << 63

test "the source checks" { assert(true) }
"#,
    );
    for (name, count) in [
        ("m.at_minus_one", "-1"),
        ("m.at_sixty_four", "64"),
        ("m.arithmetic_at_sixty_four", "64"),
        ("m.logical_at_minus_one", "-1"),
    ] {
        let mut machine = Machine::new(&c.program, &c.resolved, &c.check);
        let d = machine
            .call(name, Vec::new(), ply_span::Span::DUMMY)
            .expect_err(name);
        assert_eq!(d.message, "shift count out of range", "{name}");
        assert!(
            d.labels.iter().any(|l| l.message.contains(count)),
            "the refusal must name the count it refused: {d:#?}"
        );
    }
    // 63 is inside the word, and a bound written `0..=64` would still refuse nothing here —
    // `at_sixty_four` is what catches that one.
    let mut machine = Machine::new(&c.program, &c.resolved, &c.check);
    machine
        .call("m.at_sixty_three", Vec::new(), ply_span::Span::DUMMY)
        .expect("63 is a shift of an `Int`");
}

/// `wrap_*` is what a program calls when the wrap is the point, and `+` stays the raising spelling.
#[test]
fn the_wrapping_builtins_answer_where_the_operators_raise() {
    run(r#"
fn max_int() -> Int = 9223372036854775807
fn min_int() -> Int = 0 - 9223372036854775807 - 1

test "wrap_add carries past the top and comes back at the bottom" {
  assert_eq(wrap_add(max_int(), 1), min_int());
  assert_eq(wrap_add(max_int(), 2), min_int() + 1);
  assert_eq(wrap_add(1, 2), 3);
  assert_eq(wrap_add(max_int(), min_int()), 0 - 1)
}

test "wrap_sub goes the other way" {
  assert_eq(wrap_sub(min_int(), 1), max_int());
  assert_eq(wrap_sub(3, 5), 0 - 2);
  assert_eq(wrap_sub(0, min_int()), min_int())
}

test "wrap_mul wraps where `*` raises" {
  assert_eq(wrap_mul(max_int(), 2), 0 - 2);
  assert_eq(wrap_mul(min_int(), 2), 0);
  assert_eq(wrap_mul(6, 7), 42);
  assert_eq(wrap_mul(4294967296, 4294967296), 0)
}

// Wrapping is a total function of the pattern, so it agrees with the masked arithmetic a 32-bit
// mixing step uses.
test "wrapping agrees with masking below 32 bits" {
  assert_eq(wrap_add(4294967295, 1) & 4294967295, 0);
  assert_eq(wrap_mul(65537, 65537) & 4294967295, 131073)
}
"#);

    // The same three expressions written with the operators, which raise.
    let c = compile(
        r#"
fn add() -> Int = 9223372036854775807 + 1
fn sub() -> Int = (0 - 9223372036854775807 - 1) - 1
fn mul() -> Int = 9223372036854775807 * 2

test "the source checks" { assert(true) }
"#,
    );
    for name in ["m.add", "m.sub", "m.mul"] {
        let message = refused(&c, name);
        assert!(message.contains("integer overflow"), "`{name}`: {message}");
    }
}

/// A hash's mixing step, run on both engines, which is what the whole surface exists for.
#[test]
fn a_mixing_step_answers_the_same_number_on_both_engines() {
    run(r#"
fn mask32() -> Int = 4294967295

fn rotl32(x: Int, n: Int) -> Int =
  ((x << n) | (x >>> (32 - n))) & mask32()

fn mix(h: Int, b: Int) -> Int =
  rotl32(wrap_mul(h ^ b, 16777619) & mask32(), 13)

fn digest(bs: Bytes) -> Int =
  fold(range(0, bytes_len(bs)), 2166136261, |h, i| mix(h, bytes_at(bs, i)))

// The number itself, so a mixing step that changed would be caught rather than only a divergence.
test "the step answers the number it answered" {
  assert_eq(digest(b""), 2166136261);
  assert_eq(digest(b"the quick brown fox"), 1059872060);
  assert_eq(digest(b"the quick brown fux"), 1907644151);
  assert(digest(b"the quick brown fox") <= mask32())
}

test "rotation is a permutation of the low 32 bits" {
  assert_eq(rotl32(rotl32(1, 13), 19), 1);
  assert_eq(rotl32(mask32(), 7), mask32());
  assert_eq(rotl32(1, 31), 2147483648)
}
"#);
}

/// The names are not reserved, and a module that defines one gets its own — on both engines, which
/// is where the two `lookup` orders could disagree.
#[test]
fn a_module_definition_shadows_a_wrapping_builtin() {
    run(r#"
fn wrap_add(a: Int, b: Int) -> Int = 0

test "the module's own definition wins" { assert_eq(wrap_add(1, 2), 0) }
"#);
}
