//! The natives strings and byte strings reach the runtime through, called as a unit binds them over
//! hand-built bytes: each answers what the value builtin of its name answers, as machine data.

use crate::bare::{context, helper};
use ply_compiled::c::builtin_helper_name;
use ply_compiled::heap::Word;
use ply_compiled::rt::Ctx;
use ply_core::builtins::call;
use ply_core::{Builtin, Decimal, Fixed, IntTy, Span, Value};

type A1 = unsafe extern "C" fn(*mut Ctx, Word) -> Word;
type A2 = unsafe extern "C" fn(*mut Ctx, Word, Word) -> Word;
type A3 = unsafe extern "C" fn(*mut Ctx, Word, Word, Word) -> Word;
type N2 = unsafe extern "C" fn(*mut Ctx, Word, Word);

fn native<F: Copy>(name: &str) -> F {
    helper(&builtin_helper_name(name))
}

/// What native `name` answers over the bytes `b`, as their address and count.
fn over(ctx: &mut Ctx, name: &str, b: &[u8]) -> Word {
    unsafe { native::<A2>(name)(ctx, b.as_ptr() as Word, b.len() as Word) }
}

/// The `n` taken bytes copied out, as emitted code copies them.
fn taken(ctx: &mut Ctx, n: Word) -> Vec<u8> {
    let mut out = vec![0u8; usize::try_from(n).expect("a count")];
    unsafe { native::<N2>("?taken")(ctx, out.as_mut_ptr() as Word, n) };
    out
}

/// What native `name` leaves as taken bytes over the bytes `b`.
fn taken_over(ctx: &mut Ctx, name: &str, b: &[u8]) -> Vec<u8> {
    let n = over(ctx, name, b);
    taken(ctx, n)
}

fn builtin(b: Builtin, args: Vec<Value>) -> Value {
    call(b, args, Span::DUMMY).expect("the builtin answers")
}

fn bytes_of(v: Value) -> Vec<u8> {
    match &v {
        Value::Str(s) => s.as_bytes().to_vec(),
        Value::Bytes(b) => b.to_vec(),
        other => panic!("{} where bytes were", other.type_name()),
    }
}

#[test]
fn utf8_is_told_apart_and_mended_as_the_builtins_do() {
    let mut ctx = context();
    let cases: [&[u8]; 5] = [
        b"",
        b"plain",
        "caf\u{e9} \u{1f600}".as_bytes(),
        b"\xff\xfe",
        b"ab\xc3",
    ];
    for b in cases {
        let valid = builtin(Builtin::BytesIsUtf8, vec![Value::bytes(b)]);
        assert_eq!(Value::Bool(over(&mut ctx, "?utf8_valid", b) == 1), valid);
        let lossy = builtin(Builtin::StringOfBytesLossy, vec![Value::bytes(b)]);
        assert_eq!(taken_over(&mut ctx, "?utf8_lossy", b), bytes_of(lossy));
    }
    assert_eq!(ctx.failed, 0);
}

#[test]
fn white_space_is_the_builtins() {
    let mut ctx = context();
    for s in [
        "",
        "MiXeD",
        "stra\u{df}e \u{130}",
        "  \t tab\u{a0}\n",
        "\u{3000}x\u{2003}",
    ] {
        let start = over(&mut ctx, "?trim_start", s.as_bytes()) as usize;
        let end = over(&mut ctx, "?trim_end", s.as_bytes()) as usize;
        let trimmed = bytes_of(builtin(Builtin::StringTrim, vec![Value::str(s)]));
        if trimmed.is_empty() {
            assert!(start >= end, "{s:?} trims to nothing");
        } else {
            assert_eq!(&s.as_bytes()[start..end], trimmed.as_slice(), "{s:?}");
        }
    }
    assert_eq!(ctx.failed, 0);
}

#[test]
fn numbers_are_written_as_their_builtins_write_them() {
    let mut ctx = context();
    for v in [0, -1, 42, i64::MIN, i64::MAX] {
        let n = unsafe { native::<A1>("?int_text")(&mut ctx, v) };
        let want = bytes_of(builtin(Builtin::IntToString, vec![Value::Int(v)]));
        assert_eq!(taken(&mut ctx, n), want);
    }
    for f in [0.0, -0.5, 1e300, f64::INFINITY, f64::NAN, 0.1 + 0.2] {
        let n = unsafe { native::<A1>("?float_text")(&mut ctx, ply_core::float::bits(f) as Word) };
        let want = bytes_of(builtin(Builtin::FloatToString, vec![Value::Float(f)]));
        assert_eq!(taken(&mut ctx, n), want);
    }
    for d in [Decimal::new(-12345, 3), Decimal::new(150, 2), Decimal::ZERO] {
        let b = d.serialize();
        let lo = i64::from_le_bytes(b[..8].try_into().unwrap());
        let hi = i64::from_le_bytes(b[8..].try_into().unwrap());
        let n = unsafe { native::<A2>("?decimal_text")(&mut ctx, lo, hi) };
        let want = bytes_of(builtin(Builtin::DecimalToString, vec![Value::Decimal(d)]));
        assert_eq!(taken(&mut ctx, n), want);
    }
    for (ty, bits, b) in [
        (IntTy::U128, u128::MAX, Builtin::U128ToString),
        (IntTy::I128, (-7i128) as u128, Builtin::I128ToString),
        (IntTy::I128, i128::MIN as u128, Builtin::I128ToString),
    ] {
        let signed = Word::from(ty == IntTy::I128);
        let (lo, hi) = (bits as u64 as Word, (bits >> 64) as u64 as Word);
        let n = unsafe { native::<A3>("?wide_text")(&mut ctx, signed, lo, hi) };
        let want = bytes_of(builtin(b, vec![Value::Fixed(Fixed::new(ty, bits))]));
        assert_eq!(taken(&mut ctx, n), want);
    }
    assert_eq!(ctx.failed, 0);
}

/// Word `k` of what a parse left taken.
fn word(ctx: &mut Ctx, k: Word) -> Word {
    unsafe { native::<A1>("?taken_word")(ctx, k) }
}

#[test]
fn a_parse_answers_its_number_in_words_or_minus_one_for_none() {
    let mut ctx = context();
    for s in ["1.5", "-0", "inf", "nope", ""] {
        let n = over(&mut ctx, "?float_parse", s.as_bytes());
        match &builtin(Builtin::FloatOfString, vec![Value::str(s)]) {
            Value::Ctor { args, .. } if !args.is_empty() => {
                assert_eq!(n, 8, "{s:?}");
                assert_eq!(
                    Value::Float(f64::from_bits(word(&mut ctx, 0) as u64)),
                    args[0]
                );
            }
            _ => assert_eq!(n, -1, "{s:?}"),
        }
    }
    for s in ["1.50", "-0.001", "x", ""] {
        let n = over(&mut ctx, "?decimal_parse", s.as_bytes());
        match &builtin(Builtin::DecimalOfString, vec![Value::str(s)]) {
            Value::Ctor { args, .. } if !args.is_empty() => {
                assert_eq!(n, 16, "{s:?}");
                let mut b = [0u8; 16];
                b[..8].copy_from_slice(&word(&mut ctx, 0).to_le_bytes());
                b[8..].copy_from_slice(&word(&mut ctx, 1).to_le_bytes());
                let d = Decimal::deserialize(b);
                assert_eq!(Value::Decimal(d), args[0]);
                // The scale survives, which `to_string` shows.
                assert_eq!(d.to_string(), s);
            }
            _ => assert_eq!(n, -1, "{s:?}"),
        }
    }
    let wide = |ctx: &mut Ctx, signed: Word, s: &str| unsafe {
        native::<A3>("?wide_parse")(ctx, signed, s.as_ptr() as Word, s.len() as Word)
    };
    let max = u128::MAX.to_string();
    assert_eq!(wide(&mut ctx, 0, &max), 16);
    assert_eq!((word(&mut ctx, 0), word(&mut ctx, 1)), (-1, -1));
    assert_eq!(wide(&mut ctx, 1, "-2"), 16);
    assert_eq!((word(&mut ctx, 0), word(&mut ctx, 1)), (-2, -1));
    assert_eq!(wide(&mut ctx, 1, &max), -1);
    assert_eq!(wide(&mut ctx, 0, "-1"), -1);
    assert_eq!(ctx.failed, 0);
}

#[test]
fn a_word_past_the_taken_bytes_fails_the_run() {
    let mut ctx = context();
    assert_eq!(over(&mut ctx, "?float_parse", b"2"), 8);
    word(&mut ctx, 1);
    assert_ne!(ctx.failed, 0);
}

#[test]
fn digests_and_packings_are_the_builtins() {
    let mut ctx = context();
    let data: Vec<u8> = (0..4000u32).map(|i| (i * 7 % 251) as u8).collect();
    for b in [&b""[..], b"x", &data] {
        for (name, builtin_of) in [
            ("?blake3", Builtin::BytesBlake3),
            ("?gzip", Builtin::BytesGzip),
            ("?zstd", Builtin::BytesZstd),
        ] {
            let want = bytes_of(builtin(builtin_of, vec![Value::bytes(b)]));
            assert_eq!(taken_over(&mut ctx, name, b), want, "{name}");
        }
        let packed = taken_over(&mut ctx, "?gzip", b);
        assert_eq!(taken_over(&mut ctx, "?gunzip", &packed), b);
    }
    assert_eq!(over(&mut ctx, "?gunzip", b"not gzip"), -1);
    assert_eq!(ctx.failed, 0);
}

#[test]
fn a_credential_holds_its_bytes_and_verifies_only_its_own_string() {
    let mut ctx = context();
    let made = |ctx: &mut Ctx, b: &[u8], text: Word| unsafe {
        native::<A3>("?secret_of")(ctx, b.as_ptr() as Word, b.len() as Word, text)
    };
    let verify = |ctx: &mut Ctx, s: Word, b: &[u8]| unsafe {
        native::<A3>("?secret_verify")(ctx, s, b.as_ptr() as Word, b.len() as Word)
    };
    let text = made(&mut ctx, b"hunter2", 1);
    let raw = made(&mut ctx, b"hunter2", 0);
    assert_ne!(text, 0);
    assert_ne!(raw, 0);
    assert_eq!(verify(&mut ctx, text, b"hunter2"), 1);
    assert_eq!(verify(&mut ctx, text, b"hunter3"), 0);
    assert_eq!(verify(&mut ctx, text, b""), 0);
    // A `Secret<Bytes>` is no stored password.
    assert_eq!(verify(&mut ctx, raw, b"hunter2"), 0);
    assert_eq!(ctx.failed, 0);
}
