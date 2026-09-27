//! The trusted computing base, as one list.

use crate::signal::{self, Accepting, Shutdown};
use crate::{certgen, config, fs, process, random, sched, tcp, time, trace};
use ply_eval::Value;
use ply_eval::host::{HostRegistry, HostRuntime, MachineId, Pending, ShutdownReport};
use ply_span::{Diagnostic, Span, codes};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// How long a park waits on the socket pool while the database also holds a token.
const ALTERNATE: Duration = Duration::from_micros(250);

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
    /// The two readings `std.time` answers, counting from when this host was built.
    time: Arc<time::TimeHost>,
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
            net: Arc::new(tcp::TcpHost::with_credentials(credentials)),
            config: Arc::new(config::Snapshot::unopened()),
            trace: Arc::new(trace::Trace::default()),
            shutdown: None,
            fs: Arc::new(fs::FsHost::new(fs::Roots::new())),
            process: None,
            time: Arc::new(time::TimeHost::new()),
        }
    }

    pub fn rooted(self, roots: fs::Roots) -> Host {
        Host {
            fs: Arc::new(fs::FsHost::new(roots)),
            ..self
        }
    }

    pub fn roots(&self) -> &fs::Roots {
        self.fs.roots()
    }

    pub fn with_process(self, process: process::ProcessHost) -> Host {
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
        certgen::register(&mut registry);
        signal::register(&mut registry, self.shutdown.as_ref());
        process::register(&mut registry, self.process.as_ref());
        registry
    }

    /// What a [`ply_eval::host::HostAnswer::Pending`] is polled on.
    pub fn runtime(&self) -> Rc<dyn HostRuntime> {
        Rc::new(Facilities {
            net: Arc::clone(&self.net),
            fs: Arc::clone(&self.fs),
            process: self.process.clone(),
            trace: Arc::clone(&self.trace),
            shutdown: self.shutdown.clone(),
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
            process::Sink::Real {
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
}

impl HostRuntime for Facilities {
    fn poll(&self, pending: &Pending) -> Result<Option<Value>, Diagnostic> {
        if self.net.owns(pending) {
            return self.net.poll(pending);
        }
        if self.fs.owns(pending) {
            return self.fs.poll(pending);
        }
        if let Some(process) = &self.process
            && process.owns(pending)
        {
            return process.poll(pending);
        }
        Err(err_unowned(pending))
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
        // Separate condition variables cannot be waited on together, so a park blocks on one only
        // when no other facility has work.
        let filesystem_waiting = self.fs.outstanding() > 0;
        let spawn_waiting = self
            .process
            .as_ref()
            .is_some_and(|process| process.outstanding() > 0);
        if self.net.outstanding() > 0 {
            if filesystem_waiting || spawn_waiting {
                return self.net.park_until(ALTERNATE);
            }
            return self.net.park();
        }
        if filesystem_waiting {
            if spawn_waiting {
                return self.fs.park_until(ALTERNATE);
            }
            return self.fs.park();
        }
        if let Some(process) = &self.process
            && spawn_waiting
        {
            return process.park();
        }
        Err(err_nothing_outstanding())
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

    /// The process-level teardown, in a pinned order.
    /// The run's own teardown: the drain deadline governs scheduling and the socket pool, so
    /// nothing here waits on it.
    fn shutdown(&self, _drain_ms: u64) -> ShutdownReport {
        let mut report = ShutdownReport {
            spans_abandoned: self.trace.open_spans(),
            ..ShutdownReport::default()
        };
        // The sink flushes before the run's own state is gone.
        self.trace.flush();
        report.records_flushed = Some(self.trace.counts().events as usize);
        report
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

    /// Closes the spans this entry point left open.
    fn end_entry_point(&self, machine: MachineId) -> Result<(), Diagnostic> {
        match self.trace.end_entry_point(machine) {
            None => Ok(()),
            Some(spans) => Err(spans),
        }
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
        .note("W5 has no cancellation, so a request still running here is not unwound and is not handed a 503: its connection closes with no response")
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
