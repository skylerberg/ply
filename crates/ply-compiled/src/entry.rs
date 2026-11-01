use crate::c::Native;
use crate::flat::{Reader, Writer};
use crate::native::{Address, I64, TakenBytes};
use crate::rt::{Context, EntryBytes, FAILED, FAILED_UNWOUND_TO_HANDLER, borrowed};
pub use ply_core::host::Grants;
use ply_core::host::{HostBinding, HostRuntime, RuntimeFactory};
use ply_core::{Diagnostic, Seed, Severity, SourceId, Span, Symbol, codes};
use ply_host::observe::Recorder;
use std::collections::HashMap;
use std::io::Write;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Once, OnceLock, PoisonError};
use std::time::{Duration, Instant};

pub struct Opened {
    native: Native,
    nanos: u64,
}

unsafe impl Send for Opened {}
unsafe impl Sync for Opened {}

impl Opened {
    pub fn open(library: &std::path::Path) -> Result<Arc<Opened>, String> {
        let started = Instant::now();
        let native = crate::c::load_from_path(library).map_err(|e| format!("{e:#}"))?;
        Ok(Arc::new(Opened {
            native,
            nanos: elapsed_nanos(started),
        }))
    }

    pub fn linked() -> Result<Arc<Opened>, String> {
        let started = Instant::now();
        let native = crate::c::load_linked().map_err(|e| format!("{e:#}"))?;
        Ok(Arc::new(Opened {
            native,
            nanos: elapsed_nanos(started),
        }))
    }

    pub fn roots(&self) -> usize {
        self.native.entry_point_names().len()
    }

    pub fn nanos(&self) -> u64 {
        self.nanos
    }

    pub fn root_named(&self, span: Span) -> Option<&Symbol> {
        let roots = &self.native.tables().roots;
        usize::try_from(span.source.0)
            .ok()
            .and_then(|at| roots.get(at))
            .map(|r| &r.name)
    }
}

pub const NO_ARGUMENTS: &[u8] = b"PLV1\x09\x00";

pub(crate) fn site_span(at: usize) -> Span {
    Span::new(SourceId(u32::try_from(at).unwrap_or(u32::MAX - 1)), 0, 0)
}

pub trait Binder: Send + Sync {
    fn bind(&self, grants: Grants) -> Result<Arc<dyn Bound>, Vec<Diagnostic>>;

    fn hosting(&self, grants: &Grants, w: &mut Writer);

    fn moved(&self, grants: &Grants, trace: &str, caller: Option<&Recorder>) -> Option<String>;
}

pub trait Bound: Send + Sync {
    fn binding(&self) -> Arc<HostBinding>;

    fn runtime_factory(&self) -> RuntimeFactory;

    fn requested_exit(&self) -> Option<i32>;

    fn traced(&self, recorder: &Recorder, host: bool) -> Option<String>;

    fn unbind(&self) -> Vec<(String, Vec<u8>)>;
}

pub struct Watch<'a> {
    pub by: &'a dyn Bound,
    pub caller: Option<Arc<Recorder>>,
    pub recorded: bool,
}

#[derive(Clone, Debug)]
pub struct Run {
    pub timeout_ms: u64,
    pub seed: Option<(Seed, u32)>,
    pub re_executed: bool,
    pub measure: bool,
    pub tested: bool,
    pub label: String,
}

impl Run {
    pub fn unbounded() -> Run {
        Run {
            timeout_ms: 0,
            seed: None,
            re_executed: false,
            measure: false,
            tested: false,
            label: String::new(),
        }
    }
}

pub enum Ended {
    Returned(Vec<u8>),
    Failed(Diagnostic),
    Panicked(String),
    NoRoot,
    NotReturnable(&'static str),
}

#[derive(Clone, Debug, Default)]
pub struct Usage {
    pub nanos: u64,
    pub host: bool,
    pub performs: u64,
    pub retained: Retained,
    pub runtime_blocks: u64,
    pub live: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Retained {
    pub native_objects: u64,
    pub scheduled: u64,
    pub task_stacks: u64,
    pub frame_tables: u64,
}

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
    let _named = (!run.label.is_empty()).then(|| RunningTest::named(&run.label));
    let mut context = Box::new(opened.native.context());
    let own = (watch.as_ref())
        .filter(|w| w.recorded)
        .map(|_| Arc::new(Recorder::default()));
    let recorder = own
        .clone()
        .or_else(|| watch.as_ref().and_then(|w| w.caller.clone()));
    if let Some(bound) = bound {
        let made = bound.runtime_factory();
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
        context.set_host(bound.binding(), Some(runtime), Some(factory));
    }
    context.re_executed = run.re_executed;
    context.trace_performs(run.tested);
    let (seed, steps) = run
        .seed
        .clone()
        .unwrap_or_else(|| (Seed::default(), ply_core::sim::DEFAULT_STEPS));
    context.seed = seed;
    context.sim_steps = steps.max(1);
    let started = Instant::now();
    let entered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        entered(&mut context, entry, arguments, run, root)
    }));
    let nanos = if run.measure {
        elapsed_nanos(started)
    } else {
        0
    };
    let mut outcome = match entered {
        Ok((ended, last)) => Outcome {
            ended,
            usage: Usage {
                nanos,
                host: context.host_use.used_nondeterminism(),
                performs: last.performs,
                retained: last.held,
                runtime_blocks: last.runtime_blocks,
                live: last.live,
            },
            warnings: std::mem::take(&mut context.teardown),
            interleaving: last.interleaving,
            trace: None,
        },
        Err(payload) => {
            abandoned(context);
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

pub fn run_to_result(
    opened: &Opened,
    root: &str,
    arguments: Vec<u8>,
    timeout_ms: u64,
) -> Result<Vec<u8>, String> {
    let run = Run {
        timeout_ms,
        ..Run::unbounded()
    };
    on_program_thread(
        || match enter(opened, root, arguments, &run, None, None).ended {
            Ended::Returned(bytes) => Ok(bytes),
            Ended::Failed(diagnostic) => Err(diagnostic.to_string()),
            Ended::Panicked(said) => Err(said),
            Ended::NoRoot => Err(format!("the unit is entered at no root `{root}`")),
            Ended::NotReturnable(reason) => Err(reason.to_string()),
        },
    )
    .unwrap_or_else(|| Err(format!("`{root}` could not be given a thread")))
}

const HEARTBEAT: Duration = Duration::from_secs(30);

struct RunningTest {
    token: u64,
}

fn running_tests() -> &'static Mutex<HashMap<u64, (String, Instant)>> {
    static RUNNING: OnceLock<Mutex<HashMap<u64, (String, Instant)>>> = OnceLock::new();
    static STARTED: Once = Once::new();
    STARTED.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("ply-heartbeat".into())
            .spawn(heartbeat);
    });
    RUNNING.get_or_init(|| Mutex::new(HashMap::new()))
}

fn heartbeat() {
    loop {
        std::thread::sleep(HEARTBEAT);
        let running = running_tests()
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let tests: Vec<(String, Instant)> = running.values().cloned().collect();
        drop(running);
        for line in overdue_lines(&tests, Instant::now(), HEARTBEAT) {
            let _ = writeln!(std::io::stderr(), "{line}");
        }
    }
}

pub fn overdue_lines(tests: &[(String, Instant)], now: Instant, after: Duration) -> Vec<String> {
    let mut overdue: Vec<(Duration, &str)> = tests
        .iter()
        .map(|(name, started)| (now.saturating_duration_since(*started), name.as_str()))
        .filter(|(ran, _)| *ran >= after)
        .collect();
    overdue.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
    overdue
        .into_iter()
        .map(|(ran, name)| format!("   running for {}s: {name}", ran.as_secs()))
        .collect()
}

impl RunningTest {
    fn named(name: &str) -> RunningTest {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let token = NEXT.fetch_add(1, Ordering::Relaxed);
        running_tests()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(token, (name.to_string(), Instant::now()));
        RunningTest { token }
    }
}

impl Drop for RunningTest {
    fn drop(&mut self) {
        running_tests()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.token);
    }
}

pub fn on_program_thread<T: Send>(enter: impl FnOnce() -> T + Send) -> Option<T> {
    let branch = crate::parallel::thread_branch();
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(crate::rt::PROGRAM_STACK)
            .spawn_scoped(scope, move || {
                crate::parallel::inherit_branch(branch);
                enter()
            })
            .ok()?
            .join()
            .ok()
    })
}

struct Last {
    performs: u64,
    held: Retained,
    interleaving: Option<Vec<u8>>,
    runtime_blocks: u64,
    live: u64,
}

fn abandoned(mut context: Box<Context>) {
    if let Some(rt) = context.runtime.clone() {
        let _ = rt.end_entry_point(context.id);
    }
    if std::mem::take(&mut context.pooled) {
        crate::parallel::workers_ended(context.id.0);
    }
    std::mem::forget(context);
}

struct RestoreThreadOnDrop(*mut Context);

impl Drop for RestoreThreadOnDrop {
    fn drop(&mut self) {
        unsafe { (*self.0).restore_thread_state() };
    }
}

fn entered(
    context: &mut Context,
    entry: crate::rt::Entry,
    arguments: Vec<u8>,
    run: &Run,
    root: &str,
) -> (Ended, Last) {
    context.begin_with_time_limit(run.timeout_ms);
    let giving_back = RestoreThreadOnDrop(std::ptr::from_mut(context));
    context.open_entry_call(arguments);
    let entering = std::ptr::from_mut(context);
    let none: [i64; 0] = [];
    crate::rt::on_known_stack(entering, || unsafe { entry(entering, none.as_ptr()) });
    if context.failed == crate::rt::FAILED_OUT_OF_STACK {
        crate::rt::release_unused_stack_pages();
    }
    let mut last = returned(context);
    let ended = if context.failed != 0 {
        failure(context)
    } else {
        ended_with_result(context, root)
    };
    if run.tested && matches!(ended, Ended::Returned(_)) {
        last.live = context.teardown_and_count_live() as u64;
    }
    last.runtime_blocks = context.heap.runtime_blocks() as u64;
    context.end();
    std::mem::forget(giving_back);
    (ended, last)
}

fn returned(context: &mut Context) -> Last {
    let held = context.retained_at_root.take().unwrap_or(Retained {
        native_objects: context.heap.natives() as u64,
        scheduled: 0,
        task_stacks: 0,
        frame_tables: context.stack_slots() as u64,
    });
    if let Some(rt) = context.runtime.clone() {
        let warned = rt.end_entry_point(context.id);
        context.teardown.extend(warned);
    }
    Last {
        performs: context.performed.len() as u64,
        held,
        interleaving: context.interleaving.take(),
        runtime_blocks: 0,
        live: 0,
    }
}

fn ended_with_result(context: &mut Context, root: &str) -> Ended {
    match crate::rt::close_entry_call(context) {
        Ok(EntryBytes::Bytes(bytes)) => Ended::Returned(bytes),
        Ok(EntryBytes::Function) => {
            Ended::NotReturnable("its result holds a function the program wrote")
        }
        Ok(EntryBytes::EscapingHandle(escapee)) => {
            let boundary = ply_core::Boundary::ReturnedFromEntry { name: root };
            Ended::Failed(ply_core::escape::rejected(&boundary, &escapee, Span::DUMMY))
        }
        Ok(EntryBytes::RetainsHandle) => {
            Ended::NotReturnable("its result holds a handle of its run")
        }
        Err(reason) => Ended::Failed(crate::rt::err_marshalling(&reason)),
    }
}

fn failure(context: &mut Context) -> Ended {
    let failed = context
        .take_failure()
        .unwrap_or_else(|| match context.failed {
            FAILED_UNWOUND_TO_HANDLER => Diagnostic::error(
                codes::RUNTIME_ERROR,
                "a `handle` clause unwound past the compiled fragment's entry",
            ),
            FAILED => Diagnostic::error(
                codes::INTERNAL_ERROR,
                "the compiled body failed without recording why",
            )
            .primary(context.site(), "the last site it reached"),
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

static INSTALLED_BINDER: OnceLock<Box<dyn Binder>> = OnceLock::new();

pub fn install(binder: Box<dyn Binder>) {
    let _ = INSTALLED_BINDER.set(binder);
}

pub fn natives() -> Vec<crate::c::MachineNative> {
    use crate::c::runtime_native;
    vec![
        runtime_native(
            "unit_open",
            rt_unit_open as unsafe extern "C" fn(_, _, _) -> _,
        ),
        runtime_native("unit_close", rt_unit_close as unsafe extern "C" fn(_, _)),
        runtime_native(
            "unit_report",
            rt_unit_report as unsafe extern "C" fn(_, _, _, _),
        ),
        runtime_native(
            "unit_bind",
            rt_unit_bind as unsafe extern "C" fn(_, _, _) -> _,
        ),
        runtime_native(
            "unit_unbind",
            rt_unit_unbind as unsafe extern "C" fn(_, _) -> _,
        ),
        runtime_native(
            "unit_hosting",
            rt_unit_hosting as unsafe extern "C" fn(_, _, _) -> _,
        ),
        runtime_native(
            "unit_moved",
            rt_unit_moved as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        runtime_native(
            "unit_enter",
            rt_unit_enter as unsafe extern "C" fn(_, _, _, _, _, _, _, _) -> _,
        ),
    ]
}

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

unsafe extern "C" fn rt_unit_open(context: *mut Context, p: Address, n: I64) -> TakenBytes {
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
    unsafe { &mut *context }.given(w.finish())
}

unsafe extern "C" fn rt_unit_close(_context: *mut Context, unit: I64) {
    units().by.remove(&unit.0);
}

unsafe extern "C" fn rt_unit_report(_context: *mut Context, p: Address, n: I64, to_stderr: I64) {
    let line = String::from_utf8_lossy(unsafe { borrowed(p, n) });
    let _ = if to_stderr.0 != 0 {
        writeln!(std::io::stderr(), "{line}")
    } else {
        writeln!(std::io::stdout(), "{line}")
    };
}

unsafe extern "C" fn rt_unit_bind(context: *mut Context, p: Address, n: I64) -> TakenBytes {
    let c = unsafe { &mut *context };
    let Some(grants) = read_or_fail(c, unsafe { borrowed(p, n) }, read_grants) else {
        return TakenBytes(0);
    };
    if !allowed(c, "machine.bind") {
        return TakenBytes(0);
    }
    let made = match INSTALLED_BINDER.get() {
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

unsafe extern "C" fn rt_unit_unbind(context: *mut Context, binding: I64) -> TakenBytes {
    let bound = bindings().by.remove(&binding.0);
    let reports = bound.map(|b| b.unbind()).unwrap_or_default();
    let mut w = Writer::new();
    w.list(reports.iter(), |w, (name, body)| {
        w.str(name).bytes(body);
    });
    unsafe { &mut *context }.given(w.finish())
}

unsafe extern "C" fn rt_unit_hosting(context: *mut Context, p: Address, n: I64) -> TakenBytes {
    let c = unsafe { &mut *context };
    let Some(grants) = read_or_fail(c, unsafe { borrowed(p, n) }, read_grants) else {
        return TakenBytes(0);
    };
    let Some(binder) = INSTALLED_BINDER.get() else {
        return TakenBytes(c.fail(err_no_binder()));
    };
    let mut w = Writer::new();
    binder.hosting(&grants, &mut w);
    c.given(w.finish())
}

unsafe extern "C" fn rt_unit_moved(
    context: *mut Context,
    gp: Address,
    gn: I64,
    tp: Address,
    tn: I64,
) -> TakenBytes {
    let c = unsafe { &mut *context };
    let Some(grants) = read_or_fail(c, unsafe { borrowed(gp, gn) }, read_grants) else {
        return TakenBytes(0);
    };
    if !allowed(c, "machine.moved") {
        return TakenBytes(0);
    }
    let Some(binder) = INSTALLED_BINDER.get() else {
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

#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn rt_unit_enter(
    context: *mut Context,
    unit: I64,
    root_pointer: Address,
    root_n: I64,
    arguments_pointer: Address,
    arguments_length: I64,
    run_pointer: Address,
    run_length: I64,
) -> TakenBytes {
    let c = unsafe { &mut *context };
    let Some(asked) = read_or_fail(
        c,
        unsafe { borrowed(run_pointer, run_length) },
        read_run_options,
    ) else {
        return TakenBytes(0);
    };
    if (asked.run.timeout_ms > 0 || asked.run.measure) && !allowed(c, "machine.enter") {
        return TakenBytes(0);
    }
    let root = String::from_utf8_lossy(unsafe { borrowed(root_pointer, root_n) }).into_owned();
    let arguments = unsafe { borrowed(arguments_pointer, arguments_length) }.to_vec();
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

fn allowed(c: &mut Context, op: &str) -> bool {
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
    c: &mut Context,
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

pub fn read_grants(r: &mut Reader<'_>) -> Result<Grants, String> {
    Ok(Grants {
        entries: r.list(|r| Ok((r.str()?, r.list(Reader::str)?)))?,
    })
}

struct EntryRequest {
    binding: i64,
    hosted: bool,
    observe: bool,
    run: Run,
}

fn read_run_options(r: &mut Reader<'_>) -> Result<EntryRequest, String> {
    let binding = r.int()?;
    let hosted = r.bool()?;
    let timeout_ms = r.count()?;
    let seed = r.option(read_seed)?;
    let re_executed = r.bool()?;
    let observe = r.bool()?;
    let measure = r.bool()?;
    let tested = r.bool()?;
    let label = r.str()?;
    Ok(EntryRequest {
        binding,
        hosted,
        observe,
        run: Run {
            timeout_ms,
            seed,
            re_executed,
            measure,
            tested,
            label,
        },
    })
}

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
        .count(u.retained.native_objects)
        .count(u.retained.scheduled)
        .count(u.retained.task_stacks)
        .count(u.retained.frame_tables)
        .count(u.runtime_blocks)
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
