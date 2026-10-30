use crate::units;
use ply_compiled::rt::{STACK_MARGIN, switch_keeping};
use ply_compiled::stack::{CHUNK, RESERVE, Stack, switch};
use std::cell::RefCell;
use std::sync::atomic::Ordering::Relaxed;

struct Pong {
    main: usize,
    task: usize,
    log: Vec<usize>,
}

thread_local! {
    static PONG: RefCell<Option<Pong>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Pong) -> R) -> R {
    PONG.with(|p| f(p.borrow_mut().as_mut().unwrap()))
}

fn taken() -> Stack {
    Stack::take().expect("the system maps a stack")
}

/// Switches back to the test's stack, storing this stack's pointer where the test reads it.
fn yield_to_main() {
    let (task, main) = with(|p| (&mut p.task as *mut usize, p.main));
    unsafe { switch(&mut *task, main) };
}

fn run_task() {
    let (main, task) = with(|p| (&mut p.main as *mut usize, p.task));
    unsafe { switch(&mut *main, task) };
}

/// Aligned only if the frame was entered as the ABI requires, so a trampoline off by a word is caught.
#[inline(never)]
fn aligned_local() -> usize {
    #[repr(align(16))]
    struct Aligned([u8; 16]);
    let a = Aligned([0; 16]);
    std::hint::black_box(&a.0) as *const [u8; 16] as usize
}

fn count(n: usize) {
    for i in 1..=n {
        let local = i * 10;
        with(|p| p.log.push(local));
        yield_to_main();
    }
}

extern "C" fn count_to(n: usize) {
    assert_eq!(
        aligned_local() % 16,
        0,
        "the task was entered off the ABI's alignment"
    );
    count(n);
    with(|p| p.log.push(0));
    yield_to_main();
    unreachable!("a finished task was resumed");
}

/// Past two chunks of a kilobyte a frame, so a descent has to extend its stack twice.
const PAD: usize = 1024;
const LEVELS: usize = (20 << 20) / PAD;

/// A descent of [`LEVELS`] frames on `stack`, with what it saw on the way down.
struct Descent {
    stack: *const Stack,
    /// Each frame's pad, from the top down.
    frames: Vec<usize>,
    /// Whether an extension the floor asked for was refused.
    rejected: bool,
    /// What it counts to at the bottom.
    counts: usize,
}

/// Recurses a frame of a kilobyte or so per level, making room below the floor as a compiled
/// prologue makes it, and counts at the bottom.
fn descend(d: &mut Descent, levels: usize) {
    let pad = std::hint::black_box([0u8; PAD]);
    let at = &pad as *const [u8; PAD] as usize;
    let stack = unsafe { &*d.stack };
    if at < stack.floor() && !stack.extend() {
        d.rejected = true;
        return;
    }
    d.frames.push(at);
    if levels == 0 {
        count(d.counts);
    } else {
        descend(d, levels - 1);
    }
    std::hint::black_box(&pad);
}

extern "C" fn descend_then_count(arg: usize) {
    let d = unsafe { &mut *(arg as *mut Descent) };
    descend(d, LEVELS);
    with(|p| p.log.push(0));
    yield_to_main();
    unreachable!("a finished task was resumed");
}

fn descent(stack: &Stack, counts: usize) -> Descent {
    Descent {
        stack,
        frames: Vec::with_capacity(LEVELS + 1),
        rejected: false,
        counts,
    }
}

fn start(sp: usize) {
    PONG.with(|p| {
        *p.borrow_mut() = Some(Pong {
            main: 0,
            task: sp,
            log: Vec::new(),
        })
    });
}

#[test]
fn a_task_runs_on_its_own_stack_and_yields_back_in_order() {
    let stack = taken();
    start(stack.prepare(count_to, 3));
    let mut seen = Vec::new();
    for _ in 0..4 {
        run_task();
        seen.push(with(|p| *p.log.last().unwrap()));
        assert!(stack.holds(with(|p| p.task)));
    }
    assert_eq!(seen, [10, 20, 30, 0]);
    PONG.with(|p| *p.borrow_mut() = None);
}

/// A recursion past a stack's first chunk stays on that stack: every frame below the one above it,
/// all in one reservation, and all of them live where the recursion stopped.
#[test]
fn a_recursion_past_a_stacks_first_chunk_stays_in_one_reservation() {
    let stack = taken();
    let mut d = descent(&stack, 1);
    let d = &raw mut d;
    start(stack.prepare(descend_then_count, d as usize));
    run_task();
    assert_eq!(with(|p| p.log.clone()), [10]);
    let stopped = with(|p| p.task);
    let (frames, rejected) = unsafe { ((*d).frames.clone(), (*d).rejected) };
    assert!(
        !rejected,
        "the stack refused to extend within its reservation"
    );
    assert_eq!(frames.len(), LEVELS + 1);
    assert!(
        frames.windows(2).all(|w| w[1] < w[0]),
        "a frame lay above its caller's"
    );
    assert!(
        frames.iter().all(|&f| stack.holds(f)),
        "a frame lay off the stack"
    );
    assert!(
        stack.top() - frames[LEVELS] > 2 * CHUNK,
        "the recursion never passed the stack's second chunk"
    );
    assert!(stack.floor() < stack.top() - 2 * CHUNK + STACK_MARGIN);
    assert!(
        stack.holds(stopped),
        "the recursion stopped on its own stack"
    );
    assert!(frames.iter().all(|&f| (stopped..stack.top()).contains(&f)));
    run_task();
    assert_eq!(with(|p| p.log.clone()), [10, 0]);
    PONG.with(|p| *p.borrow_mut() = None);
}

/// A stack grows a chunk at a time until its reservation is spent, and its floor never passes the
/// room the runtime's own frames keep above the guard.
#[test]
fn a_stack_extends_until_its_reservation_is_spent_and_its_floor_keeps_the_margin() {
    let stack = taken();
    let top = stack.top();
    assert_eq!(stack.floor(), top - CHUNK + STACK_MARGIN);
    let mut extended = 0;
    while stack.extend() {
        extended += 1;
        assert_eq!(stack.floor(), top - (extended + 1) * CHUNK + STACK_MARGIN);
    }
    assert_eq!(extended, RESERVE / CHUNK - 1);
    assert_eq!(stack.floor(), top - RESERVE + STACK_MARGIN);
    assert!(!stack.extend(), "a spent reservation extended again");
}

/// A stack given back as it was taken is the next one taken; one that grew is not kept.
#[test]
fn a_stack_given_back_unextended_is_taken_again_and_one_that_grew_is_not() {
    let first = taken();
    let top = first.top();
    drop(first);
    let again = taken();
    assert_eq!(again.top(), top, "the stack given back was not taken again");
    assert!(again.extend());
    drop(again);
    let next = taken();
    assert_eq!(
        next.floor(),
        next.top() - CHUNK + STACK_MARGIN,
        "a stack that grew was handed out again"
    );
}

/// Ten thousand suspended bodies or tasks hold ten thousand stacks at once.
#[test]
fn ten_thousand_stacks_are_held_at_once_and_given_back_on_one_thread() {
    let stacks: Vec<Stack> = (0..10_000).map(|_| taken()).collect();
    for s in &stacks {
        unsafe { ((s.top() - 8) as *mut u64).write(1) };
    }
    let tops: std::collections::HashSet<usize> = stacks.iter().map(Stack::top).collect();
    assert_eq!(tops.len(), stacks.len());
}

/// What a task that grows and then stops sees of the context it runs under.
struct Grows {
    ctx: *mut ply_compiled::rt::Ctx,
    task: *const Stack,
    main: usize,
    sp: usize,
    floor_when_resumed: usize,
}

extern "C" fn grow_then_stop(arg: usize) {
    let g = arg as *mut Grows;
    unsafe {
        let task = &*(*g).task;
        (*(*g).ctx).stack_floor.store(task.enter(), Relaxed);
        if task.extend() {
            (*(*g).ctx).stack_floor.store(task.floor(), Relaxed);
        }
        switch_keeping((*g).ctx, &raw mut (*g).sp, (*g).main);
        (*g).floor_when_resumed = (*(*g).ctx).stack_floor.load(Relaxed);
        let mut spent = 0;
        switch(&mut spent, (*g).main);
    }
    std::process::abort();
}

/// A task that grew and stopped resumes under its grown stack's floor, whatever floor the stack
/// that resumed it left behind.
#[test]
fn a_task_that_grew_resumes_under_its_own_floor() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some(loaded) = units::opened(dir.path(), &[], |_| {}) else {
        return;
    };
    let native = loaded.expect("the unit loads");
    let mut ctx = native.context();
    ctx.begin();
    let task = taken();
    let mut g = Grows {
        ctx: &raw mut ctx,
        task: &task,
        main: 0,
        sp: 0,
        floor_when_resumed: 0,
    };
    let g = &raw mut g;
    unsafe {
        let first = task.prepare(grow_then_stop, g as usize);
        switch_keeping((*g).ctx, &raw mut (*g).main, first);
        assert!(
            !task.holds((*(*g).ctx).stack_floor.load(Relaxed)),
            "the stack the task stopped to kept its floor"
        );
        (*(*g).ctx)
            .stack_floor
            .store(task.top() - CHUNK + STACK_MARGIN, Relaxed);
        switch_keeping((*g).ctx, &raw mut (*g).main, (*g).sp);
        assert!(
            task.floor() < task.top() - CHUNK + STACK_MARGIN,
            "the task never grew"
        );
        assert_eq!((*g).floor_when_resumed, task.floor());
    }
    ctx.end();
}
