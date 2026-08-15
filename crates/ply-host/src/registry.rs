//! The trusted computing base, as one list.
//!
//! This is the file ADR 0008 §2 asks for: the whole set of Rust functions a Ply
//! program's effect operations may resolve to, written by hand, in an order a
//! reviewer reads top to bottom. There is no attribute macro, no link-time
//! registry and no global constructor, because the point of the list is that it
//! is short enough to read and that adding to it is a diff.
//!
//! Everything above the boundary holds *given* these declarations are honest.
//! Nothing here can check that a handler does only what it declared — §7's
//! footprint check catches one answering outside its registration, and nothing
//! catches one that opens a file behind Ply's back. `ply hosts` and review are
//! the whole defence, which is why this file exists as a file.

use crate::db::{self, Postgres};
use crate::signal::{self, Accepting, Shutdown};
use crate::{config, sched, tcp, trace};
use ply_eval::Value;
use ply_eval::host::{HostRegistry, HostRuntime, MachineId, Pending, ShutdownReport};
use ply_span::{Diagnostic, Span, codes};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a park waits on the socket pool while the database also holds a
/// token.
///
/// The worst case a `db` answer waits behind a socket that has nothing to say,
/// so it is small; it is paid only while both facilities are outstanding, which
/// is a thread that is blocked either way.
const ALTERNATE: Duration = Duration::from_micros(250);

/// Every facility this binary can serve, built once.
///
/// The registry and the runtime come from **one** of these, and that is
/// structural rather than a convention. A registry built over one [`TcpHost`]
/// and a runtime built over another would mint tokens into a table nothing
/// polls: the run would hang rather than fail, which is the worst shape a defect
/// at this boundary can take.
///
/// Constructing one opens nothing. No socket is bound and no thread is started
/// until a handler is actually called, so `ply hosts` — which lists the trusted
/// computing base without binding it — costs an allocation.
pub struct Host {
    net: Arc<tcp::TcpHost>,
    /// The database, when the run named one. `None` is a run with no `--db`,
    /// which registers no `db` operation at all — so a program that performs one
    /// is `E0424` naming the handler that *would* have served it, and a program
    /// that performs none needs no database. That is what makes `E0431` fire
    /// exactly when the driver is in the run and the run named nothing for it to
    /// open.
    db: Option<Arc<Postgres>>,
    /// The run's configuration: every source read once, before this `Host`
    /// existed, and immutable thereafter.
    ///
    /// Always present, and empty for a run that opened no source. That is not a
    /// stand-in for `Option`: the `config` operations are registered either way,
    /// so that a hermetic run reaching one is `E0424` naming the handler that
    /// *would* have served it, and an empty snapshot answering `None` is the
    /// true answer for a run that was told nothing.
    ///
    /// Shared by `Arc` and never mutated, which is the whole of what keeps two
    /// host-backed tests that read configuration uncoupled — there is no state
    /// here for them to be coupled through, unlike the pooled connection of ADR
    /// 0014 §3.
    config: Arc<config::Snapshot>,
    /// Where this run's records go, and every span every entry point has open.
    ///
    /// Always present, and `--trace off` is [`trace::Discard`] rather than the
    /// absence of a sink: a row cannot be conditional on a flag, so turning
    /// tracing off cannot remove the perform, and an unregistered `trace` would
    /// be `E0424` at the first event — which is correct for a hermetic run and
    /// is not what "off" should mean.
    ///
    /// This is the one piece of §7's three that holds mutable state, so it is
    /// the one that could couple two tests. What keeps it from doing so is the
    /// atom: `trace.write[c]` is a **write**, per channel, so two tests
    /// recording on one channel conflict and the existing graph serialises them
    /// — and a test that installs `std.trace`'s twin discharges the atom
    /// entirely and never reaches here at all.
    trace: Arc<trace::Trace>,
    /// The stop flag and the phase machine, when this run listens for a signal.
    ///
    /// `None` under `ply test`, with or without `--host`, and that is §7's third
    /// entry rather than an omission: a stop requested once ends every test after
    /// it, so `signal` is registered **withheld** and reaching it is `E0424`
    /// naming `std.signal`'s twin. It is the one of the three new host states
    /// that no atom can isolate — `signal.read` is a read, so two readers never
    /// conflict and the conflict graph would place two tests that consult it in
    /// one group and be right — which is exactly why the answer is not to bind it
    /// rather than to schedule around it.
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

    /// The same facilities, holding the TLS material this run was configured
    /// with. Loading happens before this — [`Credentials::load`] is what raises
    /// `E0430`, and it does so before anything runs, because a server that
    /// discovers its certificate is unusable on the first handshake has already
    /// told a client it was listening.
    ///
    /// [`Credentials::load`]: crate::tls::Credentials::load
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
    ///
    /// A builder rather than a constructor argument for the reason
    /// [`configured`] is one: which sink a run gets is a flag, and a `Host` that
    /// took every flag positionally would grow a parameter per milestone.
    ///
    /// [`configured`]: Host::configured
    pub fn traced(self, trace: Arc<trace::Trace>) -> Host {
        Host { trace, ..self }
    }

    /// Where this run's records go, for the `observability` block of `ply hosts`,
    /// for the shutdown banner's counts, and for the teardown that flushes it.
    pub fn tracing(&self) -> &Arc<trace::Trace> {
        &self.trace
    }

    /// The same facilities, holding the configuration this run resolved.
    ///
    /// Taken already resolved rather than resolved here, because resolving it
    /// needs the program: `--config-schema` names a function, materialising it
    /// needs an evaluator, and `E0441` and `E0442` must be raised before any of
    /// this is constructed. By the time a `Host` exists the answer is a value.
    pub fn configured(self, config: Arc<config::Snapshot>) -> Host {
        Host { config, ..self }
    }

    /// What the run was told, for the `configuration` block of `ply hosts` and
    /// for the start-up banner. Nothing here can hand back a secret's value.
    pub fn configuration(&self) -> &Arc<config::Snapshot> {
        &self.config
    }

    /// The same facilities, plus a database.
    ///
    /// Starting the reactor is what raises `E0431` for a connection string
    /// postgres will not accept, and it happens here — before `bind`, before
    /// anything runs — because a service that discovers its database is
    /// unreachable on the first request has already told a client it was
    /// listening.
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

    /// The database this run was configured with, for the `database` block of
    /// `ply hosts` and for a test that has to assert what a scope left open.
    pub fn database(&self) -> Option<&Arc<Postgres>> {
        self.db.as_ref()
    }

    /// What the run's `--host` summary reports about TLS: how many handshakes
    /// completed, how many were refused, and why.
    pub fn handshakes(&self) -> crate::tls::HandshakeCounts {
        self.net.handshakes()
    }

    /// The credentials this run was configured with, for the `transport` block
    /// of `ply hosts`. By name and fingerprint: nothing here can hand back key
    /// material.
    pub fn credentials(&self) -> &crate::tls::Credentials {
        self.net.credentials()
    }

    /// The trusted computing base of a run served by this `Host`.
    ///
    /// Read it top to bottom. Every line is a member, and every column `ply
    /// hosts` prints — footprint, determinism, linearity, blocking — is decided
    /// by the module the line names.
    pub fn registry(&self) -> HostRegistry {
        let mut registry = HostRegistry::new();

        // `net.*` — six operations over real sockets, one of them terminating
        // TLS through `ply_host::tls`. Nondeterministic, at most once, and
        // blocking wherever the operation waits on a peer.
        tcp::register(&mut registry, Arc::clone(&self.net) as Arc<dyn tcp::Net>);

        // `config.*` — two reads of one immutable map. Registered whatever this
        // run was told, and unconditionally: the resource is `Any`, so a program
        // that reads no configuration resolves it to no atom and prints no row,
        // and a hermetic run reaching one is `E0424` naming the handler that
        // would have served it. Nondeterministic, repeatable and non-blocking,
        // and each of the three is true because the snapshot was read once and
        // cannot change — the claim rests on `Snapshot` having no mutator, not
        // on a convention.
        config::register(&mut registry, Arc::clone(&self.config));

        // `trace.*` — six operations over one sink. Registered unconditionally
        // and with the resource `Any`, so a program that records nothing
        // resolves them to no atom and prints no row, and one that records on
        // four channels prints four rows per operation — which is the difference
        // between "this sink claims everything" and "this sink claims these four
        // channels". Nondeterministic, at most once, and not blocking: a record
        // is formatted and written inline and nothing waits on a peer.
        //
        // The path a row prints is the *sink's*, so `--trace off` prints
        // `ply_host::trace::discard` rather than pretending a run is writing
        // somewhere it is not.
        trace::register(&mut registry, Arc::clone(&self.trace));

        // `task.*` — the production scheduler. Repeatable, because spawning or
        // joining a Ply task creates and observes a machine state and changes
        // nothing outside the program.
        for (op, handler) in sched::registrations() {
            registry.register(op, handler);
        }

        // `db.*` — six operations over a real postgres, and only when this run
        // named one. Nondeterministic, at most once, and blocking: every one of
        // them dispatches to the reactor thread and answers `Pending`, so an
        // outstanding query costs a pending token and no blocking-pool thread.
        // `db.rollback` is absent: `std.db`'s `transaction` handles it in Ply
        // and it never reaches the boundary.
        //
        // Registered only when this run named a database, which is the one
        // departure from "the registry is compiled in either way". The reason is
        // `db.begin`, `db.commit` and `db.abort`: they take no table, so they
        // register `HostResource::Only(Singleton)`, and `bind` makes an `Only`
        // registration whose effect nothing declares `E0421` — correctly, since
        // such a registration asserts the program has a resource it does not.
        // Registering them into every program would therefore refuse every
        // program that does not import `std.db`, which is most of them.
        //
        // `NotConfigured` is what serves a run that named a database the
        // reactor could not be started for; the missing-`--db` case is `E0431`
        // at bind, before anything runs.
        if let Some(driver) = &self.db {
            db::register(&mut registry, Arc::clone(driver) as Arc<dyn db::Driver>);
        }

        // `signal.*` — two reads of one flag, bound when this run listens for a
        // stop and **withheld** when it does not. Nondeterministic, repeatable
        // and non-blocking.
        //
        // Withheld rather than absent, because the two say different things to a
        // reader: absent is `E0303`, which means inference should have prevented
        // the perform, and withheld is `E0424`, which means the run was
        // configured not to serve it and names the twin that would. `ply test`
        // takes the second path with or without `--host`.
        signal::register(&mut registry, self.shutdown.as_ref());

        registry
    }

    /// What a [`ply_eval::host::HostAnswer::Pending`] is polled on.
    ///
    /// A machine holds this by `Rc` because it belongs to the one thread its
    /// values live on, while the facilities behind it are `Arc` and own the real
    /// threads. No Ply value ever crosses that line.
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
    ///
    /// A builder for the reason [`traced`] is one, and **the last one applied**:
    /// it wires the coordinator to the facilities it has to reach in phase 2, so
    /// a `Host` that acquired a database after this would have a drain that
    /// never counted its open transactions.
    ///
    /// [`traced`]: Host::traced
    /// The coordinator this run listens with, for the `shutdown` block of
    /// `ply hosts`. `None` under `ply test`, which binds no signal handler.
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

    /// The coordinator this run is stopping on, for the `shutdown` block of
    /// `ply hosts` and for the banner a stopping service prints.
    pub fn shutdown(&self) -> Option<&Arc<Shutdown>> {
        self.shutdown.as_ref()
    }
}

/// The listing a hermetic run retains.
///
/// `ply test` and `ply run` bind `HostBinding::hermetic_with(registry())`: the
/// registry is compiled in either way, so `E0424` can name the handler that
/// *would* have served an operation, and what `--host` adds is the binding
/// rather than the knowledge.
/// `signal` is **bound** here and withheld only by a `Host` that has no
/// coordinator, which is the same shape the postgres driver has in
/// [`registry_with_database`]: `ply hosts` answers "what does this run trust",
/// and a serving run trusts the signal handlers whether or not this particular
/// invocation is serving. What withholds them is `ply test`'s own `Host`, which
/// is where the decision belongs.
pub fn registry() -> HostRegistry {
    registry_over(Arc::new(trace::Trace::default()), false)
}

/// The same listing, over the sink a run selected and with or without the
/// postgres driver.
///
/// The sink is a parameter because the `HANDLER` column has to name the sink
/// that would actually serve the run: `ply_host::trace::discard` and
/// `ply_host::trace::json` are two different members of the trusted computing
/// base, and a listing that always printed one of them would be a listing that
/// lied about the other.
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
///
/// For a *listing* over a program that declares `std.db.db`: `ply hosts` answers
/// "what does this run trust", and the postgres driver belongs in that answer
/// whether or not this invocation named a database. It is separate from
/// [`registry`] rather than folded into it because `db.begin`, `db.commit` and
/// `db.abort` take no table, so they register `HostResource::Only`, and `bind`
/// makes an `Only` registration whose effect nothing declares `E0421` —
/// correctly. A caller therefore has to have looked at the program first.
pub fn registry_with_database() -> HostRegistry {
    registry_over(Arc::new(trace::Trace::default()), true)
}

/// The runtime, routing each token to the facility that minted it.
///
/// One facility today, and the routing is still worth writing: a token nobody
/// owns is answered with a diagnostic rather than polled forever by whichever
/// facility happened to be first. A boundary that hangs tells a reader nothing.
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

    /// Waits on every facility with work outstanding. Called only with no task
    /// enabled, so a facility with nothing outstanding has nothing to contribute
    /// and parking on it would be the deadlock.
    fn park(&self) -> Result<(), Diagnostic> {
        // A drain parks in bounded steps and never on a token. The scheduler
        // checks the deadline between turns, so a park that blocked until
        // something resolved would let one request waiting on a `recv` its peer
        // will never answer outlast the whole drain — and the run would sit
        // there with `--drain-ms` elapsed and nothing to read. It is also what
        // lets an *idle* service observe a signal at all: with no traffic there
        // is one outstanding `accept`, and until phase 2 closes the listener
        // nothing resolves it.
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
        // Two facilities behind two condition variables cannot be waited on
        // together, so a park that blocks on one alone sits through the other's
        // completion. That is not a corner: a task-per-connection server always
        // has an `accept` outstanding, so *every* query a spawned task issues
        // would wait for the next connection to arrive and wake the socket pool.
        // Bounded alternation is what `db::pool::Reactor::park_timeout` was
        // written for, and this is the caller it was written for.
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
    ///
    /// Three of the four steps are ordering-sensitive and a wrong order is a
    /// data-loss bug rather than a mess:
    ///
    /// 1. **the database** — every scope still open is `ROLLBACK`ed and **none
    ///    is committed**, and the connections holding them are closed rather
    ///    than returned;
    /// 2. **the sink** — flushed *before* the pool is gone, so a record naming a
    ///    rolled-back transaction is written by a run that still had the
    ///    connection that rolled it back;
    /// 3. **the pool** — closed. Last, because `Reactor::shutdown` closes
    ///    connections rather than returning them, and a connection closed with a
    ///    `BEGIN` still open leaves postgres to abort it whenever it notices the
    ///    disconnect: the same outcome by luck instead of by construction, and
    ///    on a server slow to notice it holds the row locks the rest of a
    ///    rolling restart is waiting on.
    ///
    /// Spans are closed by `end_entry_point`, which the machine calls on **every**
    /// exit path from an entry point including the one a drain deadline produces,
    /// so by the time this runs the records that say what a dying request was
    /// doing have already been written. What is left here is the flush.
    ///
    /// Never called from a signal handler. The handler sets a flag; this runs on
    /// the machine's thread once it has stopped running Ply code, so nothing
    /// here races a statement the program is still issuing.
    /// `drain_ms` is the **budget for the whole teardown**, and it is what each
    /// waiting step is bounded by. It used to be ignored, on the reasoning that
    /// the drain is over by the time this runs and each driver's own deadline —
    /// the pool's statement and connect timeouts — is the honest bound. That was
    /// wrong in the way that matters: a request blocked on a row lock kept the
    /// process alive until the *statement* timeout, so a run configured
    /// `--drain-ms 1000 --db-statement-ms 8000` raised `W0608` on time and then
    /// exited six seconds later, and one with the default 30-second statement
    /// timeout exited nineteen seconds after a three-second drain. `--drain-ms`
    /// has to bound the stop or it bounds nothing, and a rollback that cannot
    /// finish inside it is answered by closing the connection — which is what
    /// makes the server abandon the statement, and which is ADR 0014 §1.3's
    /// existing rule rather than a new case.
    ///
    /// The two waiting steps share the budget rather than each getting it, so
    /// the whole teardown fits inside one deadline.
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

    /// Drive until this token resolves, or until the drain deadline says the run
    /// is out of time.
    ///
    /// This is the only place a Ply computation blocks a real thread, and it is
    /// reached outside every scheduler region — which is exactly the shape
    /// `examples/desk.ply` has. A sequential accept loop never performs
    /// `task.spawn`, so no production region is ever opened and
    /// `Scheduler::next_host` — where the deadline is otherwise checked — never
    /// runs. Without the deadline here, a drain would sit inside one `net.recv`
    /// for that operation's own timeout however long ago `--drain-ms` elapsed,
    /// and the run would exit `0` having lost the request anyway.
    ///
    /// The bounded wait costs a wake-up every [`signal::DRAIN_POLL`] on a thread
    /// that is blocked regardless, and no latency: the condition variable is
    /// signalled the instant the token resolves. A run with no coordinator takes
    /// the unbounded path, so nothing outside a serving run pays even that.
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

    /// Rolls back every transaction scope this entry point left open and
    /// releases or discards the connections holding them, then closes every span
    /// it left open.
    ///
    /// **The order is pinned and it is the database first.** A span closed
    /// before the rollback could not record it; a span closed after it can, so
    /// the last records a dying request produced are the ones that say what it
    /// was doing.
    ///
    /// A connection returned to the pool with a transaction still open makes the
    /// *next* request read uncommitted rows of a request that already failed,
    /// and it is invisible from either request — which is why this runs on the
    /// diagnostic and budget-exhaustion paths and not only on the value path.
    ///
    /// Every worker gets a `Facilities` over one shared `Postgres` and one
    /// shared `Trace`, so `machine` is what keeps this teardown to its own entry
    /// point: without it, a test that never traced would close the span of one
    /// running beside it.
    fn end_entry_point(&self, machine: MachineId) -> Result<(), Diagnostic> {
        let database = self.close_database(machine);
        let spans = self.trace.end_entry_point(machine);
        // One diagnostic reaches the machine and two teardowns can produce one,
        // so the second travels as a note on the first rather than being
        // dropped. The database's is first because its order is: a lost
        // connection is a resource the run cannot get back, while an abandoned
        // span is a record that was written.
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
///
/// A failure here is `W0606` and does not change the exit code, because a
/// service that shut down uncleanly still shut down and an operator needs that
/// distinct from a drain that did not finish.
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
///
/// `W0608` and a warning, because the run's own configuration is at fault rather
/// than the program: `--drain-ms` was below what the program's own
/// `body_timeout_ms + write_timeout_ms` needs, and the run cannot check that
/// because `Limits` is a Ply value it never sees. The verdict is carried by the
/// exit code — `3` rather than `0` — so a deployment can tell a clean stop from
/// one that dropped requests, which is the whole product of a drain.
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

    /// The listing is the artifact this milestone exists to produce, so a change
    /// to it must be a change someone made on purpose. This test is not a
    /// golden-file check — it asserts the properties a reviewer relies on, which
    /// is what a golden file would only imply.
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

    /// `Repeatable` is a claim that replaying the operation changes nothing
    /// outside the program, and it is the one column that silently re-opens
    /// multi-shot resumption over the boundary. Every use of it in the trusted
    /// computing base is enumerated here, so adding one fails this test and has
    /// to be argued for.
    ///
    /// The arguments, one line each:
    ///
    /// - `config.get` and `config.secret` read a frozen `BTreeMap`. The sources
    ///   were read once, at bind time, and nothing can change one afterwards —
    ///   there is no `Snapshot` method that mutates and no `config.set` to add
    ///   one. Reading an immutable map twice is the definition of harmless, and
    ///   the moment a live reload existed this line would have to come out.
    /// - `signal.stopping` and `signal.deadline_ms` read a flag whose only
    ///   writer is the run's own shutdown coordinator. Observing it twice
    ///   changes nothing outside the program; the *answer* may differ between
    ///   the two reads, which is what `nondet` is for and not what this column
    ///   is about.
    /// - `task.*` create and observe a machine state and change nothing outside
    ///   the program, which is W2's argument unchanged.
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
