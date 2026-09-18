//! The trusted computing base, as one list.

use crate::db::{self, Postgres};
use crate::signal::{self, Accepting, Shutdown};
use crate::{config, fs, sched, tcp, trace};
use ply_eval::Value;
use ply_eval::host::{HostRegistry, HostRuntime, MachineId, Pending, ShutdownReport};
use ply_span::{Diagnostic, Span, codes};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a park waits on the socket pool while the database also holds a token.
const ALTERNATE: Duration = Duration::from_micros(250);

pub struct Host {
    net: Arc<tcp::TcpHost>,
    db: Option<Arc<Postgres>>,
    /// The run's configuration, read once before this `Host` existed and immutable thereafter.
    config: Arc<config::Snapshot>,
    trace: Arc<trace::Trace>,
    /// The stop flag and the phase machine, when this run listens for a signal.
    shutdown: Option<Arc<Shutdown>>,
    /// The roots `--fs NAME=PATH` bound, and the pool their operations wait on; empty if none.
    fs: Arc<fs::FsHost>,
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
            db: None,
            config: Arc::new(config::Snapshot::unopened()),
            trace: Arc::new(trace::Trace::default()),
            shutdown: None,
            fs: Arc::new(fs::FsHost::new(fs::Roots::new())),
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

    pub fn with_database(
        credentials: crate::tls::Credentials,
        config: db::PoolConfig,
    ) -> Result<Host, Diagnostic> {
        Ok(Host {
            net: Arc::new(tcp::TcpHost::with_credentials(credentials)),
            db: Some(Arc::new(Postgres::start(config)?)),
            config: Arc::new(crate::config::Snapshot::unopened()),
            trace: Arc::new(trace::Trace::default()),
            shutdown: None,
            fs: Arc::new(fs::FsHost::new(fs::Roots::new())),
        })
    }

    pub fn database(&self) -> Option<&Arc<Postgres>> {
        self.db.as_ref()
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
        if let Some(driver) = &self.db {
            db::register(&mut registry, Arc::clone(driver) as Arc<dyn db::Driver>);
        }
        // Registered whatever `--fs` said, so a run that bound no root gets `E0451`, not `E0424`.
        fs::register(&mut registry, Arc::clone(&self.fs));
        signal::register(&mut registry, self.shutdown.as_ref());
        registry
    }

    /// What a [`ply_eval::host::HostAnswer::Pending`] is polled on.
    pub fn runtime(&self) -> Rc<dyn HostRuntime> {
        Rc::new(Facilities {
            net: Arc::clone(&self.net),
            db: self.db.clone(),
            fs: Arc::clone(&self.fs),
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
        if let Some(db) = &self.db {
            shutdown.attach_db(Arc::clone(db) as Arc<dyn signal::Transactions>);
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
    registry_over(Arc::new(trace::Trace::default()), false)
}

pub fn registry_over(trace: Arc<trace::Trace>, database: bool) -> HostRegistry {
    let mut registry = Host::new()
        .traced(trace)
        .stopping_on(Shutdown::new(signal::Bounds::default()))
        .registry();
    if database {
        db::register(&mut registry, Arc::new(db::postgres::NotConfigured));
    }
    registry
}

/// The hermetic listing plus the `db` operations, served by an implementation that refuses.
pub fn registry_with_database() -> HostRegistry {
    registry_over(Arc::new(trace::Trace::default()), true)
}

/// The runtime, routing each token to the facility that minted it.
struct Facilities {
    net: Arc<tcp::TcpHost>,
    db: Option<Arc<Postgres>>,
    fs: Arc<fs::FsHost>,
    trace: Arc<trace::Trace>,
    shutdown: Option<Arc<Shutdown>>,
}

impl HostRuntime for Facilities {
    fn poll(&self, pending: &Pending) -> Result<Option<Value>, Diagnostic> {
        if self.net.owns(pending) {
            return self.net.poll(pending);
        }
        if let Some(db) = &self.db
            && db.owns(pending)
        {
            return db.poll(pending);
        }
        if self.fs.owns(pending) {
            return self.fs.poll(pending);
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
            if let Some(db) = &self.db
                && db.reactor().outstanding() > 0
            {
                db.reactor().park_timeout(bound)?;
                return Ok(());
            }
            if self.fs.outstanding() > 0 {
                return self.fs.park_until(bound);
            }
            if let Some(shutdown) = &self.shutdown {
                shutdown.park(bound);
            }
            return Ok(());
        }
        // Separate condition variables cannot be waited on together, so a park blocks on one only
        // when no other facility has work.
        let database_waiting = self
            .db
            .as_ref()
            .is_some_and(|db| db.reactor().outstanding() > 0);
        let filesystem_waiting = self.fs.outstanding() > 0;
        if self.net.outstanding() > 0 {
            if database_waiting || filesystem_waiting {
                return self.net.park_until(ALTERNATE);
            }
            return self.net.park();
        }
        if let Some(db) = &self.db
            && database_waiting
        {
            if filesystem_waiting {
                db.reactor().park_timeout(ALTERNATE)?;
                return Ok(());
            }
            return db.reactor().park();
        }
        if filesystem_waiting {
            return self.fs.park();
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
            self.db.as_ref().map_or(0, |db| db.open_scopes()),
        ))
    }

    /// The process-level teardown, in a pinned order.
    fn shutdown(&self, drain_ms: u64) -> ShutdownReport {
        let mut report = ShutdownReport {
            spans_abandoned: self.trace.open_spans(),
            ..ShutdownReport::default()
        };
        let until = Instant::now() + Duration::from_millis(drain_ms);
        // Every open scope is rolled back, and none committed.
        if let Some(db) = &self.db {
            fold(
                &mut report,
                db.roll_back_open_scopes(until.saturating_duration_since(Instant::now())),
            );
        }
        // The sink flushes before the pool is gone.
        self.trace.flush();
        report.records_flushed = Some(self.trace.counts().events as usize);
        if let Some(db) = &self.db {
            fold(
                &mut report,
                db.close_pool(until.saturating_duration_since(Instant::now())),
            );
        }
        report
    }

    /// Drive until this token resolves, or until the drain deadline says the run is out of time.
    fn block_on(&self, pending: Pending) -> Result<Value, Diagnostic> {
        let Some(_) = &self.shutdown else {
            if self.net.owns(&pending) {
                return self.net.block_on(pending);
            }
            if let Some(db) = &self.db
                && db.owns(&pending)
            {
                return db.block_on(pending);
            }
            if self.fs.owns(&pending) {
                return self.fs.block_on(pending);
            }
            return Err(err_unowned(&pending));
        };
        loop {
            if self.net.owns(&pending) {
                if let Some(value) = self.net.poll(&pending)? {
                    return Ok(value);
                }
                self.net.park_until(signal::DRAIN_POLL)?;
            } else if let Some(db) = &self.db
                && db.owns(&pending)
            {
                if let Some(value) = db.poll(&pending)? {
                    return Ok(value);
                }
                db.reactor().park_timeout(signal::DRAIN_POLL)?;
            } else if self.fs.owns(&pending) {
                if let Some(value) = self.fs.poll(&pending)? {
                    return Ok(value);
                }
                self.fs.park_until(signal::DRAIN_POLL)?;
            } else {
                return Err(err_unowned(&pending));
            }
            if let Some(expired) = self.drain_expired() {
                return Err(expired);
            }
        }
    }

    /// Rolls back the scopes this entry point left open, then closes the spans it left open.
    fn end_entry_point(&self, machine: MachineId) -> Result<(), Diagnostic> {
        let database = self.close_database(machine);
        let spans = self.trace.end_entry_point(machine);
        // Only one diagnostic reaches the machine, so a second travels as a note on the first.
        match (database, spans) {
            (Ok(()), None) => Ok(()),
            (Ok(()), Some(spans)) => Err(spans),
            (Err(database), None) => Err(database),
            (Err(database), Some(spans)) => Err(database.note(format!(
                "and, at the same teardown, `{}`: {}",
                spans.code, spans.message
            ))),
        }
    }
}

impl Facilities {
    fn close_database(&self, machine: MachineId) -> Result<(), Diagnostic> {
        let Some(db) = &self.db else {
            return Ok(());
        };
        let report = db.end_entry_point(machine)?;
        match report.describe() {
            None => Ok(()),
            Some(why) => Err(Diagnostic::warning(
                codes::HOST_TEARDOWN,
                format!("the database driver could not hand every connection back: {why}"),
            )
            .note("the entry point's verdict is unchanged: this is the run's own state rather than the program's")
            .note("the pool refills, and a connection it closed rather than returned is one that could not be rolled back")),
        }
    }
}

fn fold(report: &mut ShutdownReport, step: Result<db::pool::DrainReport, Diagnostic>) {
    match step {
        Ok(drained) => {
            report.transactions_rolled_back += drained.rolled_back;
            report
                .connections_closed
                .extend(drained.discarded.iter().map(|d| d.reason.clone()));
            if let Some(why) = drained.describe() {
                report.problems.push(format!(
                    "the database driver could not hand every connection back: {why}"
                ));
            }
        }
        Err(d) => report.problems.push(d.message),
    }
}

#[cold]
#[inline(never)]
fn err_drain_incomplete(shutdown: &Shutdown, connections: usize, scopes: usize) -> Diagnostic {
    let bounds = shutdown.bounds();
    let elapsed = shutdown.elapsed().unwrap_or_default();
    Diagnostic::warning(codes::DRAIN_INCOMPLETE, "the drain deadline expired")
        .primary(Span::DUMMY, "this run stopped scheduling here")
        .note(format!(
            "{connections} connection(s) abandoned with no response written"
        ))
        .note(format!(
            "{scopes} transaction(s) still open; every one of them is rolled back at teardown and none is committed"
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
