//! Entering a compiled unit: the unit opened once from its library and shared by every thread that
//! enters it, a root entered on a fresh context of its own through its `R#entered`, its arguments
//! the PLV1 list the root reads, and how the entry ended, its result the PLV1 the root wrote,
//! untouched. No front end is read: which root to enter, what its arguments are and what its result
//! or failure means are the entering program's. The natives `std.machine` declares are here, each
//! taking and returning the flat format of [`crate::flat`].
//!
//! A unit opened here has no source, so a span the runtime makes is a site: its [`SourceId`] is the
//! index of its root in the unit's roots, which [`Opened::root_named`] names, and its offsets are
//! from the start of that root's definition.

use crate::c::Native;
use crate::flat::{Reader, Writer};
use crate::native::{Addr, I64, TakenBytes};
use crate::rt::{Ctx, EntryBytes, FAILED, FAILED_TARGET, borrowed};
pub use ply_core::host::Grants;
use ply_core::host::{HostBinding, HostRuntime, RuntimeFactory};
use ply_core::{Diagnostic, Seed, Severity, SourceId, Span, Symbol, codes};
use ply_host::observe::Recorder;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Instant;

/// A unit opened for entering: its code, loaded and bound once, and how long that took.
pub struct Opened {
    native: Native,
    nanos: u64,
}

// SAFETY: what an entry reads of a unit, its tables, is what a `parallel` block's branches already
// read from many threads at once (what they change is behind an atomic or a lock); its code and its
// descriptor are read-only.
unsafe impl Send for Opened {}
unsafe impl Sync for Opened {}

impl Opened {
    /// The unit compiled into the library at `library`, or why it does not load, as when it was
    /// emitted against another runtime.
    pub fn open(library: &std::path::Path) -> Result<Arc<Opened>, String> {
        let started = Instant::now();
        let native = crate::c::load_opened(library).map_err(|e| format!("{e:#}"))?;
        Ok(Arc::new(Opened {
            native,
            nanos: elapsed_nanos(started),
        }))
    }

    /// The program this executable was linked with.
    pub fn linked() -> Result<Arc<Opened>, String> {
        let started = Instant::now();
        let native = crate::c::load_linked().map_err(|e| format!("{e:#}"))?;
        Ok(Arc::new(Opened {
            native,
            nanos: elapsed_nanos(started),
        }))
    }

    /// How many roots the unit is entered at.
    pub fn roots(&self) -> usize {
        self.native.entered_names().len()
    }

    /// Nanoseconds loading the unit took.
    pub fn nanos(&self) -> u64 {
        self.nanos
    }

    /// The root a site's span names, by the index its [`SourceId`] holds.
    pub fn root_named(&self, span: Span) -> Option<&Symbol> {
        let roots = &self.native.tables().roots;
        usize::try_from(span.source.0)
            .ok()
            .and_then(|at| roots.get(at))
            .map(|r| &r.name)
    }
}

/// The arguments of a root that takes none: the PLV1 of the empty list, its format and then the
/// list's tag and its count.
pub const NO_ARGUMENTS: &[u8] = b"PLV1\x09\x00";

/// The span a unit with no source gives the root at `at` among its roots: a site of that root.
pub(crate) fn site_span(at: usize) -> Span {
    Span::new(SourceId(u32::try_from(at).unwrap_or(u32::MAX - 1)), 0, 0)
}

/// What makes bindings: the families the runtime serves itself, which `ply-privileged` holds and
/// installs at startup.
pub trait Binder: Send + Sync {
    /// A binding of the families from `grants` alone, or why it could not be made.
    fn bind(&self, grants: Grants) -> Result<Arc<dyn Bound>, Vec<Diagnostic>>;

    /// The facts a run's disclosures print under `grants`, written as `std.machine`'s `Hosting`.
    fn hosting(&self, grants: &Grants, w: &mut Writer);

    /// The first read `trace` holds that no longer returns what it did under `grants`, as a person
    /// reads it, the reads it made to say so entered into `caller`, the asking run's record.
    fn moved(&self, grants: &Grants, trace: &str, caller: Option<&Recorder>) -> Option<String>;
}

/// The host binding an entry runs under: what serves the operations its program performs that no
/// handler of its own handles.
pub trait Bound: Send + Sync {
    fn binding(&self) -> Arc<HostBinding>;

    /// What makes the reactor a pending host response is waited on with, on the thread that enters.
    fn runtime_factory(&self) -> RuntimeFactory;

    /// The code `process.exit` asked for, once asked.
    fn requested_exit(&self) -> Option<i32>;

    /// The trace a pass of an entry is recorded with, of what `recorder` came to, read against the
    /// world this binding grants: none where nothing could stand in for the run. `host` is whether
    /// the entry reached a host handler.
    fn traced(&self, recorder: &Recorder, host: bool) -> Option<String>;

    /// The binding torn down, its children ended and its sink flushed: what its facilities report,
    /// by name.
    fn unbind(&self) -> Vec<(String, Vec<u8>)>;
}

/// What an entry's reads are entered into: a record of its own where `recorded`, which its outcome's
/// trace is made of and its caller's record takes in, and else `caller`'s, as a run made on the
/// caller's behalf.
pub struct Watch<'a> {
    pub by: &'a dyn Bound,
    pub caller: Option<Arc<Recorder>>,
    pub recorded: bool,
}

/// How an entry is bounded and what it is run with.
#[derive(Clone, Debug)]
pub struct Run {
    /// Milliseconds of wall clock the entry may take; 0 is no clock.
    pub timeout_ms: u64,
    /// The schedule its simulations take and the steps each interleaving may take; the
    /// default seed where none is named.
    pub seed: Option<(Seed, u32)>,
    /// One of several runs of one test a search makes: reaching the host is rejected.
    pub re_executed: bool,
    /// Whether to read the wall clock for how long the entry took.
    pub measure: bool,
    /// Whether the entry is a test's: its usage counts every perform, those its own handlers handle
    /// included, and the objects it leaves live.
    pub tested: bool,
}

impl Run {
    /// A run nothing bounds, as a program's own entry is.
    pub fn unbounded() -> Run {
        Run {
            timeout_ms: 0,
            seed: None,
            re_executed: false,
            measure: false,
            tested: false,
        }
    }
}

/// How an entry ended.
pub enum Ended {
    /// The root returned: its result's PLV1.
    Returned(Vec<u8>),
    /// A raise nothing handled, a runtime error, the clock run out, or a result the boundary
    /// rejects.
    Failed(Diagnostic),
    /// A Rust unwind out of the entry, with what it said.
    Panicked(String),
    /// The unit is entered at no such root.
    NoRoot,
    /// The result holds what no run but its own can hold, and the text says why.
    NotReturnable(&'static str),
}

/// What an entry cost and reached, whatever it came to.
#[derive(Clone, Debug, Default)]
pub struct Usage {
    /// Wall clock, where the run was measured.
    pub nanos: u64,
    /// Whether it reached a nondeterministic host handler.
    pub host: bool,
    /// The operations it performed: every one where the run counted them, else those a handler
    /// the runtime keeps handled.
    pub performs: u64,
    pub held: Held,
    pub updates: u64,
    pub updates_in_place: u64,
    /// Objects of the program's still live once a test's entry returned and gave back what its
    /// bodies held: each one nothing let go of. 0 for any other entry.
    pub live: u64,
}

/// What a run still held when an entry's root body returned, before the run's scheduler drained its
/// other tasks: what grows with the work a long-running program does if anything is never given
/// back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Held {
    /// Native objects the runtime made, such as a task's handle.
    pub native_objects: u64,
    /// Tasks the run's scheduler still holds.
    pub scheduled: u64,
    /// Task stacks the run's scheduler still keeps, its tasks' and those kept for its next ones.
    pub task_stacks: u64,
    /// Frame tables kept for stacks that are or were running.
    pub frame_tables: u64,
}

/// How an entry ended, what it used, what its host runtime warned of as it ended, the interleaving
/// its simulations took as `std.internal.schedule` wrote it, and the trace of what it read
/// where it was recorded.
pub struct Outcome {
    pub ended: Ended,
    pub usage: Usage,
    pub warnings: Vec<Diagnostic>,
    pub interleaving: Option<Vec<u8>>,
    pub trace: Option<String>,
}

impl Outcome {
    fn of(ended: Ended) -> Outcome {
        Outcome {
            ended,
            usage: Usage::default(),
            warnings: Vec::new(),
            interleaving: None,
            trace: None,
        }
    }
}

/// `root` of `opened` entered on a fresh context on this thread, its arguments `arguments`, the
/// PLV1 list its `R#entered` reads, under `bound` where it names one and else no host, its reads
/// recorded as `watch` says. A Rust unwind out of the Rust around the compiled call is returned as
/// [`Ended::Panicked`], the thread as the entry found it and the entry's context left unreleased;
/// one inside a runtime function compiled code calls aborts the process, as any unwind out of an
/// `extern "C"` function does.
pub fn enter(
    opened: &Opened,
    root: &str,
    arguments: Vec<u8>,
    run: &Run,
    bound: Option<&dyn Bound>,
    watch: Option<Watch<'_>>,
) -> Outcome {
    let Some(entry) = opened.native.entered(root) else {
        return Outcome::of(Ended::NoRoot);
    };
    let mut ctx = Box::new(opened.native.context());
    let own = (watch.as_ref())
        .filter(|w| w.recorded)
        .map(|_| Arc::new(Recorder::default()));
    let recorder = own
        .clone()
        .or_else(|| watch.as_ref().and_then(|w| w.caller.clone()));
    if let Some(bound) = bound {
        let made = bound.runtime_factory();
        // Each runtime the entry makes, a `parallel` branch's among them, records into one record.
        let factory: RuntimeFactory = match &recorder {
            Some(recorder) => {
                let recorder = Arc::clone(recorder);
                Arc::new(move || {
                    let runtime = made();
                    ply_host::observe::records_into(&*runtime, &recorder);
                    runtime
                })
            }
            None => made,
        };
        let runtime: Rc<dyn HostRuntime> = factory();
        ctx.set_host(bound.binding(), Some(runtime), Some(factory));
    }
    ctx.re_executed = run.re_executed;
    ctx.trace_performs(run.tested);
    let (seed, steps) = run
        .seed
        .clone()
        .unwrap_or_else(|| (Seed::default(), ply_core::sim::DEFAULT_STEPS));
    ctx.seed = seed;
    ctx.sim_steps = steps.max(1);
    // The counters are this thread's, an outer entry's among them, so the entry's are a difference.
    let counted = ply_core::rc::stats();
    let started = Instant::now();
    let entered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        entered(&mut ctx, entry, arguments, run, root)
    }));
    let nanos = if run.measure {
        elapsed_nanos(started)
    } else {
        0
    };
    let mut outcome = match entered {
        Ok((ended, last)) => {
            let counters = ply_core::rc::stats();
            Outcome {
                ended,
                usage: Usage {
                    nanos,
                    host: ctx.host_use.acted(),
                    performs: last.performs,
                    held: last.held,
                    updates: counters.updates.saturating_sub(counted.updates),
                    updates_in_place: counters
                        .updates_in_place
                        .saturating_sub(counted.updates_in_place),
                    live: last.live,
                },
                warnings: std::mem::take(&mut ctx.teardown),
                interleaving: last.interleaving,
                trace: None,
            }
        }
        Err(payload) => {
            abandoned(ctx);
            Outcome::of(Ended::Panicked(panic_message(payload.as_ref())))
        }
    };
    if let (Some(own), Some(w)) = (own, &watch) {
        if let Some(caller) = &w.caller {
            caller.absorb(&own);
        }
        outcome.trace = w.by.traced(&own, outcome.usage.host);
    }
    outcome
}

/// What `enter` returns, run on a thread of its own with a program's stack, so how deeply an entry
/// may nest does not hang on what its caller left of its own stack: `None` where no thread could be
/// made or the work unwound.
pub fn on_program_thread<T: Send>(enter: impl FnOnce() -> T + Send) -> Option<T> {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(crate::rt::PROGRAM_STACK)
            .spawn_scoped(scope, enter)
            .ok()?
            .join()
            .ok()
    })
}

/// What an entry leaves for its outcome once its root returned.
struct Last {
    performs: u64,
    held: Held,
    interleaving: Option<Vec<u8>>,
    live: u64,
}

/// An entry a Rust unwind left: its host runtime and the pool are told it is over, and its context
/// is never released, since its heap, schedulers and detached bodies are the dead entry's and
/// letting them go would release through whichever context now runs on this thread.
fn abandoned(mut ctx: Box<Ctx>) {
    if let Some(rt) = ctx.runtime.clone() {
        let _ = rt.end_entry_point(ctx.id);
    }
    if std::mem::take(&mut ctx.pooled) {
        crate::parallel::workers_ended(ctx.id.0);
    }
    std::mem::forget(ctx);
}

/// Gives the thread back as the entry found it where a Rust unwind leaves the entry.
struct GivingBack(*mut Ctx);

impl Drop for GivingBack {
    fn drop(&mut self) {
        unsafe { (*self.0).give_thread_back() };
    }
}

fn entered(
    ctx: &mut Ctx,
    entry: crate::rt::Entry,
    arguments: Vec<u8>,
    run: &Run,
    root: &str,
) -> (Ended, Last) {
    ctx.begin_bounded(run.timeout_ms);
    let giving_back = GivingBack(std::ptr::from_mut(ctx));
    ctx.open_entry(arguments);
    let entering = std::ptr::from_mut(ctx);
    let none: [i64; 0] = [];
    // SAFETY: the unit owns the entry's pages and outlives the call, `ctx` is this entry's alone,
    // and an `R#entered` takes no words.
    crate::rt::on_known_stack(entering, || unsafe { entry(entering, none.as_ptr()) });
    if ctx.failed == crate::rt::FAILED_OUT_OF_STACK {
        crate::rt::let_go_below_here();
    }
    let mut last = returned(ctx);
    let ended = if ctx.failed != 0 {
        failure(ctx)
    } else {
        ended_with_result(ctx, root)
    };
    if run.tested && matches!(ended, Ended::Returned(_)) {
        last.live = ctx.left_live() as u64;
    }
    ctx.end();
    std::mem::forget(giving_back);
    (ended, last)
}

/// What an entry leaves once its root returned, its run's scheduler drained: its host runtime told
/// the entry is over.
fn returned(ctx: &mut Ctx) -> Last {
    let held = ctx.held_at_root.take().unwrap_or(Held {
        native_objects: ctx.heap.natives() as u64,
        scheduled: 0,
        task_stacks: 0,
        frame_tables: ctx.stack_slots() as u64,
    });
    if let Some(rt) = ctx.runtime.clone() {
        let warned = rt.end_entry_point(ctx.id);
        ctx.teardown.extend(warned);
    }
    Last {
        performs: ctx.performed.len() as u64,
        held,
        interleaving: ctx.interleaving.take(),
        live: 0,
    }
}

/// How an entry whose root returned ended: its result's bytes, or why the boundary keeps it.
fn ended_with_result(ctx: &mut Ctx, root: &str) -> Ended {
    match crate::rt::close_entry(ctx) {
        Ok(EntryBytes::Bytes(bytes)) => Ended::Returned(bytes),
        Ok(EntryBytes::Function) => {
            Ended::NotReturnable("its result holds a function the program wrote")
        }
        Ok(EntryBytes::Escaping(escapee)) => {
            let boundary = ply_core::Boundary::EntryResult { name: root };
            Ended::Failed(ply_core::escape::rejected(&boundary, &escapee, Span::DUMMY))
        }
        Ok(EntryBytes::Holding) => Ended::NotReturnable("its result holds a handle of its run"),
        Err(reason) => Ended::Failed(crate::rt::err_marshalling(&reason)),
    }
}

/// How an entry that failed ended.
fn failure(ctx: &mut Ctx) -> Ended {
    let failed = ctx.take_failure().unwrap_or_else(|| match ctx.failed {
        FAILED_TARGET => Diagnostic::error(
            codes::RUNTIME_ERROR,
            "a `handle` clause unwound past the compiled fragment's entry",
        ),
        FAILED => Diagnostic::error(
            codes::INTERNAL_ERROR,
            "the compiled body failed without recording why",
        )
        .primary(ctx.site(), "the last site it reached"),
        other => Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("the compiled body failed ({other}) without recording why"),
        ),
    });
    Ended::Failed(failed)
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic with a non-string payload".to_string()
    }
}

fn elapsed_nanos(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

// --- The natives -----------------------------------------------------------------------

static BINDER: OnceLock<Box<dyn Binder>> = OnceLock::new();

/// The binder the natives make bindings with; a process that installs none binds nothing.
pub fn install(binder: Box<dyn Binder>) {
    let _ = BINDER.set(binder);
}

/// Every native `std.machine` declares, under the name it declares it by.
pub fn natives() -> Vec<crate::c::MachineNative> {
    use crate::c::machine_native;
    vec![
        machine_native(
            "unit_open",
            rt_unit_open as unsafe extern "C" fn(_, _, _) -> _,
        ),
        machine_native("unit_close", rt_unit_close as unsafe extern "C" fn(_, _)),
        machine_native(
            "unit_bind",
            rt_unit_bind as unsafe extern "C" fn(_, _, _) -> _,
        ),
        machine_native(
            "unit_unbind",
            rt_unit_unbind as unsafe extern "C" fn(_, _) -> _,
        ),
        machine_native(
            "unit_hosting",
            rt_unit_hosting as unsafe extern "C" fn(_, _, _) -> _,
        ),
        machine_native(
            "unit_moved",
            rt_unit_moved as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        machine_native(
            "unit_enter",
            rt_unit_enter as unsafe extern "C" fn(_, _, _, _, _, _, _, _) -> _,
        ),
    ]
}

/// What programs hold by the handles the natives return: never 0, which names none.
struct Handles<T> {
    next: i64,
    by: HashMap<i64, T>,
}

impl<T: Clone> Handles<T> {
    fn keep(&mut self, t: T) -> i64 {
        self.next += 1;
        self.by.insert(self.next, t);
        self.next
    }

    fn get(&self, handle: i64) -> Option<T> {
        self.by.get(&handle).cloned()
    }
}

fn handles<T>() -> Mutex<Handles<T>> {
    Mutex::new(Handles {
        next: 0,
        by: HashMap::new(),
    })
}

fn units() -> MutexGuard<'static, Handles<Arc<Opened>>> {
    static UNITS: OnceLock<Mutex<Handles<Arc<Opened>>>> = OnceLock::new();
    lock(UNITS.get_or_init(handles))
}

fn bindings() -> MutexGuard<'static, Handles<Arc<dyn Bound>>> {
    static BINDINGS: OnceLock<Mutex<Handles<Arc<dyn Bound>>>> = OnceLock::new();
    lock(BINDINGS.get_or_init(handles))
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `open`: `result(opened, string)`, `opened := int unit, int roots, int nanos, int threads`.
unsafe extern "C" fn rt_unit_open(ctx: *mut Ctx, p: Addr, n: I64) -> TakenBytes {
    let library = unsafe { borrowed(p, n) };
    let mut w = Writer::new();
    match std::str::from_utf8(library)
        .map_err(|_| "the library's path is not UTF-8".to_string())
        .and_then(|path| Opened::open(std::path::Path::new(path)))
    {
        Ok(opened) => {
            let (roots, nanos) = (opened.roots() as u64, opened.nanos());
            let unit = units().keep(opened);
            let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
            w.tag(0)
                .int(unit)
                .count(roots)
                .count(nanos)
                .count(threads as u64);
        }
        Err(reason) => {
            w.tag(1).str(&reason);
        }
    }
    unsafe { &mut *ctx }.given(w.finish())
}

/// `close`: the unit let go; an entry still running holds it until it ends.
unsafe extern "C" fn rt_unit_close(_ctx: *mut Ctx, unit: I64) {
    units().by.remove(&unit.0);
}

/// `bind`: `result(int, list(failure))`. A binding grants the host, so it is rejected unless the
/// caller is granted `machine`.
unsafe extern "C" fn rt_unit_bind(ctx: *mut Ctx, p: Addr, n: I64) -> TakenBytes {
    let c = unsafe { &mut *ctx };
    let Some(grants) = read_or_fail(c, unsafe { borrowed(p, n) }, read_grants) else {
        return TakenBytes(0);
    };
    if !allowed(c, "machine.bind") {
        return TakenBytes(0);
    }
    let made = match BINDER.get() {
        Some(binder) => binder.bind(grants),
        None => Err(vec![err_no_binder()]),
    };
    let mut w = Writer::new();
    match made {
        Ok(bound) => {
            let binding = bindings().keep(bound);
            w.tag(0).int(binding);
        }
        Err(rejected) => {
            w.tag(1)
                .list(rejected.iter(), |w, d| write_failure(w, d, None));
        }
    }
    c.given(w.finish())
}

/// `unbind`: `list(report)`, `report := string name, bytes body`.
unsafe extern "C" fn rt_unit_unbind(ctx: *mut Ctx, binding: I64) -> TakenBytes {
    let bound = bindings().by.remove(&binding.0);
    let reports = bound.map(|b| b.unbind()).unwrap_or_default();
    let mut w = Writer::new();
    w.list(reports.iter(), |w, (name, body)| {
        w.str(name).bytes(body);
    });
    unsafe { &mut *ctx }.given(w.finish())
}

/// `hosting`: the disclosures' facts under the grants.
unsafe extern "C" fn rt_unit_hosting(ctx: *mut Ctx, p: Addr, n: I64) -> TakenBytes {
    let c = unsafe { &mut *ctx };
    let Some(grants) = read_or_fail(c, unsafe { borrowed(p, n) }, read_grants) else {
        return TakenBytes(0);
    };
    let Some(binder) = BINDER.get() else {
        return TakenBytes(c.fail(err_no_binder()));
    };
    let mut w = Writer::new();
    binder.hosting(&grants, &mut w);
    c.given(w.finish())
}

/// `moved`: `option(string)`, under the roots and the binding the grants name.
unsafe extern "C" fn rt_unit_moved(
    ctx: *mut Ctx,
    gp: Addr,
    gn: I64,
    tp: Addr,
    tn: I64,
) -> TakenBytes {
    let c = unsafe { &mut *ctx };
    let Some(grants) = read_or_fail(c, unsafe { borrowed(gp, gn) }, read_grants) else {
        return TakenBytes(0);
    };
    if !allowed(c, "machine.moved") {
        return TakenBytes(0);
    }
    let Some(binder) = BINDER.get() else {
        return TakenBytes(c.fail(err_no_binder()));
    };
    let trace = String::from_utf8_lossy(unsafe { borrowed(tp, tn) }).into_owned();
    let caller = ply_host::observe::of(&*c.host_runtime());
    let moved = binder.moved(&grants, &trace, caller.as_deref());
    let mut w = Writer::new();
    w.option(moved.as_deref(), |w, m| {
        w.str(m);
    });
    c.given(w.finish())
}

/// `enter`: `outcome`, `root` of the unit entered on a context and a program thread of its own.
#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn rt_unit_enter(
    ctx: *mut Ctx,
    unit: I64,
    root_p: Addr,
    root_n: I64,
    arguments_p: Addr,
    arguments_n: I64,
    run_p: Addr,
    run_n: I64,
) -> TakenBytes {
    let c = unsafe { &mut *ctx };
    let Some(asked) = read_or_fail(c, unsafe { borrowed(run_p, run_n) }, read_run) else {
        return TakenBytes(0);
    };
    if (asked.run.timeout_ms > 0 || asked.run.measure) && !allowed(c, "machine.enter") {
        return TakenBytes(0);
    }
    let root = String::from_utf8_lossy(unsafe { borrowed(root_p, root_n) }).into_owned();
    let arguments = unsafe { borrowed(arguments_p, arguments_n) }.to_vec();
    let Some(opened) = units().get(unit.0) else {
        return TakenBytes(c.fail(err_unknown_handle("unit", unit.0)));
    };
    let bound = match asked.binding {
        0 => None,
        handle => match bindings().get(handle) {
            Some(bound) => Some(bound),
            None => return TakenBytes(c.fail(err_unknown_handle("binding", handle))),
        },
    };
    let caller = ply_host::observe::of(&*c.host_runtime());
    let entered = on_program_thread(|| {
        let watch = bound.as_deref().map(|by| Watch {
            by,
            caller,
            recorded: asked.observe,
        });
        let hosted = bound.as_deref().filter(|_| asked.hosted);
        enter(&opened, &root, arguments, &asked.run, hosted, watch)
    });
    let Some(outcome) = entered else {
        return TakenBytes(c.fail(err_no_thread()));
    };
    let exit = bound.as_ref().and_then(|b| b.requested_exit());
    let mut w = Writer::new();
    write_outcome(&mut w, &opened, &outcome, exit);
    c.given(w.finish())
}

/// Whether the caller is granted `machine`, the context failed where it is not.
fn allowed(c: &mut Ctx, op: &str) -> bool {
    if c.binding.grants().allows("machine") {
        return true;
    }
    c.fail(
        Diagnostic::error(
            codes::CAPABILITY_UNDECLARED,
            format!(
                "`{op}` passes the host to the run it makes, and this run is not granted `machine`"
            ),
        )
        .note("`--allow machine` grants it; `hermetic_machine` needs no grant"),
    );
    false
}

fn read_or_fail<T>(
    c: &mut Ctx,
    bytes: &[u8],
    read: impl FnOnce(&mut Reader<'_>) -> Result<T, String>,
) -> Option<T> {
    let mut r = Reader::new(bytes);
    match read(&mut r).and_then(|t| r.end().map(|()| t)) {
        Ok(t) => Some(t),
        Err(reason) => {
            c.fail(err_unread(&reason));
            None
        }
    }
}

/// `grants := list(entry)`, `entry := string name, list(string) fields`.
pub fn read_grants(r: &mut Reader<'_>) -> Result<Grants, String> {
    Ok(Grants {
        entries: r.list(|r| Ok((r.str()?, r.list(Reader::str)?)))?,
    })
}

/// A run as `std.machine` writes it, with what it says of the binding.
struct Asked {
    binding: i64,
    hosted: bool,
    observe: bool,
    run: Run,
}

/// `run := int binding, bool hosted, int timeout_ms, option(seed), bool re_executed, bool observe,
/// bool measure, bool tested`.
fn read_run(r: &mut Reader<'_>) -> Result<Asked, String> {
    let binding = r.int()?;
    let hosted = r.bool()?;
    let timeout_ms = r.count()?;
    let seed = r.option(read_seed)?;
    let re_executed = r.bool()?;
    let observe = r.bool()?;
    let measure = r.bool()?;
    let tested = r.bool()?;
    Ok(Asked {
        binding,
        hosted,
        observe,
        run: Run {
            timeout_ms,
            seed,
            re_executed,
            measure,
            tested,
        },
    })
}

/// `seed := u64 root, list(int) path, int steps`.
fn read_seed(r: &mut Reader<'_>) -> Result<(Seed, u32), String> {
    let root = r.varint()?;
    let path = r.list(|r| {
        let at = r.count()?;
        u16::try_from(at).map_err(|_| format!("a choice of {at}, past what a point offers"))
    })?;
    let steps = r.count()?;
    Ok((
        Seed::at(root, path),
        u32::try_from(steps.max(1)).unwrap_or(u32::MAX),
    ))
}

/// `outcome := ended, option(int) exit, usage, list(failure) warnings, option(interleaving),
/// option(bytes) trace`; `ended := tag 0 bytes answer | 1 failure | 2 string panic | 3 no root |
/// 4 string why unanswerable`; `usage := int nanos, bool host, int performs, int native_objects,
/// int scheduled, int task_stacks, int frame_tables, int updates, int updates_in_place, int live`.
fn write_outcome(w: &mut Writer, opened: &Opened, outcome: &Outcome, exit: Option<i32>) {
    match &outcome.ended {
        Ended::Returned(bytes) => {
            w.tag(0).bytes(bytes);
        }
        Ended::Failed(d) => {
            w.tag(1);
            write_failure(w, d, Some(opened));
        }
        Ended::Panicked(message) => {
            w.tag(2).str(message);
        }
        Ended::NoRoot => {
            w.tag(3);
        }
        Ended::NotReturnable(reason) => {
            w.tag(4).str(reason);
        }
    }
    w.option(exit, |w, code| {
        w.int(i64::from(code));
    });
    let u = &outcome.usage;
    w.count(u.nanos)
        .bool(u.host)
        .count(u.performs)
        .count(u.held.native_objects)
        .count(u.held.scheduled)
        .count(u.held.task_stacks)
        .count(u.held.frame_tables)
        .count(u.updates)
        .count(u.updates_in_place)
        .count(u.live);
    w.list(outcome.warnings.iter(), |w, d| {
        write_failure(w, d, Some(opened))
    });
    w.option(outcome.interleaving.as_deref(), |w, interleaving| {
        w.written(interleaving);
    });
    w.option(outcome.trace.as_deref(), |w, t| {
        w.bytes(t.as_bytes());
    });
}

/// `failure := string code, tag severity, string message, list(label), list(string) notes,
/// list(bytes) values`; `label := option(site), bool primary, string text`; each value its PLV1,
/// as [`Diagnostic::values`] holds it. A label at no site of `opened` is unplaced.
pub fn write_failure(w: &mut Writer, d: &Diagnostic, opened: Option<&Opened>) {
    w.str(d.code)
        .tag(match d.severity {
            Severity::Error => 0,
            Severity::Warning => 1,
        })
        .str(&d.message);
    w.list(d.labels.iter(), |w, l| {
        write_site(w, opened, l.span);
        w.bool(l.primary).str(&l.message);
    });
    w.list(d.notes.iter(), |w, n| {
        w.str(n);
    });
    w.list(d.values.iter(), |w, v| {
        w.bytes(v);
    });
}

/// `option(site)`, `site := string root, int start, int end`.
fn write_site(w: &mut Writer, opened: Option<&Opened>, span: Span) {
    let root = opened
        .filter(|_| span != Span::DUMMY)
        .and_then(|o| o.root_named(span));
    w.option(root, |w, root| {
        w.str(root.as_str())
            .count(u64::from(span.start))
            .count(u64::from(span.end));
    });
}

#[cold]
fn err_unread(reason: &str) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("`std.machine` handed the runtime bytes it does not read: {reason}"),
    )
    .note("the module writes the format the runtime reads; this is Ply's fault")
}

#[cold]
fn err_unknown_handle(what: &str, handle: i64) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("no {what} is held as {handle}"),
    )
    .note("a handle is one `std.machine` returned and has not let go; this is Ply's fault")
}

#[cold]
fn err_no_binder() -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        "this process serves no host families: no binder was installed",
    )
    .note("a `ply` installs its binder before it enters its program; this is Ply's fault")
}

#[cold]
fn err_no_thread() -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        "the thread an entry runs on could not be made, or its work unwound",
    )
}
