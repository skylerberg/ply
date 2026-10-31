use crate::blocks::Blocks;
use crate::emitted::Block;
use crate::heap::{self, Word};
use crate::rt::{Ctx, PROGRAM_STACK, stack_floor};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::Ordering::Relaxed;

fn call(ctx: *mut Ctx, f: Word) -> Word {
    unsafe { Block::of(f).call(ctx, &[]) }
}

fn let_go(ctx: *mut Ctx, f: Word) {
    unsafe { Block::of(f).let_go_function(ctx) }
}

fn share(ctx: *mut Ctx, branches: &[Word]) -> bool {
    branches.iter().all(|f| unsafe { Block::of(*f).share(ctx) })
}

thread_local! {
    static BRANCH_OF: Cell<u64> = const { Cell::new(0) };

    static WORKERS: RefCell<HashMap<u64, Blocks>> = RefCell::new(HashMap::new());
}

fn worker_taken(machine: u64) -> Option<Blocks> {
    rayon::current_thread_index()?;
    Some(WORKERS.with(|w| w.borrow_mut().remove(&machine).unwrap_or_default()))
}

fn worker_returned(machine: u64, mut blocks: Blocks) {
    WORKERS.with(|w| {
        let mut held = w.borrow_mut();
        if let Some(nested) = held.remove(&machine) {
            blocks.adopt(nested);
        }
        held.insert(machine, blocks);
    })
}

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

pub(crate) fn workers_ended(machine: u64) {
    if let Some(pool) = pool() {
        pool.broadcast(|_| drop(WORKERS.with(|w| w.borrow_mut().remove(&machine))));
    }
}

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

pub(crate) unsafe fn run(ctx: *mut Ctx, slots: *mut Word, n: usize, allowed: bool) {
    let branches = unsafe { std::slice::from_raw_parts_mut(slots, n) };
    let c = unsafe { &mut *ctx };
    let words = (c.entering_desc, c.entering_words);
    c.entering_desc = 0;
    c.entering_words = 0;
    let nested = BRANCH_OF.with(|of| of.get() != 0 && of.get() != c.id.0);
    let at_once = !nested && allowed && bridged_only(c, words);
    match pool() {
        Some(pool) if n >= 2 && at_once && share(ctx, branches) => unsafe {
            in_parallel(ctx, pool, branches, words)
        },
        _ => unsafe { in_order(ctx, branches, words) },
    }
}

fn bridged_only(c: &Ctx, (desc, words): (Word, Word)) -> bool {
    if desc == 0 {
        return true;
    }
    let n = unsafe { *(desc as *const Word) };
    (0..n).all(|i| unsafe { *(words as *const Word).add(i as usize) } == c.bridge)
}

unsafe fn in_order(ctx: *mut Ctx, branches: &mut [Word], words: (Word, Word)) {
    for i in 0..branches.len() {
        let closure = branches[i];
        let c = unsafe { &mut *ctx };
        (c.entering_desc, c.entering_words) = words;
        let result = call(ctx, closure);
        let_go(ctx, closure);
        if unsafe { (*ctx).failed } != 0 {
            branches[i] = 0;
            for rest in &mut branches[i + 1..] {
                let_go(ctx, *rest);
                *rest = 0;
            }
            return;
        }
        branches[i] = result;
    }
}

struct Branch {
    ctx: Box<Ctx>,
    closure: Word,
    result: Word,
}

struct Unwatch(*mut Ctx);

impl Drop for Unwatch {
    fn drop(&mut self) {
        unsafe { (*self.0).unwatch() };
    }
}

struct Handed(*mut Branch);

unsafe impl Send for Handed {}

impl Handed {
    unsafe fn run(self) {
        let branch = unsafe { &mut *self.0 };
        let ctx: *mut Ctx = &mut *branch.ctx;
        let c = unsafe { &mut *ctx };
        c.stack_floor.store(stack_floor(), Relaxed);
        let _unwatch = Unwatch(ctx);
        c.watch();
        let branch_was = BRANCH_OF.with(|of| of.replace(c.id.0));
        let own = worker_taken(c.id.0).map(|w| c.heap.swap_blocks(w));
        let heap_was = heap::swap_current(&mut c.heap);
        let site_was = heap::poison::swap(&raw const c.site_root);
        let running_was = crate::rt::swap_running(ctx);
        branch.result = call(ctx, branch.closure);
        if c.failed == crate::rt::FAILED_OUT_OF_STACK {
            crate::rt::let_go_below_here();
        }
        let_go(ctx, branch.closure);
        c.runtime = None;
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
    words: (Word, Word),
) {
    let parent = unsafe { &mut *ctx };
    parent.pooled = true;
    let mut runs: Vec<Branch> = branches
        .iter()
        .map(|&closure| {
            let mut ctx = Box::new(parent.branch());
            (ctx.entering_desc, ctx.entering_words) = words;
            Branch {
                ctx,
                closure,
                result: 0,
            }
        })
        .collect();
    pool.in_place_scope(|scope| {
        for run in runs.iter_mut() {
            let handed = Handed(run);
            scope.spawn(move |_| unsafe { handed.run() });
        }
    });
    let mut ended = false;
    for (slot, run) in branches.iter_mut().zip(runs) {
        let Branch { ctx, result, .. } = run;
        let mut ctx = *ctx;
        if ended {
            *slot = if ctx.failed == 0 { result } else { 0 };
            parent.absorb(ctx);
            continue;
        }
        let failure = ctx.take_branch_failure();
        *slot = if failure.is_some() { 0 } else { result };
        parent.absorb(ctx);
        if let Some(f) = failure {
            parent.fail_from_branch(f);
            ended = true;
        }
    }
}
