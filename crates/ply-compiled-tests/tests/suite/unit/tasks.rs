use crate::bare::context;
use ply_compiled::native::{Addr, I64};
use ply_compiled::rt::{Ctx, FAILED_ABORT, FAILED_CANCELLED, rt_taken};
use ply_compiled::tasks::{
    self, Body, FAILED, FINISHED, Message, RAISED, Region, Shield, Shields, UNWOUND, failure_of,
    rt_host_now, rt_ticket_taken, rt_ticket_watch, rt_tickets_answered, rt_tickets_park,
};
use ply_core::{Span, codes};
use ply_host::jobs::{self, Work};
use std::cell::{Cell, RefCell};
use std::sync::Arc;

thread_local! {
    static HEARD: RefCell<Vec<i64>> = const { RefCell::new(Vec::new()) };
    static RELEASED: Cell<usize> = const { Cell::new(0) };
}

fn heard() -> Vec<i64> {
    HEARD.with(|h| std::mem::take(&mut *h.borrow_mut()))
}

fn asking(code: i64, n: i64) -> Message {
    Message {
        code,
        numbers: [n, 0],
        ..Message::default()
    }
}

/// Asks its loop twice, the second time one more than it heard, then keeps what it heard last.
unsafe fn asks_twice(_ctx: *mut Ctx, region: usize) {
    let region = region as *mut Region;
    let first = unsafe { tasks::to_loop(region, asking(1, 10)) };
    let second = unsafe { tasks::to_loop(region, asking(2, first.numbers[0] + 1)) };
    HEARD.with(|h| h.borrow_mut().push(second.numbers[0]));
}

unsafe fn released(_ctx: *mut Ctx, _arg: usize) {
    RELEASED.with(|r| r.set(r.get() + 1));
}

fn body(run: unsafe fn(*mut Ctx, usize), region: *mut Region) -> Body {
    Body {
        run,
        release: released,
        arg: region as usize,
    }
}

#[test]
fn a_task_runs_on_its_own_stack_until_it_asks_and_on_from_there_when_resumed() {
    let mut ctx = context();
    let mut region = Region::open(&mut ctx, true, Span::DUMMY);
    let r: *mut Region = &mut *region;
    region.start(1, body(asks_twice, r));
    let first = unsafe { tasks::resume(r, 1, Message::default()) };
    assert_eq!((first.code, first.numbers[0]), (1, 10));
    let second = unsafe { tasks::resume(r, 1, asking(0, 41)) };
    assert_eq!((second.code, second.numbers[0]), (2, 42));
    let end = unsafe { tasks::resume(r, 1, asking(0, 7)) };
    assert_eq!(end.code, FINISHED);
    assert_eq!(heard(), [7]);
    region.free(1);
    assert_eq!(
        RELEASED.with(Cell::get),
        0,
        "a body that ran is not released again"
    );
    drop(region);
}

#[test]
fn tasks_interleave_through_their_loop_each_where_it_stopped() {
    let mut ctx = context();
    let mut region = Region::open(&mut ctx, false, Span::DUMMY);
    let r: *mut Region = &mut *region;
    region.start(1, body(asks_twice, r));
    region.start(2, body(asks_twice, r));
    unsafe {
        assert_eq!(tasks::resume(r, 1, Message::default()).code, 1);
        assert_eq!(tasks::resume(r, 2, Message::default()).code, 1);
        assert_eq!(tasks::resume(r, 2, asking(0, 100)).numbers[0], 101);
        assert_eq!(tasks::resume(r, 1, asking(0, 200)).numbers[0], 201);
        assert_eq!(tasks::resume(r, 1, asking(0, 1)).code, FINISHED);
        assert_eq!(tasks::resume(r, 2, asking(0, 2)).code, FINISHED);
    }
    assert_eq!(heard(), [1, 2]);
    drop(region);
}

unsafe fn unwinds(ctx: *mut Ctx, _: usize) {
    unsafe { (*ctx).failed = FAILED_CANCELLED };
}

unsafe fn raises(ctx: *mut Ctx, _: usize) {
    unsafe { (*ctx).failed = FAILED_ABORT };
}

unsafe fn breaks(ctx: *mut Ctx, _: usize) {
    unsafe { (*ctx).failed = 1 };
}

/// A body that did not answer leaves the context clear for the loop, and how it ended is put back
/// where the loop asks.
#[test]
fn a_body_that_unwinds_raises_or_fails_says_so_and_what_it_left_is_put_back() {
    let mut ctx = context();
    let c: *mut Ctx = &mut ctx;
    let mut region = Region::open(c, true, Span::DUMMY);
    let r: *mut Region = &mut *region;
    region.start(1, body(unwinds, r));
    region.start(2, body(raises, r));
    region.start(3, body(breaks, r));
    let ended = |id| unsafe { tasks::resume(r, id, Message::default()).code };
    assert_eq!(ended(1), UNWOUND);
    assert_eq!(unsafe { (*c).failed }, 0);
    assert_eq!(ended(2), RAISED);
    assert_eq!(unsafe { (*c).failed }, 0);
    assert_eq!(ended(3), FAILED);
    assert_eq!(unsafe { (*c).failed }, 0);
    assert!(!region.restore(1, None), "an unwound task leaves nothing");
    assert!(region.restore(2, None));
    assert_eq!(unsafe { (*c).failed }, FAILED_ABORT);
    unsafe { (*c).failed = 0 };
    assert!(region.restore(3, Some("noted".to_string())));
    assert_eq!(unsafe { (*c).failed }, 1);
    unsafe { (*c).failed = 0 };
    drop(region);
}

/// Asks its loop once, then ends.
unsafe fn asks_once(_ctx: *mut Ctx, region: usize) {
    unsafe { tasks::to_loop(region as *mut Region, asking(1, 0)) };
}

#[test]
fn a_region_keeps_the_stacks_its_tasks_gave_back_as_many_as_ran_at_once() {
    let mut ctx = context();
    let mut region = Region::open(&mut ctx, false, Span::DUMMY);
    let r: *mut Region = &mut *region;
    for id in 1..=3 {
        region.start(id, body(asks_once, r));
        unsafe { tasks::resume(r, id, Message::default()) };
    }
    assert_eq!(region.stacks(), 3);
    for id in 1..=3 {
        assert_eq!(
            unsafe { tasks::resume(r, id, Message::default()) }.code,
            FINISHED
        );
        region.free(id);
    }
    assert_eq!(
        region.stacks(),
        3,
        "the stacks wait for the region's next tasks"
    );
    for id in 4..=5 {
        region.start(id, body(asks_once, r));
        unsafe { tasks::resume(r, id, Message::default()) };
    }
    assert_eq!(
        region.stacks(),
        3,
        "the next tasks ran on stacks given back"
    );
    for id in 4..=5 {
        unsafe { tasks::resume(r, id, Message::default()) };
        region.free(id);
    }
    drop(region);
}

#[test]
fn a_task_the_region_never_ran_lets_go_of_its_body() {
    let mut ctx = context();
    let mut region = Region::open(&mut ctx, true, Span::DUMMY);
    let r: *mut Region = &mut *region;
    let before = RELEASED.with(Cell::get);
    region.start(1, body(asks_once, r));
    region.start(2, body(asks_once, r));
    region.free(1);
    assert_eq!(RELEASED.with(Cell::get), before + 1);
    drop(region);
    assert_eq!(RELEASED.with(Cell::get), before + 2);
}

/// Stands in an `acquire`, asks its loop, and leaves it, keeping whether a cancel landed there.
unsafe fn acquires(_ctx: *mut Ctx, region: usize) {
    let region = region as *mut Region;
    let id = unsafe { (*region).shield(Shield::Acquire) }.expect("a task runs");
    unsafe { tasks::to_loop(region, asking(1, 0)) };
    let lands = unsafe { (*region).unshield(id, Shield::Acquire) };
    HEARD.with(|h| h.borrow_mut().push(i64::from(lands)));
}

#[test]
fn a_cancel_held_back_for_a_bracket_lands_as_the_task_leaves_it() {
    let mut ctx = context();
    let mut region = Region::open(&mut ctx, true, Span::DUMMY);
    let r: *mut Region = &mut *region;
    region.start(1, body(acquires, r));
    unsafe { tasks::resume(r, 1, Message::default()) };
    let inside = Shields {
        acquiring: 1,
        releasing: 0,
        stopping: false,
    };
    assert_eq!(region.shields(1), inside);
    assert_eq!(region.shielded(), [(1, inside)]);
    region.hold_back(1, true);
    assert_eq!(
        unsafe { tasks::resume(r, 1, Message::default()) }.code,
        FINISHED
    );
    assert_eq!(heard(), [1]);
    assert_eq!(region.shields(1), Shields::default());
    drop(region);
}

#[test]
fn a_regions_failure_reads_as_the_scheduler_writes_it() {
    let ctx = context();
    // What `std.internal.schedule`'s `failure_bytes` writes, pinned there.
    let bytes = b"\x05E0414\x01m\x01\x04\x02\x06\x01\x01t\x01\x01n";
    let d = failure_of(&ctx, bytes).expect("it reads");
    assert_eq!(d.code, codes::DEADLOCK);
    assert_eq!(d.message, "m");
    assert_eq!(d.labels.len(), 1);
    assert!(d.labels[0].primary);
    assert_eq!(d.labels[0].message, "t");
    assert_eq!(d.notes, ["n"]);
    let unknown = failure_of(&ctx, b"\x02E1\x01m\x00\x00").expect_err("no such code");
    assert!(unknown.contains("`E1`"), "{unknown}");
    assert!(failure_of(&ctx, &bytes[..5]).is_err());
}

/// The bytes a native left taken, `n` of them, copied out as emitted code copies them.
fn taken(ctx: &mut Ctx, n: i64) -> Vec<u8> {
    let mut out = vec![0u8; usize::try_from(n).expect("a count")];
    unsafe { rt_taken(ctx, Addr(out.as_mut_ptr()), I64(n)) };
    out
}

#[test]
fn a_watched_ticket_wakes_the_park_and_its_answer_is_taken_in_bytes() {
    let mut ctx = context();
    let runtime = ply_host::Host::new().runtime();
    let run = Arc::clone(ply_host::registry::run_of(&*runtime).expect("a host made it"));
    ctx.set_host(
        Arc::new(ply_core::HostBinding::hermetic()),
        Some(runtime),
        None,
    );
    let submit = |answer: &'static [u8]| {
        jobs::submit(
            Work::Computing,
            &run,
            Span::DUMMY,
            "test",
            "a test job",
            move || Ok(answer.to_vec()),
        )
        .expect("a thread is had")
        .token
    };
    let watched = submit(b"watched");
    let unwatched = submit(b"unwatched");
    assert_eq!(
        unsafe { rt_ticket_watch(&mut ctx, I64(watched as i64)) }.0,
        1
    );
    assert_eq!(unsafe { rt_tickets_park(&mut ctx, I64(-1)) }.0, 1);
    let n = unsafe { rt_tickets_answered(&mut ctx) };
    assert_eq!(taken(&mut ctx, n.0), watched.to_le_bytes());
    let n = unsafe { rt_ticket_taken(&mut ctx, I64(watched as i64)) };
    assert_eq!(taken(&mut ctx, n.0), b"watched");
    let n = unsafe { rt_tickets_answered(&mut ctx) };
    assert_eq!(taken(&mut ctx, n.0), b"", "a ticket taken is answered once");
    assert!(ctx.take_failure().is_none());
    assert_ne!(unwatched, watched, "each job has a ticket of its own");
    let earlier = unsafe { rt_host_now(&mut ctx) }.0;
    assert!(unsafe { rt_host_now(&mut ctx) }.0 >= earlier);
    assert!(ctx.take_failure().is_none());
}

#[test]
fn a_ticket_another_run_handed_out_is_refused() {
    let mut ctx = context();
    ctx.set_host(
        Arc::new(ply_core::HostBinding::hermetic()),
        Some(ply_host::Host::new().runtime()),
        None,
    );
    assert_eq!(
        unsafe { rt_ticket_watch(&mut ctx, I64(u32::MAX as i64)) }.0,
        0
    );
    assert!(ctx.take_failure().is_some());
}
