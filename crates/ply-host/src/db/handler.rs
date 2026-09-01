//! The host handler: what a `perform` of a `db` operation actually reaches.

use super::scope::{Access, Isolation, Owner};
use super::value;
use super::{Op, Scan, check_footprint};
use ply_core::ty::{Footprint, Resource};
use ply_eval::host::{HostAnswer, HostHandler, HostRegistry, HostRequest, HostRuntime};
use ply_span::{Diagnostic, Span, codes};
use std::sync::Arc;

/// What a `db` implementation has to answer.
pub trait Driver: Send + Sync {
    /// The Rust path `ply hosts` prints.
    fn path(&self, op: Op) -> &'static str;

    /// One data statement, on this task's open scope or on a connection of its own.
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
    /// The label the call site wrote: the statement's principal table, and the resource the
    /// registry resolved this atom against.
    pub at: &'a Resource,
    pub sql: &'a str,
    pub params: Vec<super::Param>,
    /// Every atom the statement reaches, from [`scan`].
    pub touched: Footprint,
    pub scan: &'a Scan,
    /// Whose scope stack this statement runs on: the machine and the task.
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
    /// Shared across the operations, because a table set is a function of the statement text and of
    /// nothing else — including of which operation performed it.
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
                // The scan first, and the footprint second, both before a connection is acquired: a
                // statement the driver will not run and a table the row never declared each cost a
                // diagnostic rather than a round trip.
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
