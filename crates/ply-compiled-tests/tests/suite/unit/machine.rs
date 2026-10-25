//! The natives strings and byte strings reach the runtime through, called as a unit binds them over
//! hand-built bytes: each answers what the value builtin of its name answers, as machine data.

use crate::bare::{context, helper};
use ply_compiled::c::builtin_helper_name;
use ply_compiled::heap::Word;
use ply_compiled::rt::Ctx;
use ply_core::builtins::call;
use ply_core::{Builtin, Span, Value};

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
fn digests_and_packings_are_the_builtins() {
    let mut ctx = context();
    let data: Vec<u8> = (0..4000u32).map(|i| (i * 7 % 251) as u8).collect();
    for b in [&b""[..], b"x", &data] {
        for (name, builtin_of) in [("?gzip", Builtin::BytesGzip), ("?zstd", Builtin::BytesZstd)] {
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
