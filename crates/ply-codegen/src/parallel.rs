//! `parallel { .. }`: branches the checker showed touch no resource in common, run at once on
//! threads of their own and answered as evaluating them left to right answers.

use crate::heap::{self, Word};
use crate::rt::{Ctx, FAILED_OUT_OF_STEPS, call_value, stack_floor};
use std::cell::Cell;
use std::sync::OnceLock;

thread_local! {
    /// The floor of the stack a block waits on, while it waits: the pool runs other branches on a
    /// waiting worker, on that same stack.
    static WAITING_FLOOR: Cell<usize> = const { Cell::new(0) };
}

/// Each pool thread's own stack; a branch nesting deeper grows onto stacks of the runtime's own.
const STACK: usize = 16 << 20;

/// The threads branches run on, one per core; none on a machine with one.
fn pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
    POOL.get_or_init(|| {
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        (threads > 1)
            .then(|| {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .stack_size(STACK)
                    .thread_name(|i| format!("ply-parallel-{i}"))
                    .build()
                    .ok()
            })
            .flatten()
    })
    .as_ref()
}

/// Runs the `n` nullary closures at `slots`, writing each one's answer over it.
pub(crate) unsafe fn run(ctx: *mut Ctx, slots: *mut Word, n: usize) {
    let branches = unsafe { std::slice::from_raw_parts_mut(slots, n) };
    let at_once = unsafe { &*ctx }.runs_branches_at_once();
    match pool() {
        Some(pool) if n >= 2 && at_once && heap::share(branches) => unsafe {
            in_parallel(ctx, pool, branches)
        },
        _ => unsafe { in_order(ctx, branches) },
    }
}

/// What the block means: each branch in turn, until one fails.
unsafe fn in_order(ctx: *mut Ctx, branches: &mut [Word]) {
    for i in 0..branches.len() {
        let closure = branches[i];
        branches[i] = call_value(ctx, closure, &[]);
        heap::dec(closure);
        if unsafe { (*ctx).failed } != 0 {
            for rest in &mut branches[i + 1..] {
                heap::dec(*rest);
                *rest = 0;
            }
            return;
        }
    }
}

/// One branch: its own context, the closure it calls, and what that answered.
struct Branch {
    ctx: Box<Ctx>,
    closure: Word,
    answer: Word,
}

/// A branch handed to a pool thread. The parent waits for every branch before it reads one again,
/// and nothing else names a branch's context or what its closure reaches but shared objects.
struct Handed(*mut Branch);

unsafe impl Send for Handed {}

impl Handed {
    /// Runs the branch on this thread, which may be one already running another branch further up
    /// its stack: what that one set for itself is put back after.
    unsafe fn run(self) {
        let branch = unsafe { &mut *self.0 };
        let ctx: *mut Ctx = &mut *branch.ctx;
        let c = unsafe { &mut *ctx };
        let waiting = WAITING_FLOOR.with(|f| f.get());
        c.stack_floor = if waiting != 0 { waiting } else { stack_floor() };
        let heap_was = heap::swap_current(&mut c.heap);
        let site_was = heap::poison::swap(&raw const c.site_root);
        branch.answer = call_value(ctx, branch.closure, &[]);
        heap::dec(branch.closure);
        // A reactor belongs to the thread that made it, and this branch is over.
        c.runtime = None;
        heap::poison::swap(site_was);
        heap::swap_current(heap_was);
    }
}

unsafe fn in_parallel(ctx: *mut Ctx, pool: &rayon::ThreadPool, branches: &mut [Word]) {
    let parent = unsafe { &mut *ctx };
    let mut runs: Vec<Branch> = branches
        .iter()
        .map(|&closure| Branch {
            ctx: Box::new(parent.branch()),
            closure,
            answer: 0,
        })
        .collect();
    let waited = WAITING_FLOOR.with(|f| f.replace(parent.stack_floor));
    pool.in_place_scope(|scope| {
        for run in runs.iter_mut() {
            let handed = Handed(run);
            scope.spawn(move |_| unsafe { handed.run() });
        }
    });
    WAITING_FLOOR.with(|f| f.set(waited));
    // Left to right: the first branch that failed, or that ran past what the budget had left once
    // the branches before it had spent theirs, is how evaluating them in turn would have ended.
    let left = parent.steps_left();
    let mut spent = 0i64;
    let mut ended = false;
    for (slot, run) in branches.iter_mut().zip(runs) {
        let Branch { ctx, answer, .. } = run;
        let mut ctx = *ctx;
        if ended {
            heap::dec(answer);
            *slot = 0;
            parent.absorb(ctx);
            continue;
        }
        spent = spent.saturating_add(ctx.ticks);
        let out_of_steps =
            ctx.failed == FAILED_OUT_OF_STEPS || left.is_some_and(|left| spent > left);
        let failure = ctx.take_branch_failure();
        *slot = answer;
        parent.absorb(ctx);
        if out_of_steps {
            parent.fail_out_of_steps();
            ended = true;
        } else if let Some((code, diagnostic)) = failure {
            parent.fail_from_branch(code, diagnostic);
            ended = true;
        }
    }
}
