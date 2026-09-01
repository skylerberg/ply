//! The trusted computing base, as one list.

use crate::db::{self, Postgres};
use crate::signal::{self, Accepting, Shutdown};
use crate::{config, sched, tcp, trace};
use ply_eval::Value;
use ply_eval::host::{HostRegistry, HostRuntime, MachineId, Pending, ShutdownReport};
use ply_span::{Diagnostic, Span, codes};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a park waits on the socket pool while the database also holds a token.
const ALTERNATE: Duration = Duration::from_micros(250);

/// Every facility this binary can serve, built once.
pub struct Host {
    net: Arc<tcp::TcpHost>,
    /// The database, when the run named one.
    db: Option<Arc<Postgres>>,
    /// The run's configuration: every source read once, before this `Host` existed, and immutable
    /// thereafter.
    config: Arc<config::Snapshot>,
    /// Where this run's records go, and every span every entry point has open.
    trace: Arc<trace::Trace>,
    /// The stop flag and the phase machine, when this run listens for a signal.
    shutdown: Option<Arc<Shutdown>>,
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

    /// The same facilities, holding the TLS material this run was configured with.
    pub fn with_credentials(credentials: crate::tls::Credentials) -> Host {
        Host {
            net: Arc::new(tcp::TcpHost::with_credentials(credentials)),
            db: None,
            config: Arc::new(config::Snapshot::unopened()),
            trace: Arc::new(trace::Trace::default()),
            shutdown: None,
        }
    }

    /// The same facilities, writing records to the sink this run selected.
    pub fn traced(self, trace: Arc<trace::Trace>) -> Host {
        Host { trace, ..self }
    }

    /// Where this run's records go, for the `observability` block of `ply hosts`, for the shutdown
    /// banner's counts, and for the teardown that flushes it.
    pub fn tracing(&self) -> &Arc<trace::Trace> {
        &self.trace
    }

    /// The same facilities, holding the configuration this run resolved.
    pub fn configured(self, config: Arc<config::Snapshot>) -> Host {
        Host { config, ..self }
    }

    /// What the run was told, for the `configuration` block of `ply hosts` and for the start-up
    /// banner.
    pub fn configuration(&self) -> &Arc<config::Snapshot> {
        &self.config
    }

    /// The same facilities, plus a database.
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
        })
    }

    /// The database this run was configured with, for the `database` block of `ply hosts` and for a
    /// test that has to assert what a scope left open.
    pub fn database(&self) -> Option<&Arc<Postgres>> {
        self.db.as_ref()
    }

    /// What the run's `--host` summary reports about TLS: how many handshakes completed, how many
    /// were refused, and why.
    pub fn handshakes(&self) -> crate::tls::HandshakeCounts {
        self.net.handshakes()
    }

    /// The credentials this run was configured with, for the `transport` block of `ply hosts`.
    pub fn credentials(&self) -> &crate::tls::Credentials {
        self.net.credentials()
    }

    /// The trusted computing base of a run served by this `Host`.
    pub fn registry(&self) -> HostRegistry {
        let mut registry = HostRegistry::new();

        // `net.*` — six operations over real sockets, one of them terminating TLS through
        // `ply_host::tls`.
        tcp::register(&mut registry, Arc::clone(&self.net) as Arc<dyn tcp::Net>);

        // `config.*` — two reads of one immutable map.
        config::register(&mut registry, Arc::clone(&self.config));

        // `trace.*` — six operations over one sink.
        trace::register(&mut registry, Arc::clone(&self.trace));

        // `task.*` — the production scheduler.
        for (op, handler) in sched::registrations() {
            registry.register(op, handler);
        }

        // `db.*` — six operations over a real postgres, and only when this run named one.
        if let Some(driver) = &self.db {
            db::register(&mut registry, Arc::clone(driver) as Arc<dyn db::Driver>);
        }

        // `signal.*` — two reads of one flag, bound when this run listens for a stop and
        // **withheld** when it does not.
        signal::register(&mut registry, self.shutdown.as_ref());

        registry
    }

    /// What a [`ply_eval::host::HostAnswer::Pending`] is polled on.
    pub fn runtime(&self) -> Rc<dyn HostRuntime> {
        Rc::new(Facilities {
            net: Arc::clone(&self.net),
            db: self.db.clone(),
            trace: Arc::clone(&self.trace),
            shutdown: self.shutdown.clone(),
        })
    }

    /// The socket table, for a test that needs to know which port it got.
    pub fn net(&self) -> &Arc<tcp::TcpHost> {
        &self.net
    }

    /// The same facilities, listening for a stop.
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

    /// The coordinator this run is stopping on, for the `shutdown` block of `ply hosts` and for the
    /// banner a stopping service prints.
    pub fn shutdown(&self) -> Option<&Arc<Shutdown>> {
        self.shutdown.as_ref()
    }
}

/// The listing a hermetic run retains.
pub fn registry() -> HostRegistry {
    registry_over(Arc::new(trace::Trace::default()), false)
}

/// The same listing, over the sink a run selected and with or without the postgres driver.
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

/// The same, plus the `db` operations, served by an implementation that refuses.
pub fn registry_with_database() -> HostRegistry {
    registry_over(Arc::new(trace::Trace::default()), true)
}

/// The runtime, routing each token to the facility that minted it.
struct Facilities {
    net: Arc<tcp::TcpHost>,
    db: Option<Arc<Postgres>>,
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
        Err(err_unowned(pending))
    }

    /// Waits on every facility with work outstanding.
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
            if let Some(shutdown) = &self.shutdown {
                shutdown.park(bound);
            }
            return Ok(());
        }
        // Two facilities behind two condition variables cannot be waited on together, so a park
        // that blocks on one alone sits through the other's completion.
        let database_waiting = self
            .db
            .as_ref()
            .is_some_and(|db| db.reactor().outstanding() > 0);
        if self.net.outstanding() > 0 {
            if database_waiting {
                return self.net.park_until(ALTERNATE);
            }
            return self.net.park();
        }
        if let Some(db) = &self.db
            && database_waiting
        {
            return db.reactor().park();
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

    /// The process-level teardown, in the order ADR 0015 §4.4 pins.
    fn shutdown(&self, drain_ms: u64) -> ShutdownReport {
        let mut report = ShutdownReport {
            spans_abandoned: self.trace.open_spans(),
            ..ShutdownReport::default()
        };
        let until = Instant::now() + Duration::from_millis(drain_ms);
        // 1. the database: every open scope rolled back, and none committed.
        if let Some(db) = &self.db {
            fold(
                &mut report,
                db.roll_back_open_scopes(until.saturating_duration_since(Instant::now())),
            );
        }
        // 2. the sink, before the pool is gone.
        self.trace.flush();
        report.records_flushed = Some(self.trace.counts().events as usize);
        // 3. the pool.
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
            } else {
                return Err(err_unowned(&pending));
            }
            if let Some(expired) = self.drain_expired() {
                return Err(expired);
            }
        }
    }

    /// Rolls back every transaction scope this entry point left open and releases or discards the
    /// connections holding them, then closes every span it left open.
    fn end_entry_point(&self, machine: MachineId) -> Result<(), Diagnostic> {
        let database = self.close_database(machine);
        let spans = self.trace.end_entry_point(machine);
        // One diagnostic reaches the machine and two teardowns can produce one, so the second
        // travels as a note on the first rather than being dropped.
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

/// Fold one teardown step's outcome into the run's report.
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

/// The drain deadline, expired.
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

#[cfg(test)]
mod tests {
    use super::*;
    use ply_eval::host::{Determinism, Linearity};

    /// The listing is the artifact this milestone exists to produce, so a change to it must be a
    /// change someone made on purpose.
    #[test]
    fn the_trusted_computing_base_declares_everything_it_must() {
        let registry = registry();
        assert!(
            !registry.is_empty(),
            "a registry that loads nothing is indistinguishable from a registry that failed to load"
        );
        for op in registry.ops() {
            assert!(
                op.path.starts_with("ply_host::"),
                "`{op}` is identified as `{}`, which names no Rust path a reviewer can find",
                op.path
            );
            assert_eq!(
                op.determinism,
                Determinism::Nondeterministic,
                "`{op}` claims to be a function of the program state; nothing in W1 is"
            );
        }
    }

    /// `Repeatable` is a claim that replaying the operation changes nothing outside the program,
    /// and it is the one column that silently re-opens multi-shot resumption over the boundary.
    #[test]
    fn every_repeatable_operation_is_one_that_was_argued_for() {
        let repeatable: Vec<String> = registry()
            .ops()
            .filter(|op| op.linearity == Linearity::Repeatable)
            .map(|op| op.to_string())
            .collect();
        assert_eq!(
            repeatable,
            [
                "std.config.config.get[..]",
                "std.config.config.secret[..]",
                "task.spawn[..]",
                "task.join[..]",
                "task.yield[..]",
                "std.signal.signal.stopping[..]",
                "std.signal.signal.deadline_ms[..]",
            ]
        );
    }

    #[test]
    fn a_registry_and_a_runtime_come_from_one_host() {
        let host = Host::new();
        assert!(!host.registry().is_empty());
        let runtime = host.runtime();
        let stray = Pending {
            token: 0,
            label: "stray",
        };
        assert_eq!(
            runtime
                .poll(&stray)
                .expect_err("a token nothing minted is refused")
                .code,
            codes::INTERNAL_ERROR
        );
    }
}
