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

/// Deterministic, so a failing case is a seed a reader can reproduce.
struct Xorshift(u64);

impl Xorshift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

fn decimal(text: &str) -> Value {
    Value::Decimal(text.parse().unwrap())
}

fn mode(name: &str) -> Value {
    Value::ctor(name, Vec::new())
}

/// A `Decimal` as it prints, which shows its scale where `==` does not.
fn written(v: Value) -> String {
    match v {
        Value::Decimal(d) => d.to_string(),
        other => panic!("not a `Decimal`: {other:?}"),
    }
}

/// The C backend's runtime is held to the same answers in `ply-compiled-tests`.
#[test]
fn a_decimal_quotient_has_the_scale_asked_for_and_is_rounded_once() {
    for (a, b, scale, rounding, want) in [
        ("3", "2", 3, "HalfEven", "1.500"),
        ("1", "3", 3, "HalfEven", "0.333"),
        ("100", "100", 2, "Down", "1.00"),
        ("3.00", "2", 0, "HalfEven", "2"),
        ("0", "7", 2, "Up", "0.00"),
        ("-1", "3", 2, "Floor", "-0.34"),
        ("-1", "3", 2, "Ceiling", "-0.33"),
        ("1", "-3", 2, "Up", "-0.34"),
        ("-1", "-3", 2, "Down", "0.33"),
        ("-1", "300", 2, "HalfEven", "0.00"),
        // No digit past the twenty-eighth is rounded before the mode reads what is left.
        ("2", "3", 28, "Down", "0.6666666666666666666666666666"),
        ("2", "3", 28, "HalfEven", "0.6666666666666666666666666667"),
        ("1", "3", 28, "Up", "0.3333333333333333333333333334"),
        // Below a half by one part in ten to the twenty-ninth, which no `Decimal` holds.
        ("4.9999999999999999999999999995", "10", 0, "HalfUp", "0"),
        (
            "79228162514264337593543950335",
            "3",
            0,
            "Down",
            "26409387504754779197847983445",
        ),
        (
            "79228162514264337593543950335",
            "1",
            0,
            "Up",
            "79228162514264337593543950335",
        ),
        (
            "55459713759985036315480765235",
            "7",
            1,
            "Down",
            "7922816251426433759354395033.5",
        ),
    ] {
        let args = vec![decimal(a), decimal(b), Value::Int(scale), mode(rounding)];
        assert_eq!(
            written(found(Builtin::DecimalDiv, args)),
            want,
            "`Decimal::div({a}, {b}, {scale}, {rounding})`"
        );
    }
}

#[test]
fn a_decimal_rounding_rounds_what_is_past_its_scale_and_fills_what_is_short_of_it() {
    for (d, scale, rounding, want) in [
        ("1.5", 3, "HalfEven", "1.500"),
        ("1.50", 1, "HalfEven", "1.5"),
        ("7", 2, "Down", "7.00"),
        ("7", 28, "Down", "7.0000000000000000000000000000"),
        ("0.5", 0, "HalfEven", "0"),
        ("1.5", 0, "HalfEven", "2"),
        ("2.5", 0, "HalfEven", "2"),
        ("2.5", 0, "HalfUp", "3"),
        ("-2.5", 0, "HalfUp", "-3"),
        ("-2.5", 0, "Down", "-2"),
        ("-2.5", 0, "Up", "-3"),
        ("-2.5", 0, "Ceiling", "-2"),
        ("-2.5", 0, "Floor", "-3"),
        ("2.5001", 0, "HalfEven", "3"),
        ("-0.001", 2, "Floor", "-0.01"),
        ("-0.001", 2, "HalfEven", "0.00"),
    ] {
        let args = vec![decimal(d), Value::Int(scale), mode(rounding)];
        assert_eq!(
            written(found(Builtin::DecimalRound, args)),
            want,
            "`Decimal::round({d}, {scale}, {rounding})`"
        );
    }
}

#[test]
fn a_decimal_that_does_not_fit_at_the_scale_asked_for_is_refused_and_never_cut_short() {
    let most = "79228162514264337593543950335";
    let refused = |b: Builtin, args: Vec<Value>, what: &str| {
        let raised = done(b, args).unwrap_err();
        assert_eq!(raised.code, codes::RUNTIME_ERROR, "{raised}");
        assert_eq!(raised.message, format!("`Decimal` overflow in {what}"));
    };
    for (d, scale) in [(most, 1), ("8", 28), ("-8", 28)] {
        refused(
            Builtin::DecimalRound,
            vec![decimal(d), Value::Int(scale), mode("Down")],
            "rounding",
        );
    }
    for (a, b, scale, rounding) in [
        (most, "1", 1, "Down"),
        ("1", "0.1", 28, "Down"),
        (most, "0.5", 0, "Down"),
        // The greatest mantissa and five sevenths, which `Up` takes to two to the ninety-sixth.
        ("55459713759985036315480765235", "7", 1, "Up"),
    ] {
        refused(
            Builtin::DecimalDiv,
            vec![decimal(a), decimal(b), Value::Int(scale), mode(rounding)],
            "division",
        );
    }
}

/// A divisor whose only prime factors are two and five has a quotient that ends, which
/// `rust_decimal` divides exactly: there its own rounding is the answer to agree with.
#[test]
fn a_decimal_quotient_agrees_with_an_exact_division_rounded_over_ten_thousand_pairs() {
    use rust_decimal::{Decimal, RoundingStrategy};
    let modes = [
        ("HalfEven", RoundingStrategy::MidpointNearestEven),
        ("HalfUp", RoundingStrategy::MidpointAwayFromZero),
        ("Down", RoundingStrategy::ToZero),
        ("Up", RoundingStrategy::AwayFromZero),
        ("Ceiling", RoundingStrategy::ToPositiveInfinity),
        ("Floor", RoundingStrategy::ToNegativeInfinity),
    ];
    let mut rng = Xorshift(0x9E37_79B9_7F4A_7C15);
    for case in 0..10_000 {
        let a = Decimal::new(
            rng.below(2_000_000_000_000) as i64 - 1_000_000_000_000,
            rng.below(7) as u32,
        );
        let ending = 2i64.pow(rng.below(7) as u32) * 5i64.pow(rng.below(5) as u32);
        let sign = if rng.below(2) == 0 { 1 } else { -1 };
        let b = Decimal::new(sign * ending, rng.below(4) as u32);
        let scale = rng.below(13) as u32;
        let (name, strategy) = modes[rng.below(modes.len())];
        let exact = a.checked_div(b).expect("a quotient that ends, and fits");
        let want = exact.round_dp_with_strategy(scale, strategy);
        let got = found(
            Builtin::DecimalDiv,
            vec![
                Value::Decimal(a),
                Value::Decimal(b),
                Value::Int(i64::from(scale)),
                mode(name),
            ],
        );
        let Value::Decimal(got) = got else {
            panic!("case {case}: not a `Decimal`");
        };
        assert_eq!(
            got, want,
            "case {case}: `Decimal::div({a}, {b}, {scale}, {name})`"
        );
        assert_eq!(
            got.scale(),
            scale,
            "case {case}: `Decimal::div({a}, {b}, {scale}, {name})` is {got}"
        );
    }
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
