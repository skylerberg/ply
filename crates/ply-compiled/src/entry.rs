//! Entering a compiled unit: the unit opened once from its library and shared by every thread that
//! enters it, a root entered on a fresh context of its own through its `R#entered`, its arguments
//! the PLV1 list the root reads, and how the entry ended, its answer the PLV1 the root wrote,
//! untouched. No front end is read: which root to enter, what its arguments are and what its answer
//! or failure means are the entering program's.
//!
//! A unit opened here has no source, so a span the runtime makes is a site: its [`SourceId`] is the
//! index of its root in the unit's roots, which [`Opened::root_named`] names, and its offsets are
//! from the start of that root's definition.

use crate::c::Native;
use crate::rt::{Ctx, EntryBytes, FAILED, FAILED_OUT_OF_FUEL, FAILED_UNWIND};
use ply_core::host::{HostBinding, HostRuntime, RuntimeFactory};
use ply_core::{Diagnostic, Footprint, Held, Seed, SourceId, Span, Symbol, codes};
use std::rc::Rc;
use std::sync::Arc;
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
        let (native, _) = crate::c::load_opened(library, None).map_err(|e| format!("{e:#}"))?;
        Ok(Arc::new(Opened {
            native,
            nanos: elapsed_nanos(started),
        }))
    }

    /// The program this executable was linked with.
    pub fn linked() -> Result<Arc<Opened>, String> {
        let started = Instant::now();
        let (native, _) = crate::c::load_linked(None).map_err(|e| format!("{e:#}"))?;
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

/// The arguments of a root that takes none: the PLV1 of the empty list.
pub fn no_arguments() -> Vec<u8> {
    ply_core::codec::encode(&ply_core::Value::list(Vec::new()))
        .expect("the empty list is written whole")
}

/// The span a unit with no source gives the root at `at` among its roots: a site of that root.
pub(crate) fn site_span(at: usize) -> Span {
    Span::new(SourceId(u32::try_from(at).unwrap_or(u32::MAX - 1)), 0, 0)
}

/// What a run is lent, as named entries, each a list of fields: the runtime's own handlers read the
/// ones they serve (`fs NAME PATH`, `exec NAME PATH`, `allow FAMILY`, `argv ARG..`, ...), and every
/// entry is kept for the real handlers a program declares.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grants {
    pub entries: Vec<(String, Vec<String>)>,
}

impl Grants {
    /// The fields of each entry named `name`, in order.
    pub fn named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a [String]> + 'a {
        (self.entries.iter())
            .filter(move |(n, _)| n == name)
            .map(|(_, fields)| fields.as_slice())
    }

    /// Whether an entry is named `name`.
    pub fn has(&self, name: &str) -> bool {
        self.named(name).next().is_some()
    }

    /// The grants with one entry more.
    pub fn with(mut self, name: &str, fields: &[&str]) -> Grants {
        self.entries.push((
            name.to_string(),
            fields.iter().map(|f| f.to_string()).collect(),
        ));
        self
    }

    /// Whether `allow` names the family `family`.
    pub fn allows(&self, family: &str) -> bool {
        self.named("allow")
            .any(|fields| fields.first().is_some_and(|f| f == family))
    }
}

/// The host binding an entry runs under: what serves the operations its program performs that no
/// handler of its own answers.
pub trait Bound: Send + Sync {
    fn binding(&self) -> Arc<HostBinding>;

    /// What makes the reactor a pending host answer is waited on with, on the thread that enters.
    fn runtime_factory(&self) -> Option<RuntimeFactory>;
}

/// How an entry is bounded and what it is run with.
#[derive(Clone, Debug)]
pub struct Run {
    /// Calls the entry may make; 0 is no bound.
    pub steps: i64,
    /// Milliseconds of wall clock the entry may take; 0 is no clock.
    pub timeout_ms: u64,
    /// How deeply its calls may nest.
    pub depth: i64,
    /// The schedule its `simulate` regions take and the steps each interleaving may take; the
    /// default seed where none is named.
    pub seed: Option<(Seed, u32)>,
    /// One of several runs of one test a search makes: reaching the host is refused.
    pub re_executed: bool,
    /// What the root may perform, which a host answer outside is refused against.
    pub declared: Option<Footprint>,
    /// Whether to read the wall clock for how long the entry took.
    pub measure: bool,
}

impl Run {
    /// A run bounded by nothing but how deeply its calls nest, as a program's own entry is.
    pub fn unbounded(depth: i64) -> Run {
        Run {
            steps: 0,
            timeout_ms: 0,
            depth,
            seed: None,
            re_executed: false,
            declared: None,
            measure: false,
        }
    }
}

/// How an entry ended.
pub enum Ended {
    /// The root answered: its answer's PLV1.
    Answered(Vec<u8>),
    /// A raise nothing answered, a runtime error, a budget spent, or an answer the boundary refuses.
    Failed(Diagnostic),
    /// A Rust unwind out of the entry, with what it said.
    Panicked(String),
    /// The unit is entered at no such root.
    NoRoot,
    /// The answer holds what no run but its own can hold, which is why.
    Unanswerable(&'static str),
}

/// What an entry cost and reached, whatever it came to.
#[derive(Clone, Debug, Default)]
pub struct Usage {
    /// Wall clock, where the run was measured.
    pub nanos: u64,
    /// Whether it reached a nondeterministic host handler.
    pub host: bool,
    /// Calls the entry made.
    pub steps: u64,
    /// The operations it performed, handled ones included.
    pub performs: u64,
    pub held: Held,
    pub updates: u64,
    pub updates_in_place: u64,
}

/// How an entry ended, what it used, what its host runtime warned of as it ended, and what its
/// `simulate` regions did.
pub struct Outcome {
    pub ended: Ended,
    pub usage: Usage,
    pub warnings: Vec<Diagnostic>,
    pub record: Option<ply_core::region::Record>,
}

impl Outcome {
    fn of(ended: Ended) -> Outcome {
        Outcome {
            ended,
            usage: Usage::default(),
            warnings: Vec::new(),
            record: None,
        }
    }
}

/// `root` of `opened` entered on a fresh context on this thread, its arguments `arguments`, the
/// PLV1 list its `R#entered` reads, under `bound` where it names one and else no host. A Rust
/// unwind out of the entry is answered as [`Ended::Panicked`], the thread as the entry found it.
pub fn enter(
    opened: &Opened,
    root: &str,
    arguments: Vec<u8>,
    run: &Run,
    bound: Option<&dyn Bound>,
) -> Outcome {
    let Some(entry) = opened.native.entered(root) else {
        return Outcome::of(Ended::NoRoot);
    };
    let mut ctx = Box::new(opened.native.context());
    let binding = match bound {
        Some(bound) => {
            let factory = bound.runtime_factory();
            let runtime: Option<Rc<dyn HostRuntime>> = factory.as_ref().map(|make| make());
            ctx.set_host(bound.binding(), runtime, factory);
            Some(bound.binding())
        }
        None => None,
    };
    ctx.declared = run.declared.clone();
    ctx.re_executed = run.re_executed;
    let (seed, steps) = run
        .seed
        .clone()
        .unwrap_or_else(|| (Seed::default(), ply_core::sim::DEFAULT_STEPS));
    ctx.seed = seed;
    ctx.sim_steps = steps.max(1);
    let id = ctx.id;
    ply_core::rc::reset();
    let started = Instant::now();
    let entered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        entered(&mut ctx, entry, arguments, run, root)
    }));
    let nanos = if run.measure {
        elapsed_nanos(started)
    } else {
        0
    };
    if let Some(binding) = &binding {
        binding.machine_ended(id);
    }
    match entered {
        Ok((ended, last)) => {
            let counters = ply_core::rc::stats();
            Outcome {
                ended,
                usage: Usage {
                    nanos,
                    host: ctx.host_use.acted(),
                    steps: last.steps,
                    performs: last.performs,
                    held: last.held,
                    updates: counters.updates,
                    updates_in_place: counters.updates_in_place,
                },
                warnings: std::mem::take(&mut ctx.teardown),
                record: last.record,
            }
        }
        Err(payload) => Outcome::of(Ended::Panicked(panic_message(payload.as_ref()))),
    }
}

/// What an entry leaves for its outcome once its root returned.
struct Last {
    steps: u64,
    performs: u64,
    held: Held,
    record: Option<ply_core::region::Record>,
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
    ctx.begin_bounded(run.depth, run.steps, run.timeout_ms);
    let giving_back = GivingBack(std::ptr::from_mut(ctx));
    ctx.open_entry(arguments, Vec::new());
    let entering = std::ptr::from_mut(ctx);
    let none: [i64; 0] = [];
    // SAFETY: the unit owns the entry's pages and outlives the call, `ctx` is this entry's alone,
    // and an `R#entered` takes no words.
    crate::rt::on_known_stack(entering, || unsafe { entry(entering, none.as_ptr()) });
    let last = returned(ctx);
    let ended = if ctx.failed != 0 {
        failure(ctx, run.depth)
    } else {
        answered(ctx, root)
    };
    ctx.end();
    std::mem::forget(giving_back);
    (ended, last)
}

/// What an entry leaves once its root returned: its production region's other tasks run to their
/// ends first, and its host runtime told the entry is over.
fn returned(ctx: &mut Ctx) -> Last {
    let production = ctx.sims.last().filter(|sim| sim.is_production());
    let held = Held {
        bridges: ctx.heap.bridges() as u64,
        scheduled: production.map_or(0, |sim| sim.scheduled() as u64),
        task_stacks: production.map_or(0, |sim| sim.task_slots() as u64),
        frame_tables: ctx.stack_slots() as u64,
    };
    if ctx.sims.last().is_some_and(|sim| sim.is_production()) {
        unsafe { crate::simulate::finish_root(std::ptr::from_mut(ctx)) };
    }
    if let Some(rt) = ctx.runtime.clone() {
        let warned = rt.end_entry_point(ctx.id);
        ctx.teardown.extend(warned);
    }
    Last {
        steps: u64::try_from(ctx.ticks).unwrap_or(0),
        performs: ctx.performed.len() as u64,
        held,
        record: ctx.record.take(),
    }
}

/// How an entry whose root returned ended: its answer's bytes, or why the boundary keeps it.
fn answered(ctx: &mut Ctx, root: &str) -> Ended {
    match crate::rt::close_entry_bytes(ctx) {
        Ok(EntryBytes::Bytes(bytes)) => Ended::Answered(bytes),
        Ok(EntryBytes::Function) => {
            Ended::Unanswerable("its answer holds a function the program wrote")
        }
        Ok(EntryBytes::Holding(value)) => {
            let boundary = ply_core::Boundary::EntryAnswer { name: root };
            match ply_core::escape::check(&boundary, &value, Span::DUMMY) {
                Err(refused) => Ended::Failed(refused),
                Ok(()) => Ended::Unanswerable("its answer holds a handle of its run"),
            }
        }
        Err(why) => Ended::Failed(crate::rt::err_crossing(&why)),
    }
}

/// How an entry that failed ended.
fn failure(ctx: &mut Ctx, depth: i64) -> Ended {
    let failed = if ctx.failed == FAILED_OUT_OF_FUEL {
        Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("recursion limit of {depth} nested calls exceeded"),
        )
        .primary(ctx.site(), "the call that overran it")
    } else {
        ctx.take_failure().unwrap_or_else(|| match ctx.failed {
            FAILED_UNWIND => Diagnostic::error(
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
        })
    };
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
