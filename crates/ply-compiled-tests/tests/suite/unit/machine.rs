//! The natives strings and byte strings reach the runtime through, called as a unit binds them over
//! hand-built bytes: each answers what the value builtin of its name answers, as machine data.

use crate::bare::{context, helper};
use ply_compiled::c::builtin_helper_name;
use ply_compiled::heap::Word;
use ply_compiled::rt::Ctx;

type A3 = unsafe extern "C" fn(*mut Ctx, Word, Word, Word) -> Word;

fn native<F: Copy>(name: &str) -> F {
    helper(&builtin_helper_name(name))
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
