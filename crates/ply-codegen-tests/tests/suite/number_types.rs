//! Fixed-width integers under the backend: that a compiled body answers what the interpreter
//! answers at each width, and that a signature naming one is declined rather than answered wrongly.

use crate::fragment::{Loaded, call, unit};
use ply_eval::{Machine, Value};

/// What the interpreter answers, which is the only thing the compiled answer is checked against:
/// a constant written here by hand would be checking my arithmetic rather than the two engines'
/// agreement.
fn interpreted(loaded: &'static Loaded, name: &str, args: &[Value]) -> Option<Value> {
    let mut machine = Machine::new(loaded.program, loaded.resolved, loaded.check);
    machine
        .call(name, args.to_vec(), ply_span::Span::DUMMY)
        .ok()
}

/// Every operation the family has, wrapped so the *signature* is `Int`: a fixed width may not
/// cross the seam (ADR 0039), so the widths live inside the bodies and the answers come back as
/// `Int`s. That is exactly the shape `std.hash` has.
const WIDTHS: &str = r#"
fn add_u8(a: Int, b: Int) -> Int = int_of_u8(u8_of_int(a) + u8_of_int(b))
fn wrap_u8(a: Int, b: Int) -> Int = int_of_u8(wrap_add(u8_of_int(a), u8_of_int(b)))
fn wrap_u32(a: Int, b: Int) -> Int = int_of_u32(wrap_add(u32_of_int(a), u32_of_int(b)))
fn mul_u16(a: Int, b: Int) -> Int = int_of_u16(u16_of_int(a) * u16_of_int(b))
fn div_i8(a: Int, b: Int) -> Int = int_of_i8(i8_of_int(a) / i8_of_int(b))
fn rem_u32(a: Int, b: Int) -> Int = int_of_u32(u32_of_int(a) % u32_of_int(b))
fn neg_i16(a: Int) -> Int = int_of_i16(-i16_of_int(a))

fn xor_u32(a: Int, b: Int) -> Int = int_of_u32(u32_of_int(a) ^ u32_of_int(b))
fn not_u8(a: Int) -> Int = int_of_u8(~u8_of_int(a))
fn shl_u8(a: Int, n: Int) -> Int = int_of_u8(u8_of_int(a) << n)
fn shr_i8(a: Int, n: Int) -> Int = int_of_i8(i8_of_int(a) >> n)
fn ushr_i8(a: Int, n: Int) -> Int = int_of_i8(i8_of_int(a) >>> n)
fn rotr_u32(a: Int, n: Int) -> Int = int_of_u32(rotr(u32_of_int(a), n))
fn rotr_u8(a: Int, n: Int) -> Int = int_of_u8(rotr(u8_of_int(a), n))

fn lt_u8(a: Int, b: Int) -> Bool = u8_of_int(a) < u8_of_int(b)
fn lt_i8(a: Int, b: Int) -> Bool = i8_of_int(a) < i8_of_int(b)
fn eq_u32(a: Int, b: Int) -> Bool = u32_of_int(a) == u32_of_int(b)

fn from_literal() -> Int = int_of_u32(0x6A09_E667u32)
fn literal_arithmetic(a: Int) -> Int = int_of_u8(wrap_add(u8_of_int(a), 200u8))

// A record whose fields are `U32`, which is the shape the integer kernel threads through a round:
// the widths are held in the fields and the seam never sees one.
type Quad = { a: U32, b: U32, c: U32, d: U32 }

fn quarter(q: Quad, mx: U32) -> Quad = {
  let a1 = wrap_add(wrap_add(q.a, q.b), mx);
  let d1 = rotr(q.d ^ a1, 16);
  let c1 = wrap_add(q.c, d1);
  let b1 = rotr(q.b ^ c1, 12);
  {a: a1, b: b1, c: c1, d: d1}
}

fn round_trip(seed: Int) -> Int = {
  let w = u32_of_int(seed);
  let q = quarter({a: w, b: wrap_add(w, 1u32), c: 0x3C6E_F372u32, d: 0xA54F_F53Au32}, w);
  int_of_u32(q.a ^ q.b ^ q.c ^ q.d)
}

// An `if` whose branches answer a width, which is the shape `std.hash`'s flags are built with.
// It is here because it once compiled to `ubfx x5, x0, #0, #64` --- a join block parameter typed
// `I64` where both branches passed an `I32`, which Cranelift's verifier accepted, the assembler
// encoded and the processor refused with SIGILL.
fn flags(i: Int, last: Bool, is_root: Bool) -> Int =
  int_of_u32(
    (if i == 0 { 1u32 } else { 0u32 })
    | (if last { 2u32 } else { 0u32 })
    | (if last && is_root { 8u32 } else { 0u32 }))

// A loop over the widths, so the fused-loop path carries them too.
fn mixed(n: Int) -> Int =
  int_of_u32(fold(range(0, n), 0u32, |acc: U32, i: Int| rotr(acc ^ u32_of_int(i), 7)))
"#;

/// A width may not cross the seam, so these are declined rather than answered.
const CROSSES: &str = r#"
fn narrows(n: Int) -> U32 = u32_of_int(n)
fn widens(w: U32) -> Int = int_of_u32(w)
type Word = { w: U32 }
fn boxed(n: Int) -> Word = {w: u32_of_int(n)}
"#;

#[test]
fn a_compiled_body_answers_what_the_interpreter_answers_at_each_width() {
    let (loaded, unit) = unit(WIDTHS);
    let cases: &[(&str, Vec<Value>, Value)] = &[
        // Checked at the type's own width, and the sum is the type's.
        (
            "m.add_u8",
            vec![Value::Int(200), Value::Int(55)],
            Value::Int(255),
        ),
        (
            "m.wrap_u8",
            vec![Value::Int(255), Value::Int(1)],
            Value::Int(0),
        ),
        (
            "m.wrap_u32",
            vec![Value::Int(4294967295), Value::Int(2)],
            Value::Int(1),
        ),
        (
            "m.mul_u16",
            vec![Value::Int(256), Value::Int(255)],
            Value::Int(65280),
        ),
        (
            "m.div_i8",
            vec![Value::Int(-128), Value::Int(2)],
            Value::Int(-64),
        ),
        (
            "m.rem_u32",
            vec![Value::Int(300), Value::Int(256)],
            Value::Int(44),
        ),
        ("m.neg_i16", vec![Value::Int(1)], Value::Int(-1)),
        // The pattern is the type's, not sixty-four bits of it.
        ("m.not_u8", vec![Value::Int(0)], Value::Int(255)),
        (
            "m.xor_u32",
            vec![Value::Int(0xF0F0_F0F0), Value::Int(0x0F0F_0F0F)],
            Value::Int(0xFFFF_FFFF),
        ),
        (
            "m.shl_u8",
            vec![Value::Int(1), Value::Int(7)],
            Value::Int(128),
        ),
        (
            "m.shl_u8",
            vec![Value::Int(128), Value::Int(1)],
            Value::Int(0),
        ),
        // The two right shifts differ exactly where the type is signed.
        (
            "m.shr_i8",
            vec![Value::Int(-2), Value::Int(1)],
            Value::Int(-1),
        ),
        (
            "m.ushr_i8",
            vec![Value::Int(-2), Value::Int(1)],
            Value::Int(127),
        ),
        // A rotate turns the whole word at its own width.
        (
            "m.rotr_u32",
            vec![Value::Int(1), Value::Int(1)],
            Value::Int(2147483648),
        ),
        (
            "m.rotr_u8",
            vec![Value::Int(1), Value::Int(1)],
            Value::Int(128),
        ),
        (
            "m.rotr_u8",
            vec![Value::Int(0xAB), Value::Int(8)],
            Value::Int(0xAB),
        ),
        // Unsigned compares unsigned and signed compares signed.
        (
            "m.lt_u8",
            vec![Value::Int(255), Value::Int(0)],
            Value::Bool(false),
        ),
        (
            "m.lt_i8",
            vec![Value::Int(-128), Value::Int(0)],
            Value::Bool(true),
        ),
        (
            "m.eq_u32",
            vec![Value::Int(7), Value::Int(7)],
            Value::Bool(true),
        ),
        ("m.from_literal", vec![], Value::Int(0x6A09_E667)),
        (
            "m.flags",
            vec![Value::Int(0), Value::Bool(true), Value::Bool(true)],
            Value::Int(11),
        ),
        (
            "m.flags",
            vec![Value::Int(3), Value::Bool(false), Value::Bool(false)],
            Value::Int(0),
        ),
        (
            "m.literal_arithmetic",
            vec![Value::Int(100)],
            Value::Int(44),
        ),
    ];
    for (name, args, want) in cases {
        let got = call(unit, name, args);
        assert_eq!(
            got.as_ref(),
            Some(want),
            "`{name}{args:?}` answered {got:?}, not {want:?}"
        );
        assert_eq!(
            got,
            interpreted(loaded, name, args),
            "`{name}{args:?}`: the two engines disagree"
        );
    }
    // Two whose answers are not worth writing out by hand — a record of widths threaded through a
    // body, and a loop over them — checked against the interpreter alone, which is the property
    // that matters for both.
    for (name, args) in [
        ("m.round_trip", vec![Value::Int(0xDEAD_BEEF)]),
        ("m.round_trip", vec![Value::Int(0)]),
        ("m.mixed", vec![Value::Int(16)]),
        ("m.mixed", vec![Value::Int(0)]),
    ] {
        let got = call(unit, name, &args);
        assert!(got.is_some(), "`{name}` was declined");
        assert_eq!(
            got,
            interpreted(loaded, name, &args),
            "`{name}{args:?}`: the two engines disagree"
        );
    }
}

/// A body still compiles and still calls its neighbours directly; what is refused is the
/// *crossing*, because a width is held as a tagged immediate and would arrive as an `Int`.
#[test]
fn a_signature_naming_a_width_is_declined_rather_than_answered() {
    let (_, unit) = unit(CROSSES);
    for (name, args) in [
        ("m.narrows", vec![Value::Int(7)]),
        ("m.widens", vec![Value::Int(7)]),
        ("m.boxed", vec![Value::Int(7)]),
    ] {
        assert_eq!(
            call(unit, name, &args),
            None,
            "`{name}` crossed the seam, which would answer an `Int` where a width was declared"
        );
    }
}
