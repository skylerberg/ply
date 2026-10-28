use crate::bare::{context, helper};
use ply_compiled::heap::{HEADER, Word};
use ply_compiled::hosted::meter::{rt_meter_allocations, rt_meter_bytes, rt_meter_steps};
use ply_compiled::rt::Ctx;

type Alloc = unsafe extern "C" fn(*mut Ctx, Word) -> Word;
type Free = unsafe extern "C" fn(*mut Ctx, Word, Word);
type Entered = unsafe extern "C" fn(*mut Ctx) -> Word;
type Left = unsafe extern "C" fn(*mut Ctx, Word);

/// A block of one word, with its header.
const BOX: Word = HEADER as Word + 8;

/// The calls, objects and bytes the entry has spent, as the three natives read them.
fn reading(ctx: &mut Ctx) -> [i64; 3] {
    unsafe {
        [
            rt_meter_steps(ctx).0,
            rt_meter_allocations(ctx).0,
            rt_meter_bytes(ctx).0,
        ]
    }
}

fn spent(before: [i64; 3], after: [i64; 3]) -> [i64; 3] {
    [0, 1, 2].map(|i| after[i] - before[i])
}

#[test]
fn a_tests_entry_reads_what_it_has_spent_so_far_the_schedulers_work_aside() {
    let mut ctx = context();
    ctx.tested = true;
    ctx.begin(1_000);
    let c: *mut Ctx = &mut ctx;
    let before = reading(&mut ctx);
    ctx.ticks += 7;
    let mine = unsafe { helper::<Alloc>("rt_alloc_bytes")(c, BOX) };
    let was = unsafe { helper::<Entered>("rt_scheduler_entered")(c) };
    ctx.ticks += 100;
    let theirs = unsafe { helper::<Alloc>("rt_alloc_bytes")(c, BOX) };
    unsafe { helper::<Left>("rt_scheduler_left")(c, was) };
    let after = reading(&mut ctx);
    assert!(ctx.take_failure().is_none());
    assert_eq!(spent(before, after), [7, 1, BOX]);
    for w in [mine, theirs] {
        unsafe { helper::<Free>("rt_free")(c, w, BOX) };
    }
    ctx.end();
}

#[test]
fn an_entry_that_is_no_tests_fails_at_a_reading() {
    let mut ctx = context();
    ctx.begin(1_000);
    let _ = reading(&mut ctx);
    let failed = ctx.take_failure().expect("the reading fails the entry");
    assert_eq!(failed.code, ply_core::codes::HOST_UNBOUND, "{failed}");
    assert!(failed.message.contains("only `ply test`"), "{failed}");
    ctx.end();
}
