use crate::bare::{context, helper};
use ply_compiled::rt::Ctx;

type Made = unsafe extern "C" fn(*mut Ctx, i64) -> i64;
type Held = unsafe extern "C" fn(*mut Ctx, i64) -> i64;

#[test]
fn a_scoped_value_can_be_read_repeatedly_and_reading_it_after_release_fails() {
    let made: Made = helper("builtin_elaborated_hold_made");
    let read: Held = helper("builtin_elaborated_hold_read");
    let taken: Held = helper("builtin_elaborated_hold_taken");
    let mut ctx = context();
    ctx.begin();
    let c: *mut Ctx = &mut ctx;
    for word in [4096i64, 8, 0, -8] {
        let h = unsafe { made(c, word) };
        assert_eq!(unsafe { read(c, h) }, word);
        assert_eq!(unsafe { read(c, h) }, word);
        assert_eq!(unsafe { taken(c, h) }, word);
        assert!(ctx.take_failure().is_none());
        assert_eq!(
            unsafe { read(c, h) },
            0,
            "a scoped value let go holds nothing"
        );
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
