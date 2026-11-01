use crate::bare::{helper, new_context};
use ply_compiled::rt::Context;

type Made = unsafe extern "C" fn(*mut Context, i64) -> i64;
type ScopedCall = unsafe extern "C" fn(*mut Context, i64) -> i64;

#[test]
fn a_scoped_value_can_be_read_repeatedly_and_reading_it_after_release_fails() {
    let made: Made = helper("builtin_elaborated_hold_made");
    let read: ScopedCall = helper("builtin_elaborated_hold_read");
    let taken: ScopedCall = helper("builtin_elaborated_hold_taken");
    let mut context = new_context();
    context.begin();
    let c: *mut Context = &mut context;
    for word in [4096i64, 8, 0, -8] {
        let h = unsafe { made(c, word) };
        assert_eq!(unsafe { read(c, h) }, word);
        assert_eq!(unsafe { read(c, h) }, word);
        assert_eq!(unsafe { taken(c, h) }, word);
        assert!(context.take_failure().is_none());
        assert_eq!(
            unsafe { read(c, h) },
            0,
            "a scoped value let go holds nothing"
        );
        let said = context
            .take_failure()
            .expect("a read after the release fails")
            .message;
        assert!(said.contains("read"), "{said}");
        context.failed = 0;
        assert_eq!(unsafe { taken(c, h) }, 0);
        assert!(context.take_failure().is_some(), "a second release fails");
        context.failed = 0;
    }
    context.end();
}
