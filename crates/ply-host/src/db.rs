//! The `db` effect and the postgres driver that serves it.

pub mod handler;
pub mod pool;
pub mod postgres;
pub mod scan;
pub mod scope;
pub mod stmt;
pub mod types;
pub mod value;

pub use handler::{Driver, Statement, register, registry};
pub use pool::{PoolConfig, Reactor};
pub use postgres::Postgres;
pub use scan::{Kind, Scan, Tables};
pub use scope::{Access, Isolation, Owner, ScopeTable, Step};
pub use stmt::{Answer, Cache, Prepared};
pub use types::{Datum, DbError, Json, Param};

use ply_eval::{Determinism, HostOp, HostResource, Linearity};
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_ty::Footprint;

pub const DECLARATION: &str = ply_std::DB;

pub const MODULE: &str = "std.db";

pub const EFFECT: &str = "std.db.db";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    Query,
    Execute,
    Returning,
    Begin,
    Commit,
    Abort,
}

impl Op {
    pub const ALL: [Op; 6] = [
        Op::Query,
        Op::Execute,
        Op::Returning,
        Op::Begin,
        Op::Commit,
        Op::Abort,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Op::Query => "query",
            Op::Execute => "execute",
            Op::Returning => "returning",
            Op::Begin => "begin",
            Op::Commit => "commit",
            Op::Abort => "abort",
        }
    }

    pub fn what(self) -> &'static str {
        match self {
            Op::Query => "`db.query`",
            Op::Execute => "`db.execute`",
            Op::Returning => "`db.returning`",
            Op::Begin => "`db.begin`",
            Op::Commit => "`db.commit`",
            Op::Abort => "`db.abort`",
        }
    }

    pub fn path(self) -> &'static str {
        match self {
            Op::Query => "ply_host::db::query",
            Op::Execute => "ply_host::db::execute",
            Op::Returning => "ply_host::db::returning",
            Op::Begin => "ply_host::db::begin",
            Op::Commit => "ply_host::db::commit",
            Op::Abort => "ply_host::db::abort",
        }
    }

    pub fn arity(self) -> usize {
        match self {
            Op::Query | Op::Execute | Op::Returning => 2,
            Op::Begin => 2,
            Op::Commit | Op::Abort => 0,
        }
    }

    pub fn takes_table(self) -> bool {
        matches!(self, Op::Query | Op::Execute | Op::Returning)
    }

    pub fn writes(self) -> bool {
        !matches!(self, Op::Query)
    }

    pub fn declaration(self) -> HostOp {
        HostOp {
            effect: Symbol::new(EFFECT),
            op: Symbol::new(self.name()),
            resource: if self.takes_table() {
                HostResource::Any
            } else {
                HostResource::Only(ply_ty::Resource::Singleton)
            },
            determinism: Determinism::Nondeterministic,
            linearity: Linearity::AtMostOnce,
            blocking: true,
            // A `Param` has no `PSecret` case, so a credential cannot be bound into a statement.
            secrets: false,
            path: self.path(),
        }
    }
}

pub fn check_footprint(
    scan: &Scan,
    op: Op,
    label: &ply_ty::Resource,
    declared: Option<&Footprint>,
    span: Span,
) -> Result<Footprint, Diagnostic> {
    use ply_ty::Mode;
    use ply_ty::{EffectAtom, Resource};

    // A `select` whose `with` holds a `delete` writes too, so it must not run beside readers.
    if (scan.kind.writes() || !scan.tables.written.is_empty()) && !op.writes() {
        return Err(Diagnostic::error(
            codes::DB_STATEMENT_REFUSED,
            format!(
                "a `{}` statement was performed as {}, which publishes a read",
                scan.kind.as_str(),
                op.what()
            ),
        )
        .primary(span, "this statement changes rows")
        .note("`db.query` is the only `read` operation, and its atom is what two read-only endpoints are scheduled side by side on")
        .note("perform it as `db.execute` or `db.returning`"));
    }

    let effect = Symbol::new(EFFECT);
    let mut atoms = Vec::new();
    for table in &scan.tables.written {
        atoms.push(EffectAtom::new(
            effect.clone(),
            Resource::Named(Symbol::new(table.clone())),
            Mode::Write,
        ));
    }
    for table in &scan.tables.read {
        atoms.push(EffectAtom::new(
            effect.clone(),
            Resource::Named(Symbol::new(table.clone())),
            Mode::Read,
        ));
    }

    if let Resource::Named(named) = label
        && !scan.tables.all().contains(named.as_str())
    {
        return Err(Diagnostic::error(
            codes::DB_FOOTPRINT_UNDECLARED,
            format!(
                "{} names `{named}`, and the statement touches {}",
                op.what(),
                describe(scan)
            ),
        )
        .primary(span, "this label is not a table the statement touches")
        .note("the label is the statement's principal table and the row `ply hosts` prints a line for")
        .note("label the call site with a table the statement names"));
    }

    if let Some(declared) = declared {
        for atom in &atoms {
            if declared.atoms().any(|a| a == atom) {
                continue;
            }
            // A declared read does not cover a write: two readers may run side by side.
            return Err(Diagnostic::error(
                codes::DB_FOOTPRINT_UNDECLARED,
                format!(
                    "this statement touches `{atom}`, which is not in the declared row of the definition that reached it"
                ),
            )
            .primary(span, format!("the statement touches {}", describe(scan)))
            .note(format!(
                "the row declares {}",
                if declared.is_empty() {
                    "nothing".to_string()
                } else {
                    declared
                        .atoms()
                        .map(|a| format!("`{a}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            ))
            .note("a statement's tables are a function of its text rather than of the call site's label, so a join reaches tables the label never named")
            .note("add the atom to the definition's row, or split the statement"));
        }
    }

    Ok(Footprint::from_atoms(atoms))
}

fn describe(scan: &Scan) -> String {
    let written: Vec<&str> = scan.tables.written.iter().map(String::as_str).collect();
    let read: Vec<&str> = scan.tables.read.iter().map(String::as_str).collect();
    match (written.as_slice(), read.as_slice()) {
        ([], []) => "no table".to_string(),
        (w, []) => format!("`{}` (written)", w.join("`, `")),
        ([], r) => format!("`{}` (read)", r.join("`, `")),
        (w, r) => format!(
            "`{}` (written) and `{}` (read)",
            w.join("`, `"),
            r.join("`, `")
        ),
    }
}
