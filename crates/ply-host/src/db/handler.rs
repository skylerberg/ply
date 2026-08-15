//! The host handler: what a `perform` of a `db` operation actually reaches.
//!
//! One adapter for every implementation, exactly as [`tcp::register`] is one for
//! the socket and the script. ADR 0008 §5 wants a simulated twin satisfying the
//! same declared signature, and the cheapest way to keep that true is for there
//! to be one place the signature, the arity checks, the value decoding, the
//! statement scan and the footprint refusal are written.
//!
//! Everything that can be decided without a round trip is decided **here**,
//! before the implementation is called at all:
//!
//! - the arity and the argument shapes, which inference already checked, so a
//!   failure is Ply's fault and says so;
//! - the statement scan, which is [`codes::DB_STATEMENT_REFUSED`] for a
//!   construct the driver will not run — a stacked statement, a `now()`, a
//!   set-returning function whose tables no scanner can name;
//! - the footprint, which is [`codes::DB_FOOTPRINT_UNDECLARED`] for a table
//!   outside the entry point's declared row.
//!
//! That ordering is the point. All three refuse **before** a connection is
//! acquired and before a row moves, so a wrong row costs a diagnostic rather
//! than a statement that already ran against a table the scheduler believed
//! nobody was touching.
//!
//! [`tcp::register`]: crate::tcp::register

use super::scope::{Access, Isolation, Owner};
use super::value;
use super::{Op, Scan, check_footprint};
use ply_core::ty::{Footprint, Resource};
use ply_eval::host::{HostAnswer, HostHandler, HostRegistry, HostRequest, HostRuntime};
use ply_span::{Diagnostic, Span, codes};
use std::sync::Arc;

/// What a `db` implementation has to answer.
///
/// Arguments arrive decoded, the statement arrives scanned, and the footprint
/// arrives checked: an implementation is handed a statement it is allowed to
/// run and a set of atoms it is allowed to touch, and it never re-derives
/// either. That is what stops a real driver and a simulated one from coming to
/// differ about what a legal call is.
pub trait Driver: Send + Sync {
    /// The Rust path `ply hosts` prints. It must identify the implementation
    /// rather than the effect: a listing that named the postgres driver for a
    /// run served by something else would be the trusted computing base lying
    /// about itself.
    fn path(&self, op: Op) -> &'static str;

    /// One data statement, on this task's open scope or on a connection of its
    /// own.
    ///
    /// `touched` is what the scan computed — every atom this statement reaches,
    /// which is a superset of the one the registry resolved. An implementation
    /// carries it back to the machine so a wrong row fails loudly on its first
    /// execution instead of quietly forever.
    fn statement(&self, request: Statement<'_>) -> Result<HostAnswer, Diagnostic>;

    /// `BEGIN`, or a savepoint if this task already holds a scope.
    fn begin(
        &self,
        level: Isolation,
        access: Access,
        owner: Owner,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic>;

    fn commit(&self, owner: Owner, span: Span) -> Result<HostAnswer, Diagnostic>;

    fn abort(&self, owner: Owner, span: Span) -> Result<HostAnswer, Diagnostic>;
}

/// A data statement, checked and ready to run.
pub struct Statement<'a> {
    pub op: Op,
    /// The label the call site wrote: the statement's principal table, and the
    /// resource the registry resolved this atom against.
    pub at: &'a Resource,
    pub sql: &'a str,
    pub params: Vec<super::Param>,
    /// Every atom the statement reaches, from [`scan`]. Never empty for a
    /// statement that names a relation.
    pub touched: Footprint,
    pub scan: &'a Scan,
    /// Whose scope stack this statement runs on: the machine and the task. Both
    /// halves, because every entry point outside a scheduler region reports the
    /// same task and `ply test` runs several of them at once.
    pub owner: Owner,
    pub span: Span,
}

/// Register every operation of `db` against an implementation.
pub fn register(registry: &mut HostRegistry, driver: Arc<dyn Driver>) {
    let cache = Arc::new(super::stmt::Cache::default());
    for op in Op::ALL {
        let mut declaration = op.declaration();
        declaration.path = driver.path(op);
        registry.register(
            declaration,
            Arc::new(Operation {
                op,
                driver: Arc::clone(&driver),
                cache: Arc::clone(&cache),
            }),
        );
    }
}

/// A registry serving `db` and nothing else.
pub fn registry(driver: Arc<dyn Driver>) -> HostRegistry {
    let mut registry = HostRegistry::new();
    register(&mut registry, driver);
    registry
}

struct Operation {
    op: Op,
    driver: Arc<dyn Driver>,
    /// Shared across the operations, because a table set is a function of the
    /// statement text and of nothing else — including of which operation
    /// performed it.
    cache: Arc<super::stmt::Cache>,
}

impl HostHandler for Operation {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        let owner: Owner = (req.machine, req.task);
        if req.args.len() != self.op.arity() {
            return Err(arity(self.op, req.args.len(), span));
        }
        match self.op {
            Op::Begin => {
                let level = value::isolation(&req.args[0], span)?;
                let access = value::access(&req.args[1], span)?;
                self.driver.begin(level, access, owner, span)
            }
            Op::Commit => self.driver.commit(owner, span),
            Op::Abort => self.driver.abort(owner, span),
            Op::Query | Op::Execute | Op::Returning => {
                let sql = value::statement(&req.args[0], span)?;
                let params = value::params(&req.args[1], span)?;
                // The scan first, and the footprint second, both before a
                // connection is acquired: a statement the driver will not run
                // and a table the row never declared each cost a diagnostic
                // rather than a round trip.
                let scan = self.cache.scan(&sql, span)?;
                let touched =
                    check_footprint(&scan, self.op, &req.atom.resource, req.declared, span)?;
                self.driver.statement(Statement {
                    op: self.op,
                    at: &req.atom.resource,
                    sql: &sql,
                    params,
                    touched,
                    scan: &scan,
                    owner,
                    span,
                })
            }
        }
    }
}

#[cold]
fn arity(op: Op, got: usize, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!(
            "{} was performed with {got} arguments and takes {}",
            op.what(),
            op.arity()
        ),
    )
    .primary(span, "this perform reached the database driver")
    .note("inference checks a perform's arity, so reaching this means the evaluator was handed a module that was never checked")
}

#[cfg(test)]
mod tests;
