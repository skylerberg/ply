use ply_core::builtins::*;
use ply_core::{Diagnostic, Span, Value, codes};

fn ints(xs: &[i64]) -> Value {
    Value::list(xs.iter().copied().map(Value::Int).collect())
}

fn done(b: Builtin, args: Vec<Value>) -> Result<Value, Diagnostic> {
    call(b, args, Span::DUMMY)
}

fn found(b: Builtin, args: Vec<Value>) -> Value {
    done(b, args).unwrap()
}

#[test]
fn every_builtin_is_reachable_by_the_name_it_reports() {
    for b in Builtin::all() {
        assert_eq!(Builtin::from_name(b.name()), Some(*b));
    }
}

/// A builtin called through a value with other than its words is refused by name, not read past.
#[test]
fn a_builtin_given_other_than_its_words_is_an_arity_error() {
    for b in Builtin::all() {
        let given = vec![Value::Unit; b.words() + 1];
        let refused = ply_core::builtins::call(*b, given, Span::DUMMY)
            .expect_err("a call past the words was answered");
        assert_eq!(
            refused.code,
            ply_core::codes::ARITY_MISMATCH,
            "`{}`",
            b.name()
        );
        assert!(refused.message.contains(b.name()), "{}", refused.message);
    }
}

#[test]
fn no_builtin_is_listed_twice() {
    let mut names: Vec<&str> = Builtin::all().iter().map(|b| b.name()).collect();
    names.sort_unstable();
    let mut unique = names.clone();
    unique.dedup();
    assert_eq!(names, unique, "`Builtin::all()` lists a builtin twice");
}

#[test]
fn a_digest_agrees_with_equality_and_refuses_what_has_no_hash() {
    let digest = |v: Value| found(Builtin::Digest, vec![v]);
    let decimal = |s: &str| Value::Decimal(s.parse().unwrap());
    assert_eq!(digest(decimal("1.50")), digest(decimal("1.5")));
    let narrow = ply_core::Fixed::of(ply_core::IntTy::U8, 7).unwrap();
    assert_eq!(
        digest(Value::Fixed(narrow)),
        digest(Value::Int(7)),
        "compiled code holds a narrow width as the Int it reads as"
    );
    assert_eq!(
        digest(Value::ctor("orders.Shipped", vec![Value::Int(1)])),
        digest(Value::ctor("app.orders.Shipped", vec![Value::Int(1)])),
        "where a module stands is no part of a value"
    );
    assert_ne!(digest(ints(&[1, 2])), digest(ints(&[2, 1])));
    for v in [Value::Float(1.0), Value::secret_text("pw")] {
        let d = done(Builtin::Digest, vec![v]).unwrap_err();
        assert_eq!(d.code, codes::RUNTIME_ERROR);
    }
}

#[test]
fn a_value_encoded_is_what_its_digest_hashes() {
    let v = Value::ctor("orders.Shipped", vec![decimal_of("1.50"), ints(&[1, 2])]);
    let encoded = found(Builtin::Encoded, vec![v.clone()]);
    let Value::Bytes(encoded) = &encoded else {
        panic!("`?encoded` answers bytes");
    };
    assert_eq!(
        Value::bytes(blake3::hash(encoded).as_bytes()),
        found(Builtin::Digest, vec![v])
    );
}

fn decimal_of(s: &str) -> Value {
    Value::Decimal(s.parse().unwrap())
}

#[test]
fn rotr32_turns_the_low_word_and_answers_it_non_negative() {
    let cases: &[(i64, i64, i64)] = &[
        (1, 1, 0x8000_0000),
        (0x8000_0000, 31, 1),
        (0x1234_5678, 0, 0x1234_5678),
        (0x1234_5678, 32, 0x1234_5678),
        (0x1234_5678, 4, 0x8123_4567),
        (0x1_0000_0001, 1, 0x8000_0000),
        (-1, 7, 0xFFFF_FFFF),
        (2, -1, 4),
    ];
    for &(x, n, want) in cases {
        assert_eq!(
            done(Builtin::Rotr32, vec![Value::Int(x), Value::Int(n)]).unwrap(),
            Value::Int(want),
            "rotr32({x}, {n})"
        );
    }
}

/// Every case is at or across the 64-bit edge, the only place these differ from `+`, `-`, `*`.
#[test]
fn the_wrapping_builtins_are_modulo_two_to_the_sixty_fourth() {
    let cases: &[(Builtin, i64, i64, i64)] = &[
        (Builtin::WrapAdd, i64::MAX, 1, i64::MIN),
        (Builtin::WrapAdd, i64::MIN, -1, i64::MAX),
        (Builtin::WrapAdd, i64::MAX, i64::MAX, -2),
        (Builtin::WrapAdd, 2, 3, 5),
        (Builtin::WrapSub, i64::MIN, 1, i64::MAX),
        (Builtin::WrapSub, i64::MAX, -1, i64::MIN),
        (Builtin::WrapSub, 0, i64::MIN, i64::MIN),
        (Builtin::WrapMul, i64::MAX, 2, -2),
        (Builtin::WrapMul, i64::MIN, -1, i64::MIN),
        (Builtin::WrapMul, 1 << 32, 1 << 32, 0),
        (Builtin::WrapMul, 6, 7, 42),
    ];
    for &(b, x, y, want) in cases {
        assert_eq!(
            done(b, vec![Value::Int(x), Value::Int(y)]).unwrap(),
            Value::Int(want),
            "`{}`({x}, {y})",
            b.name()
        );
    }
}

/// The checker refuses a non-`Int` first; this is what an unchecked body meets.
#[test]
fn a_wrapping_builtin_refuses_a_non_int_and_nothing_else() {
    let d = done(Builtin::WrapAdd, vec![Value::Int(1), Value::str("2")])
        .expect_err("a `String` is not an `Int`");
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    assert!(d.message.contains("wrap_add"), "{}", d.message);
}
