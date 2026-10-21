//! `parallel` blocks under handlers: whether a block's branches leave this thread is decided by
//! what the block's site says they may perform, held against each clause installed above it.

use crate::units::{self, def};
use ply_compiled::heap::{imm, imm_value};
use ply_core::Symbol;
use std::sync::atomic::{AtomicPtr, Ordering};

const HELD: i64 = 0;
const OTHER: i64 = 1;
const CONN: i64 = 2;
const WIRE: i64 = 3;
/// A clause on every resource, or a site that names none.
const ANY: i64 = -1;
/// No handler installed, or a site that says nothing of what its branches perform.
const NONE: i64 = -2;

/// `m.block(clause effect, clause resource, reached effect, reached resource)`: a handler of one
/// clause installed where the first names an effect, and under it a block of two branches, each
/// answering whether it was run in a context of its own, which is what a branch on another thread
/// is given. The block answers how many were.
fn block() -> [units::Def; 2] {
    [
        def(
            "m.branch",
            1,
            "  return ply_imm(ply_imm_value(p0) != (int64_t)(intptr_t)ctx);",
        ),
        def(
            "m.block",
            4,
            "  int64_t held = ply_imm_value(p0), reached = ply_imm_value(p2);\n\
             \x20 Word clause[6] = {held, ply_imm_value(p1), held, ply_imm(0), 0, 0};\n\
             \x20 Word depth = 0;\n\
             \x20 if (held >= 0) depth = rt_handle_push_p(ctx, (Word)(intptr_t)clause, 1, 0);\n\
             \x20 Word env[1] = {ply_imm((int64_t)(intptr_t)ctx)};\n\
             \x20 Word slots[2];\n\
             \x20 slots[0] = rt_closure_p(ctx, 0, 0, (Word)(intptr_t)env, 1);\n\
             \x20 slots[1] = rt_closure_p(ctx, 0, 0, (Word)(intptr_t)env, 1);\n\
             \x20 Word reach[2] = {reached, ply_imm_value(p3)};\n\
             \x20 if (reached < 0) rt_parallel_p(ctx, (Word)(intptr_t)slots, 2);\n\
             \x20 else rt_parallel_past_p(ctx, (Word)(intptr_t)slots, 2, (Word)(intptr_t)reach, 1);\n\
             \x20 Word apart = ply_imm(ply_imm_value(slots[0]) + ply_imm_value(slots[1]));\n\
             \x20 if (held >= 0) rt_handle_land_p(ctx, depth, 0);\n\
             \x20 return apart;",
        ),
    ]
}

#[test]
fn a_block_leaves_this_thread_past_every_handler_its_site_says_it_cannot_reach() {
    // On one core every block runs its branches in turn, and there is nothing to tell apart.
    if std::thread::available_parallelism().map_or(1, |n| n.get()) < 2 {
        return;
    }
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some(loaded) = units::opened(dir.path(), &block(), |e| {
        e.fields = ["held", "other", "conn", "wire"].map(Symbol::new).to_vec();
        e.lambdas = vec![format!("{}_entry", units::symbol("m.branch"))];
    }) else {
        return;
    };
    let (native, refused) = loaded.expect("the unit loads");
    assert!(refused.is_empty());
    let apart = |held: i64, on: i64, reached: i64, at: i64| {
        let answer = units::entered(
            &native,
            "m.block",
            10,
            &[imm(held), imm(on), imm(reached), imm(at)],
        )
        .expect("the block answers");
        imm_value(answer)
    };
    // The clause's effect and resource, the reached effect and resource, and how many branches left.
    for (held, on, reached, at, left, when) in [
        (NONE, ANY, NONE, ANY, 2, "under no handler"),
        (NONE, ANY, OTHER, ANY, 2, "under no handler, its reach said"),
        (HELD, ANY, OTHER, ANY, 2, "past a handler of another effect"),
        (HELD, ANY, HELD, ANY, 0, "under a handler it reaches"),
        (HELD, ANY, NONE, ANY, 0, "under any handler, reach unsaid"),
        (HELD, CONN, HELD, WIRE, 2, "past another resource's clause"),
        (HELD, CONN, HELD, CONN, 0, "under a clause on its resource"),
        (HELD, CONN, HELD, ANY, 0, "under a clause it may name"),
        (HELD, ANY, HELD, WIRE, 0, "under a clause on every resource"),
    ] {
        assert_eq!(apart(held, on, reached, at), left, "{when}");
    }
}

/// Blocks laid out as emitted code lays them: a count at offset 0, freed by size.
mod emitted {
    use crate::bare::{context, helper};
    use ply_compiled::emitted::{Handed, PlyFnDesc};
    use ply_compiled::heap::{HEADER, Word};
    use ply_compiled::rt::Ctx;
    use std::sync::atomic::{AtomicBool, Ordering};

    type Alloc = unsafe extern "C" fn(*mut Ctx, Word) -> Word;
    type Free = unsafe extern "C" fn(*mut Ctx, Word, Word);
    type Run = unsafe extern "C" fn(*mut Ctx, *mut Word, Word);

    /// A box of one word, the number it holds, and a closure block of its descriptor alone.
    const BLOCK: Word = HEADER as Word + 8;

    fn block(ctx: *mut Ctx, word: Word) -> Word {
        let w = unsafe { helper::<Alloc>("rt_alloc_bytes")(ctx, BLOCK) };
        unsafe { *(w as *mut Word).add(2) = word };
        w
    }

    fn held(w: Word) -> Word {
        unsafe { *(w as *const Word).add(2) }
    }

    fn free(ctx: *mut Ctx, w: Word) {
        unsafe { helper::<Free>("rt_free")(ctx, w, BLOCK) };
    }

    unsafe extern "C" fn release(ctx: *mut Ctx, f: Handed) -> Word {
        free(ctx, unsafe { std::mem::transmute::<Handed, Word>(f) });
        0
    }

    /// Captures nothing that cannot cross threads.
    unsafe extern "C" fn crosses(_ctx: *mut Ctx, _f: Word) -> Word {
        1
    }

    /// Captures what may not leave this thread.
    unsafe extern "C" fn stays(_ctx: *mut Ctx, _f: Word) -> Word {
        0
    }

    static UNRUN_RAN: AtomicBool = AtomicBool::new(false);

    unsafe extern "C" fn first(ctx: *mut Ctx, _f: Word, _args: *const Word) -> Word {
        block(ctx, 1)
    }

    unsafe extern "C" fn second(ctx: *mut Ctx, _f: Word, _args: *const Word) -> Word {
        block(ctx, 2)
    }

    /// The branch after one that fails in turn, which no other test runs.
    unsafe extern "C" fn unrun(ctx: *mut Ctx, _f: Word, _args: *const Word) -> Word {
        UNRUN_RAN.store(true, Ordering::SeqCst);
        block(ctx, 3)
    }

    unsafe extern "C" fn fails(ctx: *mut Ctx, _f: Word, _args: *const Word) -> Word {
        unsafe { (*ctx).failed = 1 };
        0
    }

    const fn desc(
        uniform: unsafe extern "C" fn(*mut Ctx, Word, *const Word) -> Word,
        share: unsafe extern "C" fn(*mut Ctx, Word) -> Word,
    ) -> PlyFnDesc {
        PlyFnDesc {
            uniform,
            typed: None,
            convention: 0,
            release,
            share,
            keep: stays,
        }
    }

    static FIRST: PlyFnDesc = desc(first, crosses);
    static SECOND: PlyFnDesc = desc(second, crosses);
    static FAILS: PlyFnDesc = desc(fails, crosses);
    static FAILS_HERE: PlyFnDesc = desc(fails, stays);
    static UNRUN: PlyFnDesc = desc(unrun, crosses);

    fn multicore() -> bool {
        std::thread::available_parallelism().map_or(1, |n| n.get()) >= 2
    }

    /// A block of a branch for each of `descs`, run: its slots as `rt_branches` leaves them.
    fn branches(ctx: &mut Ctx, descs: [&'static PlyFnDesc; 2]) -> [Word; 2] {
        let c: *mut Ctx = ctx;
        let mut slots = descs.map(|d| block(c, d as *const PlyFnDesc as Word));
        unsafe { helper::<Run>("rt_branches")(c, slots.as_mut_ptr(), 2) };
        slots
    }

    /// Each branch's closure block is taken and let go once it has run, and its answer written
    /// over it, at once on threads of their own where the closures may cross.
    #[test]
    fn branches_emitted_code_made_answer_in_their_slots_and_are_let_go() {
        let mut ctx = context();
        ctx.begin(1_000);
        let [a, b] = branches(&mut ctx, [&FIRST, &SECOND]);
        assert_eq!(ctx.failed, 0);
        assert_eq!((held(a), held(b)), (1, 2));
        free(&mut ctx, a);
        free(&mut ctx, b);
        assert_eq!(ctx.heap.live(), 0, "a branch's closure was not let go");
        ctx.end();
    }

    /// In turn, a branch that fails leaves 0, and those after it are let go unrun.
    #[test]
    fn a_failing_branch_in_turn_leaves_0_and_those_after_it_are_let_go_unrun() {
        let mut ctx = context();
        ctx.begin(1_000);
        let slots = branches(&mut ctx, [&FAILS_HERE, &UNRUN]);
        assert_eq!(ctx.failed, 1);
        assert_eq!(slots, [0, 0]);
        assert!(!UNRUN_RAN.load(Ordering::SeqCst));
        assert_eq!(ctx.heap.live(), 0);
        ctx.failed = 0;
        ctx.end();
    }

    /// At once, a branch that answered after one before it failed leaves its answer in its slot
    /// for emitted code, which knows its type, to let go.
    #[test]
    fn a_branch_that_answered_past_one_that_failed_is_left_for_emitted_code() {
        if !multicore() {
            return;
        }
        let mut ctx = context();
        ctx.begin(1_000);
        let [failed, answered] = branches(&mut ctx, [&FAILS, &SECOND]);
        assert_eq!((ctx.failed, failed), (1, 0));
        assert_eq!(held(answered), 2);
        free(&mut ctx, answered);
        assert_eq!(ctx.heap.live(), 0);
        ctx.failed = 0;
        ctx.end();
    }
}

/// The unit `entered_inside` enters, for the length of the one test that sets it.
static UNIT: AtomicPtr<ply_compiled::c::Native> = AtomicPtr::new(std::ptr::null_mut());

/// A machine of its own entered on whichever thread calls: `m.block` under no handler, which
/// answers how many of its two branches left that thread.
extern "C" fn entered_inside() -> i64 {
    let native = unsafe { &*UNIT.load(Ordering::Acquire) };
    let none = [imm(NONE), imm(ANY), imm(NONE), imm(ANY)];
    units::entered(native, "m.block", 10, &none).expect("the block answers")
}

/// A thread waiting on a block takes up other branches of the pool, so a machine that waited inside
/// another machine's branch would carry that machine's other work on its own stack.
#[test]
fn a_machine_entered_on_a_branch_runs_its_own_blocks_in_turn() {
    if std::thread::available_parallelism().map_or(1, |n| n.get()) < 2 {
        return;
    }
    let mut defs = Vec::from(block());
    defs.push(def(
        "m.enters",
        1,
        "  int64_t (*entered)(void) = (int64_t (*)(void))(intptr_t)ply_imm_value(p0);\n\
         \x20 return entered();",
    ));
    defs.push(def(
        "m.outer",
        1,
        "  Word env[1] = {p0};\n\
         \x20 Word slots[2];\n\
         \x20 slots[0] = rt_closure_p(ctx, 1, 0, (Word)(intptr_t)env, 1);\n\
         \x20 slots[1] = rt_closure_p(ctx, 1, 0, (Word)(intptr_t)env, 1);\n\
         \x20 rt_parallel_p(ctx, (Word)(intptr_t)slots, 2);\n\
         \x20 return ply_imm(ply_imm_value(slots[0]) + ply_imm_value(slots[1]));",
    ));
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some(loaded) = units::opened(dir.path(), &defs, |e| {
        e.lambdas = ["m.branch", "m.enters"]
            .map(|name| format!("{}_entry", units::symbol(name)))
            .to_vec();
    }) else {
        return;
    };
    let (native, refused) = loaded.expect("the unit loads");
    assert!(refused.is_empty());
    let none = [imm(NONE), imm(ANY), imm(NONE), imm(ANY)];
    let alone = units::entered(&native, "m.block", 10, &none).expect("the block answers");
    assert_eq!(
        imm_value(alone),
        2,
        "a block entered on a thread of its own"
    );
    UNIT.store(std::ptr::from_ref(&native).cast_mut(), Ordering::Release);
    let callback = entered_inside as extern "C" fn() -> i64 as usize as i64;
    let inside = units::entered(&native, "m.outer", 10, &[imm(callback)]);
    UNIT.store(std::ptr::null_mut(), Ordering::Release);
    assert_eq!(
        imm_value(inside.expect("the outer block answers")),
        0,
        "the two blocks entered on the outer block's branches"
    );
}
