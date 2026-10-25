//! The natives strings and byte strings reach the runtime through, called as a unit binds them over
//! hand-built bytes: each answers what the value builtin of its name answers, as machine data.

use crate::bare::{context, helper};
use ply_compiled::c::builtin_helper_name;
use ply_compiled::heap::Word;
use ply_compiled::rt::Ctx;
use ply_core::builtins::call;
use ply_core::{Builtin, Decimal, Span, Value};

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
fn a_decimal_is_written_as_its_builtin_writes_it() {
    let mut ctx = context();
    for d in [Decimal::new(-12345, 3), Decimal::new(150, 2), Decimal::ZERO] {
        let b = d.serialize();
        let lo = i64::from_le_bytes(b[..8].try_into().unwrap());
        let hi = i64::from_le_bytes(b[8..].try_into().unwrap());
        let n = unsafe { native::<A2>("?decimal_text")(&mut ctx, lo, hi) };
        let want = bytes_of(builtin(Builtin::DecimalToString, vec![Value::Decimal(d)]));
        assert_eq!(taken(&mut ctx, n), want);
    }
    assert_eq!(ctx.failed, 0);
}

/// Word `k` of what a parse left taken.
fn word(ctx: &mut Ctx, k: Word) -> Word {
    unsafe { native::<A1>("?taken_word")(ctx, k) }
}

#[test]
fn a_decimal_parse_answers_its_two_words_or_minus_one_for_none() {
    let mut ctx = context();
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
    assert_eq!(ctx.failed, 0);
}

#[test]
fn a_word_past_the_taken_bytes_fails_the_run() {
    let mut ctx = context();
    assert_eq!(over(&mut ctx, "?decimal_parse", b"2"), 16);
    word(&mut ctx, 2);
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
