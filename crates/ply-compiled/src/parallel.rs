//! `parallel { .. }`: branches the checker showed touch no resource in common, run at once on
//! threads of their own and answered as evaluating them left to right answers.

use crate::blocks::Blocks;
use crate::emitted::Block;
use crate::heap::{self, Word};
use crate::rt::{Ctx, FAILED_OUT_OF_STEPS, PROGRAM_STACK, call_value, stack_floor};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::OnceLock;

/// What a `parallel` block's branches are: closures the release's code made, or closure blocks
/// emitted code made. Each is taken, called with nothing, and let go once it has run.
#[derive(Clone, Copy)]
pub(crate) enum Branches {
    Words,
    Blocks,
}

impl Branches {
    fn call(self, ctx: *mut Ctx, f: Word) -> Word {
        match self {
            Branches::Words => call_value(ctx, f, &[]),
            Branches::Blocks => unsafe { Block::of(f).call(ctx, &[]) },
        }
    }

    fn let_go(self, ctx: *mut Ctx, f: Word) {
        match self {
            Branches::Words => heap::dec(f),
            Branches::Blocks => unsafe { Block::of(f).let_go_function(ctx) },
        }
    }

    /// What is left in the slot of a branch that answered after one before it failed: the release's
    /// code's answer is let go here, emitted code's is left for the code that knows its type.
    fn abandoned(self, answer: Word) -> Word {
        match self {
            Branches::Words => {
                heap::dec(answer);
                0
            }
            Branches::Blocks => answer,
        }
    }

    /// Marks everything the branches hold shared, or answers that one cannot leave this thread.
    fn share(self, ctx: *mut Ctx, branches: &[Word]) -> bool {
        match self {
            Branches::Words => heap::share(branches),
            Branches::Blocks => branches.iter().all(|f| unsafe { Block::of(*f).share(ctx) }),
        }
    }
}

thread_local! {
    /// The machine whose branch this thread is running, while it runs one, and 0 otherwise.
    static BRANCH_OF: Cell<u64> = const { Cell::new(0) };

    /// By machine, the blocks the branches a pool thread runs take from, kept from one branch to the
    /// next: what a branch leaves behind its answer stays this thread's, freed back to it from
    /// wherever it dies, and the room around it is the next branch's, where a heap of the branch's
    /// own would leave it to the parent. A machine's go when its entry ends (`workers_ended`).
    static WORKERS: RefCell<HashMap<u64, Blocks>> = RefCell::new(HashMap::new());
}

/// The blocks a branch of `machine` running on this thread takes from: the pool thread's, kept from
/// it until `worker_returned` hands them back; none on a thread outside the pool.
fn worker_taken(machine: u64) -> Option<Blocks> {
    rayon::current_thread_index()?;
    Some(WORKERS.with(|w| w.borrow_mut().remove(&machine).unwrap_or_default()))
}

/// `blocks` back as this pool thread's for `machine`, with whatever a branch nested in the one that
/// took them left there meanwhile.
fn worker_returned(machine: u64, mut blocks: Blocks) {
    WORKERS.with(|w| {
        let mut held = w.borrow_mut();
        if let Some(nested) = held.remove(&machine) {
            blocks.adopt(nested);
        }
        held.insert(machine, blocks);
    })
}

/// What `read` makes of the blocks each pool thread holds for `machine`.
pub(crate) fn worker_blocks<T: Send>(machine: u64, read: impl Fn(&Blocks) -> T + Sync) -> Vec<T> {
    match pool() {
        Some(pool) => pool
            .broadcast(|_| WORKERS.with(|w| w.borrow().get(&machine).map(&read)))
            .into_iter()
            .flatten()
            .collect(),
        None => Vec::new(),
    }
}

/// Lets go of the blocks every pool thread holds for `machine`, whose entry has ended, so nothing
/// in them is live.
pub(crate) fn workers_ended(machine: u64) {
    if let Some(pool) = pool() {
        pool.broadcast(|_| drop(WORKERS.with(|w| w.borrow_mut().remove(&machine))));
    }
}

/// The threads branches run on, one per core; none on a machine with one.
fn pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
    POOL.get_or_init(|| {
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        (threads > 1)
            .then(|| {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .stack_size(PROGRAM_STACK)
                    .thread_name(|i| format!("ply-parallel-{i}"))
                    .build()
                    .ok()
            })
            .flatten()
    })
    .as_ref()
}

/// Runs the `n` nullary closures at `slots`, writing each one's answer over it, at once where
/// `allowed`. Where the block fails, a branch that failed or never ran leaves 0 in its slot.
pub(crate) unsafe fn run(ctx: *mut Ctx, slots: *mut Word, n: usize, allowed: bool, kind: Branches) {
    let branches = unsafe { std::slice::from_raw_parts_mut(slots, n) };
    let c = unsafe { &*ctx };
    // A thread that waits on a block takes up the pool's other branches meanwhile. A machine
    // entered inside another's branch, as a test a run's lane enters is, would then carry a whole
    // other test on its stack and its clock, so its own blocks run in turn.
    let nested = BRANCH_OF.with(|of| of.get() != 0 && of.get() != c.id.0);
    let at_once = !nested && allowed;
    match pool() {
        Some(pool) if n >= 2 && at_once && kind.share(ctx, branches) => unsafe {
            in_parallel(ctx, pool, branches, kind)
        },
        _ => unsafe { in_order(ctx, branches, kind) },
    }
}

/// What the block means: each branch in turn, until one fails.
unsafe fn in_order(ctx: *mut Ctx, branches: &mut [Word], kind: Branches) {
    for i in 0..branches.len() {
        let closure = branches[i];
        let answer = kind.call(ctx, closure);
        kind.let_go(ctx, closure);
        if unsafe { (*ctx).failed } != 0 {
            branches[i] = 0;
            for rest in &mut branches[i + 1..] {
                kind.let_go(ctx, *rest);
                *rest = 0;
            }
            return;
        }
        branches[i] = answer;
    }
}

/// One branch: its own context, the closure it calls, and what that answered.
struct Branch {
    ctx: Box<Ctx>,
    closure: Word,
    answer: Word,
    kind: Branches,
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
        c.stack_floor = stack_floor();
        let branch_was = BRANCH_OF.with(|of| of.replace(c.id.0));
        let own = worker_taken(c.id.0).map(|w| c.heap.swap_blocks(w));
        let heap_was = heap::swap_current(&mut c.heap);
        let site_was = heap::poison::swap(&raw const c.site_root);
        let running_was = crate::rt::swap_running(ctx);
        let mine = c.instances();
        let instances_were = ply_core::instances::swap(mine);
        branch.answer = branch.kind.call(ctx, branch.closure);
        branch.kind.let_go(ctx, branch.closure);
        // A reactor belongs to the thread that made it, and this branch is over.
        c.runtime = None;
        ply_core::instances::swap(instances_were);
        crate::rt::swap_running(running_was);
        heap::poison::swap(site_was);
        heap::swap_current(heap_was);
        if let Some(own) = own {
            worker_returned(c.id.0, c.heap.swap_blocks(own));
        }
        BRANCH_OF.with(|of| of.set(branch_was));
    }
}

unsafe fn in_parallel(
    ctx: *mut Ctx,
    pool: &rayon::ThreadPool,
    branches: &mut [Word],
    kind: Branches,
) {
    let parent = unsafe { &mut *ctx };
    parent.pooled = true;
    let mut runs: Vec<Branch> = branches
        .iter()
        .map(|&closure| Branch {
            ctx: Box::new(parent.branch()),
            closure,
            answer: 0,
            kind,
        })
        .collect();
    pool.in_place_scope(|scope| {
        for run in runs.iter_mut() {
            let handed = Handed(run);
            scope.spawn(move |_| unsafe { handed.run() });
        }
    });
    // Left to right: the first branch that failed, or that ran past what the budget had left once
    // the branches before it had spent theirs, is how evaluating them in turn would have ended.
    let left = parent.steps_left();
    let mut spent = 0i64;
    let mut ended = false;
    for (slot, run) in branches.iter_mut().zip(runs) {
        let Branch { ctx, answer, .. } = run;
        let mut ctx = *ctx;
        if ended {
            *slot = if ctx.failed == 0 {
                kind.abandoned(answer)
            } else {
                0
            };
            parent.absorb(ctx);
            continue;
        }
        spent = spent.saturating_add(ctx.ticks);
        let out_of_steps =
            ctx.failed == FAILED_OUT_OF_STEPS || left.is_some_and(|left| spent > left);
        let failure = ctx.take_branch_failure();
        *slot = if failure.is_some() { 0 } else { answer };
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
