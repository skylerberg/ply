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
    /// past a failure first puts this where it stood ([`HandlerFrame::land`]). It is the running
    /// stack's own: a switch saves it with the stack it leaves and puts back the other's.
    pub fuel: i64,
    /// The lowest address a compiled frame may begin at: C frames can overflow within the fuel.
    pub stack_floor: usize,
    /// The site the body stored before a fallible call, as bytes from its root's start; the root
    /// is `-1` until one is.
    pub site_root: i64,
    pub site_start: i64,
    pub site_end: i64,
    /// Calls this entry has made: the prologue counts one, and so does every pass of a loop.
    pub ticks: i64,
    /// The tick at which compiled code calls [`rt_tick`] back; `i64::MAX` when neither the budget
    /// nor the clock bounds this entry, so nothing calls back at all.
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
    /// entry, which takes them and sets both to 0; 0 for none, which hands it the frame bridge.
    pub entering_desc: Word,
    pub entering_words: Word,
    /// Whether a perform a clause answers in place is recorded through [`rt_performed`]: while a
    /// test reads what its entries performed, a cost mark is open or a region runs.
    pub recording: i64,
    /// The evidence object of every effect that hands each operation to the frame protocol
    /// ([`BRIDGE`]), for code that holds no word answering one.
    pub bridge: Word,
    /// The calls this entry may make; 0 is no bound.
    pub(super) step_budget: i64,
    /// Calls and allocations a memoized answer cost when it was computed, charged to the entry that
    /// read it, so `metered` counts the same whether a constant was memoized or not. Not budgeted.
    pub charged: Cost,
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
    /// One per stack run in this entry, the entry's own first; `current` is the one running.
    pub(crate) stacks: Vec<Frames>,
    /// Indices of `stacks` a finished production task gave back, which nothing names any longer.
    pub(super) free_stacks: Vec<usize>,
    pub(crate) current: usize,
    /// Every atom a compiled `perform` performed since the entry began, for the machine's trace.
    pub performed: Vec<EffectAtom>,
    /// The marks [`rt_cost_mark`] took that no [`rt_cost_spent`] has read back yet, the latest last.
    pub(crate) cost_marks: Vec<CostMark>,
    /// The regions live in this entry, innermost last; the checker forbids nesting, so at most one.
    pub sims: Vec<crate::simulate::Simulation>,
    /// The detached bodies this entry has opened, named by index from their frames and tokens.
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
    pub(crate) declared: Option<ply_core::Footprint>,
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
    pub(crate) trail: ply_core::region::Trail,
    /// What the entry's regions did, for the search, once the last one has completed.
    pub record: Option<ply_core::region::Record>,
    pub(crate) entered_sims: u32,
    /// Where an unwind is going and what it carries: the frame's depth and the clause's value.
    pub(crate) unwind: Option<(usize, usize, Word)>,
    pub(crate) aborting: Option<Aborting>,
    /// The message of the raise [`FAILED_PENDING`] stands for; its diagnostic is `diagnostic`.
    pub(crate) pending: Option<String>,
    /// Whether whoever runs this context's entries reads what they performed.
    pub(crate) traced: bool,
    /// The value a clause handed to `resume` in tail position, read back when the clause returns.
    pub(super) resumed: Option<Word>,
    /// How many `handle`s this context has installed, which names the next one's frame.
    pub(super) handlers_made: u64,
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
    pending: Option<String>,
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
        Ctx {
            failed: 0,
            fuel: 0,
            stack_floor: 0,
            site_root: -1,
            site_start: 0,
            site_end: 0,
            ticks: 0,
            charged: Cost::default(),
            next_tick: i64::MAX,
            frames: std::ptr::null_mut(),
            release_depth: 0,
            release_waiting: 0,
            target: 0,
            entering_desc: 0,
            entering_words: 0,
            recording: 0,
            bridge: bridge_word(),
            step_budget: 0,
            grown: 0,
            deadline: None,
            time_budget_ms: 0,
            heap: Heap::new(),
            last_entry: 0,
            tables,
            diagnostic: None,
            stacks: vec![Frames::under(None)],
            free_stacks: Vec::new(),
            current: 0,
            performed: Vec::new(),
            cost_marks: Vec::new(),
            sims: Vec::new(),
            detached: Vec::new(),
            starting_detached: None,
            entry: 0,
            binding: Arc::new(ply_core::HostBinding::unbound()),
            runtime: None,
            runtime_factory: ply_host::registry::unbound(),
            declared: None,
            re_executed: false,
            host_use: ply_core::host::HostUse::default(),
            host_ops: 0,
            last_linear: None,
            id: ply_core::host::MachineId::next(),
            pooled: false,
            teardown: Vec::new(),
            seed: ply_core::Seed::default(),
            sim_steps: ply_core::sim::DEFAULT_STEPS,
            trail: ply_core::region::Trail::new(ply_core::Seed::default()),
            record: None,
            entered_sims: 0,
            unwind: None,
            aborting: None,
            pending: None,
            traced: false,
            resumed: None,
            handlers_made: 0,
            holds_made: 0,
            outer: (std::ptr::null_mut(), std::ptr::null()),
            release_later: Vec::new(),
            continuations_died: Vec::new(),
            crossings: Vec::new(),
            taken: Vec::new(),
            outer_running: std::ptr::null_mut(),
        }
    }

    /// A name for the `handle` being installed, which no other in this context shares.
    pub(crate) fn next_handler(&mut self) -> u64 {
        self.handlers_made += 1;
        self.handlers_made
    }

    /// `call`, made while the handler's variables `held` are the clause's: a clause or the `return`
    /// clause of a handler that has some. Takes `held`.
    pub(crate) fn holding<T>(
        ctx: *mut Ctx,
        held: Option<(u64, SharedVars)>,
        call: impl FnOnce() -> T,
    ) -> T {
        let Some(h) = held else {
            return call();
        };
        let stack = unsafe { &*ctx }.current;
        unsafe { &mut *ctx }.stacks[stack].held.push(h);
        let r = call();
        if let Some((_, v)) = unsafe { &mut *ctx }.stacks[stack].held.pop() {
            release_vars(v);
        }
        r
    }

    /// The variables of the running clause of `handler`: on this stack, or on one this stack was
    /// entered from, as a body or a task opened inside the clause runs.
    pub(crate) fn held_by(&self, handler: u64) -> Option<SharedVars> {
        let mut at = Some(self.current);
        for _ in 0..self.stacks.len() {
            let frames = &self.stacks[at?];
            if let Some((_, v)) = frames.held.iter().rev().find(|(h, _)| *h == handler) {
                return Some(Rc::clone(v));
            }
            at = frames.entered_from.or(frames.parent);
        }
        None
    }

    /// Keeps `k` on the running stack while a runtime frame calls into the program; answers the
    /// stack, for [`Ctx::kept_back`].
    pub(crate) fn keep(&mut self, k: Kept) -> usize {
        let s = self.current;
        self.stacks[s].kept.push(k);
        s
    }

    /// What the frame that kept on stack `s` kept there: the last kept there.
    pub(crate) fn kept_back(&mut self, s: usize) -> Kept {
        self.stacks[s]
            .kept
            .pop()
            .expect("a runtime frame finds what it kept when its call returns")
    }

    /// A context for one branch of a `parallel` block this entry reached: its unit, host, seed and
    /// what is left of its bounds, and nothing of its own, so it can run on another thread.
    pub(crate) fn branch(&self) -> Ctx {
        let mut b = Ctx::new(Arc::clone(&self.tables));
        b.fuel = self.fuel;
        // Zero is no bound, so a budget spent to the last call stays a bound of one.
        b.step_budget = if self.step_budget > 0 {
            (self.step_budget - self.ticks).max(1)
        } else {
            0
        };
        b.deadline = self.deadline;
        b.time_budget_ms = self.time_budget_ms;
        b.arm_tick();
        b.entry = self.entry;
        b.binding = Arc::clone(&self.binding);
        b.runtime_factory = Arc::clone(&self.runtime_factory);
        b.declared = self.declared.clone();
        b.re_executed = self.re_executed;
        b.id = self.id;
        b.seed = self.seed.clone();
        b.sim_steps = self.sim_steps;
        b.stacks[0].list = self.crossing_frames();
        // What a branch performed is this entry's, read wherever this entry's is.
        b.traced = self.recording != 0;
        b.rearm_recording();
        b
    }

    /// Arms or disarms [`Ctx::recording`] by who reads what this entry performs now.
    pub(crate) fn rearm_recording(&mut self) {
        self.recording =
            i64::from(self.traced || !self.cost_marks.is_empty() || !self.sims.is_empty());
    }

    /// Whoever runs this context's entries reads what they perform, or stops reading it.
    pub fn trace_performs(&mut self, traced: bool) {
        self.traced = traced;
        self.rearm_recording();
    }

    /// The frames in reach of the running stack, outermost first, each with whether its clauses
    /// may answer from another thread at all: only where every frame below it crosses whole, since
    /// what a clause performs and raises is answered below it.
    fn frames_in_reach(&self) -> Vec<(&HandlerFrame, bool)> {
        let mut chain = Vec::new();
        let mut stack = Some(self.current);
        while let Some(s) = stack {
            chain.push(s);
            stack = self.stacks[s].parent;
        }
        let mut below_crosses = true;
        let mut frames = Vec::new();
        for s in chain.into_iter().rev() {
            for f in &self.stacks[s].list {
                frames.push((f, below_crosses));
                below_crosses &= f.crosses_whole();
            }
        }
        frames
    }

    /// Copies of what the frames in reach of the running stack answer from another thread,
    /// outermost first, as a branch's own stack holds them.
    fn crossing_frames(&self) -> Vec<HandlerFrame> {
        self.frames_in_reach()
            .into_iter()
            .filter(|(_, crossing)| *crossing)
            .filter_map(|(f, _)| f.crossing_copy())
            .collect()
    }

    /// Takes back what a finished branch did, in the order the branches are written: its memory,
    /// its calls, and what it performed.
    pub(crate) fn absorb(&mut self, mut branch: Ctx) {
        self.heap.adopt_heap(std::mem::take(&mut branch.heap));
        self.pooled |= branch.pooled;
        self.ticks = self.ticks.saturating_add(branch.ticks);
        self.charged = self.charged.plus(branch.charged);
        self.grown += branch.grown;
        self.performed.append(&mut branch.performed);
        self.host_use.absorb(&branch.host_use);
        self.host_ops = self.host_ops.saturating_add(branch.host_ops);
        if branch.last_linear.is_some() {
            self.last_linear = branch.last_linear.take();
        }
        self.teardown.append(&mut branch.teardown);
    }

    /// Whether a `parallel` block's branches may run on other threads: nothing they perform can then
    /// reach a handler this entry holds, a region's scheduler or this thread's host runtime, all of
    /// which live on this thread. `reach` is every operation the branches may perform that they do
    /// not answer themselves, where the block's site knows it; a block that does not is kept from any
    /// handler at all.
    pub(crate) fn runs_branches_at_once(&self, reach: Option<&[Reached]>) -> bool {
        if !self.sims.is_empty() {
            return false;
        }
        self.frames_in_reach()
            .into_iter()
            .all(|(f, crossing)| match reach {
                Some(reach) => !f.within(reach, crossing),
                None => f.crosses_whole(),
            })
    }

    /// The calls left before this entry's budget is spent, or `None` when nothing bounds it.
    pub(crate) fn steps_left(&self) -> Option<i64> {
        (self.step_budget > 0).then(|| self.step_budget - self.ticks)
    }

    /// Fails this entry as one of its `parallel` branches failed.
    pub(crate) fn fail_from_branch(&mut self, f: BranchFailure) {
        if self.failed == 0 {
            self.failed = f.code;
            self.pending = f.pending;
        }
        if self.diagnostic.is_none() {
            self.diagnostic = f.diagnostic;
        }
    }

    /// Fails this entry as running out of its step budget does.
    pub(crate) fn fail_out_of_steps(&mut self) {
        self.ticks = self.step_budget.saturating_add(1);
        self.tick();
    }

    /// Between calls, and only between calls, bounded as this thread's entries are.
    pub fn begin(&mut self, fuel: i64) {
        self.begin_bounded(fuel, step_budget(), time_budget_ms());
    }

    /// [`Ctx::begin`], the entry bounded by `steps` calls and `time_budget_ms` of wall clock, 0
    /// for no bound.
    pub fn begin_bounded(&mut self, fuel: i64, steps: i64, time_budget_ms: u64) {
        self.failed = 0;
        self.fuel = fuel;
        self.ticks = 0;
        self.charged = Cost::default();
        self.step_budget = steps.max(0);
        self.grown = 0;
        self.time_budget_ms = time_budget_ms;
        self.deadline = (self.time_budget_ms > 0).then(|| {
            std::time::Instant::now() + std::time::Duration::from_millis(self.time_budget_ms)
        });
        self.arm_tick();
        self.stack_floor = stack_floor();
        self.frames = std::ptr::null_mut();
        self.site_root = -1;
        self.last_linear = None;
        self.diagnostic = None;
        self.stacks.clear();
        self.stacks.push(Frames::under(None));
        self.free_stacks.clear();
        self.current = 0;
        self.performed.clear();
        self.cost_marks.clear();
        self.sims.clear();
        self.starting_detached = None;
        self.trail = ply_core::region::Trail::new(self.seed.clone());
        self.record = None;
        self.entered_sims = 0;
        self.handlers_made = 0;
        self.holds_made = 0;
        self.unwind = None;
        self.aborting = None;
        self.pending = None;
        self.resumed = None;
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
    }

    /// When compiled code must call back next: the call one past the budget, the end of this
    /// chunk, or never, when the entry is bounded by neither the budget nor a clock.
    pub(super) fn arm_tick(&mut self) {
        let chunk = self.ticks.saturating_add(TICK_CHUNK);
        self.next_tick = match (self.step_budget > 0, self.deadline.is_some()) {
            (true, _) => self.step_budget.saturating_add(1).min(chunk),
            (false, true) => chunk,
            (false, false) => i64::MAX,
        };
    }

    /// The call counter reached [`Ctx::next_tick`]: charge the budget, read the clock, and say
    /// when to call back. Both bounds are decided here, so a call itself costs one increment and
    /// one compare, and the clock is read once every [`TICK_CHUNK`] calls rather than on any.
    pub(super) fn tick(&mut self) {
        if self.step_budget > 0 && self.ticks > self.step_budget {
            let budget = self.step_budget;
            let d = Diagnostic::error(
                codes::STEP_BUDGET,
                format!("did not finish within its budget of {budget} calls"),
            )
            .primary(Span::DUMMY, "still running here")
            .note("`--steps N` raises the budget; 0 is no bound");
            self.fail_with(FAILED_OUT_OF_STEPS, d);
            return;
        }
        if let Some(deadline) = self.deadline
            && std::time::Instant::now() > deadline
        {
            let ms = self.time_budget_ms;
            let d = Diagnostic::warning(
                codes::RUN_ABANDONED,
                format!("abandoned after {ms} ms of wall clock"),
            )
            .primary(Span::DUMMY, "still running here")
            .note("the clock says nothing about the program, so this run decided nothing")
            .note("`--timeout MS` sets the clock; 0 is no clock");
            self.fail_with(FAILED_ABANDONED, d);
            return;
        }
        self.arm_tick();
    }

    /// The thread's heap, poison site and running context put back as they were when this entry
    /// began: what [`Ctx::end`] does first, and all a Rust unwind out of the entry can do.
    pub(crate) fn give_thread_back(&mut self) {
        heap::poison::swap(self.outer.1);
        heap::swap_current(self.outer.0);
        swap_running(self.outer_running);
        self.outer = (std::ptr::null_mut(), std::ptr::null());
        self.outer_running = std::ptr::null_mut();
    }

    /// The other end of [`Ctx::begin`]: the entry gives back what it used.
    pub fn end(&mut self) {
        // A region left open lets go of the values its tasks passed while the entry's heap lives.
        drop(std::mem::take(&mut self.sims));
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

    /// How many of the entry's blocks are live, its branches' on the pool's threads among them.
    pub fn live(&self) -> usize {
        self.census().iter().map(|(_, n, _)| n).sum()
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

    pub(crate) fn frames(&mut self) -> &mut Vec<HandlerFrame> {
        &mut self.stacks[self.current].list
    }

    /// A new stack's frames, chained under `parent`; its index names it.
    pub(crate) fn open_stack(&mut self, parent: Option<usize>) -> usize {
        match self.free_stacks.pop() {
            Some(stack) => {
                self.stacks[stack] = Frames::under(parent);
                stack
            }
            None => {
                self.stacks.push(Frames::under(parent));
                self.stacks.len() - 1
            }
        }
    }

    /// Gives back a dead stack's index, its frames already gone; nothing may name it.
    pub(crate) fn close_stack(&mut self, stack: usize) {
        debug_assert!(
            self.stacks[stack].list.is_empty(),
            "a stack gave its index back while still holding frames"
        );
        self.free_stacks.push(stack);
    }

    /// The stacks' frame tables this entry holds, given-back ones included: what a leak grows.
    pub fn stack_slots(&self) -> usize {
        self.stacks.len()
    }

    pub(crate) fn fail(&mut self, d: Diagnostic) -> i64 {
        self.fail_with(FAILED, d)
    }

    /// Fails as a raise of `abort.raise`, bound for the innermost `handle` in reach with a clause
    /// for it; with none, the entry fails with `d` as [`Ctx::fail`] would.
    pub(crate) fn raise(&mut self, d: Diagnostic, message: String) -> i64 {
        if self.failed != 0 {
            return 0;
        }
        let Some(at) = self.raise_handler("abort", "raise") else {
            return self.pend(d, message);
        };
        let (stack, depth, clause) = at;
        let clause = &self.stacks[stack].list[depth].clauses[clause];
        let message = Value::str(message);
        // The clause takes the message as emitted code lays a `String`, lifted through the type its
        // `handle` site handed.
        let Some(ty) = clause.string else {
            return self.fail(
                error("an `abort.raise` clause emitted code installed has no `String` type")
                    .note("this is Ply's fault, not the program's"),
            );
        };
        let Some(w) = lifted(self, &message, ty) else {
            return 0;
        };
        let args = Raised(vec![unsafe { Carried::new(w, ty) }]);
        self.bind_raise(at, args, d)
    }

    /// Leaves `abort.raise` of `message`, which no frame answers, pending for the compiled code
    /// holding the word that does; `d` is what the entry fails with should none take it.
    pub(crate) fn pend(&mut self, d: Diagnostic, message: String) -> i64 {
        if self.failed != 0 {
            return 0;
        }
        self.pending = Some(message);
        self.fail_with(FAILED_PENDING, d)
    }

    /// Sends a raise on its way to the clause at `at`, which is handed `args`; `d` is what the
    /// entry fails with should that frame be gone.
    pub(super) fn bind_raise(
        &mut self,
        at: (usize, usize, usize),
        args: Raised,
        d: Diagnostic,
    ) -> i64 {
        let (stack, depth, clause) = at;
        let diagnostic = self.placed(d);
        self.aborting = Some(Aborting {
            stack,
            depth,
            clause,
            args,
            diagnostic,
        });
        self.failed = FAILED_ABORT;
        0
    }

    /// The stack, depth and clause that answer a raise of `effect.op`. Searched as a `perform`
    /// searches, so frames hidden while a clause runs are passed over.
    pub(super) fn raise_handler(&self, effect: &str, op: &str) -> Option<(usize, usize, usize)> {
        let mut stack = self.current;
        loop {
            let found = self.stacks[stack]
                .list
                .iter()
                .enumerate()
                .rev()
                .find_map(|(depth, f)| {
                    let clause = f
                        .clauses
                        .iter()
                        .position(|cl| cl.answers_raise(effect, op))?;
                    Some((stack, depth, clause))
                });
            if found.is_some() {
                return found;
            }
            stack = self.stacks[stack].parent?;
        }
    }

    /// How a finished branch failed, a raise bound past it as the failure it carries; one pending
    /// is carried on pending.
    pub(crate) fn take_branch_failure(&mut self) -> Option<BranchFailure> {
        if self.failed == 0 {
            return None;
        }
        Some(match self.aborting.take() {
            Some(a) if self.failed == FAILED_ABORT => BranchFailure {
                code: FAILED,
                diagnostic: Some(a.into_failure()),
                pending: None,
            },
            _ => BranchFailure {
                code: self.failed,
                diagnostic: self.diagnostic.take(),
                pending: self.pending.take(),
            },
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

    /// [`Ctx::site`] and the definition the body that stored it belongs to.
    pub(crate) fn step_site(&self) -> StepSite {
        StepSite {
            definition: self.root_of(self.site_root).map(|r| r.name.clone()),
            span: self.site(),
        }
    }

    /// Puts `access` into the running step, made where the body last stored a site.
    pub(crate) fn record_access(&mut self, access: Access) {
        let at = self.step_site();
        self.trail.record_access(access, at);
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
        self.diagnostic
            .take()
            .or_else(|| self.aborting.take().map(|a| a.diagnostic))
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
