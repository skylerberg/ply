//! The trusted computing base, as one list.

// A line for each family, here and wherever else they are listed, in alphabetical order: two
// families added at once then touch different lines.
use crate::certgen;
use crate::clock;
use crate::config;
use crate::dns;
use crate::fs;
use crate::os;
use crate::password;
use crate::pool::{Bell, Inbox, Pool, Pooled};
use crate::process;
use crate::random;
use crate::sched;
use crate::signal::{self, Accepting, Shutdown};
use crate::sqlite;
use crate::tcp;
use crate::term;
use crate::time;
use crate::trace;
use crate::udp;
use ply_eval::host::{HostRegistry, HostRuntime, MachineId, Pending, ShutdownReport};
use ply_eval::{Diagnostic, Span, Symbol, TaskId, Value, codes};
use std::rc::Rc;
use std::sync::Arc;

pub struct Host {
    /// Rung by every pool below, so a park can wait on all of them at once.
    bell: Arc<Bell>,
    /// The run's configuration, read once before this `Host` existed and immutable thereafter.
    config: Arc<config::Snapshot>,
    /// The roots `--fs NAME=PATH` bound, and the pool their operations wait on; empty if none.
    fs: Arc<fs::FsHost>,
    net: Arc<tcp::TcpHost>,
    /// The pool `std.password`'s hashes are made on.
    password: Arc<password::PasswordHost>,
    /// The arguments and streams `ply run --host` was given; `None` withholds `process`.
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
            password: Arc::new(password::PasswordHost::new()),
            process: None,
            shutdown: None,
            sqlite: Arc::new(sqlite::SqliteHost::new()),
            time: Arc::new(time::TimeHost::new()),
            trace: Arc::new(trace::Trace::default()),
        }
    }

    /// Every facility whose operations wait on a pool: a runtime routes a token to these and to
    /// nothing else, and has each ring the bell.
    fn pools(&self) -> Vec<Arc<dyn Pooled>> {
        let mut pools: Vec<Arc<dyn Pooled>> = vec![
            // A line each, as the families above are listed.
            self.fs.clone(),
            self.net.clone(),
            self.password.clone(),
        ];
        if let Some(process) = &self.process {
            pools.push(process.clone());
        }
        pools
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

    pub fn registry(&self) -> HostRegistry {
        let mut registry = HostRegistry::new();
        certgen::register(&mut registry);
        clock::register(&mut registry, Arc::clone(&self.time));
        config::register(&mut registry, Arc::clone(&self.config));
        dns::register(&mut registry, Arc::clone(&self.net));
        // Registered whatever `--fs` said, so a run that bound no root gets `E0451`, not `E0424`.
        fs::register(&mut registry, Arc::clone(&self.fs));
        os::register(&mut registry);
        password::register(&mut registry, Arc::clone(&self.password));
        process::register(&mut registry, self.process.as_ref());
        random::register(&mut registry);
        sched::register(&mut registry);
        signal::register(&mut registry, self.shutdown.as_ref());
        // Its operations wait on the pool of the roots it is handed.
        sqlite::register(
            &mut registry,
            Arc::clone(&self.fs),
            Arc::clone(&self.sqlite),
        );
        tcp::register(&mut registry, Arc::clone(&self.net) as Arc<dyn tcp::Net>);
        term::register(&mut registry, self.process.as_ref());
        time::register(&mut registry, Arc::clone(&self.time));
        trace::register(&mut registry, Arc::clone(&self.trace));
        udp::register(&mut registry, Arc::clone(&self.net));
        registry
    }

    /// One per machine: the pools are shared, but a runtime collects only the tokens it watches.
    pub fn runtime(&self) -> Rc<dyn HostRuntime> {
        Rc::new(Facilities {
            pools: self
                .pools()
                .into_iter()
                .map(|facility| {
                    facility.pool().ring(&self.bell);
                    Watched {
                        facility,
                        inbox: Arc::default(),
                    }
                })
                .collect(),
            net: Arc::clone(&self.net),
            process: self.process.clone(),
            trace: Arc::clone(&self.trace),
            shutdown: self.shutdown.clone(),
            sqlite: Arc::clone(&self.sqlite),
            time: Arc::clone(&self.time),
            bell: Arc::clone(&self.bell),
        })
    }

    pub fn net(&self) -> &Arc<tcp::TcpHost> {
        &self.net
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

/// The listing a hermetic run retains.
pub fn registry() -> HostRegistry {
    registry_over(Arc::new(trace::Trace::default()))
}

pub fn registry_over(trace: Arc<trace::Trace>) -> HostRegistry {
    Host::new()
        .traced(trace)
        .stopping_on(Shutdown::new(signal::Bounds::default()))
        .with_process(process::ProcessHost::new(
            Vec::new(),
            process::OutputSink::Real {
                out: process::Stream::Out,
            },
        ))
        .registry()
}

/// The runtime, routing each token to the facility that minted it.
struct Facilities {
    pools: Vec<Watched>,
    /// The sockets, for the connections a drain that ran out of time abandoned.
    net: Arc<tcp::TcpHost>,
    /// The run's own process, for its children and its streams at the end.
    process: Option<Arc<process::ProcessHost>>,
    trace: Arc<trace::Trace>,
    shutdown: Option<Arc<Shutdown>>,
    /// The database connections, for those an entry point left open.
    sqlite: Arc<sqlite::SqliteHost>,
    time: Arc<time::TimeHost>,
    bell: Arc<Bell>,
}

/// A facility, and the tokens of its pool this runtime watches, as they resolve.
struct Watched {
    facility: Arc<dyn Pooled>,
    inbox: Arc<Inbox>,
}

impl Watched {
    fn pool(&self) -> &Pool {
        self.facility.pool()
    }
}

impl Facilities {
    /// Whether some pool holds an operation that has finished and not been collected.
    fn ready(&self) -> bool {
        self.pools.iter().any(|watched| watched.pool().ready())
    }

    fn owner(&self, pending: &Pending) -> Result<&Watched, Diagnostic> {
        self.pools
            .iter()
            .find(|watched| watched.pool().owns(pending))
            .ok_or_else(|| err_unowned(pending))
    }
}

impl HostRuntime for Facilities {
    fn watch(&self, pending: &Pending) -> Result<(), Diagnostic> {
        let owner = self.owner(pending)?;
        owner.pool().watch(pending, &owner.inbox)
    }

    fn resolved(&self) -> Vec<(u64, Result<Value, Diagnostic>)> {
        self.pools
            .iter()
            .flat_map(|watched| watched.pool().collect(&watched.inbox))
            .collect()
    }

    fn park(&self) -> Result<(), Diagnostic> {
        let mut busy = self
            .pools
            .iter()
            .filter(|watched| watched.pool().outstanding() > 0);
        let waited_on = (busy.next(), busy.next());
        // A drain parks in bounded steps and never on a token.
        if self.stopping() {
            let bound = signal::DRAIN_POLL;
            return match waited_on {
                (None, _) => {
                    if let Some(shutdown) = &self.shutdown {
                        shutdown.park(bound);
                    }
                    Ok(())
                }
                (Some(only), None) => only.pool().park_until(bound),
                (Some(_), Some(_)) => {
                    let seen = self.bell.rung();
                    if !self.ready() {
                        self.bell.wait_past_for(seen, bound);
                    }
                    Ok(())
                }
            };
        }
        match waited_on {
            (None, _) => Err(err_nothing_outstanding()),
            // The pool's own wait, which only its operations end: the bell rings at a stop too.
            (Some(only), None) => only.pool().park(),
            // Several pools' condition variables cannot be waited on together, so the bell each
            // of them rings is waited on instead.
            (Some(_), Some(_)) => {
                let seen = self.bell.rung();
                if !self.ready() {
                    self.bell.wait_past(seen);
                }
                Ok(())
            }
        }
    }

    fn now(&self) -> Result<i64, Diagnostic> {
        Ok(self.time.elapsed_ns())
    }

    fn park_until(&self, deadline: i64) -> Result<(), Diagnostic> {
        // Read before the stop and the pools are looked at, so a ring after either look ends the
        // wait at once.
        let seen = self.bell.rung();
        let Ok(left) = u64::try_from(deadline.saturating_sub(self.time.elapsed_ns())) else {
            return Ok(());
        };
        let mut bound = std::time::Duration::from_nanos(left);
        // A drain parks in bounded steps, so its deadline is seen while every task sleeps.
        if self.stopping() {
            bound = bound.min(signal::DRAIN_POLL);
        }
        if !self.ready() {
            self.bell.wait_past_for(seen, bound);
        }
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
        // Last, since a buffer whose reader has gone ends the process here.
        if let Some(process) = &self.process {
            process.settle();
        }
        ShutdownReport {
            spans_left_open: usize::try_from(self.trace.left_open()).unwrap_or(usize::MAX),
        }
    }

    /// Drive until this token resolves, or until the drain deadline says the run is out of time.
    fn block_on(&self, pending: Pending) -> Result<Value, Diagnostic> {
        if self.shutdown.is_none() {
            return self.owner(&pending)?.pool().block_on(pending);
        }
        loop {
            let pool = self.owner(&pending)?.pool();
            if let Some(value) = pool.poll(&pending)? {
                return Ok(value);
            }
            pool.park_until(signal::DRAIN_POLL)?;
            if let Some(expired) = self.drain_expired() {
                return Err(expired);
            }
        }
    }

    fn unanswered(&self, effect: &Symbol, op: &Symbol) {
        if let Some(process) = &self.process {
            process.unanswered(effect, op);
        }
    }

    /// Closes the spans this entry point left open, and warns of them, and the database
    /// connections it left open.
    fn end_entry_point(&self, machine: MachineId) -> Vec<Diagnostic> {
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

#[cold]
#[inline(never)]
fn err_unowned(pending: &Pending) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("no host facility minted the pending token `{pending}`"),
    )
    .primary(Span::DUMMY, "this token belongs to no facility in this run")
    .note("a handler answered `Pending` with a token from a runtime other than the one this run is driving")
    .note("this is Ply's fault: report it with the program that produced it")
}

#[cold]
#[inline(never)]
fn err_nothing_outstanding() -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        "the run asked the host runtime to wait with nothing outstanding to wait for",
    )
    .primary(
        Span::DUMMY,
        "no task is enabled and no host operation is pending",
    )
    .note("waiting here would never return, so it is refused instead")
    .note("this is Ply's fault: report it with the program that produced it")
}
