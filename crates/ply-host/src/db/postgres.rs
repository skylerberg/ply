//! The postgres [`Driver`]: the piece that joins the scanner, the type mapping,
//! the scope table and the connection pool into something a Ply `db` operation
//! resolves to.
//!
//! Nothing here decides anything. The scan happened in [`handler`], the
//! footprint was checked there, the depth arithmetic and the savepoint names
//! belong to [`scope`], and the connections belong to [`pool`]. What this module
//! owns is the *sequencing*: which of those to ask, in what order, and what the
//! answer to a resolved token means.
//!
//! [`handler`]: super::handler
//! [`scope`]: super::scope
//! [`pool`]: super::pool
//!
//! ## Why a token carries a continuation
//!
//! Every `db` operation is `blocking: true` and answers `Pending`, so the
//! machine parks and later polls. The work that has to happen *after* the server
//! answers is not the same for every operation — a `BEGIN` that succeeded opens
//! a scope and one that failed hands its connection straight back, a `COMMIT`
//! pops a scope and releases a lease either way — and none of it can happen on
//! the reactor thread, which holds no scope table. So each token is filed here
//! against what to do when it resolves, and [`Postgres::finish`] is the one
//! place that does it.
//!
//! The table is keyed by the reactor's own token, so there is no second token
//! space and `Reactor::owns` stays the single answer to "is this mine".

use super::handler::{Driver, Statement};
use super::pool::{self, Cleanup, LeaseId, Opened, Outcome, Reactor};
use super::scope::{Access, Isolation, Owner, ScopeTable, Step, sqlstate};
use super::types::DbError;
use super::{Op, stmt, value};
use ply_eval::Value;
use ply_eval::host::{HostAnswer, MachineId, Pending};
use ply_span::{Diagnostic, Span};
use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

/// What a resolved token still has to do.
enum Next {
    /// A data statement. The connection was borrowed and given back by the
    /// reactor, or it belongs to a scope that is still open — but the owner is
    /// kept, because a `Failed` inside a scope aborts it and the close has to
    /// know.
    Statement { owner: Owner },
    /// A `BEGIN`. The lease arrives with the answer, so both are settled here.
    Begin {
        owner: Owner,
        level: Isolation,
        access: Access,
    },
    /// A `SAVEPOINT`, which runs on a connection the scope already holds.
    Savepoint {
        owner: Owner,
        level: Isolation,
        access: Access,
        lease: LeaseId,
    },
    /// `COMMIT` or `ROLLBACK` on the outermost scope: the scope is popped and
    /// the connection released whatever the server said.
    Close {
        owner: Owner,
        lease: LeaseId,
        /// Whether this was a `COMMIT`. An abort clears the failed state and a
        /// commit does not, which is the whole of what the poison flag decides.
        commit: bool,
    },
    /// `RELEASE SAVEPOINT` or `ROLLBACK TO SAVEPOINT`: a scope is popped and no
    /// connection changes hands.
    Release { owner: Owner, commit: bool },
    /// A control operation the driver refused without a round trip. Nothing ran
    /// and no scope moved; the answer was decided before the token was minted.
    Refused { error: DbError },
}

impl Next {
    /// Whose entry point posted this token, so a teardown drops only its own.
    ///
    /// `None` for a refusal, which belongs to no scope stack: it was decided
    /// before the token was minted and no teardown has anything to say about it.
    fn owner(&self) -> Option<Owner> {
        match self {
            Next::Statement { owner }
            | Next::Begin { owner, .. }
            | Next::Savepoint { owner, .. }
            | Next::Close { owner, .. }
            | Next::Release { owner, .. } => Some(*owner),
            Next::Refused { .. } => None,
        }
    }
}

/// A `db` implementation over a real postgres.
pub struct Postgres {
    reactor: Reactor,
    /// Per entry point, not per run: `end_entry_point` empties it. Two tasks of
    /// one entry point are two stacks in it and neither can see the other's.
    scopes: Mutex<ScopeTable>,
    pending: Mutex<BTreeMap<u64, Next>>,
    statements: usize,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A control statement's answer, as the Ply `Answer` a program matches on.
///
/// `Count(0)` rather than a unit: `std.db` gives every operation one `Answer`,
/// and a transaction that opened affected no rows.
fn opened_ok() -> Value {
    value::answer(&stmt::Answer::Count(0))
}

fn failed(e: &DbError) -> Value {
    value::answer(&stmt::Answer::Failed(e.clone()))
}

/// What a close the server accepted answers.
///
/// A `COMMIT` on a block a statement already aborted is accepted by postgres —
/// with the command tag `ROLLBACK`, which `SimpleQueryMessage` does not carry —
/// so a driver that asked only whether the server errored would tell a program
/// its writes are durable when every one of them is gone. `std.db`'s
/// `transaction` matches only `Failed` on the commit, so this is the one place
/// that difference can be made.
fn aborted_or_ok(commit: bool, poisoned: bool) -> Value {
    if commit && poisoned {
        return failed(&DbError::new(
            sqlstate::TRANSACTION_ABORTED,
            "",
            "a statement in this transaction failed, so the commit rolled it back".to_string(),
        ));
    }
    opened_ok()
}

/// A peer that went away, which ADR 0014 §3.2 makes a value rather than a
/// diagnostic: a database that restarted is a peer, and ADR 0013 §7.1 already
/// decided what those are.
fn unreachable(why: &str) -> Value {
    failed(&DbError::connection(why.to_string()))
}

impl Postgres {
    /// Opens the pool. The connection is established lazily by `deadpool`, so a
    /// server that is down is `E0431` at bind time and not here.
    pub fn start(config: pool::PoolConfig) -> Result<Postgres, Diagnostic> {
        let statements = config.statements;
        Ok(Postgres {
            reactor: Reactor::start(config)?,
            scopes: Mutex::new(ScopeTable::new()),
            pending: Mutex::new(BTreeMap::new()),
            statements,
        })
    }

    pub fn reactor(&self) -> &Reactor {
        &self.reactor
    }

    /// How deep `owner`'s scope stack is. For a test that asserts an entry point
    /// left nothing open.
    pub fn depth(&self, owner: Owner) -> usize {
        lock(&self.scopes).depth(owner)
    }

    pub fn owns(&self, pending: &Pending) -> bool {
        self.reactor.owns(pending)
    }

    fn file(&self, pending: Pending, next: Next) -> HostAnswer {
        lock(&self.pending).insert(pending.token, next);
        HostAnswer::Pending(pending)
    }

    /// What a resolved token means, on the machine's thread.
    fn finish(&self, token: u64, outcome: Outcome) -> Result<Value, Diagnostic> {
        let Some(next) = lock(&self.pending).remove(&token) else {
            // A token this driver minted is filed before it can resolve, so
            // reaching here means the reactor answered one it did not mint.
            return Err(Diagnostic::error(
                ply_span::codes::INTERNAL_ERROR,
                format!("the database driver was handed token #{token}, which it never posted"),
            ));
        };
        match next {
            Next::Statement { owner } => Ok(match outcome {
                Outcome::Done(payload) => match *payload
                    .downcast::<Result<stmt::Answer, Diagnostic>>()
                    .map_err(|_| mismatched("a statement"))?
                {
                    // A statement the server refused has put the block it ran in
                    // into the failed state, and postgres answers the `COMMIT`
                    // that follows without an error. The scope remembers here so
                    // that the close can tell the program the truth.
                    Ok(answer) => {
                        if matches!(answer, stmt::Answer::Failed(_)) {
                            lock(&self.scopes).statement_failed(owner);
                        }
                        value::answer(&answer)
                    }
                    Err(refusal) => return Err(refusal),
                },
                Outcome::Unreachable(why) => {
                    lock(&self.scopes).statement_failed(owner);
                    unreachable(&why)
                }
                Outcome::Lease(_) => return Err(mismatched("a statement")),
            }),
            Next::Begin {
                owner,
                level,
                access,
            } => match outcome {
                Outcome::Done(payload) => {
                    let opened = payload
                        .downcast::<Opened>()
                        .map_err(|_| mismatched("`db.begin`"))?;
                    let ran = *opened
                        .payload
                        .downcast::<Result<(), DbError>>()
                        .map_err(|_| mismatched("`db.begin`"))?;
                    match ran {
                        Ok(()) => {
                            lock(&self.scopes).opened(owner, opened.lease, level, access);
                            Ok(opened_ok())
                        }
                        // The `BEGIN` the server refused opened nothing, so the
                        // scope table never hears about it and the connection
                        // goes back rolled back — it may have been the transaction
                        // that failed rather than the socket.
                        Err(e) => {
                            self.reactor.release(opened.lease, Cleanup::Rollback)?;
                            Ok(failed(&e))
                        }
                    }
                }
                Outcome::Unreachable(why) => Ok(unreachable(&why)),
                Outcome::Lease(lease) => {
                    self.reactor.release(lease, Cleanup::Clean)?;
                    Err(mismatched("`db.begin`"))
                }
            },
            Next::Savepoint {
                owner,
                level,
                access,
                lease,
            } => match control_result(outcome, "`db.begin`")? {
                Ok(()) => {
                    lock(&self.scopes).opened(owner, lease, level, access);
                    Ok(opened_ok())
                }
                Err(e) => Ok(failed(&e)),
            },
            Next::Close {
                owner,
                lease,
                commit,
            } => {
                let ran = control_result(outcome, "a transaction control")?;
                let closed = lock(&self.scopes).closed(owner, commit);
                // A close the server refused is always a rollback on release:
                // nothing here can tell a deferred constraint from a connection
                // that died mid-`COMMIT`, and the second must not go back to the
                // pool carrying a transaction.
                let cleanup = match &ran {
                    Ok(()) => Cleanup::Clean,
                    Err(_) => Cleanup::Rollback,
                };
                self.reactor.release(lease, cleanup)?;
                Ok(match ran {
                    Ok(()) => aborted_or_ok(commit, closed.poisoned),
                    Err(e) => failed(&e),
                })
            }
            Next::Refused { error } => Ok(failed(&error)),
            Next::Release { owner, commit } => {
                let ran = control_result(outcome, "a transaction control")?;
                let closed = lock(&self.scopes).closed(owner, commit);
                Ok(match ran {
                    Ok(()) => aborted_or_ok(commit, closed.poisoned),
                    Err(e) => failed(&e),
                })
            }
        }
    }

    pub fn poll(&self, pending: &Pending) -> Result<Option<Value>, Diagnostic> {
        match self.reactor.poll(pending)? {
            None => Ok(None),
            Some(outcome) => self.finish(pending.token, outcome).map(Some),
        }
    }

    pub fn block_on(&self, pending: Pending) -> Result<Value, Diagnostic> {
        let outcome = self.reactor.block_on(pending)?;
        self.finish(pending.token, outcome)
    }

    /// Roll back every scope still open and release or discard the connections
    /// holding them.
    ///
    /// Called on every exit path from an entry point. The report is data for a
    /// run-level warning: a discarded connection is the run's own fault and does
    /// not change the entry point's verdict.
    pub fn end_entry_point(&self, machine: MachineId) -> Result<pool::DrainReport, Diagnostic> {
        // Per entry point rather than per process, so the bound is the driver's
        // own: this runs on the value path of every request, not only at a stop,
        // and a run that is not stopping has no deadline to honour.
        let config = self.reactor.config();
        let bound = config.statement + config.connect;
        let leases = lock(&self.scopes).end_entry_point(machine);
        // This machine's tokens only. Clearing the table would resolve a
        // statement another entry point still has in flight to `E0505`, reported
        // as a defect in Ply against whichever definition happened to have posted
        // it.
        lock(&self.pending).retain(|_, next| next.owner().is_none_or(|o| o.0 != machine));
        let mut report = self.reactor.drain(&leases, bound)?;
        report.merge(self.reactor.take_discards());
        Ok(report)
    }

    /// Transaction scopes open right now, across every entry point.
    ///
    /// What the shutdown banner reports as "transactions open", and what a test
    /// asserts a drain left behind. A count and not a list: what an operator
    /// needs to know is that there were three, not which leases they were on.
    pub fn open_scopes(&self) -> usize {
        lock(&self.scopes).open_leases().len()
    }

    /// Step 1 of the process-level teardown: `ROLLBACK` every scope still open,
    /// whichever entry point opened it, and **wait for the rollbacks**.
    ///
    /// Separate from [`close_pool`](Postgres::close_pool) because the sink is
    /// flushed between them: a record naming a rolled-back transaction has to be
    /// written by a run that still holds the connection that rolled it back.
    ///
    /// Waited for rather than left to the disconnect. Closing a connection with
    /// a `BEGIN` still open does get the transaction aborted — by postgres, when
    /// it notices — which is the same outcome by luck instead of by
    /// construction, and on a server slow to notice it holds the row locks the
    /// rest of a rolling restart is waiting on.
    ///
    /// **Nothing is committed.** Not the outermost scope of a body that had one
    /// statement left, not a savepoint, not a scope whose every statement
    /// succeeded. The client got no answer, so a retry is what fixes it — and a
    /// retry cannot fix a committed half-transaction.
    pub fn roll_back_open_scopes(&self, budget: Duration) -> Result<pool::DrainReport, Diagnostic> {
        let leases = lock(&self.scopes).shutdown();
        // Every pending answer, because there is no entry point left to hand one
        // to: a token resolved after this would be a value nobody polls.
        lock(&self.pending).clear();
        let mut report = self.reactor.drain(&leases, budget)?;
        report.merge(self.reactor.take_discards());
        Ok(report)
    }

    /// Step 3: close the pool. Connections are closed rather than returned, and
    /// one whose `ROLLBACK` failed is discarded — which is ADR 0014 §1.3's
    /// existing rule and needs no new case.
    pub fn close_pool(&self, budget: Duration) -> Result<pool::DrainReport, Diagnostic> {
        self.reactor.shutdown(budget)
    }

    /// The statement text a control step runs, as a job on a connection.
    fn control(sql: String) -> pool::Job {
        pool::job(move |connection| async move {
            let out = stmt::control(&connection, &sql).await;
            (connection, out)
        })
    }
}

/// A control step's answer, unwrapped from the outcome shapes that cannot occur.
fn control_result(outcome: Outcome, what: &'static str) -> Result<Result<(), DbError>, Diagnostic> {
    match outcome {
        Outcome::Done(payload) => Ok(*payload
            .downcast::<Result<(), DbError>>()
            .map_err(|_| mismatched(what))?),
        Outcome::Unreachable(why) => Ok(Err(DbError::connection(why))),
        Outcome::Lease(_) => Err(mismatched(what)),
    }
}

#[cold]
fn mismatched(what: &str) -> Diagnostic {
    Diagnostic::error(
        ply_span::codes::INTERNAL_ERROR,
        format!("the database driver's token for {what} resolved to something else"),
    )
    .note("the reactor and the driver disagree about what a token was posted for")
}

impl Driver for Postgres {
    fn path(&self, op: Op) -> &'static str {
        op.path()
    }

    fn statement(&self, request: Statement<'_>) -> Result<HostAnswer, Diagnostic> {
        let what = request.op.what();
        let owner = request.owner;
        let lease = lock(&self.scopes).route(owner, what, request.span)?;
        let sql = request.sql.to_string();
        let params = request.params;
        let bound = self.statements;
        let span = request.span;
        let job = pool::job(move |connection| async move {
            let out = stmt::execute(&connection, &sql, &params, bound, span).await;
            (connection, out)
        });
        let pending = match lease {
            Some(lease) => self.reactor.on(lease, span, what, job)?,
            None => self.reactor.borrow(span, what, job)?,
        };
        Ok(self.file(pending, Next::Statement { owner }))
    }

    fn begin(
        &self,
        level: Isolation,
        access: Access,
        owner: Owner,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        let step = lock(&self.scopes).begin(owner, level, access);
        match step {
            Step::Open { sql } => {
                let pending =
                    self.reactor
                        .lease_running(span, Op::Begin.what(), Postgres::control(sql))?;
                Ok(self.file(
                    pending,
                    Next::Begin {
                        owner,
                        level,
                        access,
                    },
                ))
            }
            Step::Nested { lease, sql } => {
                let pending =
                    self.reactor
                        .on(lease, span, Op::Begin.what(), Postgres::control(sql))?;
                Ok(self.file(
                    pending,
                    Next::Savepoint {
                        owner,
                        level,
                        access,
                        lease,
                    },
                ))
            }
            // Answered through the reactor rather than inline: `db.begin` is
            // registered `blocking: true`, so a value returned from `call` is
            // `E0428` — the machine's thread having done the work. A refusal the
            // driver decided without a round trip is still an answer that has to
            // arrive as one.
            Step::Refused(e) => {
                let pending = self.reactor.settled(
                    span,
                    Op::Begin.what(),
                    Box::new(Ok::<(), DbError>(())) as pool::Payload,
                )?;
                Ok(self.file(pending, Next::Refused { error: e }))
            }
            Step::Close { .. } => Err(mismatched(Op::Begin.what())),
        }
    }

    fn commit(&self, owner: Owner, span: Span) -> Result<HostAnswer, Diagnostic> {
        let step = lock(&self.scopes).commit(owner, span)?;
        self.close_step(step, owner, span, Op::Commit)
    }

    fn abort(&self, owner: Owner, span: Span) -> Result<HostAnswer, Diagnostic> {
        let step = lock(&self.scopes).abort(owner, span)?;
        self.close_step(step, owner, span, Op::Abort)
    }
}

impl Postgres {
    fn close_step(
        &self,
        step: Step,
        owner: Owner,
        span: Span,
        op: Op,
    ) -> Result<HostAnswer, Diagnostic> {
        let commit = op == Op::Commit;
        match step {
            Step::Close {
                lease,
                sql,
                cleanup: _,
            } => {
                let pending = self
                    .reactor
                    .on(lease, span, op.what(), Postgres::control(sql))?;
                Ok(self.file(
                    pending,
                    Next::Close {
                        owner,
                        lease,
                        commit,
                    },
                ))
            }
            Step::Nested { lease, sql } => {
                let pending = self
                    .reactor
                    .on(lease, span, op.what(), Postgres::control(sql))?;
                Ok(self.file(pending, Next::Release { owner, commit }))
            }
            Step::Refused(e) => {
                let pending = self.reactor.settled(
                    span,
                    op.what(),
                    Box::new(Ok::<(), DbError>(())) as pool::Payload,
                )?;
                Ok(self.file(pending, Next::Refused { error: e }))
            }
            Step::Open { .. } => Err(mismatched(op.what())),
        }
    }
}

/// The `db` implementation a *listing* is taken over.
///
/// `ply hosts` answers "what does this run trust", and the postgres driver is
/// in the trusted computing base whether or not this particular invocation
/// opened a connection — so the rows have to be printable without one. This
/// serves those rows and refuses every operation.
///
/// It is never what a bound run gets: `Hosts::open` raises `E0431` before
/// anything evaluates when the run's reach names a `db` atom and no `--db` was
/// given. Reaching one of these methods therefore means a run got past that,
/// and the same code with a real span is the honest answer.
pub struct NotConfigured;

impl NotConfigured {
    fn refuse(span: Span) -> Diagnostic {
        Diagnostic::error(
            ply_span::codes::DB_NOT_CONFIGURED,
            "this run performs a `db` operation and named no database",
        )
        .primary(span, "there is no server for this to reach")
        .note("pass `--db postgres://user@host:5432/database`, or set `PLY_DB_URL`")
    }
}

impl Driver for NotConfigured {
    fn path(&self, op: Op) -> &'static str {
        op.path()
    }

    fn statement(&self, request: Statement<'_>) -> Result<HostAnswer, Diagnostic> {
        Err(NotConfigured::refuse(request.span))
    }

    fn begin(
        &self,
        _: Isolation,
        _: Access,
        _: Owner,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        Err(NotConfigured::refuse(span))
    }

    fn commit(&self, _: Owner, span: Span) -> Result<HostAnswer, Diagnostic> {
        Err(NotConfigured::refuse(span))
    }

    fn abort(&self, _: Owner, span: Span) -> Result<HostAnswer, Diagnostic> {
        Err(NotConfigured::refuse(span))
    }
}

/// The SQLSTATEs this module can produce without asking a server, re-exported so
/// a caller reading a `Failed` can name them.
pub use sqlstate::{
    ACTIVE_TRANSACTION, NO_ACTIVE_TRANSACTION, PROGRAM_LIMIT_EXCEEDED, TRANSACTION_ABORTED,
};

/// What the shutdown coordinator asks the database for: one number, for one line
/// of output and for the drain's own report. It answers and changes nothing.
impl crate::signal::Transactions for Postgres {
    fn open_scopes(&self) -> usize {
        Postgres::open_scopes(self)
    }
}
