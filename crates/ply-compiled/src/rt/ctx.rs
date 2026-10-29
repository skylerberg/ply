//! The context an entry runs in.

use super::*;

/// Entries begun in this process, every context's: contexts share a unit's memo, so an entry's
/// number must be unique across them, not just within one.
pub(super) static ENTRIES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[repr(C)]
pub struct Ctx {
    pub failed: i64,
    /// Nested native calls the running stack is still allowed. A call spends one and its return
    /// gives it back; a frame a failure returns through gives nothing back, so whatever runs on
    /// past a failure first puts this where it stood. It is the running
    /// stack's own: a switch saves it with the stack it leaves and puts back the other's.
    pub fuel: i64,
    /// The lowest address a compiled frame may begin at: C frames can overflow within the fuel.
    /// The watchdog raises it past every frame once the entry's clock runs out ([`watched`]).
    pub stack_floor: std::sync::atomic::AtomicUsize,
    /// The site the body stored before a fallible call, as bytes from its root's start; the root
    /// is `-1` until one is.
    pub site_root: i64,
    pub site_start: i64,
    pub site_end: i64,
    /// A counter the released compiler's code adds each call to, and the count past which it calls
    /// [`rt_tick`]: `i64::MAX`, so it never does.
    pub ticks: i64,
    pub next_tick: i64,
    /// The innermost frame record on the running stack's chain: [`crate::frames::base`] at the
    /// bottom of a detached body's own stack, the one kind that records, and null on any other. A
    /// switch saves it with the stack it leaves and puts back the other's.
    pub frames: *mut crate::frames::PlyFrame,
    /// How deeply drop roots run nested: one at [`RELEASE_DEPTH`] puts its release off through
    /// [`rt_release_later`], and the outermost drains what waits through [`rt_release_drain`].
    /// Compiled code reads and writes it, so it stays among the fields before the runtime's own.
    pub release_depth: i64,
    /// How many releases wait in the context's buffer for the outermost drop root to drain them.
    pub release_waiting: i64,
    /// The `handle` instance a [`FAILED_TARGET`] failure unwinds to, which its landing compares
    /// with its own. Compiled code reads and writes it, as it does the three below.
    pub target: Word,
    /// The row descriptor and words the next closure the runtime calls is handed by its uniform
    /// entry, which takes them and sets both to 0; 0 for none, which hands it the bridge.
    pub entering_desc: Word,
    pub entering_words: Word,
    /// Whether a perform a clause answers in place is recorded through [`rt_performed`]: while a
    /// test reads what its entries performed or a seeded region runs.
    pub recording: i64,
    /// The evidence object of every effect for code that holds no word answering one
    /// ([`BRIDGE`]).
    pub bridge: Word,
    /// A word no code reads, where the release's compiler lays the context out with one.
    pub hosted: Word,
    /// What an entry hands its root for an effect nothing answers ([`UNSERVED`]).
    pub unserved: Word,
    /// Where the perform a clause answers stood, as the site fields hold one, which compiled code
    /// stores as it enters the clause: what a region's request is placed at.
    pub performed_root: i64,
    pub performed_start: i64,
    pub performed_end: i64,
    /// The task running, as `abi.ply`'s `ctx_struct` says: what an instance records as it opens,
    /// which only that task may leave.
    pub task: i64,
    /// What compiled code takes a small block from the heap by, and gives one back to, without
    /// calling the runtime: the heap's own, wherever the heap moves.
    pub alloc: *mut crate::heap::Alloc,
    /// The fuel this entry began with: how deeply its calls may nest.
    pub(super) depth_bound: i64,
    /// Chunks of stack this entry's calls made writable past what their stacks started with, so
    /// growing is observable.
    pub grown: u64,
    /// When the running entry's time budget is spent, if it has one.
    pub(super) deadline: Option<std::time::Instant>,
    pub(super) time_budget_ms: u64,
    pub heap: Heap,
    /// Objects the last entry allocated, kept because [`Ctx::end`] clears the heap's count.
    pub(super) last_entry: usize,
    pub tables: Arc<Tables>,
    /// Why the last entry failed.
    pub diagnostic: Option<Diagnostic>,
    /// What the runtime's frames on each stack run in this entry keep ([`Kept`]), the innermost
    /// last, the entry's own stack first; `current` is the one running.
    pub(crate) stacks: Vec<Vec<Kept>>,
    /// Indices of `stacks` a finished production task gave back, which nothing names any longer.
    pub(super) free_stacks: Vec<usize>,
    pub(crate) current: usize,
    /// Every atom a compiled `perform` performed since the entry began, for the machine's trace.
    pub performed: Vec<EffectAtom>,
    /// The detached bodies this entry has opened, named by index from their instances and tokens.
    pub(crate) detached: Vec<crate::detached::Detached>,
    pub(crate) starting_detached: Option<usize>,
    /// The running entry's number, which no other entry in the process shares: a continuation
    /// carries it, since its index names a body of that entry alone.
    pub(crate) entry: u64,
    /// The host boundary: what a `perform` nothing on the stack answers reaches.
    pub(crate) binding: Arc<ply_core::HostBinding>,
    pub(crate) runtime: Option<Rc<dyn ply_core::HostRuntime>>,
    /// What makes a reactor, which a `parallel` branch on another thread needs one of its own of:
    /// the bound run's, or one over a host of this context's own.
    pub(crate) runtime_factory: ply_core::RuntimeFactory,
    pub(crate) re_executed: bool,
    pub(crate) host_use: ply_core::host::HostUse,
    pub(crate) host_ops: u64,
    /// The last at-most-once host operation this entry performed, for `E0426`.
    pub(crate) last_linear: Option<crate::host::HostMark>,
    pub(crate) id: ply_core::host::MachineId,
    /// Whether the entry's branches ran on the pool's threads, whose blocks then hold its objects too.
    pub(crate) pooled: bool,
    /// What the host runtime said when an entry ended, for the machine's teardown warnings.
    pub(crate) teardown: Vec<Diagnostic>,
    /// The seed the driver set, and the step budget, for the regions this entry opens.
    pub(crate) seed: ply_core::Seed,
    pub(crate) sim_steps: u32,
    pub(crate) entered_sims: u32,
    /// The regions `std.internal.schedule` opened on this context, innermost last, which a bracket
    /// shields its running task in and a seeded step records what it touched for.
    pub(crate) regions: Vec<(*mut crate::tasks::Region, [i64; 3])>,
    /// What the running seeded step touched, each beside where it was made.
    pub footprint: Vec<(crate::tasks::Touched, [i64; 3])>,
    /// The trail the entry's last seeded region left, and the interleaving its regions took, each as
    /// the scheduler wrote it.
    pub(crate) trail_kept: Option<Vec<u8>>,
    /// What the run held as its root's body returned, where the run's region noted it.
    pub(crate) held_at_root: Option<crate::entry::Held>,
    pub interleaving: Option<Vec<u8>>,
    /// The raise [`FAILED_PENDING`] stands for; its diagnostic is `diagnostic`.
    pub(crate) pending: Option<Unanswered>,
    /// The words of the raise `rt_pending_raised_at` last handed compiled code, with a word to
    /// spare past them, as a perform's arguments have.
    pub(crate) pending_words: Vec<Word>,
    /// Whether whoever runs this context's entries reads what they performed.
    pub(crate) traced: bool,
    /// How many holds this entry has made, which numbers the next.
    pub(super) holds_made: u64,
    /// The heap and poison site of the entry this one began inside, put back when it ends.
    pub(super) outer: (*mut Heap, *const i64),
    /// Releases a drop root at [`RELEASE_DEPTH`] put off: each word with the root that releases it.
    pub(crate) release_later: Vec<(crate::emitted::Handed, crate::emitted::DropRoot)>,
    /// Stops whose continuation's last holder let it go, by body and capture, for the next drain to
    /// drop: never inside the release that let it go.
    pub(crate) continuations_died: Vec<(usize, usize)>,
    /// The crossings under way, the innermost last.
    pub(crate) crossings: Vec<Crossing>,
    /// The bytes a native answered, until compiled code copies them out with [`rt_taken`].
    pub(crate) taken: Vec<u8>,
    /// The context running on this thread when this one's entry began, put back when it ends.
    pub(super) outer_running: *mut Ctx,
}

/// How a `parallel` branch failed, as its parent fails in turn.
pub(crate) struct BranchFailure {
    code: i64,
    diagnostic: Option<Diagnostic>,
    pending: Option<Unanswered>,
}

/// A raise no clause answers in place, left for the code around the failure, whose innermost
/// `handle` answering it takes it: `abort.raise` of a message, or another raise with what it
/// carries.
pub(crate) enum Unanswered {
    Abort(String),
    Raised {
        effect: Symbol,
        op: Symbol,
        args: Raised,
    },
}

impl Unanswered {
    /// Lets go of what the raise carries.
    pub(crate) fn release(self) {
        if let Unanswered::Raised { args, .. } = self {
            args.release();
        }
    }

    /// The same raise, what it carries held once more.
    pub(crate) fn copied(&self) -> Unanswered {
        match self {
            Unanswered::Abort(message) => Unanswered::Abort(message.clone()),
            Unanswered::Raised { effect, op, args } => Unanswered::Raised {
                effect: effect.clone(),
                op: op.clone(),
                args: args.copied(),
            },
        }
    }
}

thread_local! {
    /// The context whose entry runs on this thread, for what lets go of a carried value where no
    /// context is at hand.
    static RUNNING: std::cell::Cell<*mut Ctx> = const { std::cell::Cell::new(std::ptr::null_mut()) };
}

/// The context whose entry runs on this thread.
pub(crate) fn running() -> *mut Ctx {
    let c = RUNNING.with(std::cell::Cell::get);
    debug_assert!(
        !c.is_null(),
        "a carried value was let go of outside an entry"
    );
    c
}

/// Makes `c` the context running on this thread, answering the one that was.
pub(crate) fn swap_running(c: *mut Ctx) -> *mut Ctx {
    RUNNING.with(|r| r.replace(c))
}

/// `f` of the context whose entry runs on this thread, which a host operation that entry performs
/// reads while the entry waits on it; `None` outside an entry.
pub fn with_running<T>(f: impl FnOnce(&Ctx) -> T) -> Option<T> {
    unsafe { RUNNING.with(std::cell::Cell::get).as_ref() }.map(f)
}

impl Ctx {
    pub fn new(tables: Arc<Tables>) -> Ctx {
        let mut heap = Heap::new();
        let alloc = heap.alloc();
        Ctx {
            failed: 0,
            fuel: 0,
            stack_floor: std::sync::atomic::AtomicUsize::new(0),
            site_root: -1,
            site_start: 0,
            site_end: 0,
            ticks: 0,
            next_tick: i64::MAX,
            frames: std::ptr::null_mut(),
            release_depth: 0,
            release_waiting: 0,
            target: 0,
            entering_desc: 0,
            entering_words: 0,
            recording: 0,
            bridge: object_word(&BRIDGE),
            hosted: 0,
            unserved: object_word(&UNSERVED),
            performed_root: -1,
            performed_start: 0,
            performed_end: 0,
            task: 0,
            alloc,
            depth_bound: 0,
            grown: 0,
            deadline: None,
            time_budget_ms: 0,
            heap,
            last_entry: 0,
            tables,
            diagnostic: None,
            stacks: vec![Vec::new()],
            free_stacks: Vec::new(),
            current: 0,
            performed: Vec::new(),
            detached: Vec::new(),
            starting_detached: None,
            entry: 0,
            binding: Arc::new(ply_core::HostBinding::unbound()),
            runtime: None,
            runtime_factory: ply_host::registry::unbound(),
            re_executed: false,
            host_use: ply_core::host::HostUse::default(),
            host_ops: 0,
            last_linear: None,
            id: ply_core::host::MachineId::next(),
            pooled: false,
            teardown: Vec::new(),
            seed: ply_core::Seed::default(),
            sim_steps: ply_core::sim::DEFAULT_STEPS,
            entered_sims: 0,
            regions: Vec::new(),
            footprint: Vec::new(),
            trail_kept: None,
            held_at_root: None,
            interleaving: None,
            pending: None,
            pending_words: Vec::new(),
            traced: false,
            holds_made: 0,
            outer: (std::ptr::null_mut(), std::ptr::null()),
            release_later: Vec::new(),
            continuations_died: Vec::new(),
            crossings: Vec::new(),
            taken: Vec::new(),
            outer_running: std::ptr::null_mut(),
        }
    }

    /// Keeps `k` on the running stack while a runtime frame calls into the program; answers the
    /// stack, for [`Ctx::kept_back`].
    pub(crate) fn keep(&mut self, k: Kept) -> usize {
        let s = self.current;
        self.stacks[s].push(k);
        s
    }

    /// What the frame that kept on stack `s` kept there: the last kept there.
    pub(crate) fn kept_back(&mut self, s: usize) -> Kept {
        self.stacks[s]
            .pop()
            .expect("a runtime frame finds what it kept when its call returns")
    }

    /// A context for one branch of a `parallel` block this entry reached: its unit, host, seed and
    /// what is left of its bounds, and nothing of its own, so it can run on another thread.
    pub(crate) fn branch(&self) -> Ctx {
        let mut b = Ctx::new(Arc::clone(&self.tables));
        b.fuel = self.fuel;
        b.depth_bound = self.depth_bound;
        b.deadline = self.deadline;
        b.time_budget_ms = self.time_budget_ms;
        b.entry = self.entry;
        b.binding = Arc::clone(&self.binding);
        b.runtime_factory = Arc::clone(&self.runtime_factory);
        b.re_executed = self.re_executed;
        b.id = self.id;
        b.seed = self.seed.clone();
        b.sim_steps = self.sim_steps;
        // A branch runs in the task its block is in.
        b.task = self.task;
        // What a branch performed is this entry's, read wherever this entry's is.
        b.traced = self.recording != 0;
        b.rearm_recording();
        b
    }

    /// Arms or disarms [`Ctx::recording`] by who reads what this entry performs now: whoever runs
    /// its entries, or a seeded region's steps. A production region reads none.
    pub(crate) fn rearm_recording(&mut self) {
        let seeded = (self.regions.iter()).any(|(r, _)| unsafe { (**r).is_seeded() });
        self.recording = i64::from(self.traced || seeded);
    }

    /// Whoever runs this context's entries reads what they perform, or stops reading it.
    pub fn trace_performs(&mut self, traced: bool) {
        self.traced = traced;
        self.rearm_recording();
    }

    /// Takes back what a finished branch did, in the order the branches are written: its memory
    /// and what it performed.
    pub(crate) fn absorb(&mut self, mut branch: Ctx) {
        self.heap.adopt_heap(std::mem::take(&mut branch.heap));
        self.pooled |= branch.pooled;
        self.grown += branch.grown;
        self.performed.append(&mut branch.performed);
        self.host_use.absorb(&branch.host_use);
        self.host_ops = self.host_ops.saturating_add(branch.host_ops);
        if branch.last_linear.is_some() {
            self.last_linear = branch.last_linear.take();
        }
        self.teardown.append(&mut branch.teardown);
    }

    /// Whether a `parallel` block's branches may run on other threads: where no seeded region
    /// records their steps. A branch performs no `task` operation, and one that waits waits on its
    /// own thread, so the run's region keeps no block in turn.
    pub(crate) fn runs_branches_at_once(&self) -> bool {
        !(self.regions.iter()).any(|(r, _)| unsafe { (**r).is_seeded() })
    }

    /// Fails this entry as one of its `parallel` branches failed.
    pub(crate) fn fail_from_branch(&mut self, f: BranchFailure) {
        if self.failed == 0 {
            self.failed = f.code;
            self.pending = f.pending;
        } else if let Some(p) = f.pending {
            p.release();
        }
        if self.diagnostic.is_none() {
            self.diagnostic = f.diagnostic;
        }
    }

    /// Between calls, and only between calls, with no clock.
    pub fn begin(&mut self, fuel: i64) {
        self.begin_bounded(fuel, 0);
    }

    /// [`Ctx::begin`], the entry bounded by `time_budget_ms` of wall clock, 0 for none. The context
    /// stays where it is until [`Ctx::end`], since the watchdog writes to it.
    pub fn begin_bounded(&mut self, fuel: i64, time_budget_ms: u64) {
        self.unwatch();
        self.failed = 0;
        self.fuel = fuel;
        self.depth_bound = fuel;
        self.grown = 0;
        self.heap.set_scheduling(false);
        self.time_budget_ms = time_budget_ms;
        self.deadline = (self.time_budget_ms > 0).then(|| {
            std::time::Instant::now() + std::time::Duration::from_millis(self.time_budget_ms)
        });
        self.stack_floor.store(stack_floor(), Relaxed);
        self.frames = std::ptr::null_mut();
        self.site_root = -1;
        self.last_linear = None;
        self.diagnostic = None;
        self.stacks.clear();
        self.stacks.push(Vec::new());
        self.free_stacks.clear();
        self.current = 0;
        self.performed.clear();
        self.starting_detached = None;
        self.entered_sims = 0;
        self.regions.clear();
        self.footprint.clear();
        self.trail_kept = None;
        self.held_at_root = None;
        self.interleaving = None;
        self.holds_made = 0;
        if let Some(p) = self.pending.take() {
            p.release();
        }
        self.release_depth = 0;
        self.release_waiting = 0;
        self.target = 0;
        self.entering_desc = 0;
        self.entering_words = 0;
        self.rearm_recording();
        self.release_later.clear();
        self.crossings.clear();
        self.taken.clear();
        // Every path out of an entry calls `end`; this catches one that did not.
        if self.heap.allocated() != 0 {
            self.end();
        }
        self.detached.clear();
        self.continuations_died.clear();
        self.entry = ENTRIES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        // Another unit's entry may be running further up this thread: it gets its own back at `end`.
        self.outer = (
            heap::swap_current(&mut self.heap),
            heap::poison::swap(&raw const self.site_root),
        );
        self.outer_running = swap_running(self);
        self.watch();
    }

    /// Has the watchdog end this context's run once its deadline passes, where it has one. The
    /// context stays where it is until [`Ctx::unwatch`].
    pub(crate) fn watch(&self) {
        if let Some(deadline) = self.deadline {
            watched(&self.stack_floor, deadline);
        }
    }

    /// Takes the context off the watchdog's list, before it moves or its memory goes.
    pub(crate) fn unwatch(&self) {
        if self.deadline.is_some() {
            unwatched(&self.stack_floor);
        }
    }

    /// Fails the entry where its wall clock ran out, answering whether it did.
    pub(super) fn out_of_time(&mut self) -> bool {
        if self.deadline.is_none_or(|d| std::time::Instant::now() <= d) {
            return false;
        }
        let ms = self.time_budget_ms;
        let d = Diagnostic::warning(
            codes::RUN_ABANDONED,
            format!("abandoned after {ms} ms of wall clock"),
        )
        .primary(Span::DUMMY, "still running here")
        .note("the clock says nothing about the program, so this run decided nothing")
        .note("`--timeout MS` sets the clock; 0 is no clock");
        self.fail_with(FAILED_ABANDONED, d);
        true
    }

    /// The thread's heap, poison site and running context put back as they were when this entry
    /// began, and the watchdog told it is over: what [`Ctx::end`] does first, and all a Rust unwind
    /// out of the entry can do.
    pub(crate) fn give_thread_back(&mut self) {
        self.unwatch();
        heap::poison::swap(self.outer.1);
        heap::swap_current(self.outer.0);
        swap_running(self.outer_running);
        self.outer = (std::ptr::null_mut(), std::ptr::null());
        self.outer_running = std::ptr::null_mut();
    }

    /// How many of the program's objects are still live once what the entry's bodies hold is given
    /// back: each one a value nothing let go of.
    pub fn left_live(&mut self) -> usize {
        crate::detached::teardown(self);
        self.live()
    }

    /// The other end of [`Ctx::begin`]: the entry gives back what it used.
    pub fn end(&mut self) {
        if let Some(p) = self.pending.take() {
            p.release();
        }
        crate::detached::teardown(self);
        debug_assert!(
            self.release_later.is_empty() && self.release_depth == 0,
            "an entry ended with a release still running or waiting"
        );
        self.give_thread_back();
        self.last_entry = self.heap.allocated();
        if std::env::var("PLY_C_PHASES").is_ok() {
            eprintln!(
                "entry: {} objects allocated, {} recycled, {}MB held, {} stacks grown",
                self.heap.allocated(),
                self.heap.recycled(),
                self.held() / 1_000_000,
                self.grown
            );
            let live = self.census();
            if live.iter().map(|(_, n, _)| n).sum::<usize>() > 1000 {
                for (size, n, bytes) in live.into_iter().take(24) {
                    eprintln!(
                        "  live at end: blocks of {size} bytes past the header: {n}, {}MB",
                        bytes / 1_000_000
                    );
                }
            }
        }
        self.heap.end();
        if std::mem::take(&mut self.pooled) {
            crate::parallel::workers_ended(self.id.0);
        }
    }

    /// What `read` makes of the blocks the pool's threads hold of this entry's branches, summed.
    fn workers(&self, read: impl Fn(&crate::blocks::Blocks) -> usize + Sync) -> usize {
        if self.pooled {
            crate::parallel::worker_blocks(self.id.0, read)
                .into_iter()
                .sum()
        } else {
            0
        }
    }

    /// The bytes of the blocks the entry's live objects take, its branches' on the pool's threads
    /// among them, read from the pages without a walk.
    pub fn live_bytes(&self) -> usize {
        self.heap.live_bytes() + self.workers(|b| b.live())
    }

    /// The bytes of the memory holding the entry's objects, whatever is live in it.
    pub fn held(&self) -> usize {
        self.heap.held() + self.workers(|b| b.held())
    }

    /// How many of the blocks the program made are live, its branches' on the pool's threads among
    /// them: the scheduler's own are not counted.
    pub fn live(&self) -> usize {
        let branches: usize = if self.pooled {
            crate::parallel::worker_blocks(self.id.0, heap::program_live_in)
                .into_iter()
                .sum()
        } else {
            0
        };
        self.heap.program_live() + branches
    }

    /// The entry's live blocks by the payload bytes they hold, the most bytes first, with their count
    /// and payload bytes, its branches' on the pool's threads among them: a block's header names no
    /// type.
    pub fn census(&self) -> Vec<(usize, usize, usize)> {
        let mut rows = self.heap.census();
        if self.pooled {
            for tally in crate::parallel::worker_blocks(self.id.0, heap::census_in) {
                rows = heap::census_merged(rows, tally);
            }
        }
        rows
    }

    pub fn set_host(
        &mut self,
        binding: Arc<ply_core::HostBinding>,
        runtime: Option<Rc<dyn ply_core::HostRuntime>>,
        factory: Option<ply_core::RuntimeFactory>,
    ) {
        self.binding = binding;
        self.runtime = runtime;
        if let Some(factory) = factory {
            self.runtime_factory = factory;
        }
    }

    /// The reactor this context waits on, made on first need on a `parallel` branch's thread.
    pub(crate) fn host_runtime(&mut self) -> Rc<dyn ply_core::HostRuntime> {
        if let Some(runtime) = &self.runtime {
            return Rc::clone(runtime);
        }
        let runtime = (self.runtime_factory)();
        self.runtime = Some(Rc::clone(&runtime));
        runtime
    }

    /// A new stack, keeping nothing yet; its index names it.
    pub(crate) fn open_stack(&mut self) -> usize {
        match self.free_stacks.pop() {
            Some(stack) => stack,
            None => {
                self.stacks.push(Vec::new());
                self.stacks.len() - 1
            }
        }
    }

    /// Gives back a dead stack's index; nothing may name it.
    pub(crate) fn close_stack(&mut self, stack: usize) {
        for k in std::mem::take(&mut self.stacks[stack]) {
            k.release();
        }
        self.free_stacks.push(stack);
    }

    /// The stacks' tables of what they keep this entry holds, given-back ones included: what a
    /// leak grows.
    pub fn stack_slots(&self) -> usize {
        self.stacks.len()
    }

    pub(crate) fn fail(&mut self, d: Diagnostic) -> i64 {
        self.fail_with(FAILED, d)
    }

    /// Fails as a raise of `abort.raise` of `message` the runtime makes, left pending for the code
    /// around the failure, whose innermost `handle` answering it takes it: an instance's landing,
    /// or a function holding a word for `abort`, which raises it through that word as it leaves.
    /// `d` is what the entry fails with should none take it.
    pub(crate) fn pend(&mut self, d: Diagnostic, message: String) -> i64 {
        if self.failed != 0 {
            return 0;
        }
        self.pending = Some(Unanswered::Abort(message));
        self.fail_with(FAILED_PENDING, d)
    }

    /// Fails as the raise of `effect.op` carrying `args`, which no clause answers in place, left
    /// pending as [`Ctx::pend`] leaves `abort.raise`. Takes `args`.
    pub(crate) fn pend_raised(
        &mut self,
        d: Diagnostic,
        effect: Symbol,
        op: Symbol,
        args: Raised,
    ) -> i64 {
        if self.failed != 0 {
            args.release();
            return 0;
        }
        self.pending = Some(Unanswered::Raised { effect, op, args });
        self.fail_with(FAILED_PENDING, d)
    }

    /// How a finished branch failed, a raise bound past it as the failure it carries; one pending
    /// is carried on pending.
    pub(crate) fn take_branch_failure(&mut self) -> Option<BranchFailure> {
        if self.failed == 0 {
            return None;
        }
        Some(BranchFailure {
            code: self.failed,
            diagnostic: self.diagnostic.take(),
            pending: self.pending.take(),
        })
    }

    pub(super) fn fail_with(&mut self, code: i64, d: Diagnostic) -> i64 {
        if self.failed == 0 {
            self.failed = code;
        }
        if self.diagnostic.is_none() {
            self.diagnostic = Some(self.placed(d));
        }
        0
    }

    /// The span the body last stored, or `Span::DUMMY` before any has.
    pub fn site(&self) -> Span {
        self.span_at(self.site_root, self.site_start, self.site_end)
    }

    /// Puts `touched` into the running seeded step of the innermost region `std.internal.schedule`
    /// opened, made where the body last stored a site; nothing where no such step runs, and nothing
    /// of a `task` or `clock` operation.
    pub(crate) fn touched(&mut self, touched: crate::tasks::Touched) {
        // The scheduler orders what its own operations touch itself.
        if let crate::tasks::Touched::Atom { effect, .. } = &touched
            && matches!(effect.as_str(), "task" | "clock")
        {
            return;
        }
        let stepping = (self.regions.last()).is_some_and(|(r, _)| unsafe { (**r).is_stepping() });
        if stepping {
            self.footprint
                .push((touched, [self.site_root, self.site_start, self.site_end]));
        }
    }

    /// The name of the definition whose root's id is `root`; none for an id no root has.
    pub(crate) fn root_name(&self, root: i64) -> Option<&Symbol> {
        self.root_of(root).map(|r| &r.name)
    }

    /// The root whose id is `root`; none for an id below 0 or that no root has.
    fn root_of(&self, root: i64) -> Option<&Root> {
        let id = u64::try_from(root).ok()?;
        let roots = &self.tables.roots;
        let at = roots.binary_search_by_key(&id, |r| r.id).ok()?;
        Some(&roots[at])
    }

    /// The span a site names: offsets `start..end` from the start of root `root`'s definition, as
    /// the body stores one; `Span::DUMMY` where no root has that id or its offsets leave the source.
    pub(crate) fn span_at(&self, root: i64, start: i64, end: i64) -> Span {
        let Some(defined) = self.root_of(root).map(|r| r.span).filter(|s| !s.is_dummy()) else {
            return Span::DUMMY;
        };
        let at = |offset: i64| {
            i64::from(defined.start)
                .checked_add(offset)
                .and_then(|o| u32::try_from(o).ok())
        };
        match (at(start), at(end)) {
            (Some(start), Some(end)) => Span::new(defined.source, start, end),
            _ => Span::DUMMY,
        }
    }

    /// `d` anchored at the body's stored site when it names no place of its own.
    pub(super) fn placed(&self, mut d: Diagnostic) -> Diagnostic {
        let site = self.site();
        if site == Span::DUMMY {
            return d;
        }
        let at = d
            .labels
            .iter()
            .position(|l| l.primary)
            .or_else(|| (!d.labels.is_empty()).then_some(0));
        match at {
            Some(i) if d.labels[i].span == Span::DUMMY => d.labels[i].span = site,
            Some(_) => {}
            None => d = d.primary(site, "here"),
        }
        d
    }

    pub fn take_failure(&mut self) -> Option<Diagnostic> {
        // A raise left pending is over once its failure is taken: what it carries goes back, and
        // the host hears of one nothing answered, which may end the run its way.
        match self.pending.take() {
            Some(Unanswered::Raised { effect, op, args }) => {
                self.host_runtime().unanswered(&effect, &op);
                args.release();
            }
            Some(Unanswered::Abort(_)) | None => {}
        }
        self.diagnostic.take()
    }

    /// What the run asked of the host since the last take, and how many of those asks were
    /// at-most-once operations.
    pub fn take_host_use(&mut self) -> (ply_core::host::HostUse, u64) {
        (
            std::mem::take(&mut self.host_use),
            std::mem::take(&mut self.host_ops),
        )
    }
}

/// A runtime failure inside compiled code.
pub(super) fn error(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::RUNTIME_ERROR, message.into()).primary(Span::DUMMY, "in compiled code")
}

/// The `n` words at `ptr`, which may be null where `n` is 0.
pub(super) fn args_of<'a>(ptr: *const i64, n: i64) -> &'a [Word] {
    if n == 0 {
        return &[];
    }
    unsafe { std::slice::from_raw_parts(ptr, n as usize) }
}
