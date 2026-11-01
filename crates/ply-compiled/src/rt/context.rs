use super::*;

pub(super) static ENTRIES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[repr(C)]
pub struct Context {
    pub failed: i64,
    pub(crate) entry: u64,
    pub stack_floor: std::sync::atomic::AtomicUsize,
    pub site_root: i64,
    pub site_start: i64,
    pub site_end: i64,
    pub(super) time_budget_ms: u64,
    pub grown: u64,
    pub frames: *mut crate::frames::PlyFrame,
    pub release_depth: i64,
    pub release_waiting: i64,
    pub target: Word,
    pub entering_descriptor: Word,
    pub entering_words: Word,
    pub recording: i64,
    pub bridge: Word,
    pub hosted: Word,
    pub unserved: Word,
    pub performed_root: i64,
    pub performed_start: i64,
    pub performed_end: i64,
    pub task: i64,
    pub alloc: *mut crate::heap::Alloc,
    pub constants: *const AtomicI64,
    pub(super) deadline: Option<std::time::Instant>,
    pub heap: Heap,
    pub tables: Arc<Tables>,
    pub diagnostic: Option<Diagnostic>,
    pub(crate) stacks: Vec<Vec<SavedState>>,
    pub(super) free_stacks: Vec<usize>,
    pub(crate) current: usize,
    pub performed: Vec<Effect>,
    pub(crate) detached: Vec<crate::detached::Detached>,
    pub(crate) starting_detached: Option<usize>,
    pub(crate) binding: Arc<ply_core::HostBinding>,
    pub(crate) runtime: Option<Rc<dyn ply_core::HostRuntime>>,
    pub(crate) runtime_factory: ply_core::RuntimeFactory,
    pub(crate) re_executed: bool,
    pub(crate) host_use: ply_core::host::HostUse,
    pub(crate) id: ply_core::host::MachineId,
    pub(crate) pooled: bool,
    pub(crate) teardown: Vec<Diagnostic>,
    pub(crate) seed: ply_core::Seed,
    pub(crate) sim_steps: u32,
    pub(crate) entered_sims: u32,
    pub(crate) schedulers: Vec<(*mut crate::tasks::Scheduler, [i64; 3])>,
    pub accesses: Vec<(crate::tasks::Access, [i64; 3])>,
    pub(crate) last_trail: Option<Vec<u8>>,
    pub(crate) retained_at_root: Option<crate::entry::Retained>,
    pub interleaving: Option<Vec<u8>>,
    pub(crate) pending: Option<Unhandled>,
    pub(crate) pending_words: Vec<Word>,
    pub(crate) traced: bool,
    pub(super) scoped_made: u64,
    pub(super) outer: (*mut Heap, *const i64),
    pub(crate) release_later: Vec<(crate::emitted::Handed, crate::emitted::DropRoot)>,
    pub(crate) continuations_died: Vec<(usize, usize)>,
    pub(crate) marshallings: Vec<Marshalling>,
    pub(crate) taken: Vec<u8>,
    pub(super) outer_running: *mut Context,
}

pub(crate) struct BranchFailure {
    code: i64,
    diagnostic: Option<Diagnostic>,
    pending: Option<Unhandled>,
}

pub(crate) enum Unhandled {
    Abort(String),
    Raised {
        effect: Symbol,
        op: Symbol,
        args: Raised,
    },
}

impl Unhandled {
    pub(crate) fn release(self) {
        if let Unhandled::Raised { args, .. } = self {
            args.release();
        }
    }
}

thread_local! {
    static RUNNING: std::cell::Cell<*mut Context> = const { std::cell::Cell::new(std::ptr::null_mut()) };
}

pub(crate) fn running() -> *mut Context {
    let c = RUNNING.with(std::cell::Cell::get);
    debug_assert!(
        !c.is_null(),
        "a carried value was let go of outside an entry"
    );
    c
}

pub(crate) fn swap_running(c: *mut Context) -> *mut Context {
    RUNNING.with(|r| r.replace(c))
}

pub fn with_running<T>(f: impl FnOnce(&Context) -> T) -> Option<T> {
    unsafe { RUNNING.with(std::cell::Cell::get).as_ref() }.map(f)
}

impl Context {
    pub fn new(tables: Arc<Tables>) -> Context {
        let mut heap = Heap::new();
        let alloc = heap.alloc();
        let constants = tables.memo.as_ptr();
        Context {
            failed: 0,
            stack_floor: std::sync::atomic::AtomicUsize::new(0),
            site_root: -1,
            site_start: 0,
            site_end: 0,
            frames: std::ptr::null_mut(),
            release_depth: 0,
            release_waiting: 0,
            target: 0,
            entering_descriptor: 0,
            entering_words: 0,
            recording: 0,
            bridge: object_word(&NO_HANDLER_OBJECT),
            hosted: 0,
            unserved: object_word(&UNHANDLED_OPERATION),
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
            retained_at_root: None,
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

    pub(crate) fn keep(&mut self, k: SavedState) -> usize {
        let s = self.current;
        self.stacks[s].push(k);
        s
    }

    pub(crate) fn pop_saved(&mut self, s: usize) -> SavedState {
        self.stacks[s]
            .pop()
            .expect("a runtime frame finds what it kept when its call returns")
    }

    pub(crate) fn branch(&self) -> Context {
        let mut b = Context::new(Arc::clone(&self.tables));
        b.deadline = self.deadline;
        b.time_budget_ms = self.time_budget_ms;
        b.entry = self.entry;
        b.binding = Arc::clone(&self.binding);
        b.runtime_factory = Arc::clone(&self.runtime_factory);
        b.re_executed = self.re_executed;
        b.id = self.id;
        b.seed = self.seed.clone();
        b.sim_steps = self.sim_steps;
        b.task = self.task;
        b.traced = self.recording != 0;
        b.refresh_recording_flag();
        b
    }

    pub(crate) fn refresh_recording_flag(&mut self) {
        let seeded = (self.schedulers.iter()).any(|(r, _)| unsafe { (**r).is_seeded() });
        self.recording = i64::from(self.traced || seeded);
    }

    pub fn trace_performs(&mut self, traced: bool) {
        self.traced = traced;
        self.refresh_recording_flag();
    }

    pub(crate) fn absorb(&mut self, mut branch: Context) {
        self.heap.adopt_heap(std::mem::take(&mut branch.heap));
        self.pooled |= branch.pooled;
        self.grown += branch.grown;
        self.performed.append(&mut branch.performed);
        self.host_use.absorb(&branch.host_use);
        self.teardown.append(&mut branch.teardown);
    }

    pub(crate) fn runs_branches_at_once(&self) -> bool {
        !(self.schedulers.iter()).any(|(r, _)| unsafe { (**r).is_seeded() })
    }

    pub(crate) fn merge_concurrent_failure(&mut self, f: BranchFailure) {
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

    pub fn begin(&mut self) {
        self.begin_with_time_limit(0);
    }

    pub fn begin_with_time_limit(&mut self, time_budget_ms: u64) {
        self.stop_deadline_watch();
        self.failed = 0;
        self.grown = 0;
        self.heap.replace_scheduler_active(false);
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
        self.retained_at_root = None;
        self.interleaving = None;
        self.scoped_made = 0;
        if let Some(p) = self.pending.take() {
            p.release();
        }
        self.release_depth = 0;
        self.release_waiting = 0;
        self.target = 0;
        self.entering_descriptor = 0;
        self.entering_words = 0;
        self.refresh_recording_flag();
        self.release_later.clear();
        self.marshallings.clear();
        self.taken.clear();
        if self.heap.retained_bytes() != 0 {
            self.end();
        }
        self.detached.clear();
        self.continuations_died.clear();
        self.entry = ENTRIES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        self.outer = (
            heap::swap_current(&mut self.heap),
            heap::poison_freed::swap(&raw const self.site_root),
        );
        self.outer_running = swap_running(self);
        self.watch();
    }

    pub(crate) fn watch(&self) {
        if let Some(deadline) = self.deadline {
            watched(&self.stack_floor, deadline);
        }
    }

    pub(crate) fn stop_deadline_watch(&self) {
        if self.deadline.is_some() {
            unwatched(&self.stack_floor);
        }
    }

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

    pub(crate) fn restore_thread_state(&mut self) {
        self.stop_deadline_watch();
        heap::poison_freed::swap(self.outer.1);
        heap::swap_current(self.outer.0);
        swap_running(self.outer_running);
        self.outer = (std::ptr::null_mut(), std::ptr::null());
        self.outer_running = std::ptr::null_mut();
    }

    pub fn teardown_and_count_live(&mut self) -> usize {
        crate::detached::teardown(self);
        self.live()
    }

    pub fn end(&mut self) {
        if let Some(p) = self.pending.take() {
            p.release();
        }
        crate::detached::teardown(self);
        debug_assert!(
            self.release_later.is_empty() && self.release_depth == 0,
            "an entry ended with a release still running or waiting"
        );
        self.restore_thread_state();
        self.heap.end();
        if std::mem::take(&mut self.pooled) {
            crate::parallel::workers_ended(self.id.0);
        }
    }

    fn workers(&self, read: impl Fn(&crate::blocks::Blocks) -> usize + Sync) -> usize {
        if self.pooled {
            crate::parallel::worker_blocks(self.id.0, read)
                .into_iter()
                .sum()
        } else {
            0
        }
    }

    pub fn live_bytes(&self) -> usize {
        self.heap.live_bytes() + self.workers(|b| b.live())
    }

    pub fn retained_bytes(&self) -> usize {
        self.heap.retained_bytes() + self.workers(|b| b.retained_bytes())
    }

    pub fn live(&self) -> usize {
        let branches: usize = if self.pooled {
            crate::parallel::worker_blocks(self.id.0, heap::program_live_block_count_in)
                .into_iter()
                .sum()
        } else {
            0
        };
        self.heap.program_live_block_count() + branches
    }

    pub fn census(&self) -> Vec<(usize, usize, usize)> {
        let mut rows = self.heap.census();
        if self.pooled {
            for tally in crate::parallel::worker_blocks(self.id.0, heap::size_histogram_in) {
                rows = heap::merge_size_histograms(rows, tally);
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

    pub(crate) fn host_runtime(&mut self) -> Rc<dyn ply_core::HostRuntime> {
        if let Some(runtime) = &self.runtime {
            return Rc::clone(runtime);
        }
        let runtime = (self.runtime_factory)();
        self.runtime = Some(Rc::clone(&runtime));
        runtime
    }

    pub(crate) fn open_stack(&mut self) -> usize {
        match self.free_stacks.pop() {
            Some(stack) => stack,
            None => {
                self.stacks.push(Vec::new());
                self.stacks.len() - 1
            }
        }
    }

    pub(crate) fn close_stack(&mut self, stack: usize) {
        for k in std::mem::take(&mut self.stacks[stack]) {
            k.release();
        }
        self.free_stacks.push(stack);
    }

    pub fn stack_slots(&self) -> usize {
        self.stacks.len()
    }

    pub(crate) fn fail(&mut self, d: Diagnostic) -> i64 {
        self.fail_with(FAILED, d)
    }

    pub(crate) fn fail_unhandled_abort(&mut self, d: Diagnostic, message: String) -> i64 {
        if self.failed != 0 {
            return 0;
        }
        self.pending = Some(Unhandled::Abort(message));
        self.fail_with(FAILED_PENDING_RAISE, d)
    }

    pub(crate) fn fail_with_pending_raise(
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
        self.fail_with(FAILED_PENDING_RAISE, d)
    }

    pub(crate) fn take_failure_to_merge(&mut self) -> Option<BranchFailure> {
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

    pub fn site(&self) -> Span {
        self.span_at(self.site_root, self.site_start, self.site_end)
    }

    pub(crate) fn touched(&mut self, touched: crate::tasks::Access) {
        if let crate::tasks::Access::Effect { effect, .. } = &touched
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

    pub(crate) fn root_name(&self, root: i64) -> Option<&Symbol> {
        self.root_of(root).map(|r| &r.name)
    }

    fn root_of(&self, root: i64) -> Option<&Root> {
        let id = u64::try_from(root).ok()?;
        let roots = &self.tables.roots;
        let at = roots.binary_search_by_key(&id, |r| r.id).ok()?;
        Some(&roots[at])
    }

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
        match self.pending.take() {
            Some(Unhandled::Raised { effect, op, args }) => {
                self.host_runtime().unhandled(&effect, &op);
                args.release();
            }
            Some(Unhandled::Abort(_)) | None => {}
        }
        self.diagnostic.take()
    }

    pub fn take_host_usage(&mut self) -> ply_core::host::HostUse {
        std::mem::take(&mut self.host_use)
    }
}

pub(super) fn error(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::RUNTIME_ERROR, message.into()).primary(Span::DUMMY, "in compiled code")
}

pub(super) fn args_of<'a>(ptr: *const i64, n: i64) -> &'a [Word] {
    if n == 0 {
        return &[];
    }
    unsafe { std::slice::from_raw_parts(ptr, n as usize) }
}
