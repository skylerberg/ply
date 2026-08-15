//! The connection pool, and the one thread postgres is spoken to from.
//!
//! ADR 0014 §3. A `db` operation is `blocking: true`: it dispatches to the
//! reactor and answers [`HostAnswer::Pending`] immediately, so a task waiting
//! on a query leaves the enabled set instead of holding a thread. That is not a
//! nicety — a blocking acquire on a scheduler thread stalls every task in the
//! region, and W1 has no cancellation to break it with.
//!
//! Four things go wrong in production and each decides a piece of this file:
//!
//! - **Acquisition under contention.** Every acquire carries
//!   [`PoolConfig::acquire`] as a deadline, and exceeding it is `E0437` naming
//!   the pool size, the number checked out and the operation that waited. A
//!   pool smaller than the number of open scopes would otherwise park every
//!   task forever with nothing to report, and the deadline is what turns that
//!   hang into a sentence.
//! - **A connection whose transaction was abandoned.** [`Cleanup::Rollback`] is
//!   issued *before* the connection goes back, and a rollback that fails closes
//!   the connection rather than returning it. A connection recycled with an
//!   open transaction makes the *next* request read uncommitted rows of a
//!   request that already failed, and it is invisible from either request.
//! - **A connection the server closed, or one that timed out mid-borrow.**
//!   Every checkout re-establishes the session state ([`session_sql`]) over the
//!   same connection, which is a round trip: a connection the server has hung
//!   up on fails there and is discarded and replaced rather than handed out.
//! - **Shutdown.** [`Reactor::drain`] and [`Reactor::shutdown`] wait for work
//!   already in flight instead of dropping it, and report what they had to
//!   abandon rather than pretending they had to abandon nothing.
//!
//! **A connection never leaves the reactor's thread.** The machine's thread
//! holds a [`LeaseId`], which is a name; the [`Connection`] behind it is owned
//! by a task on the reactor. That is what makes a scope's connection reusable
//! across the statements of a transaction without any of the lifetimes a
//! borrowed handle would need, and it keeps [`ply_eval::Value`] — which holds
//! `Rc` — on the one thread it belongs to.
//!
//! [`HostAnswer::Pending`]: ply_eval::HostAnswer::Pending

use deadpool_postgres::{
    Manager, ManagerConfig, Object, Pool as DeadPool, PoolError, RecyclingMethod, Runtime,
    TimeoutType, Timeouts,
};
use ply_eval::Pending;
use ply_span::{Diagnostic, Span, codes};
use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinSet;

/// `--db-pool`: connections in the pool.
pub const DEFAULT_POOL_SIZE: usize = 8;
/// `--db-acquire-ms`: waiting for a connection.
pub const DEFAULT_ACQUIRE_MS: u64 = 5_000;
/// `--db-statement-ms`: server-side `statement_timeout`.
pub const DEFAULT_STATEMENT_MS: u64 = 30_000;
/// `--db-idle-txn-ms`: server-side `idle_in_transaction_session_timeout`.
pub const DEFAULT_IDLE_TXN_MS: u64 = 30_000;
/// `--db-connect-ms`: establishing a connection.
pub const DEFAULT_CONNECT_MS: u64 = 5_000;

/// A connection checked out of the pool, as the reactor's tasks hold it.
pub type Connection = Object;

/// What a job hands back. Not a [`ply_eval::Value`]: a `Value` holds `Rc` and
/// never crosses a thread, so the reactor produces plain data and the machine's
/// thread builds the value when it polls.
pub type Payload = Box<dyn Any + Send>;

/// The work a driver runs on a pooled connection.
///
/// It takes the connection by value and gives it back, which is why nothing
/// here has a lifetime: an async block that owns its connection is a future
/// that borrows nothing, and the alternative — a job handed `&mut Connection` —
/// is a higher-ranked bound no closure can be inferred against.
pub type Job = Box<
    dyn FnOnce(Connection) -> Pin<Box<dyn Future<Output = (Connection, Payload)> + Send>>
        + Send
        + 'static,
>;

/// Build a [`Job`] from an ordinary async closure.
///
/// ```ignore
/// reactor.borrow(span, "`db.query`", pool::job(|c| async move {
///     let out = c.simple_query("select 1").await.map_err(|e| e.to_string());
///     (c, out)
/// }))
/// ```
pub fn job<F, Fut, T>(f: F) -> Job
where
    F: FnOnce(Connection) -> Fut + Send + 'static,
    Fut: Future<Output = (Connection, T)> + Send + 'static,
    T: Send + 'static,
{
    Box::new(move |connection| {
        Box::pin(async move {
            let (connection, value) = f(connection).await;
            (connection, Box::new(value) as Payload)
        })
    })
}

/// A connection held across the statements of one transaction scope.
///
/// A name rather than a handle: the connection itself stays on the reactor's
/// thread, and this is what the driver's per-task scope stack stores.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct LeaseId(u64);

impl LeaseId {
    /// A name for a connection this reactor did not mint.
    ///
    /// The scope table stores lease ids and decides what happens to them without
    /// ever holding a connection, so its own tests name connections without a
    /// pool behind them. Handing one of these to a [`Reactor`] is
    /// `err_unknown_lease` and not a wrong connection, which is what makes the
    /// constructor safe to expose.
    pub fn named(id: u64) -> LeaseId {
        LeaseId(id)
    }
}

impl fmt::Display for LeaseId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "lease #{}", self.0)
    }
}

/// What the driver believes it is handing back.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cleanup {
    /// No open transaction. The connection goes straight back.
    ///
    /// A mistaken `Clean` is not a silent hazard: the checkout that follows
    /// runs [`session_sql`] with a leading `ROLLBACK`, which is the second lock
    /// ADR 0014 §1.3 asks for and costs nothing, because that checkout was
    /// going to be a round trip anyway.
    Clean,
    /// A scope the driver believes is open: `ROLLBACK` before release, and if
    /// the rollback fails the connection is closed and discarded rather than
    /// returned.
    Rollback,
    /// Do not return it at all. For a session the driver knows is unusable.
    Discard,
}

/// What a pending `db` token resolves to.
#[derive(Debug)]
pub enum Outcome {
    /// A connection, held until [`Reactor::release`]. Only from
    /// [`Reactor::lease`].
    Lease(LeaseId),
    /// A job's result, for the driver to downcast and turn into a `Value`.
    Done(Payload),
    /// No connection could be established. ADR 0014 §3.2: a connect failure
    /// *during* a run is the program's `Failed` with SQLSTATE `08006`, because
    /// a database that restarted is a peer that went away — not a diagnostic,
    /// which is what a connect failure at bind time is.
    Unreachable(String),
}

/// A lease and what the job that opened it answered.
///
/// The payload is the opening job's own return value, boxed exactly as
/// [`Outcome::Done`]'s is: the driver downcasts it to the type it posted.
pub struct Opened {
    pub lease: LeaseId,
    pub payload: Payload,
}

impl fmt::Debug for Opened {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Opened").field("lease", &self.lease).finish()
    }
}

/// A connection that was closed instead of returned to the pool.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Discarded {
    pub lease: Option<LeaseId>,
    pub reason: String,
}

/// What [`Reactor::drain`] and [`Reactor::shutdown`] did.
///
/// ADR 0014 §1.3: `end_entry_point` failing is not the program's fault and does
/// not change the entry point's verdict, so this is data for a run-level
/// warning rather than a diagnostic the driver must raise.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct DrainReport {
    /// Scopes that were still open and were rolled back.
    pub rolled_back: usize,
    /// Connections closed rather than returned, and why.
    pub discarded: Vec<Discarded>,
    /// Jobs that were still running and were waited for rather than dropped.
    pub awaited: usize,
    /// Jobs that had not finished when the bound expired and were abandoned.
    /// Non-zero means a statement outlived `--db-statement-ms`, which is a
    /// server that stopped answering rather than a slow query.
    pub abandoned: usize,
}

impl DrainReport {
    /// Whether everything was handed back intact — what a run reports nothing
    /// about.
    pub fn is_clean(&self) -> bool {
        self.discarded.is_empty() && self.abandoned == 0
    }

    /// One line for a run-level warning, or `None` when there is nothing to
    /// say.
    pub fn describe(&self) -> Option<String> {
        if self.is_clean() {
            return None;
        }
        let mut parts = Vec::new();
        if !self.discarded.is_empty() {
            parts.push(format!(
                "{} connection{} closed rather than returned to the pool ({})",
                self.discarded.len(),
                if self.discarded.len() == 1 { "" } else { "s" },
                self.discarded
                    .iter()
                    .map(|d| d.reason.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        if self.abandoned > 0 {
            parts.push(format!(
                "{} operation(s) abandoned mid-flight",
                self.abandoned
            ));
        }
        Some(parts.join(", "))
    }

    /// Fold another report into this one, for a caller that drains in two
    /// steps — the leases it knows about, then whatever the reactor discarded
    /// on its own.
    pub fn merge(&mut self, other: DrainReport) {
        self.rolled_back += other.rolled_back;
        self.discarded.extend(other.discarded);
        self.awaited += other.awaited;
        self.abandoned += other.abandoned;
    }
}

/// What `ply hosts` prints in its `pool` line, and what `E0437` names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PoolStatus {
    /// `--db-pool`.
    pub size: usize,
    /// Connections actually established, which is at most `size`.
    pub open: usize,
    pub checked_out: usize,
    /// Callers waiting for one.
    pub waiting: usize,
    /// Connections held across a transaction scope.
    pub leases: usize,
}

/// Everything the run decided about the database before anything ran.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PoolConfig {
    /// The `--db` connection string, in either libpq form.
    pub url: String,
    pub size: usize,
    pub acquire: Duration,
    pub statement: Duration,
    pub idle_txn: Duration,
    pub connect: Duration,
    /// `--db-statement-cache`: prepared statements kept per connection.
    pub statements: usize,
}

impl PoolConfig {
    pub fn new(url: impl Into<String>) -> PoolConfig {
        PoolConfig {
            url: url.into(),
            size: DEFAULT_POOL_SIZE,
            acquire: Duration::from_millis(DEFAULT_ACQUIRE_MS),
            statement: Duration::from_millis(DEFAULT_STATEMENT_MS),
            idle_txn: Duration::from_millis(DEFAULT_IDLE_TXN_MS),
            connect: Duration::from_millis(DEFAULT_CONNECT_MS),
            statements: super::stmt::DEFAULT_STATEMENT_CACHE,
        }
    }

    /// The `tokio_postgres` configuration this run connects with.
    ///
    /// Every refusal here is `E0431`: it is the run's configuration at fault
    /// and it is known before anything runs.
    pub fn pg_config(&self) -> Result<tokio_postgres::Config, Diagnostic> {
        let mut config: tokio_postgres::Config = self.url.parse().map_err(|e| {
            err_not_configured(format!(
                "`--db` is not a connection string postgres accepts: {e}"
            ))
            .note("either a URL — `postgresql://user@host:5432/dbname` — or libpq keyword form")
        })?;
        match config.get_ssl_mode() {
            tokio_postgres::config::SslMode::Disable | tokio_postgres::config::SslMode::Prefer => {}
            other => {
                return Err(err_not_configured(format!(
                    "`--db` asks for `sslmode={}`, which W4 does not configure",
                    match other {
                        tokio_postgres::config::SslMode::Require => "require",
                        _ => "verify-full",
                    }
                ))
                .note("W4 accepts `sslmode=disable` and `sslmode=prefer` only")
                .note("wiring rustls into the postgres client is a real decision about the trusted computing base, and ADR 0014 §10 puts it beside W5's secrets rather than shipping it as an untested line"));
            }
        }
        if self.size == 0 {
            return Err(err_not_configured("`--db-pool` is 0, so no statement could ever run")
                .note("the pool's size is how many connections exist; a pool of none is a service that cannot answer"));
        }
        for (name, flag, value) in [
            ("statement_timeout", "--db-statement-ms", self.statement),
            (
                "idle_in_transaction_session_timeout",
                "--db-idle-txn-ms",
                self.idle_txn,
            ),
        ] {
            if value.is_zero() {
                return Err(err_not_configured(format!("`{flag}` is 0, which postgres reads as no {name} at all"))
                    .note("a statement with no server-side timeout holds a pool slot until the server restarts, and an idle transaction holds locks the rest of the service is waiting on")
                    .note("a bound nobody chose is a bound set to infinity, so this one is not optional"));
            }
        }
        config.connect_timeout(self.connect);
        // So an operator — and ADR 0014 §13's tests, which read
        // `pg_stat_activity` rather than the driver's own bookkeeping — can
        // tell this run's backends from everything else on the server.
        if config.get_application_name().is_none() {
            config.application_name("ply");
        }
        Ok(config)
    }
}

/// What every checkout runs, in one round trip.
///
/// `reset` prefixes the whole of the session reset, which is ADR 0014 §1.3's
/// second lock widened to everything a connection can carry. A `ROLLBACK` alone
/// leaves *every other piece of session state* for the next borrower, and a
/// pooled connection is the only thing two host-backed tests share:
///
/// - a `search_path` one borrower set decides which relation an unqualified name
///   resolves to for the next;
/// - a session-level advisory lock is held until the backend exits, so a later
///   borrower blocks on it until `--db-statement-ms`;
/// - a `LISTEN` delivers a notification into a conversation that never asked;
/// - a temporary table shadows a real one of the same name;
/// - a `PREPARE`d name collides with the next borrower's.
///
/// None of that is visible from either borrower and §3.4's footprint conflict
/// grouping cannot see it at all — it is a channel underneath the thing that is
/// supposed to be the whole of the isolation.
///
/// `DISCARD ALL` is still refused, for §4.3's reason: it would drop the prepared
/// statement cache the pool exists to amortise. These five do not, and the
/// checkout is already a round trip. `RESET ALL` runs **before** the two `SET`s,
/// which would otherwise be the state it reset.
///
/// One thing is deliberately **not** reset and the residual is stated rather
/// than left to be discovered: a SQL-level `PREPARE`d name. It shares
/// `pg_prepared_statements` with the protocol-level statements the cache is
/// built from, so `DEALLOCATE ALL` would invalidate a cache the pool still
/// believes is live. Nothing a program performs can leave one — `PREPARE` is
/// not among the statement shapes §2.4 admits — so the channel is closed at the
/// scanner instead.
///
/// The two `SET`s are not optional and ADR 0014 §3.2 says why: a statement with
/// no server-side timeout holds a pool slot until the server restarts, and an
/// idle transaction holds locks the rest of the service is waiting on.
pub fn session_sql(config: &PoolConfig, reset: bool) -> String {
    let mut sql = String::new();
    if reset {
        sql.push_str(
            "ROLLBACK; RESET ALL; UNLISTEN *; SELECT pg_advisory_unlock_all(); \
             CLOSE ALL; DISCARD TEMP; ",
        );
    }
    sql.push_str(&format!(
        "SET statement_timeout = {}; SET idle_in_transaction_session_timeout = {}",
        config.statement.as_millis(),
        config.idle_txn.as_millis()
    ));
    sql
}

/// The one OS thread postgres is spoken to from, and the pool on it.
///
/// One current-thread tokio runtime, because every connection's future is
/// independent and the pool bounds the concurrency: a work-stealing runtime
/// would buy nothing and make the thread count a number nobody chose.
pub struct Reactor {
    config: PoolConfig,
    shared: Arc<Shared>,
    commands: UnboundedSender<Command>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Reactor {
    /// Build the pool, start the thread, and prove the database is reachable.
    ///
    /// The proof is the point: ADR 0014 §3.2 makes a connect failure at bind
    /// time `E0431`, so a run whose database is unreachable stops before it has
    /// told anyone it was listening.
    pub fn start(config: PoolConfig) -> Result<Reactor, Diagnostic> {
        let pg = config.pg_config()?;
        let manager = Manager::from_config(
            pg,
            tokio_postgres::NoTls,
            ManagerConfig {
                // Not `Fast`, which only asks `is_closed()` and hands out a
                // hard-closed socket, and not `Clean`, whose `RESET ALL` would
                // undo the session state this very query establishes.
                recycling_method: RecyclingMethod::Custom(session_sql(&config, true)),
            },
        );
        let pool = DeadPool::builder(manager)
            .max_size(config.size)
            .runtime(Runtime::Tokio1)
            .build()
            .map_err(|e| {
                err_not_configured(format!("the connection pool could not be built: {e}"))
            })?;

        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            finished: Condvar::new(),
            next_token: AtomicU64::new(1),
            next_lease: AtomicU64::new(1),
            pool: pool.clone(),
        });
        let (commands, receiver) = unbounded_channel();
        let (ready, started) = std::sync::mpsc::channel();

        let thread_config = config.clone();
        let thread_shared = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("ply-db-reactor".to_string())
            .spawn(move || {
                reactor_thread(thread_config, pool, thread_shared, receiver, ready);
            })
            .map_err(|e| {
                err_not_configured(format!("the database reactor thread could not start: {e}"))
            })?;

        match started.recv() {
            Ok(Ok(())) => Ok(Reactor {
                config,
                shared,
                commands,
                thread: Mutex::new(Some(thread)),
            }),
            Ok(Err(why)) => {
                let _ = thread.join();
                Err(err_not_configured(why))
            }
            // The thread died without answering: it panicked before it could.
            Err(_) => {
                let _ = thread.join();
                Err(err_not_configured(
                    "the database reactor thread stopped before it could reach the server",
                ))
            }
        }
    }

    pub fn config(&self) -> &PoolConfig {
        &self.config
    }

    pub fn status(&self) -> PoolStatus {
        let status = self.shared.pool.status();
        PoolStatus {
            size: self.config.size,
            open: status.size,
            checked_out: status.size.saturating_sub(status.available),
            waiting: status.waiting,
            leases: lock(&self.shared.state).held.len(),
        }
    }

    /// Acquire a connection, run `job` on it, and give it back — one token for
    /// the whole thing.
    ///
    /// What a statement outside a transaction scope does. Three tokens for one
    /// statement would be three parks, and the scheduler would have paid for a
    /// factoring rather than for anything a program can observe.
    pub fn borrow(&self, span: Span, what: &'static str, job: Job) -> Result<Pending, Diagnostic> {
        let token = self.open(span, what)?;
        self.post(Command::Borrow { token, job }, token)
    }

    /// Acquire a connection and keep it until [`Reactor::release`].
    ///
    /// What `db.begin` does. The statements inside the scope then reuse it
    /// through [`Reactor::on`] and never wait for the pool, which is ADR 0014
    /// §3.2's "a statement inside a scope reuses the scope's connection".
    pub fn lease(&self, span: Span, what: &'static str) -> Result<Pending, Diagnostic> {
        let token = self.open(span, what)?;
        self.post(
            Command::Lease {
                token,
                opening: None,
            },
            token,
        )
    }

    /// Acquire a connection, run `job` on it, and **keep** it — one token for
    /// the acquisition and the statement that opens the scope.
    ///
    /// What `db.begin` does. Two tokens would be two parks for one perform, and
    /// worse, a `BEGIN` the server refused between them would leave the machine
    /// holding a lease nothing opened. The token answers
    /// [`Outcome::Done`] carrying an [`Opened`], so the driver learns the lease
    /// and what the opening statement said in the same resolution and can
    /// release the one when the other failed.
    pub fn lease_running(
        &self,
        span: Span,
        what: &'static str,
        job: Job,
    ) -> Result<Pending, Diagnostic> {
        let token = self.open(span, what)?;
        self.post(
            Command::Lease {
                token,
                opening: Some(job),
            },
            token,
        )
    }

    /// Run `job` on a connection already leased. Never waits for the pool.
    pub fn on(
        &self,
        lease: LeaseId,
        span: Span,
        what: &'static str,
        job: Job,
    ) -> Result<Pending, Diagnostic> {
        {
            let state = lock(&self.shared.state);
            if !state.held.contains(&lease) {
                return Err(err_unknown_lease(lease, what));
            }
        }
        let token = self.open(span, what)?;
        self.post(Command::On { token, lease, job }, token)
    }

    /// Give a lease back.
    ///
    /// Returns as soon as the cleanup is posted. The connection does not
    /// re-enter the pool until it has finished, so a borrower waiting on an
    /// exhausted pool waits for the rollback rather than racing it, and
    /// [`Reactor::drain`] is what waits for the answer.
    pub fn release(&self, lease: LeaseId, cleanup: Cleanup) -> Result<(), Diagnostic> {
        {
            let mut state = lock(&self.shared.state);
            if !state.held.remove(&lease) {
                return Err(err_unknown_lease(lease, "a release"));
            }
        }
        self.send(Command::Release {
            lease,
            cleanup,
            ack: None,
        })
    }

    /// Release these leases and wait for the rollbacks to finish.
    ///
    /// What `end_entry_point` calls on **every** exit path — a value, a
    /// diagnostic, or a spent budget. It waits for the leases named rather than
    /// for the whole reactor, because `ply test`'s workers share one pool and a
    /// drain that waited for every connection would serialise the run.
    pub fn drain(&self, leases: &[LeaseId]) -> Result<DrainReport, Diagnostic> {
        let mut report = DrainReport::default();
        let (ack, acks) = std::sync::mpsc::channel();
        let mut posted = 0;
        for lease in leases {
            {
                let mut state = lock(&self.shared.state);
                if !state.held.remove(lease) {
                    continue;
                }
            }
            self.send(Command::Release {
                lease: *lease,
                cleanup: Cleanup::Rollback,
                ack: Some(ack.clone()),
            })?;
            posted += 1;
        }
        drop(ack);
        let bound = self.config.statement + self.config.connect;
        for _ in 0..posted {
            match acks.recv_timeout(bound) {
                Ok(released) => {
                    report.rolled_back += 1;
                    if let Some(reason) = released.discarded {
                        report.discarded.push(Discarded {
                            lease: Some(released.lease),
                            reason,
                        });
                    }
                    report.awaited += released.awaited;
                }
                // The cleanup outlived `--db-statement-ms` plus the connect
                // deadline, so the server has stopped answering rather than
                // being slow. Waiting longer would trade a report for a hang.
                Err(_) => report.abandoned += 1,
            }
        }
        Ok(report)
    }

    /// Every connection closed rather than returned since the last call.
    ///
    /// [`Reactor::release`] is fire-and-forget, so a rollback that failed there
    /// has nobody waiting to hear about it; this is where those land. It is
    /// **run-level and not per-entry-point**: `ply test`'s workers share one
    /// pool, so a discard cannot be attributed to whichever entry point happens
    /// to ask next, and [`Reactor::drain`] deliberately does not fold these in.
    pub fn take_discards(&self) -> DrainReport {
        let mut state = lock(&self.shared.state);
        DrainReport {
            discarded: std::mem::take(&mut state.discarded),
            ..DrainReport::default()
        }
    }

    /// Every lease still held. What a driver's teardown hands [`drain`].
    ///
    /// [`drain`]: Reactor::drain
    pub fn leases(&self) -> Vec<LeaseId> {
        lock(&self.shared.state).held.iter().copied().collect()
    }

    /// Stop: refuse new work, finish what is in flight, roll back and close
    /// every connection, and join the thread.
    ///
    /// Idempotent, and called from [`Drop`] if a run never gets here.
    pub fn shutdown(&self) -> Result<DrainReport, Diagnostic> {
        let thread = {
            let mut held = self.thread.lock().unwrap_or_else(|e| e.into_inner());
            match held.take() {
                Some(thread) => thread,
                None => return Ok(self.take_discards()),
            }
        };
        let (ack, acked) = std::sync::mpsc::channel();
        let mut report = if self.send(Command::Stop { ack }).is_ok() {
            acked
                .recv_timeout(
                    self.config.acquire.max(self.config.statement) + self.config.connect * 2,
                )
                .unwrap_or_else(|_| DrainReport {
                    abandoned: 1,
                    ..DrainReport::default()
                })
        } else {
            DrainReport::default()
        };
        let _ = thread.join();
        lock(&self.shared.state).stopped = true;
        self.shared.finished.notify_all();
        report.merge(self.take_discards());
        Ok(report)
    }

    /// Whether this reactor minted the token. What a composed runtime routes
    /// on.
    pub fn owns(&self, pending: &Pending) -> bool {
        let state = lock(&self.shared.state);
        state.waiting.contains_key(&pending.token) || state.done.contains_key(&pending.token)
    }

    /// How many operations are outstanding. A `db` operation costs one of these
    /// and **no** [`MAX_BLOCKING_OPERATIONS`] slot: the reactor is one thread
    /// for the whole pool rather than one thread per waiting operation, so the
    /// two capacities are independent.
    ///
    /// [`MAX_BLOCKING_OPERATIONS`]: crate::tcp::MAX_BLOCKING_OPERATIONS
    pub fn outstanding(&self) -> usize {
        lock(&self.shared.state).waiting.len()
    }

    pub fn poll(&self, pending: &Pending) -> Result<Option<Outcome>, Diagnostic> {
        let mut state = lock(&self.shared.state);
        match take(&mut state, pending.token) {
            Taken::Ready(result) => result.map(Some),
            Taken::Waiting => Ok(None),
            Taken::Unknown => Err(err_unknown_token(pending)),
        }
    }

    /// Block until at least one outstanding operation has finished.
    pub fn park(&self) -> Result<(), Diagnostic> {
        let mut state = lock(&self.shared.state);
        if state.waiting.is_empty() && state.done.is_empty() {
            return Err(err_park_with_nothing_outstanding());
        }
        while state.done.is_empty() {
            if state.stopped {
                return Err(err_reactor_stopped());
            }
            state = wait(&self.shared.finished, state);
        }
        Ok(())
    }

    /// The same, with a bound. `Ok(false)` is the bound expiring.
    ///
    /// A composed [`HostRuntime`] holds more than one facility, and two
    /// facilities behind two condition variables cannot be waited on together:
    /// parking on the socket pool while the query is the token that resolves is
    /// a hang. A bounded park is what lets the composed runtime alternate.
    ///
    /// [`HostRuntime`]: ply_eval::host::HostRuntime
    pub fn park_timeout(&self, bound: Duration) -> Result<bool, Diagnostic> {
        let mut state = lock(&self.shared.state);
        if state.waiting.is_empty() && state.done.is_empty() {
            return Err(err_park_with_nothing_outstanding());
        }
        let deadline = std::time::Instant::now() + bound;
        while state.done.is_empty() {
            if state.stopped {
                return Err(err_reactor_stopped());
            }
            let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) else {
                return Ok(false);
            };
            let (guard, timed_out) = self
                .shared
                .finished
                .wait_timeout(state, left)
                .unwrap_or_else(|e| e.into_inner());
            state = guard;
            if timed_out.timed_out() && state.done.is_empty() {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Drive until this token resolves.
    ///
    /// Reached outside a scheduler region, where there is no task to park, and
    /// at bind time, where there is no machine at all. The wait is on the
    /// reactor's thread doing the work, so nothing here holds a connection.
    pub fn block_on(&self, pending: Pending) -> Result<Outcome, Diagnostic> {
        let mut state = lock(&self.shared.state);
        loop {
            match take(&mut state, pending.token) {
                Taken::Ready(result) => return result,
                Taken::Unknown => return Err(err_unknown_token(&pending)),
                Taken::Waiting => {
                    if state.stopped {
                        return Err(err_reactor_stopped());
                    }
                    state = wait(&self.shared.finished, state)
                }
            }
        }
    }

    /// A token that is already answered.
    ///
    /// Every `db` operation is `blocking: true`, so a handler that returned a
    /// value inline is `E0428` — correctly: the declaration says the work left
    /// the machine's thread and a token is coming back. Some control operations
    /// are decided **without** a round trip all the same: a nested `begin` at a
    /// different isolation level, a savepoint stack past its bound, a `commit`
    /// with nothing open. Those are refusals the driver computes on its own, and
    /// they still have to arrive the way every other answer does.
    pub fn settled(
        &self,
        span: Span,
        what: &'static str,
        payload: Payload,
    ) -> Result<Pending, Diagnostic> {
        let token = self.open(span, what)?;
        let mut state = lock(&self.shared.state);
        state.waiting.remove(&token);
        state.done.insert(token, Ok(Outcome::Done(payload)));
        drop(state);
        self.shared.finished.notify_all();
        Ok(Pending {
            token,
            label: "query",
        })
    }

    fn open(&self, span: Span, what: &'static str) -> Result<u64, Diagnostic> {
        let token = self.shared.next_token.fetch_add(1, Ordering::Relaxed);
        let mut state = lock(&self.shared.state);
        if state.stopped {
            return Err(err_reactor_stopped());
        }
        state.waiting.insert(token, Waiting { span, what });
        Ok(token)
    }

    fn post(&self, command: Command, token: u64) -> Result<Pending, Diagnostic> {
        if let Err(e) = self.send(command) {
            lock(&self.shared.state).waiting.remove(&token);
            return Err(e);
        }
        Ok(Pending {
            token,
            label: "query",
        })
    }

    fn send(&self, command: Command) -> Result<(), Diagnostic> {
        self.commands
            .send(command)
            .map_err(|_| err_reactor_stopped())
    }
}

impl Drop for Reactor {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

impl fmt::Debug for Reactor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reactor")
            .field("config", &self.config)
            .field("status", &self.status())
            .finish()
    }
}

/// One reactor serves a whole run, shared across the test runner's workers by
/// `Arc`, so it has to be shareable — and a field that broke that would be
/// added in this file, which is why the requirement is stated in this file.
const _: fn() = || {
    fn shareable<T: Send + Sync>() {}
    shareable::<Reactor>();
};

// ---------------------------------------------------------------------------
// The machine's side of the shared state.
// ---------------------------------------------------------------------------

struct Waiting {
    span: Span,
    /// The operation, as a diagnostic names it: `` `db.begin` ``.
    what: &'static str,
}

#[derive(Default)]
struct State {
    waiting: BTreeMap<u64, Waiting>,
    done: BTreeMap<u64, Result<Outcome, Diagnostic>>,
    /// Leases the machine's thread believes it holds.
    held: BTreeSet<LeaseId>,
    discarded: Vec<Discarded>,
    stopped: bool,
}

struct Shared {
    state: Mutex<State>,
    /// Signalled by a job finishing, so neither `park` nor `block_on` spins.
    finished: Condvar,
    next_token: AtomicU64,
    next_lease: AtomicU64,
    /// Cloned rather than moved to the reactor: `status()` is read from the
    /// machine's thread, and `deadpool`'s own counters are the only honest
    /// source for `E0437`'s numbers.
    pool: DeadPool,
}

impl Shared {
    fn publish(&self, token: u64, outcome: Result<Outcome, Diagnostic>) {
        let mut state = lock(&self.state);
        state.done.insert(token, outcome);
        drop(state);
        self.finished.notify_all();
    }

    fn discard(&self, lease: Option<LeaseId>, reason: String) {
        lock(&self.state)
            .discarded
            .push(Discarded { lease, reason });
    }
}

enum Taken {
    Ready(Result<Outcome, Diagnostic>),
    Waiting,
    Unknown,
}

fn take(state: &mut State, token: u64) -> Taken {
    let Some(done) = state.done.remove(&token) else {
        return if state.waiting.contains_key(&token) {
            Taken::Waiting
        } else {
            Taken::Unknown
        };
    };
    let _ = state.waiting.remove(&token);
    Taken::Ready(done)
}

/// Publishes a failure if the task carrying the token dies without answering.
///
/// A panic inside a driver's job would otherwise leave the machine parked on a
/// token nothing will ever resolve, which is the worst shape a defect at this
/// boundary can take: the run hangs with nothing to read.
struct TokenGuard {
    shared: Arc<Shared>,
    token: Option<u64>,
    what: &'static str,
}

impl TokenGuard {
    fn new(shared: &Arc<Shared>, token: u64, what: &'static str) -> TokenGuard {
        TokenGuard {
            shared: Arc::clone(shared),
            token: Some(token),
            what,
        }
    }

    fn publish(&mut self, outcome: Result<Outcome, Diagnostic>) {
        if let Some(token) = self.token.take() {
            self.shared.publish(token, outcome);
        }
    }
}

impl Drop for TokenGuard {
    fn drop(&mut self) {
        let what = self.what;
        self.publish(Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("the database reactor lost the answer to {what}"),
        )
        .note("the task carrying it ended without producing one, which means it panicked")
        .note(
            "this is Ply's fault: report it with the program that produced it",
        )));
    }
}

// ---------------------------------------------------------------------------
// The reactor's side.
// ---------------------------------------------------------------------------

enum Command {
    Borrow {
        token: u64,
        job: Job,
    },
    Lease {
        token: u64,
        /// Run on the connection before the lease is published, which is what
        /// makes `BEGIN` one token rather than two.
        opening: Option<Job>,
    },
    On {
        token: u64,
        lease: LeaseId,
        job: Job,
    },
    Release {
        lease: LeaseId,
        cleanup: Cleanup,
        ack: Option<std::sync::mpsc::Sender<Released>>,
    },
    Stop {
        ack: std::sync::mpsc::Sender<DrainReport>,
    },
}

/// What a lease's task does next.
enum LeaseCommand {
    Run {
        token: u64,
        job: Job,
        what: &'static str,
    },
    Release {
        cleanup: Cleanup,
        ack: Option<std::sync::mpsc::Sender<Released>>,
    },
}

struct Released {
    lease: LeaseId,
    discarded: Option<String>,
    /// Jobs the lease was still running when the release arrived.
    awaited: usize,
}

type Leases = Arc<Mutex<BTreeMap<LeaseId, UnboundedSender<LeaseCommand>>>>;

fn reactor_thread(
    config: PoolConfig,
    pool: DeadPool,
    shared: Arc<Shared>,
    commands: UnboundedReceiver<Command>,
    ready: std::sync::mpsc::Sender<Result<(), String>>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            let _ = ready.send(Err(format!("the database reactor has no runtime: {e}")));
            return;
        }
    };
    runtime.block_on(async move {
        // The bind-time proof. A pool that has never connected is
        // indistinguishable from a pool whose server is down, and `E0431` is
        // the difference between a run that stops and a service that discovers
        // it on the first request.
        match pool.timeout_get(&timeouts(&config)).await {
            Ok(connection) => {
                drop(connection);
                let _ = ready.send(Ok(()));
            }
            Err(e) => {
                let _ = ready.send(Err(format!(
                    "the database named by `--db` could not be reached: {}",
                    describe(&e)
                )));
                return;
            }
        }
        serve(config, pool, shared, commands).await;
    });
}

async fn serve(
    config: PoolConfig,
    pool: DeadPool,
    shared: Arc<Shared>,
    mut commands: UnboundedReceiver<Command>,
) {
    let leases: Leases = Arc::new(Mutex::new(BTreeMap::new()));
    let mut tasks: JoinSet<()> = JoinSet::new();

    loop {
        // Reaping is what keeps the set from growing for the life of the run.
        // It is not what delivers an answer — a task publishes its own, through
        // `TokenGuard` even when it panics — so it costs nothing to do it here
        // rather than to wait on it.
        while tasks.try_join_next().is_some() {}
        let Some(command) = commands.recv().await else {
            break;
        };
        match command {
            Command::Borrow { token, job } => {
                let _ = tasks.spawn(borrow(
                    pool.clone(),
                    Arc::clone(&shared),
                    config.clone(),
                    token,
                    job,
                ));
            }
            Command::Lease { token, opening } => {
                let _ = tasks.spawn(lease(
                    pool.clone(),
                    Arc::clone(&shared),
                    config.clone(),
                    Arc::clone(&leases),
                    token,
                    opening,
                ));
            }
            Command::On { token, lease, job } => {
                let what = what_of(&shared, token);
                let sender = leases
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(&lease)
                    .cloned();
                match sender {
                    Some(sender) if sender.send(LeaseCommand::Run { token, job, what }).is_ok() => {
                    }
                    _ => shared.publish(token, Err(err_unknown_lease(lease, what))),
                }
            }
            Command::Release {
                lease,
                cleanup,
                ack,
            } => {
                let sender = leases
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&lease);
                match sender {
                    Some(sender)
                        if sender
                            .send(LeaseCommand::Release {
                                cleanup,
                                ack: ack.clone(),
                            })
                            .is_ok() => {}
                    // Nothing holds it: the lease's own task already ended, so
                    // the connection is back and there is nothing to roll back.
                    _ => {
                        if let Some(ack) = ack {
                            let _ = ack.send(Released {
                                lease,
                                discarded: None,
                                awaited: 0,
                            });
                        }
                    }
                }
            }
            Command::Stop { ack } => {
                let report = stop(&config, &pool, &leases, &mut tasks).await;
                let _ = ack.send(report);
                return;
            }
        }
    }
    // The last `Reactor` was dropped without a `shutdown`. Everything still
    // leased is abandoned by definition, so it is rolled back rather than
    // returned as-is.
    let _ = stop(&config, &pool, &leases, &mut tasks).await;
}

/// Refuse new work, roll back every lease, wait for what is in flight, and
/// close the pool.
///
/// The wait is bounded, and the bound is stated rather than infinite: a
/// statement that outlived `--db-statement-ms` is a server that stopped
/// answering, and waiting longer would trade a report for a hang. What is
/// abandoned is counted rather than hidden.
async fn stop(
    config: &PoolConfig,
    pool: &DeadPool,
    leases: &Leases,
    tasks: &mut JoinSet<()>,
) -> DrainReport {
    let mut report = DrainReport::default();
    let (ack, acks) = std::sync::mpsc::channel();
    let open: Vec<UnboundedSender<LeaseCommand>> = {
        let mut held = leases.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *held).into_values().collect()
    };
    let expected = open.len();
    for sender in open {
        let _ = sender.send(LeaseCommand::Release {
            cleanup: Cleanup::Rollback,
            ack: Some(ack.clone()),
        });
    }
    drop(ack);

    // Long enough that nothing still capable of finishing is cut off: a task in
    // flight is either waiting for the pool (`acquire`) or waiting on the server
    // (`statement`), and a cleanup after either is bounded by `connect`.
    let bound = config.acquire.max(config.statement) + config.connect;
    let waited =
        tokio::time::timeout(bound, async { while tasks.join_next().await.is_some() {} }).await;
    if waited.is_err() {
        report.abandoned += tasks.len();
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    }

    for _ in 0..expected {
        match acks.try_recv() {
            Ok(released) => {
                report.rolled_back += 1;
                report.awaited += released.awaited;
                if let Some(reason) = released.discarded {
                    report.discarded.push(Discarded {
                        lease: Some(released.lease),
                        reason,
                    });
                }
            }
            Err(_) => break,
        }
    }
    pool.close();
    report
}

async fn borrow(pool: DeadPool, shared: Arc<Shared>, config: PoolConfig, token: u64, job: Job) {
    let what = what_of(&shared, token);
    let mut guard = TokenGuard::new(&shared, token, what);
    let connection = match acquire(&pool, &shared, &config, token).await {
        Ok(connection) => connection,
        Err(refusal) => {
            guard.publish(refusal);
            return;
        }
    };
    let (connection, payload) = job(connection).await;
    // Released before the answer is published, so that a machine which sees the
    // result and immediately asks for another connection finds this one back in
    // the pool rather than racing it.
    if let Err(reason) = finish(connection, Cleanup::Clean, config.connect).await {
        shared.discard(None, reason);
    }
    guard.publish(Ok(Outcome::Done(payload)));
}

async fn lease(
    pool: DeadPool,
    shared: Arc<Shared>,
    config: PoolConfig,
    leases: Leases,
    token: u64,
    opening: Option<Job>,
) {
    let what = what_of(&shared, token);
    let mut guard = TokenGuard::new(&shared, token, what);
    let mut connection = match acquire(&pool, &shared, &config, token).await {
        Ok(connection) => connection,
        Err(refusal) => {
            guard.publish(refusal);
            return;
        }
    };
    let opened = match opening {
        None => None,
        Some(job) => {
            let (returned, payload) = job(connection).await;
            connection = returned;
            Some(payload)
        }
    };
    let id = LeaseId(shared.next_lease.fetch_add(1, Ordering::Relaxed));
    let (sender, receiver) = unbounded_channel();
    // Registered on both sides *before* the answer is published: the machine
    // may post a statement the instant it reads the lease id, and a lease the
    // reactor has not recorded yet would be refused as unknown.
    leases
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id, sender);
    lock(&shared.state).held.insert(id);
    guard.publish(Ok(match opened {
        None => Outcome::Lease(id),
        Some(payload) => Outcome::Done(Box::new(Opened { lease: id, payload })),
    }));

    hold(id, connection, receiver, shared, config, leases).await;
}

/// One lease, as a task that owns its connection.
///
/// Serialising a scope's statements is not a policy: a postgres connection
/// carries one conversation, and two statements in flight on it at once is a
/// protocol violation. A task with a queue makes that structural, and it is
/// also what lets a release wait for a statement already running instead of
/// cutting it off.
async fn hold(
    id: LeaseId,
    mut connection: Connection,
    mut commands: UnboundedReceiver<LeaseCommand>,
    shared: Arc<Shared>,
    config: PoolConfig,
    leases: Leases,
) {
    let mut ran = 0usize;
    while let Some(command) = commands.recv().await {
        match command {
            LeaseCommand::Run { token, job, what } => {
                let mut guard = TokenGuard::new(&shared, token, what);
                let (returned, payload) = job(connection).await;
                connection = returned;
                ran += 1;
                guard.publish(Ok(Outcome::Done(payload)));
            }
            LeaseCommand::Release { cleanup, ack } => {
                let discarded = finish(connection, cleanup, config.connect).await.err();
                let _ = leases.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
                if let Some(reason) = &discarded
                    && ack.is_none()
                {
                    shared.discard(Some(id), reason.clone());
                }
                if let Some(ack) = ack {
                    let _ = ack.send(Released {
                        lease: id,
                        discarded,
                        awaited: ran,
                    });
                }
                return;
            }
        }
    }
    // Every sender is gone without a release: the reactor is shutting down and
    // the scope was never closed. It is abandoned, so it is rolled back.
    if let Err(reason) = finish(connection, Cleanup::Rollback, config.connect).await {
        shared.discard(Some(id), reason);
    }
    let _ = leases.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
}

/// What to hand the machine when an acquisition produced no connection: a value
/// the driver turns into `Failed`, or a diagnostic that stops the run.
type Refusal = Result<Outcome, Diagnostic>;

async fn acquire(
    pool: &DeadPool,
    shared: &Arc<Shared>,
    config: &PoolConfig,
    token: u64,
) -> Result<Connection, Refusal> {
    match pool.timeout_get(&timeouts(config)).await {
        Ok(connection) => Ok(connection),
        // The pool was full for the whole deadline. The run's configuration is
        // at fault rather than the program's — the program asked for a
        // connection and the run said how many exist — so it is a diagnostic
        // and not a `Failed`. A `Failed` is a value a program is invited to
        // swallow, and a swallowed pool exhaustion is a service that returns
        // wrong answers under exactly the load that produced it.
        Err(PoolError::Timeout(TimeoutType::Wait)) => {
            Err(Err(err_exhausted(shared, config, token)))
        }
        // Everything else is the server: unreachable, refusing the connection,
        // or too slow to establish one. ADR 0013 §7.1's rule about a peer that
        // went away, applied to the second peer.
        Err(e) => Err(Ok(Outcome::Unreachable(describe(&e)))),
    }
}

async fn finish(connection: Connection, cleanup: Cleanup, bound: Duration) -> Result<(), String> {
    match cleanup {
        Cleanup::Clean => {
            drop(connection);
            Ok(())
        }
        Cleanup::Discard => {
            drop(Object::take(connection));
            Ok(())
        }
        Cleanup::Rollback => {
            match tokio::time::timeout(bound, connection.simple_query("ROLLBACK")).await {
                Ok(Ok(_)) => {
                    drop(connection);
                    Ok(())
                }
                Ok(Err(e)) => {
                    drop(Object::take(connection));
                    Err(format!("the rollback on release failed: {e}"))
                }
                Err(_) => {
                    drop(Object::take(connection));
                    Err(format!(
                        "the rollback on release did not finish within {}ms",
                        bound.as_millis()
                    ))
                }
            }
        }
    }
}

fn timeouts(config: &PoolConfig) -> Timeouts {
    Timeouts {
        wait: Some(config.acquire),
        create: Some(config.connect),
        // The recycle round trip is `session_sql`, which talks to the server:
        // bounding it by the connect deadline is what stops a connection the
        // server has stopped answering on from consuming the whole acquire
        // deadline before it is discarded.
        recycle: Some(config.connect),
    }
}

fn what_of(shared: &Arc<Shared>, token: u64) -> &'static str {
    lock(&shared.state)
        .waiting
        .get(&token)
        .map(|w| w.what)
        .unwrap_or("a database operation")
}

fn describe(error: &PoolError) -> String {
    match error {
        PoolError::Timeout(TimeoutType::Create) => {
            "the server did not complete a connection within `--db-connect-ms`".to_string()
        }
        PoolError::Timeout(TimeoutType::Recycle) => {
            "the server did not answer on a pooled connection within `--db-connect-ms`".to_string()
        }
        PoolError::Timeout(TimeoutType::Wait) => {
            "no connection became available within `--db-acquire-ms`".to_string()
        }
        PoolError::Closed => "the connection pool is closed".to_string(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Diagnostics.
// ---------------------------------------------------------------------------

/// `E0437` — the pool was full for the whole acquire deadline.
///
/// Names the three numbers a reader needs to act: how many connections exist,
/// how many are out, and what waited. W5 owns backpressure and is where this
/// becomes a shed request rather than a stop.
fn err_exhausted(shared: &Arc<Shared>, config: &PoolConfig, token: u64) -> Diagnostic {
    let status = shared.pool.status();
    let (span, what) = {
        let state = lock(&shared.state);
        match state.waiting.get(&token) {
            Some(waiting) => (waiting.span, waiting.what),
            None => (Span::DUMMY, "a database operation"),
        }
    };
    let checked_out = status.size.saturating_sub(status.available);
    // `waiting` counts the *others*: this operation gave up its place before
    // this diagnostic was built, so it is no longer one of them.
    Diagnostic::error(
        codes::DB_POOL_EXHAUSTED,
        format!(
            "no connection became available for {what} within {}ms",
            config.acquire.as_millis()
        ),
    )
    .primary(span, "this operation waited for a connection and gave up")
    .note(format!(
        "the pool holds {} connection{}, {checked_out} checked out, with {} other operation{} waiting",
        config.size,
        if config.size == 1 { "" } else { "s" },
        status.waiting,
        if status.waiting == 1 { "" } else { "s" },
    ))
    .note("the run's configuration decides how many connections exist; the program only asked for one")
    .note("raise `--db-pool`, raise `--db-acquire-ms`, or reduce the transactions open at once")
    .note("every transaction holds a connection from `db.begin` to its commit or abort, so a pool smaller than the transactions in flight cannot make progress")
}

fn err_not_configured(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::DB_NOT_CONFIGURED, message)
        .note("the connection string is configured beside the run rather than written in the program: a password in a definition's hash is in a store designed never to forget")
}

#[cold]
#[inline(never)]
fn err_unknown_lease(lease: LeaseId, what: &str) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("{what} named {lease}, which the connection pool does not hold"),
    )
    .note("a lease is released once, and a statement performed after its scope closed has no connection to run on")
    .note("this is Ply's fault: report it with the program that produced it")
}

#[cold]
#[inline(never)]
fn err_unknown_token(pending: &Pending) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("the database reactor was polled for `{pending}`, which it did not mint"),
    )
    .note("a pending token belongs to the facility that answered the operation; polling the wrong one loses the result rather than waiting for it")
    .note("this is Ply's fault: report it with the program that produced it")
}

#[cold]
#[inline(never)]
fn err_park_with_nothing_outstanding() -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        "the database reactor was asked to wait with no operation outstanding",
    )
    .note("nothing would ever wake it; this is a scheduler bug rather than a fault in the program")
}

#[cold]
#[inline(never)]
fn err_reactor_stopped() -> Diagnostic {
    Diagnostic::error(
        codes::DB_NOT_CONFIGURED,
        "the database reactor has stopped, so no statement can reach the server",
    )
    .note("the run shut the pool down, or the reactor's thread ended before the run did")
}

/// A poisoned lock means a task thread panicked. The state behind it is four
/// collections and none has an invariant a panic can break, so recovering is
/// correct and propagating would take out the machine's thread as well.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

fn wait<'a, T>(condvar: &Condvar, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    condvar.wait(guard).unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests;
