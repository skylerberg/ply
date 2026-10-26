use crate::bare::context;
use ply_compiled::emitted::{Block, Handed, Handle, PlyFnDesc, PlyType};
use ply_compiled::heap::{self, IMMORTAL, Word};
use ply_compiled::native::{Addr, Answered, I64};
use ply_compiled::rt::{Ctx, FAILED_ABORT, FAILED_CANCELLED, FAILED_PENDING, rt_taken};
use ply_compiled::tasks::{
    self, ANSWERED, Body, FAILED, FINISHED, Message, RAISED, RESUME_UNWIND, Region, Shield,
    Shields, UNWOUND, failure_of, rt_host_now, rt_ticket_taken, rt_ticket_watch,
    rt_tickets_answered, rt_tickets_park,
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
    let mut region = Region::open(&mut ctx, true);
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
    let mut region = Region::open(&mut ctx, false);
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
    let mut region = Region::open(c, true);
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
    let mut region = Region::open(&mut ctx, false);
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
    let mut region = Region::open(&mut ctx, true);
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
    let mut region = Region::open(&mut ctx, true);
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
        Arc::new(ply_core::HostBinding::unbound()),
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
        Arc::new(ply_core::HostBinding::unbound()),
        Some(ply_host::Host::new().runtime()),
        None,
    );
    assert_eq!(
        unsafe { rt_ticket_watch(&mut ctx, I64(u32::MAX as i64)) }.0,
        0
    );
    assert!(ctx.take_failure().is_some());
}

// --- The scheduler's objects ---------------------------------------------------------------

unsafe extern "C" fn untouched(_ctx: *mut Ctx, _w: Word) -> Word {
    0
}

unsafe extern "C" fn lifted(_ctx: *mut Ctx) -> Word {
    0
}

const fn scalar(identity: u64, spelled: &'static [u8]) -> PlyType {
    PlyType {
        identity,
        spelled: spelled.as_ptr(),
        spelled_len: spelled.len() as u32,
        inc: untouched,
        dec: untouched,
        lower: untouched,
        lift: lifted,
    }
}

static INTS: PlyType = scalar(7, b"Int");
static TEXTS: PlyType = scalar(8, b"String");

/// The word a native's answer is, each of one word.
fn word_of<T>(x: T) -> Word {
    assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<Word>());
    let w = unsafe { std::mem::transmute_copy(&x) };
    std::mem::forget(x);
    w
}

fn handle(w: Word) -> Handle {
    <Handle as Answered>::of(w)
}

fn handed(w: Word) -> Handed {
    <Handed as Answered>::of(w)
}

fn let_go(ctx: &mut Ctx, w: Word) {
    heap::release_object(ctx, w);
}

#[test]
fn a_kept_value_opens_at_the_type_it_was_kept_at_and_fails_the_run_at_another() {
    let mut ctx = context();
    let c: *mut Ctx = &mut ctx;
    let k = word_of(unsafe { tasks::rt_keep(c, handed(42), &INTS) });
    assert_eq!(word_of(unsafe { tasks::rt_open(c, handle(k), &INTS) }), 42);
    assert!(ctx.take_failure().is_none());
    unsafe { tasks::rt_open(c, handle(k), &TEXTS) };
    let d = ctx
        .take_failure()
        .expect("a kept `Int` opened as a `String`");
    assert!(
        d.message.contains("`Int` was opened as `String`"),
        "{}",
        d.message
    );
    let_go(&mut ctx, k);
    assert_eq!((ctx.heap.live(), ctx.heap.bridges()), (0, 0));
}

thread_local! {
    static STACKS: Cell<Word> = const { Cell::new(0) };
    static ENTERED: RefCell<Vec<Word>> = const { RefCell::new(Vec::new()) };
}

/// A closure block no count is kept of, as emitted code lays out one that captures nothing.
#[repr(C, align(8))]
struct Immortal {
    rc: u32,
    desc: &'static PlyFnDesc,
}

unsafe extern "C" fn kept_alone(_ctx: *mut Ctx, _f: Handed) -> Word {
    0
}

const fn function(uniform: unsafe extern "C" fn(*mut Ctx, Word, *const Word) -> Word) -> PlyFnDesc {
    PlyFnDesc {
        uniform,
        typed: None,
        convention: 0,
        release: kept_alone,
        share: untouched,
        keep: untouched,
    }
}

/// Keeps the descriptor and the first word it was entered with, and answers 5 kept.
unsafe extern "C" fn answers_five(ctx: *mut Ctx, _f: Word, _args: *const Word) -> Word {
    let c = unsafe { &mut *ctx };
    let first = if c.entering_words == 0 {
        0
    } else {
        unsafe { *(c.entering_words as *const Word) }
    };
    ENTERED.with(|e| e.borrow_mut().extend([c.entering_desc, first]));
    c.entering_desc = 0;
    c.entering_words = 0;
    word_of(unsafe { tasks::rt_keep(ctx, handed(5), &INTS) })
}

/// Asks its loop code 3 with 4 and 5, and answers nothing once it hears back.
unsafe extern "C" fn asks_three(ctx: *mut Ctx, _f: Word, _args: *const Word) -> Word {
    let s = STACKS.with(Cell::get);
    unsafe { tasks::rt_task_yielded(ctx, handle(s), I64(3), I64(4), I64(5)) };
    0
}

static FIVE: PlyFnDesc = function(answers_five);
static THREE: PlyFnDesc = function(asks_three);
static ANSWERS_FIVE: Immortal = Immortal {
    rc: IMMORTAL,
    desc: &FIVE,
};
static ASKS_THREE: Immortal = Immortal {
    rc: IMMORTAL,
    desc: &THREE,
};

fn block(f: &'static Immortal) -> Block {
    <Block as Answered>::of(f as *const Immortal as Word)
}

/// Stacks for a region, a task handle in it, and `f` made a body of task 1 answering into it, the
/// row's descriptor and its words `row` and `words`.
fn started(ctx: &mut Ctx, f: &'static Immortal, row: &[Word], words: &mut [Word]) -> (Word, Word) {
    let c: *mut Ctx = ctx;
    let s = word_of(unsafe { tasks::rt_stacks_opened(c, I64(1)) });
    let region = unsafe { tasks::rt_stacks_region(c, handle(s)) };
    let t = word_of(unsafe { tasks::rt_task_made(c, region) });
    let b = word_of(unsafe {
        tasks::rt_task_body(c, handle(t), block(f), row.as_ptr(), words.as_ptr())
    });
    // The body copied the words it was handed.
    words.iter_mut().for_each(|w| *w = 0);
    unsafe { tasks::rt_task_started(c, handle(s), I64(1), handle(b)) };
    let_go(ctx, b);
    STACKS.with(|x| x.set(s));
    (s, t)
}

#[test]
fn a_task_runs_its_body_with_the_words_its_maker_held_and_answers_into_its_handle() {
    let mut ctx = context();
    let c: *mut Ctx = &mut ctx;
    let row = [1, 0, -1];
    let (s, t) = started(&mut ctx, &ANSWERS_FIVE, &row, &mut [77]);
    assert_eq!(unsafe { tasks::rt_task_said(c, handle(t), I64(1)) }.0, 1);
    let ended = unsafe { tasks::rt_task_resumed(c, handle(s), I64(1), I64(0), I64(0)) };
    assert_eq!(ended.0, FINISHED);
    assert_eq!(ENTERED.with(|e| e.take()), [row.as_ptr() as Word, 77]);
    assert_eq!(
        unsafe { tasks::rt_task_said(c, handle(t), I64(2)) }.0,
        ANSWERED
    );
    let answer = word_of(unsafe { tasks::rt_task_answer(c, handle(t)) });
    assert_eq!(
        word_of(unsafe { tasks::rt_open(c, handle(answer), &INTS) }),
        5
    );
    let_go(&mut ctx, answer);
    unsafe { tasks::rt_task_freed(c, handle(s), I64(1)) };
    let_go(&mut ctx, t);
    let_go(&mut ctx, s);
    assert!(ctx.take_failure().is_none());
    assert_eq!((ctx.heap.live(), ctx.heap.bridges()), (0, 0));
}

#[test]
fn a_task_asks_where_its_perform_stood_and_unwinds_or_raises_as_its_loop_resumes_it() {
    let mut ctx = context();
    let c: *mut Ctx = &mut ctx;
    (ctx.performed_root, ctx.performed_start, ctx.performed_end) = (9, 10, 11);
    let (s, t) = started(&mut ctx, &ASKS_THREE, &[0], &mut []);
    let asked = unsafe { tasks::rt_task_resumed(c, handle(s), I64(1), I64(0), I64(0)) };
    assert_eq!(asked.0, 3);
    let said = |f: unsafe extern "C" fn(*mut Ctx, Handle, I64) -> I64, i| unsafe { f(c, handle(s), I64(i)) }.0;
    assert_eq!(
        (
            said(tasks::rt_asked_number, 0),
            said(tasks::rt_asked_number, 1)
        ),
        (4, 5)
    );
    let site = [0, 1, 2].map(|i| said(tasks::rt_asked_site, i));
    assert_eq!(site, [9, 10, 11]);
    let ended = unsafe { tasks::rt_task_resumed(c, handle(s), I64(1), I64(RESUME_UNWIND), I64(0)) };
    assert_eq!(ended.0, UNWOUND);
    assert_eq!(ctx.failed, 0);
    unsafe { tasks::rt_task_freed(c, handle(s), I64(1)) };
    let_go(&mut ctx, t);
    let_go(&mut ctx, s);

    let (s, t) = started(&mut ctx, &ASKS_THREE, &[0], &mut []);
    unsafe { tasks::rt_task_resumed(c, handle(s), I64(1), I64(0), I64(0)) };
    let failure = b"\x05E0414\x01m\x01\x04\x02\x06\x01\x01t\x01\x01n";
    let ended = unsafe {
        tasks::rt_task_resumed_raising(
            c,
            handle(s),
            I64(1),
            Addr(failure.as_ptr() as *mut u8),
            I64(failure.len() as i64),
        )
    };
    assert_eq!(ended.0, RAISED);
    let none = Addr(std::ptr::null_mut());
    assert_eq!(
        unsafe { tasks::rt_task_restored(c, handle(s), I64(1), none, I64(0)) }.0,
        1
    );
    assert_eq!(ctx.failed, FAILED_PENDING);
    let d = ctx.take_failure().expect("the raise is put back");
    assert_eq!(d.message, "m");
    unsafe { tasks::rt_task_freed(c, handle(s), I64(1)) };
    let_go(&mut ctx, t);
    let_go(&mut ctx, s);
    assert_eq!((ctx.heap.live(), ctx.heap.bridges()), (0, 0));
}
