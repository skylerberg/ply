//! A hold as emitted code meets it: made of a word it takes, read and taken through natives that
//! never read or count what it holds.

use crate::bare::{context, helper};
use ply_compiled::rt::Ctx;

type Made = unsafe extern "C" fn(*mut Ctx, i64) -> i64;
type Held = unsafe extern "C" fn(*mut Ctx, i64) -> i64;

#[test]
fn a_hold_lends_and_hands_back_the_word_it_was_made_of_and_refuses_a_read_once_let_go() {
    let made: Made = helper("builtin_elaborated_hold_made");
    let read: Held = helper("builtin_elaborated_hold_read");
    let taken: Held = helper("builtin_elaborated_hold_taken");
    let mut ctx = context();
    ctx.begin(1_000);
    let c: *mut Ctx = &mut ctx;
    // Raw words the runtime would take for objects, if it read them: it never does.
    for word in [4096i64, 8, 0, -8] {
        let h = unsafe { made(c, word) };
        assert_eq!(unsafe { read(c, h) }, word);
        assert_eq!(unsafe { read(c, h) }, word);
        assert_eq!(unsafe { taken(c, h) }, word);
        assert!(ctx.take_failure().is_none());
        assert_eq!(unsafe { read(c, h) }, 0, "a hold let go holds nothing");
        let said = ctx
            .take_failure()
            .expect("a read after the release fails")
            .message;
        assert!(said.contains("read"), "{said}");
        ctx.failed = 0;
        assert_eq!(unsafe { taken(c, h) }, 0);
        assert!(ctx.take_failure().is_some(), "a second release fails");
        ctx.failed = 0;
    }
    ctx.end();
}
