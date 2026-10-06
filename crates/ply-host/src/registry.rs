//! The trusted computing base, as one list.

use crate::pool::{Bell, Inbox};
use crate::signal::{self, Accepting, Shutdown};
use crate::{certgen, clock, config, fs, os, process, random, sched, tcp, term, time, trace};
use ply_eval::host::{HostRegistry, HostRuntime, MachineId, Pending, ShutdownReport};
use ply_eval::{Diagnostic, Span, Symbol, TaskId, Value, codes};
use std::rc::Rc;
use std::sync::Arc;

pub struct Host {
    net: Arc<tcp::TcpHost>,
    /// The run's configuration, read once before this `Host` existed and immutable thereafter.
    config: Arc<config::Snapshot>,
    trace: Arc<trace::Trace>,
    /// The stop flag and the phase machine, when this run listens for a signal.
    shutdown: Option<Arc<Shutdown>>,
    /// The roots `--fs NAME=PATH` bound, and the pool their operations wait on; empty if none.
    fs: Arc<fs::FsHost>,
    /// The arguments and streams `ply run --host` was given; `None` withholds `process`.
    process: Option<Arc<process::ProcessHost>>,
    /// The run's clocks: what `std.time` and the language's `clock` read, and what a production
    /// region's sleeps are deadlines on.
    time: Arc<time::TimeHost>,
    /// Rung by every pool above, so a park can wait on all of them at once.
    bell: Arc<Bell>,
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
        let bell = Arc::new(Bell::default());
        let net = tcp::TcpHost::with_credentials(credentials);
        net.ring(&bell);
        let fs = fs::FsHost::new(fs::Roots::new());
        fs.ring(&bell);
        Host {
            net: Arc::new(net),
            config: Arc::new(config::Snapshot::unopened()),
            trace: Arc::new(trace::Trace::default()),
            shutdown: None,
            fs: Arc::new(fs),
            process: None,
            time: Arc::new(time::TimeHost::new()),
            bell,
        }
    }

    pub fn rooted(self, roots: fs::Roots) -> Host {
        let fs = fs::FsHost::new(roots);
        fs.ring(&self.bell);
        Host {
            fs: Arc::new(fs),
            ..self
        }
    }

    pub fn roots(&self) -> &fs::Roots {
        self.fs.roots()
    }

    pub fn with_process(self, process: process::ProcessHost) -> Host {
        process.ring(&self.bell);
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
        tcp::register(&mut registry, Arc::clone(&self.net) as Arc<dyn tcp::Net>);
        config::register(&mut registry, Arc::clone(&self.config));
        trace::register(&mut registry, Arc::clone(&self.trace));
        for (op, handler) in sched::registrations() {
            registry.register(op, handler);
        }
        random::register(&mut registry);
        // Registered whatever `--fs` said, so a run that bound no root gets `E0451`, not `E0424`.
        fs::register(&mut registry, Arc::clone(&self.fs));
        time::register(&mut registry, Arc::clone(&self.time));
        clock::register(&mut registry, Arc::clone(&self.time));
        os::register(&mut registry);
        certgen::register(&mut registry);
        signal::register(&mut registry, self.shutdown.as_ref());
        process::register(&mut registry, self.process.as_ref());
        term::register(&mut registry, self.process.as_ref());
        registry
    }

    /// One per machine: the pools are shared, but a runtime collects only the tokens it watches.
    pub fn runtime(&self) -> Rc<dyn HostRuntime> {
        Rc::new(Facilities {
            net: Arc::clone(&self.net),
            fs: Arc::clone(&self.fs),
            process: self.process.clone(),
            trace: Arc::clone(&self.trace),
            shutdown: self.shutdown.clone(),
            time: Arc::clone(&self.time),
            bell: Arc::clone(&self.bell),
            inboxes: Inboxes::default(),
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
    net: Arc<tcp::TcpHost>,
    fs: Arc<fs::FsHost>,
    process: Option<Arc<process::ProcessHost>>,
    trace: Arc<trace::Trace>,
    shutdown: Option<Arc<Shutdown>>,
    time: Arc<time::TimeHost>,
    bell: Arc<Bell>,
    inboxes: Inboxes,
}

impl Facilities {
    /// Whether some pool holds an operation that has finished and not been collected.
    fn ready(&self) -> bool {
        self.net.ready() || self.fs.ready() || self.process.as_ref().is_some_and(|p| p.ready())
    }
}

/// Per facility, the tokens this runtime watches, as they resolve.
#[derive(Default)]
struct Inboxes {
    net: Arc<Inbox>,
    fs: Arc<Inbox>,
    process: Arc<Inbox>,
}

impl HostRuntime for Facilities {
    fn watch(&self, pending: &Pending) -> Result<(), Diagnostic> {
        if self.net.owns(pending) {
            return self.net.watch_into(pending, &self.inboxes.net);
        }
        if self.fs.owns(pending) {
            return self.fs.watch_into(pending, &self.inboxes.fs);
        }
        if let Some(process) = &self.process
            && process.owns(pending)
        {
            return process.watch_into(pending, &self.inboxes.process);
        }
        Err(err_unowned(pending))
    }

    fn resolved(&self) -> Vec<(u64, Result<Value, Diagnostic>)> {
        let mut resolved = self.net.collect(&self.inboxes.net);
        resolved.extend(self.fs.collect(&self.inboxes.fs));
        if let Some(process) = &self.process {
            resolved.extend(process.collect(&self.inboxes.process));
        }
        resolved
    }

    fn park(&self) -> Result<(), Diagnostic> {
        // A drain parks in bounded steps and never on a token.
        if self.stopping() {
            let bound = signal::DRAIN_POLL;
            if self.net.outstanding() > 0 {
                return self.net.park_until(bound);
            }
            if self.fs.outstanding() > 0 {
                return self.fs.park_until(bound);
            }
            if let Some(process) = &self.process
                && process.outstanding() > 0
            {
                return process.park_until(bound);
            }
            if let Some(shutdown) = &self.shutdown {
                shutdown.park(bound);
            }
            return Ok(());
        }
        let net = self.net.outstanding() > 0;
        let fs = self.fs.outstanding() > 0;
        let process = self.process.as_ref().filter(|p| p.outstanding() > 0);
        match (net, fs, process) {
            (false, false, None) => Err(err_nothing_outstanding()),
            (true, false, None) => self.net.park(),
            (false, true, None) => self.fs.park(),
            (false, false, Some(process)) => process.park(),
            // Several pools' condition variables cannot be waited on together, so the bell each
            // of them rings is waited on instead.
            _ => {
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
        let Some(_) = &self.shutdown else {
            if self.net.owns(&pending) {
                return self.net.block_on(pending);
            }
            if self.fs.owns(&pending) {
                return self.fs.block_on(pending);
            }
            if let Some(process) = &self.process
                && process.owns(&pending)
            {
                return process.block_on(pending);
            }
            return Err(err_unowned(&pending));
        };
        loop {
            if self.net.owns(&pending) {
                if let Some(value) = self.net.poll(&pending)? {
                    return Ok(value);
                }
                self.net.park_until(signal::DRAIN_POLL)?;
            } else if self.fs.owns(&pending) {
                if let Some(value) = self.fs.poll(&pending)? {
                    return Ok(value);
                }
                self.fs.park_until(signal::DRAIN_POLL)?;
            } else if let Some(process) = self.process.as_ref().filter(|p| p.owns(&pending)) {
                if let Some(value) = process.poll(&pending)? {
                    return Ok(value);
                }
                process.park_until(signal::DRAIN_POLL)?;
            } else {
                return Err(err_unowned(&pending));
            }
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

    /// Closes the spans this entry point left open, and warns of them.
    fn end_entry_point(&self, machine: MachineId) -> Vec<Diagnostic> {
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
