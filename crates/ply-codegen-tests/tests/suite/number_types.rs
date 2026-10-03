// `Value` pins `Arc` for its shared payloads.
#![allow(clippy::arc_with_non_send_sync)]

use crate::fragment::{call, raised, unit};
use ply_eval::{Closure, ClosureKind, Fields, Fixed, IntTy, Synth, Value};
use std::sync::Arc;

/// `Int` signatures over bodies that compute in widths, as `std.hash` does.
const WIDTHS: &str = r#"
fn add_u8(a: Int, b: Int) -> Int / {abort.raise} = int_of_u8(u8_of_int(a) + u8_of_int(b))
fn wrap_u8(a: Int, b: Int) -> Int / {abort.raise} = int_of_u8(wrap_add(u8_of_int(a), u8_of_int(b)))
fn wrap_u32(a: Int, b: Int) -> Int / {abort.raise} = int_of_u32(wrap_add(u32_of_int(a), u32_of_int(b)))
fn mul_u16(a: Int, b: Int) -> Int / {abort.raise} = int_of_u16(u16_of_int(a) * u16_of_int(b))
fn div_i8(a: Int, b: Int) -> Int / {abort.raise} = int_of_i8(i8_of_int(a) / i8_of_int(b))
fn rem_u32(a: Int, b: Int) -> Int / {abort.raise} = int_of_u32(u32_of_int(a) % u32_of_int(b))
fn neg_i16(a: Int) -> Int / {abort.raise} = int_of_i16(-i16_of_int(a))

fn xor_u32(a: Int, b: Int) -> Int / {abort.raise} = int_of_u32(u32_of_int(a) ^ u32_of_int(b))
fn not_u8(a: Int) -> Int / {abort.raise} = int_of_u8(~u8_of_int(a))
fn shl_u8(a: Int, n: Int) -> Int / {abort.raise} = int_of_u8(u8_of_int(a) << n)
fn shr_i8(a: Int, n: Int) -> Int / {abort.raise} = int_of_i8(i8_of_int(a) >> n)
fn ushr_i8(a: Int, n: Int) -> Int / {abort.raise} = int_of_i8(i8_of_int(a) >>> n)
fn rotr_u32(a: Int, n: Int) -> Int / {abort.raise} = int_of_u32(rotr(u32_of_int(a), n))
fn rotr_u8(a: Int, n: Int) -> Int / {abort.raise} = int_of_u8(rotr(u8_of_int(a), n))

fn lt_u8(a: Int, b: Int) -> Bool / {abort.raise} = u8_of_int(a) < u8_of_int(b)
fn lt_i8(a: Int, b: Int) -> Bool / {abort.raise} = i8_of_int(a) < i8_of_int(b)
fn eq_u32(a: Int, b: Int) -> Bool / {abort.raise} = u32_of_int(a) == u32_of_int(b)

fn from_literal() -> Int = int_of_u32(0x6A09_E667u32)
fn literal_arithmetic(a: Int) -> Int / {abort.raise} = int_of_u8(wrap_add(u8_of_int(a), 200u8))

// `bytes_u32_le` over bytes whose kind the emitter cannot see: a value bound by a pattern,
// whose type the emitter does not read. The read goes through the runtime rather than an
// inline load.
fn first_word(x: Bytes) -> Int / {abort.raise} = match [x] { [b, ..] -> int_of_u32(bytes_u32_le(b, 0)), [] -> 0 }

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

fn round_trip(seed: Int) -> Int / {abort.raise} = {
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
fn mixed(n: Int) -> Int / {abort.raise} =
  int_of_u32(fold(range(0, n), 0u32, |acc: U32, i: Int| rotr(acc ^ u32_of_int(i), 7)))
"#;

/// Widths in signatures, fields, lists, constructors and type variables, crossing the seam both ways.
const CROSSES: &str = r#"
type Byte = | Byte(U8)
type Word = { w: U32 }
type Sample = { level: I16, label: String }

fn narrows(n: Int) -> U32 / {abort.raise} requires n >= 0 ensures int_of_u32(result) == n = u32_of_int(n)
fn widens(w: U32) -> Int requires int_of_u32(w) > 0 = int_of_u32(w)
fn boxed(n: Int) -> Word / {abort.raise} = {w: u32_of_int(n)}
law "no word is seven" forall (w: U32) { int_of_u32(w) != 7 }

fn negated(b: I8) -> I8 = -b
fn doubled(w: U32) -> U32 = wrap_add(w, w)
fn flipped(w: U64) -> U64 = w ^ 1u64
fn bytes(n: Int) -> List<U8> / {abort.raise} = map(range(0, n), |i: Int| u8_of_int(i * 100 % 256))
fn summed(xs: List<U8>) -> Int = fold(xs, 0, |acc: Int, x: U8| acc + int_of_u8(x))
fn sample(n: Int) -> Sample / {abort.raise} = {level: i16_of_int(n), label: "s"}
fn level(s: Sample) -> I16 = s.level
fn wrap(b: U8) -> Byte = Byte(b)
fn unwrap(b: Byte) -> U8 = match b { Byte(x) -> x }
law "no byte is seven" forall (b: Byte) { match b { Byte(x) -> int_of_u8(x) != 7 } }

fn first<a>(xs: List<a>) -> Option<a> = match xs { [x, ..] -> Some(x), _ -> None }
fn made<a>(make: () -> a) -> a = make()
fn applied<a, b>(f: (a) -> b, x: a) -> b = f(x)
"#;

#[test]
fn each_width_answers_what_its_type_means() {
    let (_, unit) = unit(WIDTHS);
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
            "m.first_word",
            vec![Value::Bytes(std::sync::Arc::from(&[1u8, 2, 3, 4][..]))],
            Value::Int(0x0403_0201),
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
        (
            "m.round_trip",
            vec![Value::Int(0xDEAD_BEEF)],
            Value::Int(0x7713_A5D9),
        ),
        ("m.round_trip", vec![Value::Int(0)], Value::Int(0x4892_2726)),
        ("m.mixed", vec![Value::Int(16)], Value::Int(0xE414_A0AF)),
        ("m.mixed", vec![Value::Int(0)], Value::Int(0)),
    ];
    for (name, args, want) in cases {
        let got = call(unit, name, args);
        assert_eq!(
            got.as_ref(),
            Some(want),
            "`{name}{args:?}` answered {got:?}, not {want:?}"
        );
    }
}

fn u8(n: i128) -> Value {
    Value::Fixed(Fixed::of(IntTy::U8, n).expect("a U8"))
}

fn fixed(ty: IntTy, n: i128) -> Value {
    Value::Fixed(Fixed::of(ty, n).expect("a value of the width"))
}

fn record(fields: Vec<(&str, Value)>) -> Value {
    Value::Record(Arc::new(Fields::from_unsorted(
        fields
            .into_iter()
            .map(|(name, v)| (name.into(), v))
            .collect(),
    )))
}

/// A width crosses as the tagged `Int` compiled code holds it, and reads back as the width its
/// type says, however deep: the roots are entered, not declined, and each answer is typed.
#[test]
fn a_signature_naming_a_width_is_entered_and_answers_typed() {
    let (_, unit) = unit(CROSSES);
    let bodies = unit.bodies().expect("the unit builds");
    let byte = |n: i128| Value::ctor("m.Byte", vec![u8(n)]);
    let cases: Vec<(&str, Vec<Value>, Value)> = vec![
        ("m.narrows", vec![Value::Int(7)], fixed(IntTy::U32, 7)),
        ("m.widens", vec![fixed(IntTy::U32, 7)], Value::Int(7)),
        (
            "m.boxed",
            vec![Value::Int(7)],
            record(vec![("w", fixed(IntTy::U32, 7))]),
        ),
        // A clause is entered with its owner's parameters, and an `ensures` with `result` too.
        (
            "m.narrows#ensures#0",
            vec![Value::Int(7), fixed(IntTy::U32, 7)],
            Value::Bool(true),
        ),
        (
            "m.narrows#ensures#0",
            vec![Value::Int(7), fixed(IntTy::U32, 8)],
            Value::Bool(false),
        ),
        (
            "m.widens#requires#0",
            vec![fixed(IntTy::U32, 0)],
            Value::Bool(false),
        ),
        (
            "m.law#0.body",
            vec![fixed(IntTy::U32, 7)],
            Value::Bool(false),
        ),
        (
            "m.law#0.body",
            vec![fixed(IntTy::U32, 4_294_967_295)],
            Value::Bool(true),
        ),
        ("m.negated", vec![fixed(IntTy::I8, -5)], fixed(IntTy::I8, 5)),
        (
            "m.negated",
            vec![fixed(IntTy::I8, 127)],
            fixed(IntTy::I8, -127),
        ),
        (
            "m.doubled",
            vec![fixed(IntTy::U32, 3_000_000_000)],
            fixed(IntTy::U32, 1_705_032_704),
        ),
        // Past `2^63`, where a `U64` is the runtime's own object rather than an immediate.
        (
            "m.flipped",
            vec![fixed(IntTy::U64, i128::from(u64::MAX) - 1)],
            fixed(IntTy::U64, i128::from(u64::MAX)),
        ),
        (
            "m.bytes",
            vec![Value::Int(3)],
            Value::list(vec![u8(0), u8(100), u8(200)]),
        ),
        (
            "m.summed",
            vec![Value::list(vec![u8(200), u8(100)])],
            Value::Int(300),
        ),
        (
            "m.sample",
            vec![Value::Int(-300)],
            record(vec![
                ("label", Value::str("s")),
                ("level", fixed(IntTy::I16, -300)),
            ]),
        ),
        (
            "m.level",
            vec![record(vec![
                ("label", Value::str("s")),
                ("level", fixed(IntTy::I16, -300)),
            ])],
            fixed(IntTy::I16, -300),
        ),
        ("m.wrap", vec![u8(200)], byte(200)),
        ("m.unwrap", vec![byte(9)], u8(9)),
        ("m.law#1.body", vec![byte(7)], Value::Bool(false)),
        ("m.law#1.body", vec![byte(8)], Value::Bool(true)),
    ];
    for (name, args, want) in &cases {
        assert!(
            bodies.admits(name),
            "`{name}` is not offered to the machine"
        );
        let got = call(unit, name, args);
        assert_eq!(
            got.as_ref(),
            Some(want),
            "`{name}{args:?}` answered {got:?}, not {want:?}"
        );
    }
}

/// A type variable reads as what the arguments show of it: a width where they hold one, an `Int`
/// where they hold `Int`s, and a prover's generated function shows its answers.
#[test]
fn a_type_variable_reads_as_the_arguments_show_it() {
    let (_, unit) = unit(CROSSES);
    let generated = |rule: Synth| {
        Value::Closure(Arc::new(Closure {
            name: None,
            kind: ClosureKind::Synth { arity: 0, rule },
        }))
    };
    for (name, args, want) in [
        (
            "m.first",
            vec![Value::list(vec![u8(5), u8(6)])],
            Value::ctor("Some", vec![u8(5)]),
        ),
        (
            "m.first",
            vec![Value::list(vec![Value::Int(5)])],
            Value::ctor("Some", vec![Value::Int(5)]),
        ),
        (
            "m.first",
            vec![Value::list(vec![])],
            Value::ctor("None", vec![]),
        ),
        ("m.made", vec![generated(Synth::Const(u8(3)))], u8(3)),
        (
            "m.made",
            vec![generated(Synth::Const(Value::Int(3)))],
            Value::Int(3),
        ),
    ] {
        let got = call(unit, name, &args);
        assert_eq!(
            got.as_ref(),
            Some(&want),
            "`{name}{args:?}` answered {got:?}, not {want:?}"
        );
    }
}

/// An `Int` word where nothing says whether it is an `Int` or a width is declined, never guessed:
/// here `b` is a `U8`, which only the builtin knows.
#[test]
fn an_answer_no_argument_types_is_declined_rather_than_guessed() {
    let (_, unit) = unit(CROSSES);
    let bodies = unit.bodies().expect("the unit builds");
    let narrowing = Value::Closure(Arc::new(Closure {
        name: None,
        kind: ClosureKind::Builtin(ply_eval::Builtin::U8OfInt),
    }));
    let applied = ply_eval::Symbol::new("m.applied");
    assert_eq!(
        ply_eval::Compiled::enter(&*bodies, &applied, &[narrowing, Value::Int(5)], 10_000),
        None
    );
    assert_eq!(bodies.declines().unread, 1, "{:?}", bodies.declines());
    // The same root reads its answer wherever an argument shows the variable it answers.
    let generated = Value::Closure(Arc::new(Closure {
        name: None,
        kind: ClosureKind::Synth {
            arity: 1,
            rule: Synth::Const(u8(5)),
        },
    }));
    assert_eq!(
        call(unit, "m.applied", &[generated, Value::Int(0)]),
        Some(u8(5))
    );
}

/// Operands whose `+`, `-` and `*` leave the width, each with what the wrapping builtin of the
/// same operation answers for them.
struct Operands {
    ty: &'static str,
    add: (i64, i64, i64),
    sub: (i64, i64, i64),
    mul: (i64, i64, i64),
}

const OPERANDS: [Operands; 4] = [
    Operands {
        ty: "U8",
        add: (200, 100, 44),
        sub: (100, 200, 156),
        mul: (200, 100, 32),
    },
    Operands {
        ty: "I8",
        add: (100, 100, -56),
        sub: (-100, 100, 56),
        mul: (100, 100, 16),
    },
    Operands {
        ty: "U32",
        add: (4_000_000_000, 1_000_000_000, 705_032_704),
        sub: (1, 2, 4_294_967_295),
        mul: (100_000, 100_000, 1_410_065_408),
    },
    Operands {
        ty: "I32",
        add: (2_000_000_000, 2_000_000_000, -294_967_296),
        sub: (-2_000_000_000, 2_000_000_000, 294_967_296),
        mul: (100_000, 100_000, 1_410_065_408),
    },
];

/// Where compiled code holds a value as an untyped word: a list, tuple, record or constructor
/// pattern's binders, a function value's answer bound by `let`, a closure's captures, a generic's
/// answer and a map's value.
const SHAPES: [&str; 8] = [
    "list", "tuple", "field", "ctor", "answer", "capture", "generic", "map",
];

/// `<shape>_<width>(x, y, op)` reads `x` and `y` at the width through the shape and applies `op`:
/// `+`, `-`, `*`, then `wrap_add`, `wrap_sub`, `wrap_mul`, back to `Int` in the same body so no
/// narrowing on the way out can hide a wrong answer.
fn binders() -> String {
    let mut source = String::from(
        "fn nth<t>(xs: List<t>, i: Int, d: t) -> t = match list_at(xs, i) { Some(v) -> v, None -> d }\n",
    );
    for o in &OPERANDS {
        let (ty, t) = (o.ty, o.ty.to_lowercase());
        let ops = |k: &str| {
            format!(
                "if {k} == 0 {{ int_of_{t}(a + b) }} else if {k} == 1 {{ int_of_{t}(a - b) }} \
                 else if {k} == 2 {{ int_of_{t}(a * b) }} \
                 else if {k} == 3 {{ int_of_{t}(wrap_add(a, b)) }} \
                 else if {k} == 4 {{ int_of_{t}(wrap_sub(a, b)) }} \
                 else {{ int_of_{t}(wrap_mul(a, b)) }}"
            )
        };
        let (op, k) = (ops("op"), ops("k"));
        source.push_str(&format!(
            "
type Pair{ty} = | Pair{ty}({ty}, {ty})
fn listed_{t}(xs: List<{ty}>, op: Int) -> Int = match xs {{ [a, b, ..] -> {op}, _ -> 0 }}
fn list_{t}(x: Int, y: Int, op: Int) -> Int / {{abort.raise}} = listed_{t}([{t}_of_int(x), {t}_of_int(y)], op)
fn tupled_{t}(p: ({ty}, {ty}), op: Int) -> Int = match p {{ (a, b) -> {op} }}
fn tuple_{t}(x: Int, y: Int, op: Int) -> Int / {{abort.raise}} = tupled_{t}(({t}_of_int(x), {t}_of_int(y)), op)
fn fielded_{t}(r: {{ left: {ty}, right: {ty} }}, op: Int) -> Int = match r {{ {{ left: a, right: b }} -> {op} }}
fn field_{t}(x: Int, y: Int, op: Int) -> Int / {{abort.raise}} = fielded_{t}({{ left: {t}_of_int(x), right: {t}_of_int(y) }}, op)
fn constructed_{t}(p: Pair{ty}, op: Int) -> Int = match p {{ Pair{ty}(a, b) -> {op} }}
fn ctor_{t}(x: Int, y: Int, op: Int) -> Int / {{abort.raise}} = constructed_{t}(Pair{ty}({t}_of_int(x), {t}_of_int(y)), op)
fn answered_{t}(f: (Int) -> {ty} / {{abort.raise}}, x: Int, y: Int, op: Int) -> Int / {{abort.raise}} = {{ let a = f(x); let b = f(y); {op} }}
fn answer_{t}(x: Int, y: Int, op: Int) -> Int / {{abort.raise}} = answered_{t}(|n: Int| {t}_of_int(n), x, y, op)
fn captured_{t}(a: {ty}, b: {ty}, op: Int) -> Int = {{ let f = |k: Int| {k}; f(op) }}
fn capture_{t}(x: Int, y: Int, op: Int) -> Int / {{abort.raise}} = captured_{t}({t}_of_int(x), {t}_of_int(y), op)
fn chosen_{t}(xs: List<{ty}>, op: Int) -> Int / {{abort.raise}} = {{ let a = nth(xs, 0, {t}_of_int(0)); let b = nth(xs, 1, {t}_of_int(0)); {op} }}
fn generic_{t}(x: Int, y: Int, op: Int) -> Int / {{abort.raise}} = chosen_{t}([{t}_of_int(x), {t}_of_int(y)], op)
fn valued_{t}(m: Map<Int, {ty}>, op: Int) -> Int = match map_get(m, 0) {{ Some(a) -> match map_get(m, 1) {{ Some(b) -> {op}, None -> 0 }}, None -> 0 }}
fn map_{t}(x: Int, y: Int, op: Int) -> Int / {{abort.raise}} = valued_{t}(map_insert(map_insert(map_new(), 0, {t}_of_int(x)), 1, {t}_of_int(y)), op)
"
        ));
    }
    source
}

/// However a width reached an operator, the operator is the width's: `+ - *` raise past it and the
/// wrapping builtins wrap at it.
#[test]
fn a_width_is_read_at_its_type_through_every_binder() {
    let source = binders();
    let (_, unit) = unit(&source);
    for o in &OPERANDS {
        let t = o.ty.to_lowercase();
        for shape in SHAPES {
            let name = format!("m.{shape}_{t}");
            for (op, (x, y, wrapped), what) in [
                (0, o.add, "addition"),
                (1, o.sub, "subtraction"),
                (2, o.mul, "multiplication"),
            ] {
                let args = [Value::Int(x), Value::Int(y), Value::Int(op)];
                let raise = raised(unit, &name, &args);
                assert!(
                    raise.message.contains(&format!("overflow in {what}")),
                    "`{name}{args:?}` raised {raise:?}"
                );
                let wrap = [Value::Int(x), Value::Int(y), Value::Int(op + 3)];
                assert_eq!(
                    call(unit, &name, &wrap),
                    Some(Value::Int(wrapped)),
                    "`{name}{wrap:?}`"
                );
            }
        }
    }
}

/// The comparisons, shifts, bit operators, negation and division over bound widths; the 64-bit
/// widths, which are the runtime's own words; and width builtins passed as values.
const WORDS: &str = r#"
fn lt_i8(x: Int, y: Int) -> Bool / {abort.raise} = match [i8_of_int(x), i8_of_int(y)] { [a, b] -> a < b, _ -> false }
fn ge_u32(x: Int, y: Int) -> Bool / {abort.raise} = match [u32_of_int(x), u32_of_int(y)] { [a, b] -> a >= b, _ -> false }
fn shl_u8(x: Int, n: Int) -> Int / {abort.raise} = match [u8_of_int(x)] { [a] -> int_of_u8(a << n), _ -> 0 }
fn ushr_i8(x: Int, n: Int) -> Int / {abort.raise} = match [i8_of_int(x)] { [a] -> int_of_i8(a >>> n), _ -> 0 }
fn not_u8(x: Int) -> Int / {abort.raise} = match [u8_of_int(x)] { [a] -> int_of_u8(~a), _ -> 0 }
fn neg_i8(x: Int) -> Int / {abort.raise} = match [i8_of_int(x)] { [a] -> int_of_i8(-a), _ -> 0 }
fn div_i8(x: Int, y: Int) -> Int / {abort.raise} = match [i8_of_int(x), i8_of_int(y)] { [a, b] -> int_of_i8(a / b), _ -> 0 }
fn rotr_u8(x: Int, n: Int) -> Int / {abort.raise} = match [u8_of_int(x)] { [a] -> int_of_u8(rotr(a, n)), _ -> 0 }
fn shl_u64(x: Int, n: Int) -> Int / {abort.raise} = match [u64_of_int(x)] { [a] -> int_of_u64(a << n), _ -> 0 }
fn ushr_i64(x: Int, n: Int) -> Int / {abort.raise} = match [i64_of_int(x)] { [a] -> int_of_i64(a >>> n), _ -> 0 }
fn not_i64(x: Int) -> Int / {abort.raise} = match [i64_of_int(x)] { [a] -> int_of_i64(~a), _ -> 0 }
fn shifted(w: U64, n: Int) -> U64 = w << n
fn shl_u64_param(x: Int, n: Int) -> Int / {abort.raise} = int_of_u64(shifted(u64_of_int(x), n))
fn folded_u8(x: Int, y: Int) -> Int / {abort.raise} = int_of_u8(fold([u8_of_int(x), u8_of_int(y)], 0u8, wrap_add))
fn narrowed(x: Int) -> Int / {abort.raise} = fold(map(map([x], u8_of_int), int_of_u8), 0, |acc: Int, v: Int| acc + v)
fn folded_int(x: Int, y: Int) -> Int = fold([x, y], 0, wrap_add)
"#;

#[test]
fn every_operator_reads_a_bound_width_at_its_type() {
    let (_, unit) = unit(WORDS);
    let int = |n: i64| Some(Value::Int(n));
    let answers: &[(&str, Vec<Value>, Option<Value>)] = &[
        (
            "m.lt_i8",
            vec![Value::Int(-1), Value::Int(1)],
            Some(Value::Bool(true)),
        ),
        (
            "m.ge_u32",
            vec![Value::Int(4_294_967_295), Value::Int(1)],
            Some(Value::Bool(true)),
        ),
        ("m.shl_u8", vec![Value::Int(200), Value::Int(1)], int(144)),
        ("m.ushr_i8", vec![Value::Int(-2), Value::Int(1)], int(127)),
        ("m.not_u8", vec![Value::Int(200)], int(55)),
        ("m.neg_i8", vec![Value::Int(-127)], int(127)),
        ("m.div_i8", vec![Value::Int(-128), Value::Int(2)], int(-64)),
        ("m.rotr_u8", vec![Value::Int(1), Value::Int(1)], int(128)),
        ("m.shl_u64", vec![Value::Int(3), Value::Int(1)], int(6)),
        ("m.ushr_i64", vec![Value::Int(-1), Value::Int(60)], int(15)),
        ("m.not_i64", vec![Value::Int(5)], int(-6)),
        (
            "m.shl_u64_param",
            vec![Value::Int(1), Value::Int(4)],
            int(16),
        ),
        (
            "m.folded_u8",
            vec![Value::Int(200), Value::Int(100)],
            int(44),
        ),
        ("m.narrowed", vec![Value::Int(200)], int(200)),
        (
            "m.folded_int",
            vec![Value::Int(i64::MAX), Value::Int(1)],
            int(i64::MIN),
        ),
    ];
    for (name, args, want) in answers {
        assert_eq!(&call(unit, name, args), want, "`{name}{args:?}`");
    }
    let raises: &[(&str, Vec<Value>, &str)] = &[
        (
            "m.shl_u8",
            vec![Value::Int(1), Value::Int(8)],
            "shift count out of range",
        ),
        (
            "m.shl_u64",
            vec![Value::Int(1), Value::Int(64)],
            "shift count out of range",
        ),
        ("m.neg_i8", vec![Value::Int(-128)], "overflow in negation"),
        (
            "m.div_i8",
            vec![Value::Int(-128), Value::Int(-1)],
            "overflow in division",
        ),
        ("m.narrowed", vec![Value::Int(256)], "was given 256"),
    ];
    for (name, args, what) in raises {
        let raise = raised(unit, name, args);
        assert!(
            raise.message.contains(what),
            "`{name}{args:?}` raised {raise:?}"
        );
    }
}
