//! The eight fixed-width integer types, at the checker: what pins a literal, what refuses to
//! widen, and what the conversions publish.

use crate::fixture::compile;
use ply_core::{CheckOutput, print_type};
use ply_span::{Diagnostic, Symbol, codes};

fn ok(source: &str) -> CheckOutput {
    match compile(source) {
        Ok(out) => out,
        Err(d) => panic!("expected this to check:\n{source}\ngot {d:#?}"),
    }
}

fn errors(source: &str) -> Vec<Diagnostic> {
    match compile(source) {
        Ok(_) => panic!("expected a diagnostic from:\n{source}"),
        Err(d) => d,
    }
}

fn code(source: &str) -> &'static str {
    errors(source)[0].code
}

fn sig(out: &CheckOutput, name: &str) -> String {
    print_type(&out.defs[&Symbol::new(format!("m.{name}"))].scheme.ty)
}

/// All eight exist, and arithmetic at each answers that type rather than `Int`.
#[test]
fn arithmetic_is_defined_at_each_of_the_eight() {
    let out = ok(r#"
fn a(x: U8, y: U8) -> U8 = x + y
fn b(x: U16, y: U16) -> U16 = x - y
fn c(x: U32, y: U32) -> U32 = x * y
fn d(x: U64, y: U64) -> U64 = x / y
fn e(x: I8, y: I8) -> I8 = x % y
fn f(x: I16, y: I16) -> I16 = x + y
fn g(x: I32, y: I32) -> I32 = x - y
fn h(x: I64, y: I64) -> I64 = x * y
"#);
    assert_eq!(sig(&out, "a"), "(U8, U8) -> U8");
    assert_eq!(sig(&out, "d"), "(U64, U64) -> U64");
    assert_eq!(sig(&out, "h"), "(I64, I64) -> I64");
}

/// The whole point of the family: a value can say it is thirty-two bits wide.
#[test]
fn a_record_field_can_be_a_fixed_width_type() {
    let out = ok(r#"
type Word = { hi: U32, lo: U32 }
fn combine(w: Word) -> U32 = w.hi ^ w.lo
"#);
    assert_eq!(sig(&out, "combine"), "({hi: U32, lo: U32}) -> U32");
}

/// A literal's type is its spelling, which is the decision §5.2 already took for `Decimal`: `1`,
/// `1.0`, `1m` and `1u32` are four literals with four types.
#[test]
fn a_literal_carries_its_width_as_a_suffix() {
    let out = ok(r#"
fn a() -> U8 = 255u8
fn b() -> U32 = 0x6A09_E667u32
fn c() -> I8 = -127i8
fn d(x: U16) -> U16 = x + 1u16
fn e() -> Int = 5
"#);
    assert_eq!(sig(&out, "a"), "() -> U8");
    assert_eq!(sig(&out, "b"), "() -> U32");
    assert_eq!(sig(&out, "c"), "() -> I8");
    assert_eq!(sig(&out, "d"), "(U16) -> U16");
    // An unsuffixed literal is an `Int` and stays one; nothing widens it.
    assert_eq!(sig(&out, "e"), "() -> Int");
    assert_eq!(code("fn f(x: U16) -> U16 = x + 1\n"), codes::TYPE_MISMATCH);
}

/// The lexer bounds a suffixed literal by its type, so nothing downstream has to.
#[test]
fn a_literal_outside_its_type_is_refused() {
    assert_eq!(code("fn f() -> U8 = 256u8\n"), codes::LITERAL_OUT_OF_RANGE);
    assert_eq!(code("fn f() -> I8 = 128i8\n"), codes::LITERAL_OUT_OF_RANGE);
    assert_eq!(
        code("fn f() -> U16 = 65536u16\n"),
        codes::LITERAL_OUT_OF_RANGE
    );
    ok("fn f() -> U8 = 255u8\n");
    ok("fn f() -> I8 = 127i8\n");
    // A hex literal is a bit pattern, so its bound is the width: the largest `U64` is written as
    // one, and `0xFFu8` is 255 rather than a refusal.
    ok("fn f() -> U64 = 0xFFFF_FFFF_FFFF_FFFFu64\n");
    ok("fn f() -> U8 = 0xFFu8\n");
    assert_eq!(
        code("fn f() -> U8 = 0x100u8\n"),
        codes::LITERAL_OUT_OF_RANGE
    );
}

/// No numeric tower and no implicit widening — the rule `Int`, `Float` and `Decimal` already
/// keep, extended over eight more types rather than relaxed for them.
#[test]
fn nothing_widens_implicitly() {
    assert_eq!(
        code("fn f(a: U32, b: Int) -> Int = a + b\n"),
        codes::TYPE_MISMATCH
    );
    assert_eq!(
        code("fn f(a: U8, b: U16) -> U16 = a + b\n"),
        codes::TYPE_MISMATCH
    );
    assert_eq!(code("fn f(a: U32) -> Int = a\n"), codes::TYPE_MISMATCH);
    assert_eq!(
        code("fn f(a: U32, b: I32) -> Bool = a < b\n"),
        codes::TYPE_MISMATCH
    );
}

/// Sixteen conversions, and the widths reach each other only through `Int`.
#[test]
fn the_conversions_publish_what_they_promise() {
    let out = ok(r#"
fn narrow(n: Int) -> U32 = u32_of_int(n)
fn widen(w: U32) -> Int = int_of_u32(w)
fn across(b: U8) -> U32 = u32_of_int(int_of_u8(b))
fn every(n: Int) -> I16 = i16_of_int(n)
"#);
    assert_eq!(sig(&out, "narrow"), "(Int) -> U32");
    assert_eq!(sig(&out, "widen"), "(U32) -> Int");
    assert_eq!(sig(&out, "across"), "(U8) -> U32");
    assert_eq!(sig(&out, "every"), "(Int) -> I16");
    assert_eq!(
        code("fn f(w: U32) -> U8 = u8_of_int(w)\n"),
        codes::TYPE_MISMATCH
    );
}

/// The bit operators answer their operands' type, and a shift's count is an `Int` whatever the
/// word is — a count is not a word.
#[test]
fn the_bit_operators_are_defined_at_every_integer_type() {
    let out = ok(r#"
fn and(a: U32, b: U32) -> U32 = a & b
fn xor(a: U8, b: U8) -> U8 = a ^ b
fn not(a: U64) -> U64 = ~a
fn shl(a: U32, n: Int) -> U32 = a << n
fn ushr(a: I32, n: Int) -> I32 = a >>> n
"#);
    assert_eq!(sig(&out, "and"), "(U32, U32) -> U32");
    assert_eq!(sig(&out, "not"), "(U64) -> U64");
    assert_eq!(sig(&out, "shl"), "(U32, Int) -> U32");
    assert_eq!(
        code("fn f(a: U32, n: U32) -> U32 = a << n\n"),
        codes::TYPE_MISMATCH
    );
}

/// The wrapping builtins are how a program says it meant the wrap, at every integer type.
#[test]
fn the_wrapping_builtins_and_rotr_are_defined_at_every_integer_type() {
    let out = ok(r#"
fn a(x: U32, y: U32) -> U32 = wrap_add(x, y)
fn b(x: U8, y: U8) -> U8 = wrap_mul(x, y)
fn c(x: Int, y: Int) -> Int = wrap_sub(x, y)
fn d(x: U32) -> U32 = rotr(x, 16)
fn e(x: I64) -> I64 = rotr(x, 3)
"#);
    assert_eq!(sig(&out, "a"), "(U32, U32) -> U32");
    assert_eq!(sig(&out, "c"), "(Int, Int) -> Int");
    assert_eq!(sig(&out, "d"), "(U32) -> U32");
    // `forall a. (a, a) -> a` alone would take two `String`s; the obligation at the call is what
    // refuses them.
    assert_eq!(
        code("fn f(a: String, b: String) -> String = wrap_add(a, b)\n"),
        codes::TYPE_MISMATCH
    );
    assert_eq!(
        code("fn f(a: Float, b: Float) -> Float = wrap_mul(a, b)\n"),
        codes::TYPE_MISMATCH
    );
}

/// They order and compare, so they are map keys and `derive` leaves.
#[test]
fn a_fixed_width_type_is_a_map_key_and_a_derivable_leaf() {
    let out = ok(r#"
fn lookup(m: Map<U32, String>, k: U32) -> Option<String> = map_get(m, k)
"#);
    assert_eq!(
        sig(&out, "lookup"),
        "(Map<U32, String>, U32) -> Option<String>"
    );
}
