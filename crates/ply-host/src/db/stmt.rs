//! Prepared statements: the scan, the type check, the bind and the execution.
//!
//! Preparation is where the result description arrives, so it is where every
//! per-statement check happens — once per statement per connection, never per
//! execution. Two of them run **before** any round trip, on the machine's own
//! thread, because their answer is a function of the text alone:
//!
//! - [`Cache::scan`] computes the table set, which is what refuses an undeclared
//!   table before a row moves. A statement whose footprint is wrong never
//!   reaches the server at all.
//! - the same scan refuses a construct the driver will not run, so a syntax the
//!   scanner cannot account for costs nothing.
//!
//! The rest — the parameter types, the result description, the duplicate column
//! name — needs the server's own description and happens inside [`execute`], on
//! the reactor thread, in the round trip that was going to happen anyway.
//!
//! **A prepare postgres refuses is `E0433` and not a `Failed`.** It is the
//! program's fault, it is the same every time, and it will never succeed on a
//! retry, so making it a value would invite a program to loop on it. A statement
//! that prepares and then fails at execution *is* a `Failed`, because that one
//! depends on the data.

use super::scan::{self, Scan};
use super::types::{self, BindError, Datum, DbError, Param};
use ply_span::{Diagnostic, Span, codes};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

/// `in_failed_sql_transaction`: every command but a rollback is refused, `Parse`
/// included.
const TRANSACTION_ABORTED: &str = "25P02";

/// `--db-statement-cache`: prepared statements kept per connection.
pub const DEFAULT_STATEMENT_CACHE: usize = 256;

/// One row, in the result description's own column order. The `Map` that makes
/// it canonical is built on the machine's thread, because a `Value` never
/// crosses one.
pub type Row = Vec<(String, Datum)>;

/// What a data operation answers.
///
/// One type across `query`, `execute` and `returning` rather than three, because
/// the engine, the driver and the agreement law all pattern-match one shape —
/// and because a `returning` that a schema change turned into a plain `execute`
/// should be a mismatch the program reads rather than a type error at a call
/// site that did not move.
#[derive(Clone, PartialEq, Debug)]
pub enum Answer {
    Rows(Vec<Row>),
    Count(i64),
    Failed(DbError),
}

/// A statement that prepared, and everything the prepare established about it.
#[derive(Clone, Debug)]
pub struct Prepared {
    pub statement: tokio_postgres::Statement,
    /// The result description's column names, in order. `RowCodec::columns` is
    /// checked against this at prepare time, so a `select` missing a column the
    /// codec needs is `E0433` before the first row arrives rather than a decode
    /// failure per row afterwards.
    pub columns: Vec<String>,
}

/// What the scan of a statement text costs, paid once.
///
/// Keyed by the text and shared across connections, because a table set is a
/// function of the text and of nothing else. The prepared statements themselves
/// are per connection and live in `deadpool`'s own cache, which is a different
/// thing with a different lifetime.
pub struct Cache {
    scans: Mutex<HashMap<String, Arc<Result<Scan, Diagnostic>>>>,
    bound: usize,
}

impl Default for Cache {
    fn default() -> Cache {
        Cache::new(DEFAULT_STATEMENT_CACHE)
    }
}

impl Cache {
    pub fn new(bound: usize) -> Cache {
        Cache {
            scans: Mutex::new(HashMap::new()),
            bound: bound.max(1),
        }
    }

    /// The table set for this statement, computed once.
    ///
    /// A refusal is cached too. A statement the driver will not run is refused
    /// identically every time, and re-deriving the same diagnostic per execution
    /// would make a hot failure path the slowest one in the system.
    pub fn scan(&self, sql: &str, span: Span) -> Result<Scan, Diagnostic> {
        {
            let cached = lock(&self.scans);
            if let Some(hit) = cached.get(sql) {
                return respan(hit, span);
            }
        }
        let computed = Arc::new(scan::scan(sql, span));
        let mut cached = lock(&self.scans);
        // A generation clear rather than a true LRU. The cache exists to keep a
        // hot statement from being re-scanned, and a program that overflows it
        // is one generating statement text, which is the case where nothing
        // would have hit anyway.
        if cached.len() >= self.bound {
            cached.clear();
        }
        cached.insert(sql.to_string(), Arc::clone(&computed));
        respan(&computed, span)
    }

    pub fn len(&self) -> usize {
        lock(&self.scans).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A cached diagnostic points at the `perform` that first produced it, which is
/// the wrong source location for every later one.
fn respan(cached: &Result<Scan, Diagnostic>, span: Span) -> Result<Scan, Diagnostic> {
    match cached {
        Ok(scan) => Ok(scan.clone()),
        Err(d) => {
            let mut d = d.clone();
            for label in &mut d.labels {
                label.span = span;
            }
            Err(d)
        }
    }
}

/// Prepare `sql` on this connection, bind `params`, run it, and decode the
/// answer.
///
/// Runs on the reactor thread. Nothing here holds a `Value`: parameters arrive
/// as [`Param`] and rows leave as [`Datum`], and the conversion at both ends
/// happens on the machine's thread.
///
/// `Err` is a diagnostic — a refusal that is the program's fault and will not
/// change on a retry. `Ok(Answer::Failed)` is the server working correctly and
/// telling the program something about its data.
pub async fn execute(
    connection: &deadpool_postgres::Object,
    sql: &str,
    params: &[Param],
    cache_bound: usize,
    span: Span,
) -> Result<Answer, Diagnostic> {
    // The bound on the per-connection prepared-statement cache. Cleared as a
    // generation rather than evicted one at a time, for the reason `Cache::scan`
    // gives: a program that overflows it is generating statement text.
    if connection.statement_cache.size() >= cache_bound {
        connection.statement_cache.clear();
    }

    let statement = match connection.prepare_cached(sql).await {
        Ok(statement) => statement,
        Err(e) => {
            if let Some(failure) = as_connection_failure(&e) {
                return Ok(Answer::Failed(failure));
            }
            // Postgres refuses `Parse` as well as `Execute` inside a transaction
            // block a statement already aborted, so a statement whose text this
            // connection has not seen before fails *at prepare* with `25P02`.
            // That is the failed-transaction state — §5.2's, the one the twin
            // models — and not a statement the server will never prepare: it
            // succeeds on the next attempt after the scope closes. Making it
            // `E0433` would stop a run for a condition the program is supposed to
            // read, and would do so only for the statements that happened not to
            // be in this connection's cache.
            if let Some(db) = e.as_db_error()
                && db.code().code() == TRANSACTION_ABORTED
            {
                return Ok(Answer::Failed(as_failure(&e)));
            }
            return Err(prepare_failed(&e, span));
        }
    };

    for (position, ty) in statement.params().iter().enumerate() {
        if !types::mapped(ty) {
            return Err(unmapped(&format!("parameter ${}", position + 1), ty, span));
        }
    }

    let mut columns = Vec::with_capacity(statement.columns().len());
    for column in statement.columns() {
        if !types::mapped(column.type_()) {
            return Err(unmapped(
                &format!("column `{}`", column.name()),
                column.type_(),
                span,
            ));
        }
        // A `Row` is a `Map`, so `select a.id, b.id` would silently keep one of
        // them. Refused at prepare, before the first row.
        if columns.iter().any(|name: &String| name == column.name()) {
            return Err(Diagnostic::error(
                codes::DB_PREPARE_FAILED,
                format!(
                    "this statement returns two columns named `{}`",
                    column.name()
                ),
            )
            .primary(span, "this statement reaches the database driver")
            .note("a row is a `Map` from column name to value, so one of the two would be kept and the other silently dropped")
            .note("alias one of them: `select a.id as a_id, b.id as b_id`"));
        }
        columns.push(column.name().to_string());
    }

    let bound = match types::bind(params, statement.params(), span) {
        Ok(bound) => bound,
        Err(BindError::Refused(d)) => return Err(d),
        Err(BindError::Failed(e)) => return Ok(Answer::Failed(e)),
    };
    let slots: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = bound
        .iter()
        .map(|b| b as &(dyn tokio_postgres::types::ToSql + Sync))
        .collect();

    // The result description decides which of the two the statement is, not the
    // operation the call site named: an `insert … returning` describes columns
    // and an `insert` does not.
    if statement.columns().is_empty() {
        return match connection.execute(&statement, &slots).await {
            Ok(count) => Ok(Answer::Count(count as i64)),
            Err(e) => Ok(Answer::Failed(as_failure(&e))),
        };
    }

    let rows = match connection.query(&statement, &slots).await {
        Ok(rows) => rows,
        Err(e) => return Ok(Answer::Failed(as_failure(&e))),
    };
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let mut decoded = Vec::with_capacity(columns.len());
        for (index, name) in columns.iter().enumerate() {
            match row.try_get::<_, Datum>(index) {
                Ok(datum) => decoded.push((name.clone(), datum)),
                // A decode failure is the driver refusing to answer a value the
                // column did not hold — a `numeric` past `Decimal`'s range, a
                // `NaN`, an array with a `NULL` element. Never a rounding and
                // never a substituted zero.
                Err(e) => {
                    return Err(Diagnostic::error(
                        codes::DB_PREPARE_FAILED,
                        format!("column `{name}` holds a value this driver will not decode: {e}"),
                    )
                    .primary(span, "this statement reaches the database driver")
                    .note("the alternative is a rounding or a substituted zero, which is the silent-wrong-answer shape this project exists to refuse"));
                }
            }
        }
        out.push(decoded);
    }
    Ok(Answer::Rows(out))
}

/// Run a statement that takes no parameters and returns nothing a program reads.
///
/// What transaction control is made of. It is `simple_query` rather than a
/// prepare because `BEGIN`, `COMMIT`, `SAVEPOINT` and `SET TRANSACTION` are not
/// preparable, and because caching a statement that runs once per scope would
/// fill the cache with text nothing reuses.
pub async fn control(connection: &deadpool_postgres::Object, sql: &str) -> Result<(), DbError> {
    match connection.simple_query(sql).await {
        Ok(_) => Ok(()),
        Err(e) => Err(as_failure(&e)),
    }
}

/// The SQLSTATE and the object it named. Never the message: that is postgres's
/// prose, it moves between server versions and locales, and the agreement law
/// compares `code` and `constraint` precisely so that it does not fail on a
/// server upgrade.
pub fn as_failure(e: &tokio_postgres::Error) -> DbError {
    if let Some(db) = e.as_db_error() {
        return DbError {
            code: db.code().code().to_string(),
            constraint: db
                .constraint()
                .or_else(|| db.table())
                .unwrap_or("")
                .to_string(),
            detail: db.message().to_string(),
        };
    }
    // No SQLSTATE means the conversation ended rather than the server answering:
    // a closed socket, a server that restarted, a connection reset under a
    // statement. A peer that went away is a `Failed` and not a diagnostic.
    DbError::connection(e.to_string())
}

/// Whether a prepare failed because the connection died rather than because the
/// statement was wrong. The two are opposite verdicts — one is the program's
/// fault forever and the other is a peer that went away — so they are
/// distinguished by whether the server answered at all.
fn as_connection_failure(e: &tokio_postgres::Error) -> Option<DbError> {
    if e.as_db_error().is_some() {
        return None;
    }
    Some(DbError::connection(e.to_string()))
}

#[cold]
fn prepare_failed(e: &tokio_postgres::Error, span: Span) -> Diagnostic {
    let db = e.as_db_error();
    let code = db.map(|d| d.code().code().to_string()).unwrap_or_default();
    let message = db
        .map(|d| d.message().to_string())
        .unwrap_or_else(|| e.to_string());
    let mut diagnostic = Diagnostic::error(
        codes::DB_PREPARE_FAILED,
        format!("postgres refused to prepare this statement: {message}"),
    )
    .primary(span, "this statement reaches the database driver");
    if !code.is_empty() {
        diagnostic = diagnostic.note(format!("SQLSTATE {code}"));
    }
    if let Some(hint) = db.and_then(|d| d.hint()) {
        diagnostic = diagnostic.note(hint.to_string());
    }
    diagnostic
        .note("this is a diagnostic rather than a `Failed` value: a statement the server cannot prepare fails the same way every time and will never succeed on a retry, so making it a value would invite a program to loop on it")
}

#[cold]
fn unmapped(what: &str, ty: &tokio_postgres::types::Type, span: Span) -> Diagnostic {
    let mut diagnostic = Diagnostic::error(
        codes::DB_STATEMENT_REFUSED,
        format!("{what} is `{ty}`, which is outside the pinned type mapping"),
    )
    .primary(span, "this statement reaches the database driver")
    .note("Int↔int8/int4/int2, Bool↔bool, String↔text/varchar/bpchar/name/uuid, Bytes↔bytea, Float↔float8/float4, Decimal↔numeric, Json↔json/jsonb, List<a>↔a[]");
    if let Some(advice) = types::advice(ty) {
        diagnostic = diagnostic.note(advice.to_string());
    }
    diagnostic.note(
        "a type outside the mapping is refused rather than rendered to text, because there is no Ply value that would mean the same thing and a text rendering would be a lossy one nothing could compare",
    )
}

/// A panicking job thread leaves a map with no invariant a panic can break, so
/// recovering is correct and propagating would take out the machine's thread.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests;
