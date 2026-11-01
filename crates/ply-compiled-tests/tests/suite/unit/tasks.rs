use crate::bare::new_context;
use ply_compiled::emitted::{Block, Handle, OwnedWord, PlyFunctionDescriptor, PlyType};
use ply_compiled::heap::{self, IMMORTAL, Word};
use ply_compiled::native::{Address, I64, Returned};
use ply_compiled::rt::{
    Context, FAILED_CANCELLED, FAILED_PENDING_RAISE, FAILED_UNWOUND_TO_HANDLER, rt_taken,
};
use ply_compiled::tasks::{
    self, Body, BracketPhase, FAILED, FINISHED, Message, RAISED, RESUME_UNWIND, RETURNED,
    Scheduler, Shields, UNWOUND, failure_of, rt_host_now, rt_ticket_watch, rt_tickets_answered,
    rt_tickets_park, take_ticket_bytes,
};
use ply_core::{Span, codes};
use ply_host::jobs::{self, Work};
use std::cell::{Cell, RefCell};
use std::sync::Arc;

thread_local! {
    static RECEIVED: RefCell<Vec<i64>> = const { RefCell::new(Vec::new()) };
    static RELEASED: Cell<usize> = const { Cell::new(0) };
}

fn take_received() -> Vec<i64> {
    RECEIVED.with(|h| std::mem::take(&mut *h.borrow_mut()))
}

fn request_message(code: i64, n: i64) -> Message {
    Message {
        code,
        numbers: [n, 0],
        ..Message::default()
    }
}

unsafe fn requests_twice(_context: *mut Context, scheduler: usize) {
    let scheduler = scheduler as *mut Scheduler;
    let first = unsafe { tasks::switch_to_loop(scheduler, request_message(1, 10)) };
    let second =
        unsafe { tasks::switch_to_loop(scheduler, request_message(2, first.numbers[0] + 1)) };
    RECEIVED.with(|h| h.borrow_mut().push(second.numbers[0]));
}

unsafe fn released(_context: *mut Context, _arg: usize) {
    RELEASED.with(|r| r.set(r.get() + 1));
}

fn body(run: unsafe fn(*mut Context, usize), scheduler: *mut Scheduler) -> Body {
    Body {
        run,
        release: released,
        arg: scheduler as usize,
    }
}

#[test]
fn a_task_runs_on_its_own_stack_until_it_pauses_and_continues_from_there_when_resumed() {
    let mut context = new_context();
    let mut scheduler = Scheduler::open(&mut context, true);
    let r: *mut Scheduler = &mut *scheduler;
    scheduler.start(1, body(requests_twice, r));
    let first = unsafe { tasks::resume(r, 1, Message::default()) };
    assert_eq!((first.code, first.numbers[0]), (1, 10));
    let second = unsafe { tasks::resume(r, 1, request_message(0, 41)) };
    assert_eq!((second.code, second.numbers[0]), (2, 42));
    let end = unsafe { tasks::resume(r, 1, request_message(0, 7)) };
    assert_eq!(end.code, FINISHED);
    assert_eq!(take_received(), [7]);
    scheduler.free(1);
    assert_eq!(
        RELEASED.with(Cell::get),
        0,
        "a body that ran is not released again"
    );
    drop(scheduler);
}

#[test]
fn tasks_take_turns_in_the_scheduler_loop_and_each_continues_where_it_paused() {
    let mut context = new_context();
    let mut scheduler = Scheduler::open(&mut context, false);
    let r: *mut Scheduler = &mut *scheduler;
    scheduler.start(1, body(requests_twice, r));
    scheduler.start(2, body(requests_twice, r));
    unsafe {
        assert_eq!(tasks::resume(r, 1, Message::default()).code, 1);
        assert_eq!(tasks::resume(r, 2, Message::default()).code, 1);
        assert_eq!(tasks::resume(r, 2, request_message(0, 100)).numbers[0], 101);
        assert_eq!(tasks::resume(r, 1, request_message(0, 200)).numbers[0], 201);
        assert_eq!(tasks::resume(r, 1, request_message(0, 1)).code, FINISHED);
        assert_eq!(tasks::resume(r, 2, request_message(0, 2)).code, FINISHED);
    }
    assert_eq!(take_received(), [1, 2]);
    drop(scheduler);
}

unsafe fn unwinds(context: *mut Context, _: usize) {
    unsafe { (*context).failed = FAILED_CANCELLED };
}

unsafe fn raises(context: *mut Context, _: usize) {
    unsafe { (*context).failed = FAILED_UNWOUND_TO_HANDLER };
}

unsafe fn breaks(context: *mut Context, _: usize) {
    unsafe { (*context).failed = 1 };
}

#[test]
fn a_task_body_that_unwinds_raises_or_fails_reports_which_of_the_three_and_restore_recreates_the_failure()
 {
    let mut context = new_context();
    let c: *mut Context = &mut context;
    let mut scheduler = Scheduler::open(c, true);
    let r: *mut Scheduler = &mut *scheduler;
    scheduler.start(1, body(unwinds, r));
    scheduler.start(2, body(raises, r));
    scheduler.start(3, body(breaks, r));
    let ended = |id| unsafe { tasks::resume(r, id, Message::default()).code };
    assert_eq!(ended(1), UNWOUND);
    assert_eq!(unsafe { (*c).failed }, 0);
    assert_eq!(ended(2), RAISED);
    assert_eq!(unsafe { (*c).failed }, 0);
    assert_eq!(ended(3), FAILED);
    assert_eq!(unsafe { (*c).failed }, 0);
    assert!(
        !scheduler.restore(1, None),
        "an unwound task leaves nothing"
    );
    assert!(scheduler.restore(2, None));
    assert_eq!(unsafe { (*c).failed }, FAILED_UNWOUND_TO_HANDLER);
    unsafe { (*c).failed = 0 };
    assert!(scheduler.restore(3, Some("noted".to_string())));
    assert_eq!(unsafe { (*c).failed }, 1);
    unsafe { (*c).failed = 0 };
    drop(scheduler);
}

unsafe fn requests_once(_context: *mut Context, scheduler: usize) {
    unsafe { tasks::switch_to_loop(scheduler as *mut Scheduler, request_message(1, 0)) };
}

#[test]
fn a_scheduler_reuses_stacks_of_finished_tasks_so_the_stack_count_equals_the_most_tasks_that_ran_at_once()
 {
    let mut context = new_context();
    let mut scheduler = Scheduler::open(&mut context, false);
    let r: *mut Scheduler = &mut *scheduler;
    for id in 1..=3 {
        scheduler.start(id, body(requests_once, r));
        unsafe { tasks::resume(r, id, Message::default()) };
    }
    assert_eq!(scheduler.stacks(), 3);
    for id in 1..=3 {
        assert_eq!(
            unsafe { tasks::resume(r, id, Message::default()) }.code,
            FINISHED
        );
        scheduler.free(id);
    }
    assert_eq!(
        scheduler.stacks(),
        3,
        "the stacks wait for the scheduler's next tasks"
    );
    for id in 4..=5 {
        scheduler.start(id, body(requests_once, r));
        unsafe { tasks::resume(r, id, Message::default()) };
    }
    assert_eq!(
        scheduler.stacks(),
        3,
        "the next tasks ran on stacks given back"
    );
    for id in 4..=5 {
        unsafe { tasks::resume(r, id, Message::default()) };
        scheduler.free(id);
    }
    drop(scheduler);
}

#[test]
fn freeing_a_task_that_never_ran_releases_its_closure_and_dropping_the_scheduler_releases_the_rest()
{
    let mut context = new_context();
    let mut scheduler = Scheduler::open(&mut context, true);
    let r: *mut Scheduler = &mut *scheduler;
    let before = RELEASED.with(Cell::get);
    scheduler.start(1, body(requests_once, r));
    scheduler.start(2, body(requests_once, r));
    scheduler.free(1);
    assert_eq!(RELEASED.with(Cell::get), before + 1);
    drop(scheduler);
    assert_eq!(RELEASED.with(Cell::get), before + 2);
}

unsafe fn acquires(_context: *mut Context, scheduler: usize) {
    let scheduler = scheduler as *mut Scheduler;
    let id = unsafe { (*scheduler).shield(BracketPhase::Acquire) }.expect("a task runs");
    unsafe { tasks::switch_to_loop(scheduler, request_message(1, 0)) };
    let lands = unsafe { (*scheduler).leave_phase(id, BracketPhase::Acquire) };
    RECEIVED.with(|h| h.borrow_mut().push(i64::from(lands)));
}

#[test]
fn a_cancel_requested_while_a_task_is_acquiring_a_resource_stops_the_task_when_the_acquire_ends() {
    let mut context = new_context();
    let mut scheduler = Scheduler::open(&mut context, true);
    let r: *mut Scheduler = &mut *scheduler;
    scheduler.start(1, body(acquires, r));
    unsafe { tasks::resume(r, 1, Message::default()) };
    let inside = Shields {
        acquiring: 1,
        releasing: 0,
        stopping: false,
    };
    assert_eq!(scheduler.shields(1), inside);
    assert_eq!(scheduler.shielded(), [(1, inside)]);
    scheduler.set_stopping(1, true);
    assert_eq!(
        unsafe { tasks::resume(r, 1, Message::default()) }.code,
        FINISHED
    );
    assert_eq!(take_received(), [1]);
    assert_eq!(scheduler.shields(1), Shields::default());
    drop(scheduler);
}

#[test]
fn failure_bytes_from_the_scheduler_decode_into_a_diagnostic_and_an_unknown_code_or_truncated_bytes_fail()
 {
    let context = new_context();
    let bytes = b"\x05E0414\x01m\x01\x04\x02\x06\x01\x01t\x01\x01n";
    let d = failure_of(&context, bytes).expect("it reads");
    assert_eq!(d.code, codes::DEADLOCK);
    assert_eq!(d.message, "m");
    assert_eq!(d.labels.len(), 1);
    assert!(d.labels[0].primary);
    assert_eq!(d.labels[0].message, "t");
    assert_eq!(d.notes, ["n"]);
    let unknown = failure_of(&context, b"\x02E1\x01m\x00\x00").expect_err("no such code");
    assert!(unknown.contains("`E1`"), "{unknown}");
    assert!(failure_of(&context, &bytes[..5]).is_err());
}

fn taken(context: &mut Context, n: i64) -> Vec<u8> {
    let mut out = vec![0u8; usize::try_from(n).expect("a count")];
    unsafe { rt_taken(context, Address(out.as_mut_ptr()), I64(n)) };
    out
}

#[test]
fn waiting_on_watched_jobs_returns_when_one_finishes_and_reports_that_job_s_id_as_bytes() {
    let mut context = new_context();
    let runtime = ply_host::Host::new().runtime();
    let run = Arc::clone(ply_host::registry::run_of(&*runtime).expect("a host made it"));
    context.set_host(
        Arc::new(ply_core::HostBinding::unbound()),
        Some(runtime),
        None,
    );
    let submit = |result: &'static [u8]| {
        jobs::submit(
            Work::Computing,
            &run,
            Span::DUMMY,
            "test",
            "a test job",
            move || Ok(result.to_vec()),
        )
        .expect("a thread is had")
        .token
    };
    let watched = submit(b"watched");
    let unwatched = submit(b"unwatched");
    assert_eq!(
        unsafe { rt_ticket_watch(&mut context, I64(watched as i64)) }.0,
        1
    );
    assert_eq!(unsafe { rt_tickets_park(&mut context, I64(-1)) }.0, 1);
    let n = unsafe { rt_tickets_answered(&mut context) };
    assert_eq!(taken(&mut context, n.0), watched.to_le_bytes());
    let n = take_ticket_bytes(&mut context, I64(watched as i64));
    assert_eq!(taken(&mut context, n.0), b"watched");
    let n = unsafe { rt_tickets_answered(&mut context) };
    assert_eq!(
        taken(&mut context, n.0),
        b"",
        "a ticket taken is resolved once"
    );
    assert!(context.take_failure().is_none());
    assert_ne!(unwatched, watched, "each job has a ticket of its own");
    let earlier = unsafe { rt_host_now(&mut context) }.0;
    assert!(unsafe { rt_host_now(&mut context) }.0 >= earlier);
    assert!(context.take_failure().is_none());
}

#[test]
fn watching_a_job_id_from_another_run_returns_0_and_sets_a_failure_on_the_context() {
    let mut context = new_context();
    context.set_host(
        Arc::new(ply_core::HostBinding::unbound()),
        Some(ply_host::Host::new().runtime()),
        None,
    );
    assert_eq!(
        unsafe { rt_ticket_watch(&mut context, I64(u32::MAX as i64)) }.0,
        0
    );
    assert!(context.take_failure().is_some());
}

unsafe extern "C" fn untouched(_context: *mut Context, _w: Word) -> Word {
    0
}

unsafe extern "C" fn lifted(_context: *mut Context) -> Word {
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

fn word_of<T>(x: T) -> Word {
    assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<Word>());
    let w = unsafe { std::mem::transmute_copy(&x) };
    std::mem::forget(x);
    w
}

fn handle(w: Word) -> Handle {
    <Handle as Returned>::of(w)
}

fn owned(w: Word) -> OwnedWord {
    <OwnedWord as Returned>::of(w)
}

fn let_go(context: &mut Context, w: Word) {
    heap::release_object(context, w);
}

#[test]
fn the_effects_a_step_accessed_are_returned_encoded_as_bytes_and_then_cleared() {
    let mut context = new_context();
    context.accesses.push((
        tasks::Access::Effect {
            effect: ply_core::Symbol::new("log"),
            resource: Some(ply_core::Symbol::new("x")),
            write: true,
        },
        [3, 1, 2],
    ));
    context.accesses.push((
        tasks::Access::ScopedValue {
            id: 9,
            write: false,
        },
        [0, 0, 0],
    ));
    assert_eq!(unsafe { tasks::rt_footprint_held(&mut context) }.0, 2);
    let n = unsafe { tasks::rt_footprint_taken(&mut context) };
    assert_eq!(
        taken(&mut context, n.0),
        b"\x02\x00\x03log\x00\x01x\x01\x00\x06\x02\x04\x02\x12\x00\x00\x00\x00"
    );
    assert_eq!(unsafe { tasks::rt_footprint_held(&mut context) }.0, 0);
    let n = unsafe { tasks::rt_footprint_taken(&mut context) };
    assert_eq!(taken(&mut context, n.0), b"\x00");
}

#[test]
fn a_value_saved_with_rt_keep_opens_as_its_saved_type_and_opening_it_as_another_type_sets_a_failure()
 {
    let mut context = new_context();
    let c: *mut Context = &mut context;
    let k = word_of(unsafe { tasks::rt_keep(c, owned(42), &INTS) });
    assert_eq!(word_of(unsafe { tasks::rt_open(c, handle(k), &INTS) }), 42);
    assert!(context.take_failure().is_none());
    unsafe { tasks::rt_open(c, handle(k), &TEXTS) };
    let d = context
        .take_failure()
        .expect("a kept `Int` opened as a `String`");
    assert!(
        d.message.contains("`Int` was opened as `String`"),
        "{}",
        d.message
    );
    let_go(&mut context, k);
    assert_eq!((context.heap.live(), context.heap.natives()), (0, 0));
}

thread_local! {
    static STACKS: Cell<Word> = const { Cell::new(0) };
    static ENTERED: RefCell<Vec<Word>> = const { RefCell::new(Vec::new()) };
}

#[repr(C, align(8))]
struct ImmortalClosure {
    rc: u32,
    descriptor: &'static PlyFunctionDescriptor,
}

unsafe extern "C" fn ignores_value(_context: *mut Context, _f: OwnedWord) -> Word {
    0
}

const fn function(
    uniform: unsafe extern "C" fn(*mut Context, Word, *const Word) -> Word,
) -> PlyFunctionDescriptor {
    PlyFunctionDescriptor {
        uniform,
        typed: None,
        convention: 0,
        release: ignores_value,
        share: untouched,
        keep: untouched,
    }
}

unsafe extern "C" fn results_five(context: *mut Context, _f: Word, _args: *const Word) -> Word {
    let c = unsafe { &mut *context };
    let first = if c.entering_words == 0 {
        0
    } else {
        unsafe { *(c.entering_words as *const Word) }
    };
    ENTERED.with(|e| e.borrow_mut().extend([c.entering_descriptor, first]));
    c.entering_descriptor = 0;
    c.entering_words = 0;
    word_of(unsafe { tasks::rt_keep(context, owned(5), &INTS) })
}

unsafe extern "C" fn requests_three(context: *mut Context, _f: Word, _args: *const Word) -> Word {
    let s = STACKS.with(Cell::get);
    unsafe { tasks::rt_task_yielded(context, handle(s), I64(3), I64(4), I64(5)) };
    0
}

static FIVE: PlyFunctionDescriptor = function(results_five);
static THREE: PlyFunctionDescriptor = function(requests_three);
static RESULTS_FIVE: ImmortalClosure = ImmortalClosure {
    rc: IMMORTAL,
    descriptor: &FIVE,
};
static YIELDS_CODE_THREE_CLOSURE: ImmortalClosure = ImmortalClosure {
    rc: IMMORTAL,
    descriptor: &THREE,
};

fn block(f: &'static ImmortalClosure) -> Block {
    <Block as Returned>::of(f as *const ImmortalClosure as Word)
}

fn started(
    context: &mut Context,
    f: &'static ImmortalClosure,
    descriptor: &[Word],
    words: &mut [Word],
) -> (Word, Word) {
    let c: *mut Context = context;
    let s = word_of(unsafe { tasks::rt_stacks_opened(c, I64(1), I64(-1), I64(0), I64(0)) });
    let scheduler = unsafe { tasks::rt_stacks_region(c, handle(s)) };
    let t = word_of(unsafe { tasks::rt_task_made(c, scheduler) });
    let b = word_of(unsafe {
        tasks::rt_task_body(c, handle(t), block(f), descriptor.as_ptr(), words.as_ptr())
    });
    words.iter_mut().for_each(|w| *w = 0);
    unsafe { tasks::rt_task_started(c, handle(s), I64(1), handle(b)) };
    let_go(context, b);
    STACKS.with(|x| x.set(s));
    (s, t)
}

#[test]
fn a_started_task_runs_its_body_with_its_descriptor_and_captured_values_and_stores_the_result_in_its_handle()
 {
    let mut context = new_context();
    let c: *mut Context = &mut context;
    let descriptor = [1, 0, -1];
    let (s, t) = started(&mut context, &RESULTS_FIVE, &descriptor, &mut [77]);
    assert_eq!(unsafe { tasks::rt_task_said(c, handle(t), I64(1)) }.0, 1);
    let ended = unsafe { tasks::rt_task_resumed(c, handle(s), I64(1), I64(0), I64(0)) };
    assert_eq!(ended.0, FINISHED);
    assert_eq!(
        ENTERED.with(|e| e.take()),
        [descriptor.as_ptr() as Word, 77]
    );
    assert_eq!(
        unsafe { tasks::rt_task_said(c, handle(t), I64(2)) }.0,
        RETURNED
    );
    let result = word_of(unsafe { tasks::rt_task_answer(c, handle(t)) });
    assert_eq!(
        word_of(unsafe { tasks::rt_open(c, handle(result), &INTS) }),
        5
    );
    let_go(&mut context, result);
    unsafe { tasks::rt_task_freed(c, handle(s), I64(1)) };
    let_go(&mut context, t);
    let_go(&mut context, s);
    assert!(context.take_failure().is_none());
    assert_eq!((context.heap.live(), context.heap.natives()), (0, 0));
}

#[test]
fn a_task_that_performs_an_operation_reports_its_operation_numbers_and_source_span_and_can_be_resumed_to_unwind_or_fail()
 {
    let mut context = new_context();
    let c: *mut Context = &mut context;
    (
        context.performed_root,
        context.performed_start,
        context.performed_end,
    ) = (9, 10, 11);
    let (s, t) = started(&mut context, &YIELDS_CODE_THREE_CLOSURE, &[0], &mut []);
    let asked = unsafe { tasks::rt_task_resumed(c, handle(s), I64(1), I64(0), I64(0)) };
    assert_eq!(asked.0, 3);
    let said = |f: unsafe extern "C" fn(*mut Context, Handle, I64) -> I64, i| unsafe { f(c, handle(s), I64(i)) }.0;
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
    assert_eq!(context.failed, 0);
    unsafe { tasks::rt_task_freed(c, handle(s), I64(1)) };
    let_go(&mut context, t);
    let_go(&mut context, s);

    let (s, t) = started(&mut context, &YIELDS_CODE_THREE_CLOSURE, &[0], &mut []);
    unsafe { tasks::rt_task_resumed(c, handle(s), I64(1), I64(0), I64(0)) };
    let failure = b"\x05E0414\x01m\x01\x04\x02\x06\x01\x01t\x01\x01n";
    let ended = unsafe {
        tasks::rt_task_resumed_raising(
            c,
            handle(s),
            I64(1),
            Address(failure.as_ptr() as *mut u8),
            I64(failure.len() as i64),
        )
    };
    assert_eq!(ended.0, RAISED);
    let none = Address(std::ptr::null_mut());
    assert_eq!(
        unsafe { tasks::rt_task_restored(c, handle(s), I64(1), none, I64(0)) }.0,
        1
    );
    assert_eq!(context.failed, FAILED_PENDING_RAISE);
    let d = context.take_failure().expect("the raise is put back");
    assert_eq!(d.message, "m");
    unsafe { tasks::rt_task_freed(c, handle(s), I64(1)) };
    let_go(&mut context, t);
    let_go(&mut context, s);
    assert_eq!((context.heap.live(), context.heap.natives()), (0, 0));
}
