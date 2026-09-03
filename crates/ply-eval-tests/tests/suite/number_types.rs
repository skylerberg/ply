//! The eight fixed-width integer types, evaluated: what wraps, what raises, and what the
//! conversions answer.

use crate::fixture::Compiled;
use ply_eval::Machine;

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

/// Arithmetic at a fixed width keeps `Int`'s rule: exact, or it raises. Nothing wraps silently.
#[test]
fn arithmetic_is_checked_at_every_width() {
    let c = Compiled::ran(
        r#"
test "arithmetic answers at the type's own width" {
  assert_eq(200u8 + 55u8, 255u8);
  assert_eq(1u32 - 1u32, 0u32);
  assert_eq(i8_of_int(-128) / 2i8, -64i8);
  assert_eq(300u16 % 256u16, 44u16)
}

pub fn over_u8() -> U8 = 200u8 + 56u8
pub fn under_u8() -> U8 = 0u8 - 1u8
pub fn over_i8() -> I8 = 127i8 + 1i8
pub fn negate_unsigned() -> U8 = -1u8
pub fn divide_by_zero() -> U32 = 1u32 / 0u32
"#,
    );
    assert!(refused(&c, "m.over_u8").contains("overflow in addition"));
    assert!(refused(&c, "m.under_u8").contains("overflow in subtraction"));
    assert!(refused(&c, "m.over_i8").contains("overflow in addition"));
    // At an unsigned type every negation but zero's overflows, which is what the type says.
    assert!(refused(&c, "m.negate_unsigned").contains("overflow in negation"));
    assert!(refused(&c, "m.divide_by_zero").contains("division by zero"));
}

/// The wrapping builtins are total by construction, and wrap at the operand's width rather than
/// at sixty-four bits.
#[test]
fn the_wrapping_builtins_wrap_at_the_operands_width() {
    Compiled::ran(
        r#"
test "wrap_add wraps where the type ends" {
  assert_eq(wrap_add(255u8, 1u8), 0u8);
  assert_eq(wrap_add(4294967295u32, 2u32), 1u32);
  assert_eq(wrap_sub(0u8, 1u8), 255u8);
  assert_eq(wrap_mul(16u8, 16u8), 0u8);
  assert_eq(wrap_add(127i8, 1i8), i8_of_int(-128))
}

test "the Int spelling is unchanged" {
  assert_eq(wrap_add(1, 2), 3)
}
"#,
    );
}

/// `rotr` turns the whole word, at whatever width the word is.
#[test]
fn rotr_turns_the_word_at_its_own_width() {
    Compiled::ran(
        r#"
test "a rotation at each width" {
  assert_eq(rotr(1u8, 1), 128u8);
  assert_eq(rotr(1u32, 1), 2147483648u32);
  assert_eq(rotr(16u32, 4), 1u32);
  assert_eq(rotr(0xABu8, 8), 0xABu8)
}

test "a rotation is its own inverse over the width" {
  assert_eq(rotr(rotr(0xDEAD_BEEFu32, 12), 20), 0xDEAD_BEEFu32)
}
"#,
    );
}

/// Bit operators read the type's own pattern: `~` at `U8` is the complement in eight bits, not in
/// sixty-four, and a shift's bound is the word's width.
#[test]
fn the_bit_operators_read_the_types_own_pattern() {
    let c = Compiled::ran(
        r#"
test "the pattern is the type's" {
  assert_eq(~0u8, 255u8);
  assert_eq(~0u32, 4294967295u32);
  assert_eq(~0i8, -1i8);
  assert_eq(0xF0u8 & 0x3Cu8, 0x30u8);
  assert_eq(1u8 << 7, 128u8);
  assert_eq(128u8 << 1, 0u8)
}

test "the two right shifts differ exactly where the type is signed" {
  assert_eq(-2i8 >> 1, -1i8);
  assert_eq(-2i8 >>> 1, 127i8);
  assert_eq(254u8 >> 1, 127u8)
}

pub fn past_the_width() -> U8 = 1u8 << 8
"#,
    );
    let message = refused(&c, "m.past_the_width");
    assert!(message.contains("shift count out of range"), "{message}");
}

/// Out of range raises rather than truncating, and a mask is how a program says it meant the
/// truncation.
#[test]
fn the_conversions_refuse_a_value_the_type_does_not_hold() {
    let c = Compiled::ran(
        r#"
test "in range, both ways" {
  assert_eq(int_of_u8(255u8), 255);
  assert_eq(int_of_i8(i8_of_int(-128)), -128);
  assert_eq(int_of_u32(4294967295u32), 4294967295)
}

test "a mask is how a truncation is written down" {
  assert_eq(u8_of_int(0x1234 & 0xFF), 0x34u8)
}

pub fn too_big() -> U8 = u8_of_int(256)
pub fn negative() -> U8 = u8_of_int(-1)
pub fn past_int() -> Int = int_of_u64(0xFFFF_FFFF_FFFF_FFFFu64)
"#,
    );
    assert!(refused(&c, "m.too_big").contains("was given 256"));
    assert!(refused(&c, "m.negative").contains("was given -1"));
    // The one conversion out of a fixed width that can fail: `U64` reaches past `Int`.
    assert!(
        refused(&c, "m.past_int").contains("`int_of_u64` was given 18446744073709551615"),
        "the message must name the value that did not fit"
    );
}

/// A `U64` past the largest `Int` is a value the type holds, and it is written as a bit pattern.
#[test]
fn the_sixty_four_bit_types_carry_their_whole_range() {
    Compiled::ran(
        r#"
test "the top of U64" {
  let top = 0xFFFF_FFFF_FFFF_FFFFu64;
  assert_eq(wrap_add(top, 0u64), top);
  assert_eq(wrap_add(top, 1u64), 0u64);
  assert(top > 0u64)
}

test "I64 is Int's range, at its own type" {
  assert_eq(-1i64 + 1i64, 0i64);
  assert(-1i64 < 0i64)
}
"#,
    );
}

/// Ordering and equality are by value, which is what makes them map keys.
#[test]
fn ordering_is_by_value_and_a_fixed_width_type_is_a_map_key() {
    Compiled::ran(
        r#"
test "signed orders below zero" {
  assert(i8_of_int(-128) < 0i8);
  assert(255u8 > 0u8);
  assert_eq(compare(1u32, 2u32), Less)
}

test "a map keyed by a fixed width finds what it inserted" {
  let m = map_insert(map_new(), 7u32, "seven");
  assert_eq(map_get(m, 7u32), Some("seven"));
  assert_eq(map_get(m, 8u32), None)
}
"#,
    );
}

/// A rendered value is its value, not its bits — `I8`'s `-1` renders `-1`.
#[test]
fn a_fixed_width_value_renders_as_its_value() {
    use ply_eval::{Fixed, IntTy, Value};
    assert_eq!(Value::Fixed(Fixed::new(IntTy::I8, 0xFF)).render(), "-1");
    assert_eq!(Value::Fixed(Fixed::new(IntTy::U8, 0xFF)).render(), "255");
    assert_eq!(
        Value::Fixed(Fixed::new(IntTy::U64, u64::MAX)).render(),
        "18446744073709551615"
    );
}
