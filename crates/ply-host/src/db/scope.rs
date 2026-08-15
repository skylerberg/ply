//! Transaction scopes: what `db.begin`, `db.commit` and `db.abort` do to the
//! connection they run on, and to the driver's account of what is open.
//!
//! The Ply half of a transaction is in `std.db`, and it is one `handle` whose
//! single clause discards its continuation. This is the other half, and the
//! division is the point: **the language scopes the abort and the driver scopes
//! the connection.** A `transaction` intercepts `db.rollback` and nothing else,
//! so the statements inside it are routed onto the open scope's connection by
//! this table rather than by a handler clause — which is what lets `transaction`
//! be a library function instead of one clause per table per operation.
//!
//! Nothing here issues a statement. Every operation answers a [`Step`] naming
//! the SQL to run and what to do with the connection afterwards, and the driver
//! runs it and reports back through [`ScopeTable::opened`] or
//! [`ScopeTable::closed`]. Two things fall out of
//! that shape and both are worth the indirection: the depth arithmetic, the
//! isolation rules and the savepoint names are decided by code with no runtime
//! in it and are unit-testable without a database, and the state never advances
//! on a transition the server refused.
//!
//! Four exits, and three of them are the ones a real system gets wrong:
//!
//! | exit | what happens |
//! | --- | --- |
//! | `commit()` | `COMMIT`, or `RELEASE SAVEPOINT` when nested. A failure closes the scope rolled back and is a `Failed` value |
//! | `db.rollback(r)` | the Ply clause discards the continuation and performs `abort()`, which is `ROLLBACK` or `ROLLBACK TO SAVEPOINT` |
//! | the body **raises** | the raise propagates past the `handle`; nothing was committed and the scope is **still open** |
//! | the entry point ends with a scope open | [`ScopeTable::end_entry_point`] rolls back every one of them |
//!
//! That last row is why this module holds a table rather than a stack. A
//! connection returned to the pool with a transaction still open makes the
//! *next* request read uncommitted rows of a request that already failed, and it
//! is invisible from either request.

use super::pool::{Cleanup, LeaseId};
use super::types::DbError;
use ply_eval::TaskId;
use ply_eval::host::MachineId;
use ply_span::{Diagnostic, Span, codes};
use std::collections::BTreeMap;
use std::fmt;

/// How many savepoints may be open below the outermost transaction.
///
/// Exceeding it is a `Failed` with `54000` rather than a diagnostic: it is a
/// program that recursed, and the program is what must stop. A savepoint is
/// cheap but not free — postgres keeps a subtransaction per one — and a
/// recursive helper with no bound would consume the server's resources rather
/// than its own.
pub const MAX_SAVEPOINTS: usize = 16;

/// The isolation a transaction runs at.
///
/// `ReadUncommitted` is **not** offered. Postgres implements it as read
/// committed, so a name in Ply's source that promised dirty reads would be a
/// name that lies.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Isolation {
    ReadCommitted,
    RepeatableRead,
    Serializable,
}

impl Isolation {
    /// As `BEGIN` spells it.
    pub fn sql(self) -> &'static str {
        match self {
            Isolation::ReadCommitted => "READ COMMITTED",
            Isolation::RepeatableRead => "REPEATABLE READ",
            Isolation::Serializable => "SERIALIZABLE",
        }
    }

    /// As the Ply constructor spells it, which is what a diagnostic and a
    /// `Failed`'s detail have to name for a reader to find the call site.
    pub fn as_str(self) -> &'static str {
        match self {
            Isolation::ReadCommitted => "ReadCommitted",
            Isolation::RepeatableRead => "RepeatableRead",
            Isolation::Serializable => "Serializable",
        }
    }

    /// The inverse, for the decoder that has a `Value::Ctor` and needs the
    /// level. One table rather than two: a decoder that mapped the constructor
    /// straight to SQL would be a second place the three levels are enumerated,
    /// and the one this module compares against would not be it.
    pub fn from_ctor(name: &str) -> Option<Isolation> {
        [
            Isolation::ReadCommitted,
            Isolation::RepeatableRead,
            Isolation::Serializable,
        ]
        .into_iter()
        .find(|level| level.as_str() == name)
    }
}

impl fmt::Display for Isolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Access {
    ReadWrite,
    ReadOnly,
}

impl Access {
    pub fn sql(self) -> &'static str {
        match self {
            Access::ReadWrite => "READ WRITE",
            Access::ReadOnly => "READ ONLY",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Access::ReadWrite => "ReadWrite",
            Access::ReadOnly => "ReadOnly",
        }
    }

    pub fn from_ctor(name: &str) -> Option<Access> {
        [Access::ReadWrite, Access::ReadOnly]
            .into_iter()
            .find(|access| access.as_str() == name)
    }
}

impl fmt::Display for Access {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The identity a scope belongs to: the machine that performed the operation and
/// the task inside it, if any.
///
/// **Both halves are load-bearing.** The task alone is not an identity: every
/// entry point outside a scheduler region reports `None`, and `ply test` runs the
/// members of a non-conflicting concurrency group on rayon threads, each driving
/// a machine of its own. Keyed on the task alone, all of them are one owner —
/// one entry point's statement runs inside another's transaction, on another's
/// connection, and either one's teardown ends the other's. Nothing in the
/// scheduler can prevent that: transaction control carries the singleton
/// `db.write`, so it only serialises transaction-*opening* tests against each
/// other, and a test whose row is empty is in a different group by construction.
///
/// `None` for the task is one identity rather than an absence of one — an entry
/// point that never spawned is a single thread of control, and a scope it opened
/// belongs to it.
pub type Owner = (MachineId, Option<TaskId>);

/// The SQLSTATEs this module answers with, spelled once.
///
/// Every one of them is a *transaction* that cannot be had rather than a program
/// that cannot run — a nesting too deep, a level that disagrees with the open
/// scope's, a commit with nothing to commit — so each is a `Failed(DbError)` the
/// program matches on exactly as it matches on a unique violation. The server
/// itself uses these codes for these conditions, which is why the driver does
/// not invent one.
pub mod sqlstate {
    /// `active_sql_transaction` — a nested `begin` asking for an isolation or an
    /// access the open scope cannot give it. A savepoint has neither.
    pub const ACTIVE_TRANSACTION: &str = "25001";
    /// `no_active_sql_transaction` — a `commit` or an `abort` with no scope
    /// open.
    pub const NO_ACTIVE_TRANSACTION: &str = "25P01";
    /// `program_limit_exceeded` — nesting past [`super::MAX_SAVEPOINTS`].
    pub const PROGRAM_LIMIT_EXCEEDED: &str = "54000";
    /// `in_failed_sql_transaction` — the scope a statement already aborted.
    ///
    /// Postgres answers `COMMIT` on an aborted transaction block with the command
    /// tag `ROLLBACK` and **no error**, so a driver that only asks whether the
    /// server returned an error reports a success for a transaction the server
    /// threw away. `tokio_postgres::SimpleQueryMessage` does not carry the tag,
    /// so the scope remembers instead: the first `Failed` inside it poisons it,
    /// and the close answers this.
    pub const TRANSACTION_ABORTED: &str = "25P02";
}

/// What a control operation asks the driver to do next.
///
/// Every variant that names SQL also names what the connection is for
/// afterwards, so there is no path where a `COMMIT` is issued and the decision
/// about the lease is made somewhere else.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Step {
    /// Acquire a connection and run this on it. The scope opens on
    /// [`ScopeTable::opened`] and not before, so a `BEGIN` the server refused
    /// leaves no scope behind and the connection acquired for it is the
    /// driver's to give back.
    Open { sql: String },
    /// Run this on the scope's own connection. A statement inside a scope never
    /// waits for the pool.
    Nested { lease: LeaseId, sql: String },
    /// Run this, then hand the connection back with `cleanup`.
    Close {
        lease: LeaseId,
        sql: String,
        /// What to do with the connection when the SQL **succeeded**. A close
        /// that failed is always [`Cleanup::Rollback`]: the driver cannot tell a
        /// deferred constraint from a connection that died mid-`COMMIT`, and
        /// the second must not go back to the pool carrying a transaction.
        cleanup: Cleanup,
    },
    /// Nothing to run. The answer was decided here.
    Refused(DbError),
}

/// One open scope.
#[derive(Clone, PartialEq, Eq, Debug)]
struct Scope {
    /// The level the outermost `BEGIN` set. A savepoint has no level of its
    /// own, so a nested scope carries the one it inherited — which is what makes
    /// the mismatch check one comparison against the innermost scope.
    level: Isolation,
    /// What the call site asked for. A nested `ReadOnly` inside a `ReadWrite` is
    /// accepted and its statements are still writable, because postgres has no
    /// read-only savepoint: the narrowing is **documentation and not
    /// enforcement**, which is the only honest thing to say about it.
    access: Access,
    /// Whether a statement inside this scope has already failed.
    ///
    /// Postgres puts the whole block into the failed state on the first error and
    /// then answers a `COMMIT` on it with the command tag `ROLLBACK` and no
    /// error. Without this flag `db.commit()` is a `Count(0)` — a success — for a
    /// transaction whose every write is gone, and `std.db`'s `transaction`
    /// evaluates to `Ok(value)`. That is §5.2's "the one that makes a suite pass
    /// and production fail", one layer below where the twin models it.
    poisoned: bool,
}

/// Every scope one owner has open, innermost last, and the connection they share.
#[derive(Clone, PartialEq, Eq, Debug)]
struct Held {
    lease: LeaseId,
    open: Vec<Scope>,
}

impl Held {
    fn depth(&self) -> usize {
        self.open.len()
    }

    fn innermost(&self) -> &Scope {
        self.open.last().expect("a held stack is never empty")
    }
}

/// The driver's account of every transaction scope every entry point has open.
///
/// One table for the run, keyed by [`Owner`] — the **machine** and the task —
/// which is what makes it an identity rather than a collision. `ply test`'s
/// workers each drive their own machine and every one of them performs outside a
/// scheduler region, so a table keyed on the task alone files all of them under
/// the single key `None`: one entry point's statement runs inside another's
/// transaction, and either one's teardown ends the other's. The pool is shared;
/// the account of who is inside what is per machine.
///
/// Two tasks of one entry point each in their own transaction are two stacks on
/// two connections and neither can see the other's — which is the arrangement a
/// pool exists to serve, and which is why the task is half of the key rather
/// than the whole of it.
#[derive(Default)]
pub struct ScopeTable {
    held: BTreeMap<Owner, Held>,
}

impl ScopeTable {
    pub fn new() -> ScopeTable {
        ScopeTable::default()
    }

    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    /// How deep `owner`'s scope stack is: `0` for no transaction, `1` for a
    /// transaction, `n + 1` for `n` savepoints inside one.
    pub fn depth(&self, owner: Owner) -> usize {
        self.held.get(&owner).map_or(0, Held::depth)
    }

    /// The connection a `db` operation performed by `owner` runs on.
    ///
    /// `Ok(None)` is a statement with no transaction anywhere in this entry
    /// point: it borrows a connection, runs in postgres's implicit transaction,
    /// and gives the connection back. `Ok(Some(lease))` is a statement inside
    /// its own scope, which never waits for the pool.
    ///
    /// `Err` is **`E0436`**, and it is the case where the two answers available
    /// are both wrong. A task spawned inside another task's `transaction` body
    /// has no scope of its own; running its statement on the owner's connection
    /// is a protocol violation, because a postgres connection carries one
    /// conversation and two tasks writing into it interleave two; and quietly
    /// borrowing a second connection puts the statement **outside** the
    /// transaction its author believed it was inside, so a body that rolled back
    /// would leave that one statement's work committed. So it refuses, and the
    /// program says which of the two it meant — by handing the work to the task
    /// that owns the scope, or by giving the spawned task its own `transaction`.
    ///
    /// The cost is stated rather than hidden: a task doing unrelated work while
    /// a sibling holds a transaction is refused too, because a spawn carries no
    /// relationship the host boundary can read and nothing here can tell the two
    /// apart. Refusing is the direction that fails loudly.
    pub fn route(
        &self,
        owner: Owner,
        what: &str,
        span: Span,
    ) -> Result<Option<LeaseId>, Diagnostic> {
        if let Some(held) = self.held.get(&owner) {
            return Ok(Some(held.lease));
        }
        // Only this machine's other tasks. A scope another entry point holds is
        // not this one's business at all: it runs on a connection of its own and
        // nothing here can see it, so reporting `E0436` for it would blame a
        // program for a test that happened to be running beside it.
        match self.sibling(owner) {
            Some(other) => Err(err_transaction_scope(span, what, owner, other)),
            None => Ok(None),
        }
    }

    /// Another task of the **same machine** holding a scope, if there is one.
    fn sibling(&self, owner: Owner) -> Option<Owner> {
        self.held
            .keys()
            .find(|(machine, _)| *machine == owner.0)
            .copied()
    }

    /// `db.begin`.
    ///
    /// On an empty stack this is a transaction; on a non-empty one it is a
    /// **savepoint**, which is the decision that lets a helper called both
    /// standalone and from inside a larger operation exist once. Refusing to
    /// nest was the alternative and it is worse: every such helper would exist
    /// twice, and two copies of a write path is the drift this milestone exists
    /// to measure.
    pub fn begin(&mut self, owner: Owner, level: Isolation, access: Access) -> Step {
        let Some(held) = self.held.get(&owner) else {
            return Step::Open {
                sql: format!("BEGIN ISOLATION LEVEL {} {}", level.sql(), access.sql()),
            };
        };

        // A savepoint has no isolation level and no access mode, so a nested
        // `begin` that asked for a different one would be a call site saying a
        // thing that does not happen. That is exactly the silent difference this
        // design refuses everywhere else, so it is not silent here.
        let inner = held.innermost();
        if level != inner.level {
            return Step::Refused(DbError::new(
                sqlstate::ACTIVE_TRANSACTION,
                "",
                format!(
                    "a nested transaction asked for {level} inside an open {}; a savepoint has no isolation level of its own",
                    inner.level
                ),
            ));
        }
        if access == Access::ReadWrite && inner.access == Access::ReadOnly {
            return Step::Refused(DbError::new(
                sqlstate::ACTIVE_TRANSACTION,
                "",
                format!(
                    "a nested transaction asked for {access} inside an open {}; a savepoint cannot widen what the transaction may do",
                    inner.access
                ),
            ));
        }

        if held.depth() > MAX_SAVEPOINTS {
            return Step::Refused(DbError::new(
                sqlstate::PROGRAM_LIMIT_EXCEEDED,
                "",
                format!(
                    "{MAX_SAVEPOINTS} savepoints are already open, which is the bound; this is a program that recursed"
                ),
            ));
        }

        Step::Nested {
            lease: held.lease,
            sql: format!("SAVEPOINT {}", savepoint(held.depth())),
        }
    }

    /// The transaction or savepoint that [`Step::Open`] or [`Step::Nested`]
    /// established. Called only after the server accepted it.
    pub fn opened(&mut self, owner: Owner, lease: LeaseId, level: Isolation, access: Access) {
        let held = self.held.entry(owner).or_insert_with(|| Held {
            lease,
            open: Vec::new(),
        });
        // A nested scope inherits the transaction's level, because that is what
        // it actually runs at. Recording what the call site asked for would make
        // the mismatch check compare against a level nothing set.
        let level = held.open.first().map_or(level, |root| root.level);
        held.open.push(Scope {
            level,
            access,
            poisoned: false,
        });
    }

    /// A statement performed by `owner` came back `Failed`.
    ///
    /// Postgres has put the innermost block into the failed state; every later
    /// statement in it is `25P02` from the server, and the `COMMIT` is not — it
    /// succeeds with the command tag `ROLLBACK`. This is what makes the close
    /// tell the truth about it.
    pub fn statement_failed(&mut self, owner: Owner) {
        if let Some(held) = self.held.get_mut(&owner)
            && let Some(scope) = held.open.last_mut()
        {
            scope.poisoned = true;
        }
    }

    /// Whether the innermost scope `owner` holds has already been aborted.
    pub fn is_poisoned(&self, owner: Owner) -> bool {
        self.held
            .get(&owner)
            .and_then(|held| held.open.last())
            .is_some_and(|scope| scope.poisoned)
    }

    /// `db.commit`.
    pub fn commit(&mut self, owner: Owner, span: Span) -> Result<Step, Diagnostic> {
        self.close(owner, Close::Commit, span)
    }

    /// `db.abort` — what the `db.rollback` clause performs after it has
    /// discarded the continuation, and what `sandbox` performs unconditionally.
    pub fn abort(&mut self, owner: Owner, span: Span) -> Result<Step, Diagnostic> {
        self.close(owner, Close::Abort, span)
    }

    fn close(&mut self, owner: Owner, close: Close, span: Span) -> Result<Step, Diagnostic> {
        let Some(held) = self.held.get(&owner) else {
            // Somebody else's scope is open and this performer has none. The two
            // answers the driver could give are both wrong — closing that scope
            // ends a transaction whose statements are still being written, and
            // answering "nothing to do" tells a caller its transaction committed
            // when it did not — so it refuses.
            if let Some(other) = self.sibling(owner) {
                return Err(err_transaction_scope(span, close.what(), owner, other));
            }
            return Ok(Step::Refused(DbError::new(
                sqlstate::NO_ACTIVE_TRANSACTION,
                "",
                format!("{} with no transaction open", close.what()),
            )));
        };

        let depth = held.depth();
        let lease = held.lease;
        if depth == 1 {
            return Ok(Step::Close {
                lease,
                sql: close.outermost().to_string(),
                cleanup: Cleanup::Clean,
            });
        }
        // A savepoint is released rather than committed, and rolling one back
        // releases it too: an abandoned savepoint name would accumulate one
        // subtransaction per loop iteration on a connection the pool reuses.
        let name = savepoint(depth - 1);
        Ok(Step::Nested {
            lease,
            sql: match close {
                Close::Commit => format!("RELEASE SAVEPOINT {name}"),
                Close::Abort => {
                    format!("ROLLBACK TO SAVEPOINT {name}; RELEASE SAVEPOINT {name}")
                }
            },
        })
    }

    /// The scope a [`Step::Close`] or a nested close finished with, whether the
    /// server accepted it or not.
    ///
    /// Popped either way. A failed `COMMIT` has already ended the transaction —
    /// postgres rolls one back rather than leaving it open — and a `RELEASE` the
    /// server refused leaves a savepoint the Ply scope has finished with in
    /// every case; keeping it would make the depth drift from the connection's
    /// and every later savepoint name wrong.
    pub fn closed(&mut self, owner: Owner, commit: bool) -> Closed {
        let Some(held) = self.held.get_mut(&owner) else {
            return Closed::default();
        };
        let poisoned = held.open.pop().is_some_and(|scope| scope.poisoned);
        // `RELEASE SAVEPOINT` inside an aborted subtransaction does not clear the
        // failed state — only `ROLLBACK TO SAVEPOINT` does — so a commit carries
        // the poison outward and an abort drops it with the scope.
        if poisoned
            && commit
            && let Some(outer) = held.open.last_mut()
        {
            outer.poisoned = true;
        }
        if held.open.is_empty() {
            let lease = held.lease;
            self.held.remove(&owner);
            return Closed {
                lease: Some(lease),
                poisoned,
            };
        }
        Closed {
            lease: None,
            poisoned,
        }
    }

    /// Every connection still holding an open scope, and the table emptied.
    ///
    /// What `HostRuntime::end_entry_point` calls on **every** exit path from an
    /// entry point — a value, a diagnostic, or a spent budget. The driver rolls
    /// each of these back and releases or discards it; a connection whose
    /// `ROLLBACK` fails is closed rather than returned, because the failure this
    /// exists to prevent is invisible from both of the requests it corrupts.
    pub fn end_entry_point(&mut self, machine: MachineId) -> Vec<LeaseId> {
        let mine: Vec<Owner> = self
            .held
            .keys()
            .filter(|(owner, _)| *owner == machine)
            .copied()
            .collect();
        // This machine's scopes and no others. A teardown that emptied the table
        // would roll back a transaction another entry point is still writing
        // into, on a connection this one never touched, and neither of them would
        // see it happen — which is the failure this whole key exists to prevent.
        mine.iter()
            .filter_map(|owner| self.held.remove(owner))
            .map(|held| held.lease)
            .collect()
    }

    /// Every open scope's connection, without emptying anything. For a status
    /// line and for a test that has to assert what is still open.
    pub fn open_leases(&self) -> Vec<LeaseId> {
        self.held.values().map(|held| held.lease).collect()
    }

    /// Every open scope, whichever entry point opened it, and the table left
    /// empty.
    ///
    /// The process is stopping, so there is no entry point left to corrupt —
    /// which is the one condition under which emptying the table is right, and
    /// it is why this is a separate method from
    /// [`end_entry_point`](ScopeTable::end_entry_point) rather than a flag on
    /// it. Every lease it answers is `ROLLBACK`ed and **none is committed**: a
    /// commit at a deadline commits a half-finished body, and the only thing
    /// that knows whether a body finished is the body.
    pub fn shutdown(&mut self) -> Vec<LeaseId> {
        std::mem::take(&mut self.held)
            .into_values()
            .map(|held| held.lease)
            .collect()
    }
}

/// What popping a scope left behind.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Closed {
    /// The connection to hand back, when the outermost scope closed.
    pub lease: Option<LeaseId>,
    /// Whether a statement inside the scope had already aborted it, which is
    /// what makes a `COMMIT` postgres answered without an error still a `Failed`.
    pub poisoned: bool,
}

/// Which way a scope is being closed. One enum rather than two methods, because
/// every rule below is shared and only the SQL differs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Close {
    Commit,
    Abort,
}

impl Close {
    fn outermost(self) -> &'static str {
        match self {
            Close::Commit => "COMMIT",
            Close::Abort => "ROLLBACK",
        }
    }

    fn what(self) -> &'static str {
        match self {
            Close::Commit => "`db.commit`",
            Close::Abort => "`db.abort`",
        }
    }
}

/// The name a savepoint at this depth carries.
///
/// Generated rather than taken from the program, so a statement never contains
/// anything a program chose — the same rule that keeps every parameter out of
/// statement text applies to the one identifier the driver writes itself.
fn savepoint(depth: usize) -> String {
    format!("ply_sp_{depth}")
}

/// `E0436` — a `db` operation from a performer that owns no scope, while another
/// one is open.
#[cold]
#[inline(never)]
fn err_transaction_scope(span: Span, what: &str, owner: Owner, other: Owner) -> Diagnostic {
    Diagnostic::error(
        codes::DB_TRANSACTION_SCOPE,
        format!(
            "{what} was performed by {}, which does not own the open transaction",
            describe(owner)
        ),
    )
    .primary(span, "this performer has no transaction of its own")
    .note(format!("the open scope belongs to {}", describe(other)))
    .note("a postgres connection carries one conversation, so sharing it is a protocol violation, and acquiring a second would put the statement outside the transaction its author believed it was in")
    .note("open the transaction in the task that closes it, or join the task that owns it before closing")
}

fn describe(owner: Owner) -> String {
    match owner.1 {
        Some(task) => format!("task {task}"),
        None => "the entry point".to_string(),
    }
}

#[cfg(test)]
mod tests;
