//! The context an entry runs in.

use super::*;

/// Entries begun in this process, every context's: contexts share a unit's memo, so an entry's
/// number must be unique across them, not just within one.
pub(super) static ENTRIES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[repr(C)]
pub struct Ctx {
    pub failed: i64,
    // The runtime's own, in a word between the fields emitted code reads.
    /// The running entry's number, which no other entry in the process shares: a continuation
    /// carries it, since its index names a body of that entry alone.
    pub(crate) entry: u64,
    /// The lowest address a compiled frame may begin at. The watchdog raises it past every frame
    /// once the entry's clock runs out ([`watched`]).
    pub stack_floor: std::sync::atomic::AtomicUsize,
    /// The site the program's own code last stored, on the branch of an operation that fails or
    /// before a call of code that stores none, as bytes from its root's start, or on the way into
    /// [`rt_stack_more`], with no extent, naming its whole definition; the root is `-1` until one
    /// is.
    pub site_root: i64,
    pub site_start: i64,
    pub site_end: i64,
    // The runtime's own two, in the words between the fields emitted code reads.
    /// The wall clock the running entry may take, in milliseconds; 0 is none.
    pub(super) time_budget_ms: u64,
    /// Chunks of stack this entry's calls made writable past what their stacks started with, so
    /// growing is observable.
    pub grown: u64,
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
    /// The effects descriptor and words the next closure the runtime calls is handed by its uniform
    /// entry, which takes them and sets both to 0; 0 for none, which hands it the bridge.
    pub entering_desc: Word,
    pub entering_words: Word,
    /// Whether a perform a clause handles in place is recorded through [`rt_performed`]: while a
    /// test reads what its entries performed or a seeded scheduler runs.
    pub recording: i64,
    /// The evidence object of every effect for code that holds no word handling one
    /// ([`BRIDGE`]).
    pub bridge: Word,
    /// The pages compiled code takes a small block from and gives one back to without calling the
    /// runtime ([`Heap::filling`]): set again whenever that changes ([`Ctx::filling_changed`]).
    pub(crate) filling: *mut crate::blocks::Filling,
    /// What an entry hands its root for an effect nothing handles ([`UNSERVED`]).
    pub unserved: Word,
    /// Where the perform a clause handles stood, as the site fields hold one, which compiled code
    /// stores as it enters the clause: what a scheduler's request is placed at.
    pub performed_root: i64,
    pub performed_start: i64,
    pub performed_end: i64,
    /// The task running, as `abi.ply`'s `ctx_struct` says: what an instance records as it opens,
    /// which only that task may leave.
    pub task: i64,
    /// What the release's compiled code reaches [`Ctx::filling`] through: the heap's own, wherever
    /// the heap moves.
    pub alloc: *mut crate::heap::Alloc,
    /// The unit's kept constants ([`Tables::memo`]), which compiled code reads without calling the
    /// runtime.
    pub constants: *const AtomicI64,
    /// When the running entry's time budget is spent, if it has one.
    pub(super) deadline: Option<std::time::Instant>,
    pub heap: Heap,
    pub tables: Arc<Tables>,
    /// Why the last entry failed.
    pub diagnostic: Option<Diagnostic>,
    /// What the runtime's frames on each stack run in this entry keep ([`Kept`]), the innermost
    /// last, the entry's own stack first; `current` is the one running.
    pub(crate) stacks: Vec<Vec<Kept>>,
    /// Indices of `stacks` a finished production task gave back, which nothing names any longer.
    pub(super) free_stacks: Vec<usize>,
    pub(crate) current: usize,
    /// Every effect a compiled `perform` performed since the entry began, for the machine's trace.
    pub performed: Vec<Effect>,
    /// The detached bodies this entry has opened, named by index from their instances and tokens.
    pub(crate) detached: Vec<crate::detached::Detached>,
    pub(crate) starting_detached: Option<usize>,
    /// The host boundary: what a `perform` nothing on the stack handles reaches.
    pub(crate) binding: Arc<ply_core::HostBinding>,
    pub(crate) runtime: Option<Rc<dyn ply_core::HostRuntime>>,
    /// What makes a reactor, which a `parallel` branch on another thread needs one of its own of:
    /// the bound run's, or one over a host of this context's own.
    pub(crate) runtime_factory: ply_core::RuntimeFactory,
    pub(crate) re_executed: bool,
    pub(crate) host_use: ply_core::host::HostUse,
    pub(crate) id: ply_core::host::MachineId,
    /// Whether the entry's branches ran on the pool's threads, whose blocks then hold its objects too.
    pub(crate) pooled: bool,
    /// What the host runtime said when an entry ended, for the machine's teardown warnings.
    pub(crate) teardown: Vec<Diagnostic>,
    /// The seed the driver set, and the step budget, for the schedulers this entry opens.
    pub(crate) seed: ply_core::Seed,
    pub(crate) sim_steps: u32,
    pub(crate) entered_sims: u32,
    /// Schedulers `std.internal.schedule` opened on this context, innermost last, which a bracket
    /// shields its running task in and a seeded step records what it touched for.
    pub(crate) schedulers: Vec<(*mut crate::tasks::Scheduler, [i64; 3])>,
    /// What the running seeded step touched, each beside where it was made.
    pub accesses: Vec<(crate::tasks::Touched, [i64; 3])>,
    /// The trail the entry's last seeded scheduler left, and its schedulers' interleaving, each as
    /// the scheduler wrote it.
    pub(crate) last_trail: Option<Vec<u8>>,
    /// What the run held as its root's body returned, where the run's scheduler noted it.
    pub(crate) held_at_root: Option<crate::entry::Held>,
    pub interleaving: Option<Vec<u8>>,
    /// The raise [`FAILED_PENDING`] stands for; its diagnostic is `diagnostic`.
    pub(crate) pending: Option<Unhandled>,
    /// The words of the raise `rt_pending_raised_at` last handed compiled code, with a word to
    /// spare past them, as a perform's arguments have.
    pub(crate) pending_words: Vec<Word>,
    /// Whether whoever runs this context's entries reads what they performed.
    pub(crate) traced: bool,
    /// How many scoped values this entry has made, which numbers the next.
    pub(super) scoped_made: u64,
    /// The heap and poison site of the entry this one began inside, put back when it ends.
    pub(super) outer: (*mut Heap, *const i64),
    /// Releases a drop root at [`RELEASE_DEPTH`] put off: each word with the root that releases it.
    pub(crate) release_later: Vec<(crate::emitted::Handed, crate::emitted::DropRoot)>,
    /// Stops whose continuation's last holder let it go, by body and capture, for the next drain to
    /// drop: never inside the release that let it go.
    pub(crate) continuations_died: Vec<(usize, usize)>,
    /// The marshallings under way, the innermost last.
    pub(crate) marshallings: Vec<Marshalling>,
    /// The bytes a native returned, until compiled code copies them out with [`rt_taken`].
    pub(crate) taken: Vec<u8>,
    /// The context running on this thread when this one's entry began, put back when it ends.
    pub(super) outer_running: *mut Ctx,
}

/// How a `parallel` branch failed, as its parent fails in turn.
pub(crate) struct BranchFailure {
    code: i64,
    diagnostic: Option<Diagnostic>,
    pending: Option<Unhandled>,
}

/// A raise no clause handles in place, left for the code around the failure, whose innermost
/// `handle` handling it takes it: `abort.raise` of a message, or another raise with what it
/// carries.
pub(crate) enum Unhandled {
    Abort(String),
    Raised {
        effect: Symbol,
        op: Symbol,
        args: Raised,
    },
}

impl Unhandled {
    /// Lets go of what the raise carries.
    pub(crate) fn release(self) {
        if let Unhandled::Raised { args, .. } = self {
            args.release();
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

/// Makes `c` the context running on this thread, returning the one that was.
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
        let filling = heap.filling();
        let constants = tables.memo.as_ptr();
        Ctx {
            failed: 0,
            stack_floor: std::sync::atomic::AtomicUsize::new(0),
            site_root: -1,
            site_start: 0,
            site_end: 0,
            frames: std::ptr::null_mut(),
            release_depth: 0,
            release_waiting: 0,
            target: 0,
            entering_desc: 0,
            entering_words: 0,
            recording: 0,
            bridge: object_word(&BRIDGE),
            filling,
            unserved: object_word(&UNSERVED),
            performed_root: -1,
            performed_start: 0,
            performed_end: 0,
            task: 0,
            alloc,
            constants,
            grown: 0,
            deadline: None,
            time_budget_ms: 0,
            heap,
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
            id: ply_core::host::MachineId::next(),
            pooled: false,
            teardown: Vec::new(),
            seed: ply_core::Seed::default(),
            sim_steps: ply_core::sim::DEFAULT_STEPS,
            entered_sims: 0,
            schedulers: Vec::new(),
            accesses: Vec::new(),
            last_trail: None,
            held_at_root: None,
            interleaving: None,
            pending: None,
            pending_words: Vec::new(),
            traced: false,
            scoped_made: 0,
            outer: (std::ptr::null_mut(), std::ptr::null()),
            release_later: Vec::new(),
            continuations_died: Vec::new(),
            marshallings: Vec::new(),
            taken: Vec::new(),
            outer_running: std::ptr::null_mut(),
        }
    }

    /// Keeps `k` on the running stack while a runtime frame calls into the program; returns the
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
    /// its entries, or a seeded scheduler's steps. A production scheduler reads none.
    pub(crate) fn rearm_recording(&mut self) {
        let seeded = (self.schedulers.iter()).any(|(r, _)| unsafe { (**r).is_seeded() });
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
        self.teardown.append(&mut branch.teardown);
    }

    /// Whether a `parallel` block's branches may run on other threads: where no seeded scheduler
    /// records their steps. A branch performs no `task` operation, and one that waits waits on its
    /// own thread, so the run's scheduler keeps no block in turn.
    pub(crate) fn runs_branches_at_once(&self) -> bool {
        !(self.schedulers.iter()).any(|(r, _)| unsafe { (**r).is_seeded() })
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
    pub fn begin(&mut self) {
        self.begin_bounded(0);
    }

    /// [`Ctx::begin`], the entry bounded by `time_budget_ms` of wall clock, 0 for none. The context
    /// stays where it is until [`Ctx::end`], since the watchdog writes to it.
    pub fn begin_bounded(&mut self, time_budget_ms: u64) {
        self.unwatch();
        self.failed = 0;
        self.grown = 0;
        self.program_entered();
        self.time_budget_ms = time_budget_ms;
        self.deadline = (self.time_budget_ms > 0).then(|| {
            std::time::Instant::now() + std::time::Duration::from_millis(self.time_budget_ms)
        });
        self.stack_floor.store(stack_floor(), Relaxed);
        self.frames = std::ptr::null_mut();
        self.site_root = -1;
        self.diagnostic = None;
        self.stacks.clear();
        self.stacks.push(Vec::new());
        self.free_stacks.clear();
        self.current = 0;
        self.performed.clear();
        self.starting_detached = None;
        self.entered_sims = 0;
        self.schedulers.clear();
        self.accesses.clear();
        self.last_trail = None;
        self.held_at_root = None;
        self.interleaving = None;
        self.scoped_made = 0;
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
        self.marshallings.clear();
        self.taken.clear();
        // Every path out of an entry calls `end`; this catches one that did not.
        if self.heap.held() != 0 {
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

    /// Fails the entry where its wall clock ran out, returning whether it did.
    pub(super) fn out_of_time(&mut self) -> bool {
        if self.deadline.is_none_or(|d| std::time::Instant::now() <= d) {
            return false;
        }
        let ms = self.time_budget_ms;
        let d = Diagnostic::warning(
            codes::RUN_TIMED_OUT,
            format!("timed out after {ms} ms of wall clock"),
        )
        .primary(Span::DUMMY, "still running here")
        .note("the clock says nothing about the program, so this run decided nothing")
        .note("`--timeout MS` sets the clock; 0 is no clock");
        self.fail_with(FAILED_TIMED_OUT, d);
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
    /// around the failure, whose innermost `handle` handling it takes it: an instance's landing,
    /// or a function holding a word for `abort`, which raises it through that word as it leaves.
    /// `d` is what the entry fails with should none take it.
    pub(crate) fn pend(&mut self, d: Diagnostic, message: String) -> i64 {
        if self.failed != 0 {
            return 0;
        }
        self.pending = Some(Unhandled::Abort(message));
        self.fail_with(FAILED_PENDING, d)
    }

    /// Fails as the raise of `effect.op` carrying `args`, which no clause handles in place, left
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
        self.pending = Some(Unhandled::Raised { effect, op, args });
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

    /// Puts `touched` in the running seeded step of the innermost scheduler `std.internal.schedule`
    /// opened, made where the body last stored a site; nothing where no such step runs, and nothing
    /// of a `task` or `clock` operation.
    pub(crate) fn touched(&mut self, touched: crate::tasks::Touched) {
        // The scheduler orders what its own operations touch itself.
        if let crate::tasks::Touched::Effect { effect, .. } = &touched
            && matches!(effect.as_str(), "task" | "clock")
        {
            return;
        }
        let stepping =
            (self.schedulers.last()).is_some_and(|(r, _)| unsafe { (**r).is_stepping() });
        if stepping {
            self.accesses
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
        // the host hears of one nothing handled, which may end the run its way.
        match self.pending.take() {
            Some(Unhandled::Raised { effect, op, args }) => {
                self.host_runtime().unhandled(&effect, &op);
                args.release();
            }
            Some(Unhandled::Abort(_)) | None => {}
        }
        self.diagnostic.take()
    }

    /// What the run asked of the host since the last take.
    pub fn take_host_use(&mut self) -> ply_core::host::HostUse {
        std::mem::take(&mut self.host_use)
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
