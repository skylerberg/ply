use ply_codegen::stack::{Stack, switch};
use std::cell::RefCell;

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

/// Switches back to the test's stack, storing this stack's pointer where the test reads it.
fn yield_to_main() {
    let (task, main) = with(|p| (&mut p.task as *mut usize, p.main));
    unsafe { switch(&mut *task, main) };
}

fn run_task() {
    let (main, task) = with(|p| (&mut p.main as *mut usize, p.task));
    unsafe { switch(&mut *main, task) };
}

/// The address of a sixteen-byte-aligned local, which is aligned only if the frame was
/// entered as the ABI requires; a trampoline that enters off by a word is caught here.
#[inline(never)]
fn aligned_local() -> usize {
    #[repr(align(16))]
    struct Aligned([u8; 16]);
    let a = Aligned([0; 16]);
    std::hint::black_box(&a.0) as *const [u8; 16] as usize
}

extern "C" fn count_to(n: usize) {
    assert_eq!(
        aligned_local() % 16,
        0,
        "the task was entered off the ABI's alignment"
    );
    for i in 1..=n {
        let local = i * 10;
        with(|p| p.log.push(local));
        yield_to_main();
    }
    with(|p| p.log.push(0));
    yield_to_main();
    unreachable!("a finished task was resumed");
}

#[test]
fn a_task_runs_on_its_own_stack_and_yields_back_in_order() {
    let stack = Stack::new();
    let sp = stack.prepare(count_to, 3);
    PONG.with(|p| {
        *p.borrow_mut() = Some(Pong {
            main: 0,
            task: sp,
            log: Vec::new(),
        })
    });
    let mut seen = Vec::new();
    for _ in 0..4 {
        run_task();
        seen.push(with(|p| *p.log.last().unwrap()));
        assert!(stack.holds(with(|p| p.task)));
    }
    assert_eq!(seen, [10, 20, 30, 0]);
    PONG.with(|p| *p.borrow_mut() = None);
}

#[test]
fn a_snapshot_restored_in_place_resumes_the_same_frame_again() {
    let stack = Stack::new();
    let sp = stack.prepare(count_to, 2);
    PONG.with(|p| {
        *p.borrow_mut() = Some(Pong {
            main: 0,
            task: sp,
            log: Vec::new(),
        })
    });
    let run = run_task;
    run();
    assert_eq!(with(|p| p.log.clone()), [10]);
    let captured = with(|p| p.task);
    let snapshot = stack.live(captured).to_vec();
    run();
    assert_eq!(with(|p| p.log.clone()), [10, 20]);
    unsafe { stack.restore(captured, &snapshot) };
    with(|p| p.task = captured);
    run();
    assert_eq!(
        with(|p| p.log.clone()),
        [10, 20, 20],
        "the second resumption of the captured frame counts from where it was captured"
    );
    run();
    assert_eq!(with(|p| p.log.clone()), [10, 20, 20, 0]);
    PONG.with(|p| *p.borrow_mut() = None);
}
