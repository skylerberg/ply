//! Prepared statements: the scan, the type check, the bind and the execution.

use super::scan::{self, Scan};
use super::types::{self, BindError, Datum, DbError, Param};
use ply_span::{Diagnostic, Span, codes};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

/// SQLSTATE `in_failed_sql_transaction`.
const TRANSACTION_ABORTED: &str = "25P02";

/// `--db-statement-cache`: prepared statements kept per connection.
pub const DEFAULT_STATEMENT_CACHE: usize = 256;

/// One row, in the result description's own column order.
pub type Row = Vec<(String, Datum)>;

#[derive(Clone, PartialEq, Debug)]
pub enum Answer {
    Rows(Vec<Row>),
    Count(i64),
    Failed(DbError),
}

#[derive(Clone, Debug)]
pub struct Prepared {
    pub statement: tokio_postgres::Statement,
    pub columns: Vec<String>,
}

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

    pub fn scan(&self, sql: &str, span: Span) -> Result<Scan, Diagnostic> {
        {
            let cached = lock(&self.scans);
            if let Some(hit) = cached.get(sql) {
                return respan(hit, span);
            }
        }
        let computed = Arc::new(scan::scan(sql, span));
        let mut cached = lock(&self.scans);
        // A generation clear rather than a true LRU.
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

pub async fn execute(
    connection: &deadpool_postgres::Object,
    sql: &str,
    params: &[Param],
    cache_bound: usize,
    span: Span,
) -> Result<Answer, Diagnostic> {
    if connection.statement_cache.size() >= cache_bound {
        connection.statement_cache.clear();
    }

    let statement = match connection.prepare_cached(sql).await {
        Ok(statement) => statement,
        Err(e) => {
            if let Some(failure) = as_connection_failure(&e) {
                return Ok(Answer::Failed(failure));
            }
            // An aborted transaction refuses even `Parse`, so a new statement text fails here.
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
        // A `Row` is a `Map`, so `select a.id, b.id` would silently keep one of them.
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

    // The result description, not the call site's operation, decides rows versus a count.
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
pub async fn control(connection: &deadpool_postgres::Object, sql: &str) -> Result<(), DbError> {
    match connection.simple_query(sql).await {
        Ok(_) => Ok(()),
        Err(e) => Err(as_failure(&e)),
    }
}

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
    // No SQLSTATE means the connection ended rather than the server answering.
    DbError::connection(e.to_string())
}

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

/// Poison is ignored: the map has no invariant a panic can break.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
