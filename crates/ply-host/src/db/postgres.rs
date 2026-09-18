//! The postgres [`Driver`]: joins the scope table and the connection pool into `db` operations.

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
    Statement {
        owner: Owner,
    },
    Begin {
        owner: Owner,
        level: Isolation,
        access: Access,
    },
    /// A nested `BEGIN`: a `SAVEPOINT` on the connection the scope already holds.
    Savepoint {
        owner: Owner,
        level: Isolation,
        access: Access,
        lease: LeaseId,
    },
    /// Outermost `COMMIT` or `ROLLBACK`: the connection is released whatever the server said.
    Close {
        owner: Owner,
        lease: LeaseId,
        commit: bool,
    },
    /// `RELEASE SAVEPOINT` or `ROLLBACK TO SAVEPOINT`: no connection changes hands.
    Release {
        owner: Owner,
        commit: bool,
    },
    Refused {
        error: DbError,
    },
}

impl Next {
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

pub struct Postgres {
    reactor: Reactor,
    /// Per entry point, not per run: `end_entry_point` empties it.
    scopes: Mutex<ScopeTable>,
    pending: Mutex<BTreeMap<u64, Next>>,
    statements: usize,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn opened_ok() -> Value {
    value::answer(&stmt::Answer::Count(0))
}

fn failed(e: &DbError) -> Value {
    value::answer(&stmt::Answer::Failed(e.clone()))
}

/// Postgres accepts a `COMMIT` of a failed transaction by rolling back; report that as aborted.
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

/// A lost database is a peer failure, so it is a value rather than a diagnostic.
fn unreachable(why: &str) -> Value {
    failed(&DbError::connection(why.to_string()))
}

impl Postgres {
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

    fn finish(&self, token: u64, outcome: Outcome) -> Result<Value, Diagnostic> {
        let Some(next) = lock(&self.pending).remove(&token) else {
            // Tokens are filed before they can resolve, so this one was never minted here.
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
                    // Postgres accepts `COMMIT` after a failed statement, so record the failure.
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
                        // A refused `BEGIN` opens no scope; roll back on release in case the
                        // transaction rather than the socket failed.
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
                // A refused close may be a connection that died mid-`COMMIT`, which must not go
                // back to the pool carrying a transaction.
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

    pub fn end_entry_point(&self, machine: MachineId) -> Result<pool::DrainReport, Diagnostic> {
        // Runs on every request, not only at a stop, so there is no shutdown deadline to honour.
        let config = self.reactor.config();
        let bound = config.statement + config.connect;
        let leases = lock(&self.scopes).end_entry_point(machine);
        lock(&self.pending).retain(|_, next| next.owner().is_none_or(|o| o.0 != machine));
        let mut report = self.reactor.drain(&leases, bound)?;
        report.merge(self.reactor.take_discards());
        Ok(report)
    }

    pub fn open_scopes(&self) -> usize {
        lock(&self.scopes).open_leases().len()
    }

    /// Step 1 of process teardown.
    pub fn roll_back_open_scopes(&self, budget: Duration) -> Result<pool::DrainReport, Diagnostic> {
        let leases = lock(&self.scopes).shutdown();
        // No entry point is left to poll a pending answer.
        lock(&self.pending).clear();
        let mut report = self.reactor.drain(&leases, budget)?;
        report.merge(self.reactor.take_discards());
        Ok(report)
    }

    /// Step 3 of process teardown.
    pub fn close_pool(&self, budget: Duration) -> Result<pool::DrainReport, Diagnostic> {
        self.reactor.shutdown(budget)
    }

    fn control(sql: String) -> pool::Job {
        pool::job(move |connection| async move {
            let out = stmt::control(&connection, &sql).await;
            (connection, out)
        })
    }
}

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
            // `db.begin` is registered blocking, so an inline answer would be `E0428`.
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

/// The `db` implementation for a run that names no database, such as a listing.
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

pub use sqlstate::{
    ACTIVE_TRANSACTION, NO_ACTIVE_TRANSACTION, PROGRAM_LIMIT_EXCEEDED, TRANSACTION_ABORTED,
};

impl crate::signal::Transactions for Postgres {
    fn open_scopes(&self) -> usize {
        Postgres::open_scopes(self)
    }
}
