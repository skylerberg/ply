use crate::blocks::Blocks;
use crate::emitted::Block;
use crate::heap::{self, Word};
use crate::rt::{Context, PROGRAM_STACK, stack_floor};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::Ordering::Relaxed;

pub(crate) fn thread_branch() -> u64 {
    PARALLEL_ENTRY_ID.with(Cell::get)
}

pub(crate) fn inherit_branch(machine: u64) {
    PARALLEL_ENTRY_ID.with(|of| of.set(machine));
}

fn call(context: *mut Context, f: Word) -> Word {
    unsafe { Block::of(f).call(context, &[]) }
}

fn let_go(context: *mut Context, f: Word) {
    unsafe { Block::of(f).release_function_if_last(context) }
}

fn share(context: *mut Context, branches: &[Word]) -> bool {
    branches
        .iter()
        .all(|f| unsafe { Block::of(*f).share(context) })
}

thread_local! {
    static PARALLEL_ENTRY_ID: Cell<u64> = const { Cell::new(0) };

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

pub(crate) unsafe fn run(context: *mut Context, slots: *mut Word, n: usize, allowed: bool) {
    let branches = unsafe { std::slice::from_raw_parts_mut(slots, n) };
    let c = unsafe { &mut *context };
    let words = (c.entering_descriptor, c.entering_words);
    c.entering_descriptor = 0;
    c.entering_words = 0;
    let nested = PARALLEL_ENTRY_ID.with(|of| of.get() != 0 && of.get() != c.id.0);
    let at_once = !nested && allowed && shared_only(words);
    match pool() {
        Some(pool) if n >= 2 && at_once && share(context, branches) => unsafe {
            in_parallel(context, pool, branches, words)
        },
        _ => unsafe { in_order(context, branches, words) },
    }
}

fn shared_only((descriptor, words): (Word, Word)) -> bool {
    if descriptor == 0 {
        return true;
    }
    let n = unsafe { *(descriptor as *const Word) };
    (0..n).all(|i| {
        let object = unsafe { *(words as *const Word).add(i as usize) } as *const Word;
        let flags = unsafe { *object };
        flags & crate::rt::EVIDENCE_SHARED != 0
    })
}

unsafe fn in_order(context: *mut Context, branches: &mut [Word], words: (Word, Word)) {
    for i in 0..branches.len() {
        let closure = branches[i];
        let c = unsafe { &mut *context };
        (c.entering_descriptor, c.entering_words) = words;
        let result = call(context, closure);
        let_go(context, closure);
        if unsafe { (*context).failed } != 0 {
            branches[i] = 0;
            for rest in &mut branches[i + 1..] {
                let_go(context, *rest);
                *rest = 0;
            }
            return;
        }
        branches[i] = result;
    }
}

struct Branch {
    context: Box<Context>,
    closure: Word,
    result: Word,
}

struct Unwatch(*mut Context);

impl Drop for Unwatch {
    fn drop(&mut self) {
        unsafe { (*self.0).stop_deadline_watch() };
    }
}

struct BranchPointer(*mut Branch);

unsafe impl Send for BranchPointer {}

impl BranchPointer {
    unsafe fn run(self) {
        let branch = unsafe { &mut *self.0 };
        let context: *mut Context = &mut *branch.context;
        let c = unsafe { &mut *context };
        c.stack_floor.store(stack_floor(), Relaxed);
        let _unwatch = Unwatch(context);
        c.watch();
        let branch_was = PARALLEL_ENTRY_ID.with(|of| of.replace(c.id.0));
        let own = worker_taken(c.id.0).map(|w| c.heap.swap_blocks(w));
        let heap_was = heap::swap_current(&mut c.heap);
        let site_was = heap::poison_freed::swap(&raw const c.site_root);
        let running_was = crate::rt::swap_running(context);
        branch.result = call(context, branch.closure);
        if c.failed == crate::rt::FAILED_OUT_OF_STACK {
            crate::rt::release_unused_stack_pages();
        }
        let_go(context, branch.closure);
        c.runtime = None;
        crate::rt::swap_running(running_was);
        heap::poison_freed::swap(site_was);
        heap::swap_current(heap_was);
        if let Some(own) = own {
            worker_returned(c.id.0, c.heap.swap_blocks(own));
        }
        PARALLEL_ENTRY_ID.with(|of| of.set(branch_was));
    }
}

unsafe fn in_parallel(
    context: *mut Context,
    pool: &rayon::ThreadPool,
    branches: &mut [Word],
    words: (Word, Word),
) {
    let parent = unsafe { &mut *context };
    parent.pooled = true;
    let mut runs: Vec<Branch> = branches
        .iter()
        .map(|&closure| {
            let mut context = Box::new(parent.branch());
            (context.entering_descriptor, context.entering_words) = words;
            Branch {
                context,
                closure,
                result: 0,
            }
        })
        .collect();
    pool.in_place_scope(|scope| {
        for run in runs.iter_mut() {
            let handed = BranchPointer(run);
            scope.spawn(move |_| unsafe { handed.run() });
        }
    });
    let mut ended = false;
    for (slot, run) in branches.iter_mut().zip(runs) {
        let Branch {
            context, result, ..
        } = run;
        let mut context = *context;
        if ended {
            *slot = if context.failed == 0 { result } else { 0 };
            parent.absorb(context);
            continue;
        }
        let failure = context.take_failure_to_merge();
        *slot = if failure.is_some() { 0 } else { result };
        parent.absorb(context);
        if let Some(f) = failure {
            parent.merge_concurrent_failure(f);
            ended = true;
        }
    }
}
