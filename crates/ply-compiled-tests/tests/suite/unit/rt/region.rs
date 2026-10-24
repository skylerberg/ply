//! Regions emitted code enters: their tasks' bodies are closure blocks, what the tasks pass each
//! other is kept with its type, their operations cross in through the operation's types, and what
//! each is answered is lifted through them. Every block here is laid out as emitted code lays one:
//! a count at offset 0, freed by size.

use crate::bare::{context_of, helper};
use ply_compiled::c::builtin_helper_name;
use ply_compiled::emitted::{Handed, PlyFnDesc, PlyOp, PlyType};
use ply_compiled::heap::{HEADER, Word};
use ply_compiled::native::Answered;
use ply_compiled::rt::Ctx;
use ply_core::codec::tag;
use std::cell::Cell;

type A0 = unsafe extern "C" fn(*mut Ctx) -> Word;
type A1 = unsafe extern "C" fn(*mut Ctx, Word) -> Word;
type A2 = unsafe extern "C" fn(*mut Ctx, Word, Word) -> Word;
type N1 = unsafe extern "C" fn(*mut Ctx, Word);
type N2 = unsafe extern "C" fn(*mut Ctx, Word, Word);
type N3 = unsafe extern "C" fn(*mut Ctx, Word, Word, Word);
type Dispatch = unsafe extern "C" fn(*mut Ctx, *const PlyOp, Word, Word, *const Word) -> Word;
type Region = unsafe extern "C" fn(*mut Ctx, Word, *const PlyType) -> Word;

/// The unit's field table, which names the effect and its operations by place.
const FIELDS: [&str; 7] = ["task", "spawn", "join", "channel", "send", "recv", "select"];
const SPAWN: i64 = 1;
const JOIN: i64 = 2;
const CHANNEL: i64 = 3;
const SEND: i64 = 4;
const RECV: i64 = 5;
const SELECT: i64 = 6;

fn native<F: Copy>(name: &str) -> F {
    helper(&builtin_helper_name(name))
}

fn count(w: Word) -> u32 {
    unsafe { *(w as *const u32) }
}

/// A value of `m.Box`: a block of one word, the number it holds.
const BOX: Word = HEADER as Word + 8;

fn boxed(ctx: *mut Ctx, n: Word) -> Word {
    let w = unsafe { helper::<A1>("rt_alloc_bytes")(ctx, BOX) };
    unsafe { *(w as *mut Word).byte_add(HEADER) = n };
    w
}

fn held(w: Word) -> Word {
    unsafe { *(w as *const Word).byte_add(HEADER) }
}

unsafe extern "C" fn counted_inc(_ctx: *mut Ctx, w: Word) -> Word {
    unsafe { *(w as *mut u32) += 1 };
    0
}

unsafe extern "C" fn box_dec(ctx: *mut Ctx, w: Word) -> Word {
    unsafe {
        if count(w) > 1 {
            *(w as *mut u32) -= 1;
        } else {
            helper::<N2>("rt_free")(ctx, w, BOX);
        }
    }
    0
}

unsafe extern "C" fn unread(_ctx: *mut Ctx, _w: Word) -> Word {
    0
}

unsafe extern "C" fn nothing(_ctx: *mut Ctx) -> Word {
    0
}

const fn ty(
    spelled: &'static str,
    inc: unsafe extern "C" fn(*mut Ctx, Word) -> Word,
    dec: unsafe extern "C" fn(*mut Ctx, Word) -> Word,
    lower: unsafe extern "C" fn(*mut Ctx, Word) -> Word,
    lift: unsafe extern "C" fn(*mut Ctx) -> Word,
) -> PlyType {
    PlyType {
        identity: spelled.len() as u64,
        spelled: spelled.as_ptr(),
        spelled_len: spelled.len() as u32,
        inc,
        dec,
        lower,
        lift,
    }
}

/// What the region keeps of a task's answer and of a value sent: a box, counted through its type.
static BOX_T: PlyType = ty("m.Box", counted_inc, box_dec, unread, nothing);

/// A box in the place of the operation's own type parameter, which crosses kept with its type.
unsafe extern "C" fn kept_lower(ctx: *mut Ctx, w: Word) -> Word {
    unsafe { native::<N2>("?out_carried")(ctx, w, &raw const BOX_T as Word) };
    0
}

unsafe extern "C" fn kept_lift(ctx: *mut Ctx) -> Word {
    unsafe { native::<A0>("?in_carried")(ctx) }
}

static KEPT_T: PlyType = ty("^a", counted_inc, box_dec, kept_lower, kept_lift);

/// Reads an `Option`'s head: whether it is `Some`.
unsafe fn is_some(ctx: *mut Ctx) -> bool {
    unsafe {
        let n = native::<A1>("?in_count")(ctx, tag::CTOR as Word);
        let some = native::<A2>("?in_name_is")(ctx, b"Some".as_ptr() as Word, 4) == 1;
        assert_eq!(n, Word::from(some), "`Some` of one, `None` of none");
        some
    }
}

/// `Option<^a>`, its `None` read as 0.
unsafe extern "C" fn maybe_lift(ctx: *mut Ctx) -> Word {
    unsafe { if is_some(ctx) { kept_lift(ctx) } else { 0 } }
}

static MAYBE_T: PlyType = ty("Option<^a>", unread, unread, unread, maybe_lift);

thread_local! {
    static CHOSEN: Cell<Word> = const { Cell::new(-1) };
}

/// `Option<(Int, Option<^a>)>`: the arm a select went on is noted, and what it took or sent read.
unsafe extern "C" fn chosen_lift(ctx: *mut Ctx) -> Word {
    unsafe {
        if !is_some(ctx) {
            return 0;
        }
        assert_eq!(native::<A1>("?in_count")(ctx, tag::RECORD as Word), 2);
        native::<N2>("?in_field")(ctx, b"_0".as_ptr() as Word, 2);
        CHOSEN.with(|c| c.set(native::<A0>("?in_int")(ctx)));
        native::<N2>("?in_field")(ctx, b"_1".as_ptr() as Word, 2);
        maybe_lift(ctx)
    }
}

static CHOSEN_T: PlyType = ty(
    "Option<(Int, Option<^a>)>",
    unread,
    unread,
    unread,
    chosen_lift,
);

/// A task or a channel: a runtime object, crossing as a handle.
unsafe extern "C" fn handle_dec(ctx: *mut Ctx, w: Word) -> Word {
    unsafe {
        if count(w) > 1 {
            *(w as *mut u32) -= 1;
        } else {
            helper::<N1>("rt_handle_free")(ctx, w);
        }
    }
    0
}

unsafe extern "C" fn handle_lower(ctx: *mut Ctx, w: Word) -> Word {
    unsafe { native::<N1>("?out_handle")(ctx, w) };
    0
}

unsafe extern "C" fn handle_lift(ctx: *mut Ctx) -> Word {
    unsafe { native::<A0>("?in_handle")(ctx) }
}

static HANDLE_T: PlyType = ty(
    "Chan<m.Box>",
    counted_inc,
    handle_dec,
    handle_lower,
    handle_lift,
);

/// An `Int`, whose word is the number.
unsafe extern "C" fn int_lower(ctx: *mut Ctx, w: Word) -> Word {
    unsafe { native::<N1>("?out_int")(ctx, w) };
    0
}

static INT_T: PlyType = ty("Int", unread, unread, int_lower, nothing);

/// A `Bool`, 0 or 1, read back as the tag it crosses as.
unsafe extern "C" fn bool_lower(ctx: *mut Ctx, w: Word) -> Word {
    let t = if w != 0 { tag::TRUE } else { tag::FALSE };
    unsafe { native::<N1>("?out_tag")(ctx, Word::from(t)) };
    0
}

unsafe extern "C" fn bool_lift(ctx: *mut Ctx) -> Word {
    unsafe { native::<A0>("?in_tag")(ctx) }
}

static BOOL_T: PlyType = ty("Bool", unread, unread, bool_lower, bool_lift);

/// A function value: a closure block, let go through its descriptor's release.
unsafe extern "C" fn fn_dec(ctx: *mut Ctx, w: Word) -> Word {
    unsafe {
        if count(w) > 1 {
            *(w as *mut u32) -= 1;
        } else {
            let desc = *(w as *const *const PlyFnDesc).byte_add(HEADER);
            ((*desc).release)(ctx, <Handed as Answered>::of(w));
        }
    }
    0
}

static FN_T: PlyType = ty("() -> m.Box", counted_inc, fn_dec, unread, nothing);

/// `task.select`'s arms, one sending a box on a channel: the word is a pair the test made, of the
/// channel's handle and the box.
unsafe extern "C" fn arms_dec(ctx: *mut Ctx, w: Word) -> Word {
    let pair = unsafe { Box::from_raw(w as *mut [Word; 2]) };
    unsafe {
        handle_dec(ctx, pair[0]);
        box_dec(ctx, pair[1]);
    }
    0
}

unsafe extern "C" fn arms_lower(ctx: *mut Ctx, w: Word) -> Word {
    let pair = unsafe { *(w as *const [Word; 2]) };
    let name = |ctx: *mut Ctx, n: &[u8]| unsafe {
        native::<N2>("?out_name")(ctx, n.as_ptr() as Word, n.len() as Word)
    };
    unsafe {
        native::<N2>("?out_count")(ctx, tag::LIST as Word, 1);
        native::<N2>("?out_count")(ctx, tag::RECORD as Word, 2);
        name(ctx, b"_0");
        handle_lower(ctx, pair[0]);
        name(ctx, b"_1");
        native::<N3>("?out_ctor")(ctx, b"Some".as_ptr() as Word, 4, 1);
        kept_lower(ctx, pair[1]);
    }
    0
}

static ARMS_T: PlyType = ty(
    "List<(Chan<m.Box>, Option<^a>)>",
    unread,
    arms_dec,
    arms_lower,
    nothing,
);

struct Types<const N: usize>([*const PlyType; N]);
unsafe impl<const N: usize> Sync for Types<N> {}

static BOXES: Types<1> = Types([&raw const BOX_T]);
static FNS: Types<1> = Types([&raw const FN_T]);
static HANDLES: Types<1> = Types([&raw const HANDLE_T]);
static INTS: Types<1> = Types([&raw const INT_T]);
static SENT: Types<2> = Types([&raw const HANDLE_T, &raw const KEPT_T]);
static SELECTED: Types<2> = Types([&raw const ARMS_T, &raw const BOOL_T]);

/// `task.op` at `a = m.Box`, of the parameters `params`, `own` marking those of type `a`.
fn op(op: i64, answer: &'static PlyType, own: u64, params: &'static [*const PlyType]) -> PlyOp {
    PlyOp {
        effect: 0,
        op,
        answer,
        own,
        params_len: params.len() as i64,
        params: params.as_ptr(),
        vars_len: 1,
        vars: BOXES.0.as_ptr(),
    }
}

fn dispatch(ctx: *mut Ctx, op: &PlyOp, args: &[Word]) -> Word {
    unsafe { helper::<Dispatch>("rt_dispatch")(ctx, op, -1, 1, args.as_ptr()) }
}

/// A closure block as emitted code lays one: a count, then the descriptor's address.
const CLOSURE: Word = HEADER as Word + 8;

unsafe extern "C" fn closure_release(ctx: *mut Ctx, block: Handed) -> Word {
    let w = unsafe { std::mem::transmute::<Handed, Word>(block) };
    unsafe { helper::<N2>("rt_free")(ctx, w, CLOSURE) };
    0
}

/// The closures here cross to no other thread and outlive no entry.
unsafe extern "C" fn stays_in_its_entry(_ctx: *mut Ctx, _block: Word) -> Word {
    0
}

const fn desc(uniform: unsafe extern "C" fn(*mut Ctx, Word, *const Word) -> Word) -> PlyFnDesc {
    PlyFnDesc {
        uniform,
        typed: None,
        convention: 0,
        release: closure_release,
        share: stays_in_its_entry,
        keep: stays_in_its_entry,
    }
}

fn closure(ctx: *mut Ctx, d: &'static PlyFnDesc) -> Word {
    let w = unsafe { helper::<A1>("rt_alloc_bytes")(ctx, CLOSURE) };
    unsafe { *(w as *mut *const PlyFnDesc).byte_add(HEADER) = d };
    w
}

/// Runs a region whose body is `body`, its answer a box; answers what the region answered, the
/// context's failure asserted absent.
fn region(body: &'static PlyFnDesc) -> (Ctx, Word) {
    let mut ctx = context_of(&FIELDS, Vec::new());
    ctx.begin(1_000);
    let c: *mut Ctx = &mut ctx;
    let answer = unsafe { helper::<Region>("rt_region")(c, closure(c, body), &BOX_T) };
    let failed = ctx.failed;
    let why = ctx.take_failure();
    assert_eq!(failed, 0, "{why:?}");
    (ctx, answer)
}

unsafe extern "C" fn answers_42(ctx: *mut Ctx, _f: Word, _args: *const Word) -> Word {
    boxed(ctx, 42)
}

static CHILD: PlyFnDesc = desc(answers_42);

/// Spawns a task answering a box and joins it: the join hears the task's answer.
unsafe extern "C" fn spawns_and_joins(ctx: *mut Ctx, _f: Word, _args: *const Word) -> Word {
    let body = closure(ctx, &CHILD);
    let task = dispatch(ctx, &op(SPAWN, &HANDLE_T, 0, &FNS.0), &[body]);
    dispatch(ctx, &op(JOIN, &KEPT_T, 0, &HANDLES.0), &[task])
}

static SPAWNS_AND_JOINS: PlyFnDesc = desc(spawns_and_joins);

/// A task's answer is kept through its type: its joiner is handed a count of its own, and the
/// region lets go of the one it kept as it ends, as it does of every closure and handle.
#[test]
fn a_joined_tasks_answer_crosses_back_counted_through_its_type() {
    let (mut ctx, answer) = region(&SPAWNS_AND_JOINS);
    assert_eq!((held(answer), count(answer)), (42, 1));
    unsafe { box_dec(&mut ctx, answer) };
    assert_eq!(ctx.heap.live(), 0);
    ctx.end();
}

thread_local! {
    static SENT_TAGS: Cell<(Word, Word)> = const { Cell::new((0, 0)) };
}

/// Sends two boxes on a channel of two and receives one, the first; the other is never received.
unsafe extern "C" fn sends_two_and_receives_one(
    ctx: *mut Ctx,
    _f: Word,
    _args: *const Word,
) -> Word {
    let chan = dispatch(ctx, &op(CHANNEL, &HANDLE_T, 0, &INTS.0), &[2]);
    unsafe {
        counted_inc(ctx, chan);
        counted_inc(ctx, chan);
    }
    let send = op(SEND, &BOOL_T, 0b10, &SENT.0);
    let first = dispatch(ctx, &send, &[chan, boxed(ctx, 7)]);
    let second = dispatch(ctx, &send, &[chan, boxed(ctx, 8)]);
    SENT_TAGS.with(|t| t.set((first, second)));
    dispatch(ctx, &op(RECV, &MAYBE_T, 0, &HANDLES.0), &[chan])
}

static SENDS_TWO: PlyFnDesc = desc(sends_two_and_receives_one);

/// A value sent is kept through its type until a task receives it, which is handed that count; one
/// never received is let go through its type as the region ends.
#[test]
fn a_channels_values_cross_kept_and_one_never_received_is_let_go_with_the_region() {
    let (mut ctx, answer) = region(&SENDS_TWO);
    let sent = Word::from(tag::TRUE);
    assert_eq!(SENT_TAGS.with(Cell::get), (sent, sent));
    assert_eq!((held(answer), count(answer)), (7, 1));
    unsafe { box_dec(&mut ctx, answer) };
    assert_eq!(ctx.heap.live(), 0, "the box never received was not let go");
    ctx.end();
}

/// A select of one arm sending a box on a channel with room: it goes at once, and the task hears
/// the arm and what it sent.
unsafe extern "C" fn selects_a_send(ctx: *mut Ctx, _f: Word, _args: *const Word) -> Word {
    let chan = dispatch(ctx, &op(CHANNEL, &HANDLE_T, 0, &INTS.0), &[1]);
    let arms = Box::into_raw(Box::new([chan, boxed(ctx, 5)])) as Word;
    dispatch(ctx, &op(SELECT, &CHOSEN_T, 0, &SELECTED.0), &[arms, 0])
}

static SELECTS: PlyFnDesc = desc(selects_a_send);

/// What an arm sends is lent inside the arms' encoding, and the region keeps a count of its own: the
/// channel holds one, and the sender hears the value it sent with another.
#[test]
fn a_select_keeps_what_an_arm_sends_and_answers_the_arm_it_went_on() {
    CHOSEN.with(|c| c.set(-1));
    let (mut ctx, answer) = region(&SELECTS);
    assert_eq!(CHOSEN.with(Cell::get), 0);
    assert_eq!((held(answer), count(answer)), (5, 1));
    unsafe { box_dec(&mut ctx, answer) };
    assert_eq!(ctx.heap.live(), 0);
    ctx.end();
}

/// A spawn emitted code performs names the type its task answers; a body of no type is refused,
/// and let go.
#[test]
fn a_spawn_with_no_type_for_its_tasks_answer_fails_and_lets_its_body_go() {
    unsafe extern "C" fn spawns_untyped(ctx: *mut Ctx, _f: Word, _args: *const Word) -> Word {
        let mut spawn = op(SPAWN, &HANDLE_T, 0, &FNS.0);
        spawn.vars_len = 0;
        dispatch(ctx, &spawn, &[closure(ctx, &CHILD)]);
        0
    }
    static UNTYPED: PlyFnDesc = desc(spawns_untyped);
    let mut ctx = context_of(&FIELDS, Vec::new());
    ctx.begin(1_000);
    let c: *mut Ctx = &mut ctx;
    unsafe { helper::<Region>("rt_region")(c, closure(c, &UNTYPED), &BOX_T) };
    let said = ctx.take_failure().expect("the spawn failed").message;
    assert!(said.contains("no type for what its task answers"), "{said}");
    assert_eq!(ctx.heap.live(), 0);
    ctx.failed = 0;
    ctx.end();
}
