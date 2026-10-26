//! The trusted computing base, as one list.

// A line for each family, here and wherever else they are listed, in alphabetical order: two
// families added at once then touch different lines.
use crate::clock;
use crate::config;
use crate::fs;
use crate::pool::{Bell, Run, err_nothing_outstanding};
use crate::process;
use crate::signal::{self, Accepting, Shutdown};
use crate::sqlite;
use crate::tcp;
use crate::time;
use crate::trace;
use ply_core::host::{
    HostOp, HostRegistry, HostRuntime, MachineId, Pending, RuntimeFactory, ShutdownReport,
};
use ply_core::{Diagnostic, Span, Symbol, TaskId, Value, codes};
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

pub struct Host {
    /// Rung as an answer is filed with any run of this host, and by a stop, so a park waits on
    /// every pool and the stop at once.
    bell: Arc<Bell>,
    /// The run's configuration, read once before this `Host` existed and immutable thereafter.
    config: Arc<config::Snapshot>,
    /// The roots `--fs NAME=PATH` bound, and the pool their operations wait on; empty if none.
    fs: Arc<fs::FsHost>,
    net: Arc<tcp::TcpHost>,
    /// The arguments and streams `ply run` was given; `None` withholds `process`.
    process: Option<Arc<process::ProcessHost>>,
    /// The stop flag and the phase machine, when this run listens for a signal.
    shutdown: Option<Arc<Shutdown>>,
    /// The database connections open under `fs`'s roots.
    sqlite: Arc<sqlite::SqliteHost>,
    /// The run's clocks: what `std.time` and the language's `clock` read, and what a production
    /// region's sleeps are deadlines on.
    time: Arc<time::TimeHost>,
    trace: Arc<trace::Trace>,
}

impl Default for Host {
    fn default() -> Host {
        Host::new()
    }
}

impl Host {
    pub fn new() -> Host {
        Host::with_credentials(crate::tls::Credentials::empty())
    }

    pub fn with_credentials(credentials: crate::tls::Credentials) -> Host {
        Host {
            bell: Arc::new(Bell::default()),
            config: Arc::new(config::Snapshot::unopened()),
            fs: Arc::new(fs::FsHost::new(fs::Roots::new())),
            net: Arc::new(tcp::TcpHost::with_credentials(credentials)),
            process: None,
            shutdown: None,
            sqlite: Arc::new(sqlite::SqliteHost::new()),
            time: Arc::new(time::TimeHost::new()),
            trace: Arc::new(trace::Trace::default()),
        }
    }

    pub fn rooted(self, roots: fs::Roots) -> Host {
        self.net.rooted(roots.clone());
        Host {
            fs: Arc::new(fs::FsHost::new(roots)),
            ..self
        }
    }

    pub fn roots(&self) -> &fs::Roots {
        self.fs.roots()
    }

    pub fn with_process(self, process: process::ProcessHost) -> Host {
        if let Some(shutdown) = &self.shutdown {
            shutdown.attach_children(process.children());
        }
        Host {
            process: Some(Arc::new(process)),
            ..self
        }
    }

    pub fn process(&self) -> Option<&Arc<process::ProcessHost>> {
        self.process.as_ref()
    }

    pub fn traced(self, trace: Arc<trace::Trace>) -> Host {
        Host { trace, ..self }
    }

    pub fn tracing(&self) -> &Arc<trace::Trace> {
        &self.trace
    }

    pub fn configured(self, config: Arc<config::Snapshot>) -> Host {
        Host { config, ..self }
    }

    pub fn configuration(&self) -> &Arc<config::Snapshot> {
        &self.config
    }

    pub fn handshakes(&self) -> crate::tls::HandshakeCounts {
        self.net.handshakes()
    }

    pub fn credentials(&self) -> &crate::tls::Credentials {
        self.net.credentials()
    }

    /// One per machine, each a run of its own: the pools are shared, but every operation is the
    /// run's that submitted it.
    pub fn runtime(&self) -> Rc<dyn HostRuntime> {
        Rc::new(Facilities {
            run: Run::new(Arc::clone(&self.bell)),
            config: Arc::clone(&self.config),
            fs: Arc::clone(&self.fs),
            net: Arc::clone(&self.net),
            process: self.process.clone(),
            trace: Arc::clone(&self.trace),
            shutdown: self.shutdown.clone(),
            sqlite: Arc::clone(&self.sqlite),
            time: Arc::clone(&self.time),
        })
    }

    pub fn stop(&self) -> Option<&Arc<Shutdown>> {
        self.shutdown.as_ref()
    }

    pub fn stopping_on(self, shutdown: Arc<Shutdown>) -> Host {
        shutdown.attach_net(Arc::clone(&self.net) as Arc<dyn signal::Accepting>);
        shutdown.attach_bell(&self.bell);
        if let Some(process) = &self.process {
            shutdown.attach_children(process.children());
        }
        Host {
            shutdown: Some(shutdown),
            ..self
        }
    }

    pub fn shutdown(&self) -> Option<&Arc<Shutdown>> {
        self.shutdown.as_ref()
    }
}

/// Every operation the runtime serves itself: the language's `clock`.
pub fn registry() -> HostRegistry {
    let mut registry = HostRegistry::new();
    clock::register(&mut registry);
    registry
}

/// What the runtime answers `op`, one of [`registry`]'s, performed with `args` under `runtime`.
///
/// # Errors
/// The clock refused, or `op` is none of [`registry`]'s.
pub fn answer(
    op: &HostOp,
    runtime: &dyn HostRuntime,
    args: &[Value],
    span: Span,
) -> Result<Value, Diagnostic> {
    match clock::Op::of(op) {
        Some(clock) => clock::answer(clock, runtime, args, span),
        None => Err(err_unserved(op, span)),
    }
}

#[cold]
fn err_unserved(op: &HostOp, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!(
            "`{}.{}` reached the host boundary, which answers only the language's `clock`",
            op.effect, op.op
        ),
    )
    .primary(span, "performed here")
    .note("this is a defect in Ply's host dispatch rather than in the program")
}

/// A runtime factory for a run no host was bound for: every runtime it makes, a `parallel`
/// branch's among them, is over one host of its own, made when the first runtime is.
pub fn unbound() -> RuntimeFactory {
    let host: Arc<OnceLock<Host>> = Arc::default();
    Arc::new(move || host.get_or_init(Host::new).runtime())
}

/// The facilities `runtime` lends the natives, where a host made it.
pub fn facilities_of(runtime: &dyn HostRuntime) -> Option<&Facilities> {
    runtime.facilities()?.downcast_ref()
}

/// The bytes a native's job answered `pending`, the thread waiting for them until the drain
/// deadline says the run is out of time. A wait begun past the deadline ends the run however soon
/// its job would answer, so a loop of quick waits still stops there.
///
/// # Errors
/// The job failed, the drain ran out, or no host made `runtime`.
pub fn job_bytes(runtime: &dyn HostRuntime, pending: &Pending) -> Result<Vec<u8>, Diagnostic> {
    let facilities = facilities_of(runtime).ok_or_else(err_no_host)?;
    let step = facilities.shutdown.as_ref().map(|_| signal::DRAIN_POLL);
    loop {
        if let Some(expired) = facilities.drain_expired() {
            return Err(expired);
        }
        if let Some(bytes) = facilities.run.block_on(pending, step)? {
            return Ok(bytes);
        }
    }
}

/// The run `runtime` is, which an operation it asks for is submitted for.
///
/// # Errors
/// No host made the runtime, so it holds no run.
pub fn run_of(runtime: &dyn HostRuntime) -> Result<&Arc<Run>, Diagnostic> {
    facilities_of(runtime)
        .map(Facilities::run)
        .ok_or_else(err_no_host)
}

#[cold]
fn err_no_host() -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        "a host operation was asked to wait under a runtime no host made",
    )
    .note("this is Ply's fault: report it with the program that produced it")
}

/// The runtime: one run's operations, and what it lends the natives.
pub struct Facilities {
    run: Arc<Run>,
    /// The run's configuration, which `config`'s natives read.
    config: Arc<config::Snapshot>,
    /// The roots `--fs` bound, which a database lies under.
    fs: Arc<fs::FsHost>,
    /// The sockets, for the connections a drain that ran out of time abandoned.
    net: Arc<tcp::TcpHost>,
    /// The run's own process, for its children and its streams at the end.
    process: Option<Arc<process::ProcessHost>>,
    trace: Arc<trace::Trace>,
    shutdown: Option<Arc<Shutdown>>,
    /// The database connections, for those an entry point left open.
    sqlite: Arc<sqlite::SqliteHost>,
    time: Arc<time::TimeHost>,
}

impl Facilities {
    /// The run's configuration, read once before the run began.
    pub fn configuration(&self) -> &config::Snapshot {
        &self.config
    }

    /// The run's sockets, `std.net`'s and `std.udp`'s alike.
    pub fn sockets(&self) -> &Arc<tcp::TcpHost> {
        &self.net
    }

    /// The run's operations, which every operation it asks for is submitted for.
    pub fn run(&self) -> &Arc<Run> {
        &self.run
    }

    /// The roots `--fs` bound, each by its label.
    pub fn roots(&self) -> &fs::Roots {
        self.fs.roots()
    }

    /// The files under the roots, and the locks and descriptors the run holds of them.
    pub fn files(&self) -> &Arc<fs::FsHost> {
        &self.fs
    }

    /// The database connections the run holds open.
    pub fn databases(&self) -> &Arc<sqlite::SqliteHost> {
        &self.sqlite
    }

    /// The run's own process and the programs it may start, where it was lent either.
    pub fn process(&self) -> Option<&Arc<process::ProcessHost>> {
        self.process.as_ref()
    }

    /// The stop the run listens for, where it listens for one.
    pub fn stop(&self) -> Option<&Arc<Shutdown>> {
        self.shutdown.as_ref()
    }

    /// The run's trace and the sink it writes to.
    pub fn tracing(&self) -> &Arc<trace::Trace> {
        &self.trace
    }
}

impl HostRuntime for Facilities {
    fn facilities(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    /// Ends at a stop too, which an idle service sees no other way.
    fn park(&self) -> Result<(), Diagnostic> {
        let stopped = self.stopping();
        let stop = || self.stopping() != stopped;
        // A drain parks in bounded steps, so its deadline is seen.
        if stopped {
            let bound = signal::DRAIN_POLL;
            self.run.park(Some(bound), &stop);
            return Ok(());
        }
        if self.run.park(None, &stop) {
            Ok(())
        } else {
            Err(err_nothing_outstanding())
        }
    }

    fn now(&self) -> Result<i64, Diagnostic> {
        Ok(self.time.elapsed_ns())
    }

    fn park_until(&self, deadline: i64) -> Result<(), Diagnostic> {
        let Ok(left) = u64::try_from(deadline.saturating_sub(self.time.elapsed_ns())) else {
            return Ok(());
        };
        let mut bound = std::time::Duration::from_nanos(left);
        let stopped = self.stopping();
        // A drain parks in bounded steps, so its deadline is seen while every task sleeps.
        if stopped {
            bound = bound.min(signal::DRAIN_POLL);
        }
        self.run.park(Some(bound), &|| self.stopping() != stopped);
        Ok(())
    }

    fn stopping(&self) -> bool {
        self.shutdown.as_ref().is_some_and(|s| s.stopping())
    }

    fn drain_expired(&self) -> Option<Diagnostic> {
        let shutdown = self.shutdown.as_ref()?;
        if !shutdown.drain_expired() {
            return None;
        }
        Some(err_drain_incomplete(
            shutdown,
            self.net.connections_in_flight(),
        ))
    }

    /// The run's own teardown, in a pinned order: the drain deadline governs scheduling and the
    /// socket pool, so nothing here waits on it.
    fn shutdown(&self) -> ShutdownReport {
        // However the run ended, it leaves no child running.
        if let Some(process) = &self.process {
            process.end_children();
        }
        // The sink flushes before the run's own state is gone.
        self.trace.flush();
        // `term.mode` serves every run, whatever process it was lent.
        crate::stdio::restore_terminal();
        // Last, since a buffer whose reader has gone ends the process here.
        if let Some(process) = &self.process {
            process.settle();
        }
        ShutdownReport {
            spans_left_open: usize::try_from(self.trace.left_open()).unwrap_or(usize::MAX),
        }
    }

    fn unanswered(&self, effect: &Symbol, op: &Symbol) {
        if let Some(process) = &self.process {
            process.unanswered(effect, op);
        }
    }

    /// Closes the spans this entry point left open, and warns of them, and the database
    /// connections it left open, and drops the secrets it read and did not take.
    fn end_entry_point(&self, machine: MachineId) -> Vec<Diagnostic> {
        self.run.drop_secrets();
        self.sqlite.end_machine(machine);
        self.trace.end_entry_point(machine).into_iter().collect()
    }

    /// Closes the spans the retired task left open.
    fn end_task(&self, machine: MachineId, task: TaskId) {
        self.trace.end_task(machine, task);
    }
}

#[cold]
#[inline(never)]
fn err_drain_incomplete(shutdown: &Shutdown, connections: usize) -> Diagnostic {
    let bounds = shutdown.bounds();
    let elapsed = shutdown.elapsed().unwrap_or_default();
    Diagnostic::warning(codes::DRAIN_INCOMPLETE, "the drain deadline expired")
        .primary(Span::DUMMY, "this run stopped scheduling here")
        .note(format!(
            "{connections} connection(s) abandoned with no response written"
        ))
        .note(format!(
            "the drain was {}ms and {}ms elapsed since the signal",
            bounds.drain.as_millis(),
            elapsed.as_millis()
        ))
        .note("raise `--drain-ms` above the program's own body_timeout_ms + write_timeout_ms")
        .note("Ply has no cancellation, so a request still running here is not unwound and is not handed a 503: its connection closes with no response")
}
