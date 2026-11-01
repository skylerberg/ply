use crate::config;
use crate::fs;
use crate::pool::{Run, WakeCounter, err_nothing_outstanding};
use crate::process;
use crate::signal::{self, Accepting, Shutdown};
use crate::sqlite;
use crate::tcp;
use crate::time;
use crate::trace;
use ply_core::host::{HostRuntime, MachineId, Pending, RuntimeFactory, ShutdownReport};
use ply_core::{Diagnostic, Span, Symbol, TaskId, codes};
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

pub struct Host {
    bell: Arc<WakeCounter>,
    config: Arc<config::Snapshot>,
    fs: Arc<fs::FsHost>,
    net: Arc<tcp::TcpHost>,
    process: Option<Arc<process::ProcessHost>>,
    shutdown: Option<Arc<Shutdown>>,
    sqlite: Arc<sqlite::SqliteHost>,
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
            bell: Arc::new(WakeCounter::default()),
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
        shutdown.attach_notifier(&self.bell);
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

pub fn unbound() -> RuntimeFactory {
    let host: Arc<OnceLock<Host>> = Arc::default();
    Arc::new(move || host.get_or_init(Host::new).runtime())
}

pub fn facilities_of(runtime: &dyn HostRuntime) -> Option<&Facilities> {
    runtime.facilities()?.downcast_ref()
}

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

pub struct Facilities {
    run: Arc<Run>,
    config: Arc<config::Snapshot>,
    fs: Arc<fs::FsHost>,
    net: Arc<tcp::TcpHost>,
    process: Option<Arc<process::ProcessHost>>,
    trace: Arc<trace::Trace>,
    shutdown: Option<Arc<Shutdown>>,
    sqlite: Arc<sqlite::SqliteHost>,
    time: Arc<time::TimeHost>,
}

impl Facilities {
    pub fn configuration(&self) -> &config::Snapshot {
        &self.config
    }

    pub fn sockets(&self) -> &Arc<tcp::TcpHost> {
        &self.net
    }

    pub fn run(&self) -> &Arc<Run> {
        &self.run
    }

    pub fn roots(&self) -> &fs::Roots {
        self.fs.roots()
    }

    pub fn files(&self) -> &Arc<fs::FsHost> {
        &self.fs
    }

    pub fn sqlite_host(&self) -> &Arc<sqlite::SqliteHost> {
        &self.sqlite
    }

    pub fn process(&self) -> Option<&Arc<process::ProcessHost>> {
        self.process.as_ref()
    }

    pub fn stop(&self) -> Option<&Arc<Shutdown>> {
        self.shutdown.as_ref()
    }

    pub fn tracing(&self) -> &Arc<trace::Trace> {
        &self.trace
    }
}

impl HostRuntime for Facilities {
    fn facilities(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    fn park(&self) -> Result<(), Diagnostic> {
        let stopped = self.stopping();
        let stop = || self.stopping() != stopped;
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

    fn shutdown(&self) -> ShutdownReport {
        if let Some(process) = &self.process {
            process.end_children();
        }
        self.trace.flush();
        crate::stdio::restore_terminal();
        if let Some(process) = &self.process {
            process.settle();
        }
        ShutdownReport {
            spans_left_open: usize::try_from(self.trace.left_open()).unwrap_or(usize::MAX),
        }
    }

    fn unhandled(&self, effect: &Symbol, op: &Symbol) {
        if let Some(process) = &self.process {
            process.unhandled(effect, op);
        }
    }

    fn end_entry_point(&self, machine: MachineId) -> Vec<Diagnostic> {
        self.run.drop_secrets();
        self.sqlite.end_machine(machine);
        self.trace.end_entry_point(machine).into_iter().collect()
    }

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
