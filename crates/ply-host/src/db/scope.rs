//! Transaction scopes: what `db.begin`, `db.commit` and `db.abort` do, and what is open.

use super::pool::{Cleanup, LeaseId};
use super::types::DbError;
use ply_eval::TaskId;
use ply_eval::host::MachineId;
use ply_span::{Diagnostic, Span, codes};
use std::collections::BTreeMap;
use std::fmt;

/// How many savepoints may be open below the outermost transaction.
pub const MAX_SAVEPOINTS: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Isolation {
    ReadCommitted,
    RepeatableRead,
    Serializable,
}

impl Isolation {
    pub fn sql(self) -> &'static str {
        match self {
            Isolation::ReadCommitted => "READ COMMITTED",
            Isolation::RepeatableRead => "REPEATABLE READ",
            Isolation::Serializable => "SERIALIZABLE",
        }
    }

    /// As the Ply constructor spells it, so diagnostics name what the call site wrote.
    pub fn as_str(self) -> &'static str {
        match self {
            Isolation::ReadCommitted => "ReadCommitted",
            Isolation::RepeatableRead => "RepeatableRead",
            Isolation::Serializable => "Serializable",
        }
    }

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

/// The machine that performed an operation, and the task inside it if any.
pub type Owner = (MachineId, Option<TaskId>);

pub mod sqlstate {
    /// `active_sql_transaction`: a nested `begin` asking for more than the open scope gives.
    pub const ACTIVE_TRANSACTION: &str = "25001";
    /// `no_active_sql_transaction`: a `commit` or an `abort` with no scope open.
    pub const NO_ACTIVE_TRANSACTION: &str = "25P01";
    /// `program_limit_exceeded`: nesting past [`super::MAX_SAVEPOINTS`].
    pub const PROGRAM_LIMIT_EXCEEDED: &str = "54000";
    /// `in_failed_sql_transaction`: the scope a statement already aborted.
    pub const TRANSACTION_ABORTED: &str = "25P02";
}

/// What a control operation asks the driver to do next.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Step {
    /// Acquire a connection and run this on it.
    Open {
        sql: String,
    },
    /// Run this on the scope's own connection.
    Nested {
        lease: LeaseId,
        sql: String,
    },
    /// Run this, then hand the connection back with `cleanup`.
    Close {
        lease: LeaseId,
        sql: String,
        /// What to do with the connection, applied only when the SQL succeeded.
        cleanup: Cleanup,
    },
    Refused(DbError),
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct Scope {
    /// The level the outermost `BEGIN` set.
    level: Isolation,
    access: Access,
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

    /// `0` for no transaction, `1` for a transaction, `n + 1` for `n` savepoints inside one.
    pub fn depth(&self, owner: Owner) -> usize {
        self.held.get(&owner).map_or(0, Held::depth)
    }

    pub fn route(
        &self,
        owner: Owner,
        what: &str,
        span: Span,
    ) -> Result<Option<LeaseId>, Diagnostic> {
        if let Some(held) = self.held.get(&owner) {
            return Ok(Some(held.lease));
        }
        match self.sibling(owner) {
            Some(other) => Err(err_transaction_scope(span, what, owner, other)),
            None => Ok(None),
        }
    }

    fn sibling(&self, owner: Owner) -> Option<Owner> {
        self.held
            .keys()
            .find(|(machine, _)| *machine == owner.0)
            .copied()
    }

    pub fn begin(&mut self, owner: Owner, level: Isolation, access: Access) -> Step {
        let Some(held) = self.held.get(&owner) else {
            return Step::Open {
                sql: format!("BEGIN ISOLATION LEVEL {} {}", level.sql(), access.sql()),
            };
        };

        // A savepoint has no isolation level or access mode, so a nested `begin` can't change them.
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

    /// The transaction or savepoint that [`Step::Open`] or [`Step::Nested`] established.
    pub fn opened(&mut self, owner: Owner, lease: LeaseId, level: Isolation, access: Access) {
        let held = self.held.entry(owner).or_insert_with(|| Held {
            lease,
            open: Vec::new(),
        });
        // A nested scope actually runs at the transaction's level.
        let level = held.open.first().map_or(level, |root| root.level);
        held.open.push(Scope {
            level,
            access,
            poisoned: false,
        });
    }

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

    pub fn commit(&mut self, owner: Owner, span: Span) -> Result<Step, Diagnostic> {
        self.close(owner, Close::Commit, span)
    }

    pub fn abort(&mut self, owner: Owner, span: Span) -> Result<Step, Diagnostic> {
        self.close(owner, Close::Abort, span)
    }

    fn close(&mut self, owner: Owner, close: Close, span: Span) -> Result<Step, Diagnostic> {
        let Some(held) = self.held.get(&owner) else {
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
        // Rollback also releases: an abandoned savepoint leaks a subtransaction per loop iteration
        // on a connection the pool reuses.
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

    /// The scope a close finished with, whether or not the server accepted it.
    pub fn closed(&mut self, owner: Owner, commit: bool) -> Closed {
        let Some(held) = self.held.get_mut(&owner) else {
            return Closed::default();
        };
        let poisoned = held.open.pop().is_some_and(|scope| scope.poisoned);
        // `RELEASE SAVEPOINT` does not clear an aborted subtransaction, so a commit carries the
        // poison outward and an abort drops it with the scope.
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
    pub fn end_entry_point(&mut self, machine: MachineId) -> Vec<LeaseId> {
        let mine: Vec<Owner> = self
            .held
            .keys()
            .filter(|(owner, _)| *owner == machine)
            .copied()
            .collect();
        mine.iter()
            .filter_map(|owner| self.held.remove(owner))
            .map(|held| held.lease)
            .collect()
    }

    pub fn open_leases(&self) -> Vec<LeaseId> {
        self.held.values().map(|held| held.lease).collect()
    }

    pub fn shutdown(&mut self) -> Vec<LeaseId> {
        std::mem::take(&mut self.held)
            .into_values()
            .map(|held| held.lease)
            .collect()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Closed {
    /// The connection to hand back, when the outermost scope closed.
    pub lease: Option<LeaseId>,
    /// Whether a statement already aborted the scope, making an error-free `COMMIT` a `Failed`.
    pub poisoned: bool,
}

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

fn savepoint(depth: usize) -> String {
    format!("ply_sp_{depth}")
}

/// A `db` operation from a performer that owns no scope while a sibling task's is open.
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
