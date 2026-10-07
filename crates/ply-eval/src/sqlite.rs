//! SQLite as `std.sqlite` runs it: one statement at a time, on a connection opened with settings no
//! statement can loosen and an authorizer that keeps it inside its own database.
//!
//! One [`Engine`] serves both forms. Over a file it is a connection a host handler opened and
//! holds; over an image it is the `sqlite_run` builtin, which opens the bytes it is handed, runs one
//! statement and answers the bytes that leaves, so its answer is a function of its arguments: no
//! clock, no entropy, no file and no state kept between calls. The engine holds a generator of its
//! own, and nothing a statement may do reads it: the functions that draw from it are refused, and so
//! is a row at the largest row id, past which it would choose the next.

use crate::value::{Fields, Value};
use crate::{Diagnostic, Span, Symbol, codes};
use rusqlite::config::DbConfig;
use rusqlite::functions::{Context, FunctionFlags};
use rusqlite::hooks::{Action, AuthAction, AuthContext, Authorization};
use rusqlite::limits::Limit;
use rusqlite::types::{ToSqlOutput, ValueRef};
use rusqlite::{Connection, MAIN_DB, OpenFlags, StatementStatus, ffi};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

/// The most bytes one text or blob holds, as one `fs` read answers.
pub const MAX_VALUE_BYTES: i32 = 64 * 1024 * 1024;

/// How many transactions and savepoints one connection nests: deeper is a program that recursed.
pub const MAX_DEPTH: u32 = 32;

/// The virtual-machine steps between two looks at the step budget.
const STEP_GRAIN: i32 = 1000;

/// What opens a refusal a guarded function raises, so it is told from the engine's own errors.
const REFUSED: &str = "refused: ";

/// The scalar, aggregate and window functions a statement may call. The list is closed: the
/// authorizer refuses any other name wherever it is written, a view or a trigger included.
pub const FUNCTIONS: &[&str] = &[
    "->",
    "->>",
    "abs",
    "avg",
    "char",
    "coalesce",
    "concat",
    "concat_ws",
    "count",
    "cume_dist",
    "date",
    "datetime",
    "dense_rank",
    "first_value",
    "format",
    "glob",
    "group_concat",
    "hex",
    "if",
    "ifnull",
    "iif",
    "instr",
    "json",
    "json_array",
    "json_array_length",
    "json_error_position",
    "json_extract",
    "json_group_array",
    "json_group_object",
    "json_insert",
    "json_object",
    "json_patch",
    "json_pretty",
    "json_quote",
    "json_remove",
    "json_replace",
    "json_set",
    "json_type",
    "json_valid",
    "jsonb",
    "jsonb_array",
    "jsonb_extract",
    "jsonb_group_array",
    "jsonb_group_object",
    "jsonb_insert",
    "jsonb_object",
    "jsonb_patch",
    "jsonb_remove",
    "jsonb_replace",
    "jsonb_set",
    "julianday",
    "lag",
    "last_value",
    "lead",
    "length",
    "like",
    "likelihood",
    "likely",
    "lower",
    "ltrim",
    "max",
    "min",
    "nth_value",
    "ntile",
    "nullif",
    "octet_length",
    "percent_rank",
    "printf",
    "quote",
    "rank",
    "replace",
    "round",
    "row_number",
    "rtrim",
    "sign",
    "sqlite_source_id",
    "sqlite_version",
    "strftime",
    "string_agg",
    "substr",
    "substring",
    "sum",
    "time",
    "timediff",
    "total",
    "trim",
    "typeof",
    "unhex",
    "unicode",
    "unixepoch",
    "unlikely",
    "upper",
    "zeroblob",
];

/// The functions the engine runs to carry out `ALTER TABLE` and `ANALYZE`. No statement can call
/// one, and the authorizer is asked about each as the engine reaches for it.
const INTERNAL_FUNCTIONS: &[&str] = &[
    "sqlite_rename_column",
    "sqlite_rename_table",
    "sqlite_rename_test",
    "sqlite_drop_column",
    "sqlite_rename_quotefix",
    "stat_init",
    "stat_push",
    "stat_get",
];

/// The date and time functions, each replaced by a guard that refuses the calls that read the
/// clock or the machine's zone and hands every other to the engine's own.
const CLOCKED: &[&str] = &[
    "date",
    "time",
    "datetime",
    "julianday",
    "unixepoch",
    "strftime",
    "timediff",
];

/// The pragmas a statement may run, and whether each may be given a value.
const PRAGMAS: &[(&str, bool)] = &[
    ("application_id", true),
    ("collation_list", false),
    ("compile_options", false),
    ("foreign_key_check", true),
    ("foreign_key_list", true),
    ("foreign_keys", false),
    ("function_list", false),
    ("index_info", true),
    ("index_list", true),
    ("index_xinfo", true),
    ("integrity_check", true),
    ("optimize", true),
    ("quick_check", true),
    ("schema_version", false),
    ("table_info", true),
    ("table_list", true),
    ("table_xinfo", true),
    ("user_version", true),
];

/// One value bound to a statement or read from a column.
#[derive(Clone, Debug, PartialEq)]
pub enum Scalar {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl Scalar {
    fn bytes(&self) -> i64 {
        match self {
            Scalar::Null => 0,
            Scalar::Int(_) | Scalar::Real(_) => 8,
            Scalar::Text(text) => text.len() as i64,
            Scalar::Blob(blob) => blob.len() as i64,
        }
    }
}

impl rusqlite::ToSql for Scalar {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Borrowed(match self {
            Scalar::Null => ValueRef::Null,
            Scalar::Int(n) => ValueRef::Integer(*n),
            Scalar::Real(x) => ValueRef::Real(*x),
            Scalar::Text(text) => ValueRef::Text(text.as_bytes()),
            Scalar::Blob(blob) => ValueRef::Blob(blob),
        }))
    }
}

/// What one statement may take and answer, each a bound its caller chose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The statement's text, in bytes.
    pub sql_bytes: i64,
    pub params: i64,
    pub rows: i64,
    /// The values answered, in bytes: a text's or a blob's length, eight for a number.
    pub bytes: i64,
    /// Steps of the engine's virtual machine, counted in thousands.
    pub steps: i64,
}

/// Why a statement, a transaction step or an open did not happen. `kind` is the word
/// `std.sqlite` reads; `name` is the constraint a violation names, the limit a `too_large` passed,
/// or the engine's own code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub kind: &'static str,
    pub name: String,
    pub detail: String,
}

impl Failure {
    pub fn new(kind: &'static str, name: impl Into<String>, detail: impl Into<String>) -> Failure {
        Failure {
            kind,
            name: name.into(),
            detail: detail.into(),
        }
    }

    fn refused(detail: impl Into<String>) -> Failure {
        Failure::new("refused", "", detail)
    }

    fn too_large(limit: &'static str, bound: i64) -> Failure {
        Failure::new(
            "too_large",
            limit,
            format!("more than the {bound} its caller allowed"),
        )
    }

    pub fn into_value(self) -> Value {
        record([
            ("detail", Value::str(self.detail)),
            ("kind", Value::str(self.kind)),
            ("name", Value::str(self.name)),
        ])
    }
}

/// What a statement answered.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reply {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Scalar>>,
    /// Rows the statement itself inserted, updated or deleted; a trigger's are not counted.
    pub changed: i64,
    /// The row id of the last row the statement itself inserted, and 0 when it inserted none.
    pub last_row_id: i64,
    pub failure: Option<Failure>,
    /// Whether the statement could have changed the database.
    pub wrote: bool,
    /// Whether the statement is over, so no page follows this one.
    pub ended: bool,
}

impl Reply {
    pub fn failed(failure: Failure) -> Reply {
        Reply {
            failure: Some(failure),
            ended: true,
            ..Reply::default()
        }
    }

    pub fn into_value(self) -> Value {
        record(self.fields())
    }

    fn fields(self) -> [(&'static str, Value); 6] {
        [
            ("wrote", Value::Bool(self.wrote)),
            ("changed", Value::Int(self.changed)),
            (
                "columns",
                Value::list(self.columns.into_iter().map(Value::str).collect()),
            ),
            (
                "failure",
                match self.failure {
                    Some(failure) => Value::ctor("Some", vec![failure.into_value()]),
                    None => Value::ctor("None", Vec::new()),
                },
            ),
            ("last_row_id", Value::Int(self.last_row_id)),
            (
                "rows",
                Value::list(
                    self.rows
                        .into_iter()
                        .map(|row| Value::list(row.into_iter().map(scalar_value).collect()))
                        .collect(),
                ),
            ),
        ]
    }
}

fn record<const N: usize>(fields: [(&str, Value); N]) -> Value {
    Value::Record(Arc::new(Fields::from_unsorted(
        fields
            .into_iter()
            .map(|(name, value)| (Symbol::new(name), value))
            .collect(),
    )))
}

/// A `Scalar` as the record `std.sqlite` reads: `kind` is 0 for null, 1 an integer, 2 a float, 3
/// text and 4 bytes.
fn scalar_value(scalar: Scalar) -> Value {
    let (kind, int, real, bytes) = match scalar {
        Scalar::Null => (0, 0, 0.0, Vec::new()),
        Scalar::Int(n) => (1, n, 0.0, Vec::new()),
        Scalar::Real(x) => (2, 0, x, Vec::new()),
        Scalar::Text(text) => (3, 0, 0.0, text.into_bytes()),
        Scalar::Blob(blob) => (4, 0, 0.0, blob),
    };
    record([
        ("bytes", Value::bytes(bytes)),
        ("int", Value::Int(int)),
        ("kind", Value::Int(kind)),
        ("real", Value::Float(real)),
    ])
}

fn field<'a>(
    value: &'a Value,
    name: &str,
    span: Span,
    what: &str,
) -> Result<&'a Value, Diagnostic> {
    match value {
        Value::Record(fields) => fields.named(name),
        _ => None,
    }
    .ok_or_else(|| malformed(span, what, name))
}

#[cold]
fn malformed(span: Span, what: &str, name: &str) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("{what} was handed a value with no `{name}`"),
    )
    .primary(
        span,
        "a well-typed call hands the record its declaration names",
    )
}

/// The parameters of a statement, read from the list of records `std.sqlite` binds.
pub fn scalars_of(list: &Value, span: Span, what: &str) -> Result<Vec<Scalar>, Diagnostic> {
    list.as_list(span, what)?
        .iter()
        .map(|item| {
            let kind = field(item, "kind", span, what)?.as_int(span, what)?;
            Ok(match kind {
                1 => Scalar::Int(field(item, "int", span, what)?.as_int(span, what)?),
                2 => Scalar::Real(field(item, "real", span, what)?.as_float(span, what)?),
                3 => {
                    let bytes = field(item, "bytes", span, what)?.as_bytes(span, what)?;
                    match std::str::from_utf8(bytes) {
                        Ok(text) => Scalar::Text(text.to_string()),
                        Err(_) => Scalar::Blob(bytes.to_vec()),
                    }
                }
                4 => Scalar::Blob(
                    field(item, "bytes", span, what)?
                        .as_bytes(span, what)?
                        .to_vec(),
                ),
                _ => Scalar::Null,
            })
        })
        .collect()
}

impl Limits {
    pub fn of(value: &Value, span: Span, what: &str) -> Result<Limits, Diagnostic> {
        let int = |name: &str| field(value, name, span, what)?.as_int(span, what);
        Ok(Limits {
            sql_bytes: int("sql_bytes")?,
            params: int("params")?,
            rows: int("rows")?,
            bytes: int("bytes")?,
            steps: int("steps")?,
        })
    }
}

/// How a database file is opened.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    ReadOnly,
    ReadWrite,
    Create,
}

/// What a connection is opened with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// Whether every table the connection makes must be `STRICT`.
    pub strict: bool,
    /// How long a statement waits on another connection's lock before it answers `busy`.
    pub busy_ms: i64,
    /// How many prepared statements the connection keeps.
    pub statements: i64,
}

impl Options {
    pub fn of(value: &Value, span: Span, what: &str) -> Result<Options, Diagnostic> {
        Ok(Options {
            strict: field(value, "strict", span, what)?.as_bool(span, what)?,
            busy_ms: field(value, "busy_ms", span, what)?.as_int(span, what)?,
            statements: field(value, "statements", span, what)?.as_int(span, what)?,
        })
    }
}

/// One step of a connection's transaction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Control {
    /// Opens a transaction that takes the write lock at once, or a savepoint inside one.
    Begin,
    /// Opens a transaction that reads one snapshot, or a savepoint inside one.
    BeginRead,
    Commit,
    Rollback,
}

/// What the authorizer, the progress handler and the update hook share with the engine that
/// installed them.
#[derive(Default)]
struct Guard {
    /// Set while the engine runs a statement of its own, which the authorizer lets through.
    internal: AtomicBool,
    /// Why the authorizer last refused, for the failure the refused statement answers with.
    refused: Mutex<Option<String>>,
    /// The steps the running statement has left.
    steps: AtomicI64,
    /// Whether the progress handler stopped the running statement.
    spent: AtomicBool,
    /// Whether the running statement left a row at the largest row id, past which the engine
    /// draws row ids at random.
    last_row_id_taken: AtomicBool,
}

impl Guard {
    fn refuse(&self, why: String) -> Authorization {
        *self.refused.lock().unwrap_or_else(|e| e.into_inner()) = Some(why);
        Authorization::Deny
    }

    fn take_refusal(&self) -> Option<String> {
        self.refused
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }
}

/// One open database.
pub struct Engine {
    conn: Connection,
    guard: Arc<Guard>,
    /// Open transactions and savepoints, outermost first.
    depth: u32,
    strict: bool,
    /// Whether the file was opened to be read alone.
    read_only: bool,
}

impl Engine {
    /// The database `image` holds, or an empty one for no bytes.
    pub fn memory(image: &[u8], strict: bool) -> Result<Engine, Failure> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_EXRESCODE;
        let mut conn = Connection::open_in_memory_with_flags(flags).map_err(unopened)?;
        if !image.is_empty() {
            conn.deserialize_read_exact(MAIN_DB, image, image.len(), false)
                .map_err(unopened)?;
        }
        let guard = Arc::new(Guard::default());
        configure(&conn, &guard, 0).map_err(unopened)?;
        Ok(Engine {
            conn,
            guard,
            depth: 0,
            strict,
            read_only: false,
        })
    }

    /// The database a file's connection holds. The host opens the file and says how long the
    /// connection waits on a lock, which are its to do: nothing here reads a path or a clock.
    pub fn file(conn: Connection, mode: Mode, options: Options) -> Result<Engine, Failure> {
        let guard = Arc::new(Guard::default());
        let statements = usize::try_from(options.statements).unwrap_or(0);
        configure(&conn, &guard, statements).map_err(unopened)?;
        let engine = Engine {
            conn,
            guard,
            depth: 0,
            strict: options.strict,
            read_only: mode == Mode::ReadOnly,
        };
        engine
            .internal(|conn| {
                if mode != Mode::ReadOnly {
                    // Refused where the file cannot be written or the filesystem cannot share
                    // memory, which leaves a rollback journal: as safe, and one writer at a time.
                    let _ = conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()));
                }
                conn.execute_batch("PRAGMA cell_size_check=ON")?;
                // The first read of the file, so one that is no database is refused at its open.
                conn.query_row("SELECT count(*) FROM sqlite_schema", [], |_| Ok(()))
            })
            .map_err(unopened)?;
        Ok(engine)
    }

    /// The bytes of an in-memory database as a file would hold them, and none for an empty one.
    pub fn image(&self) -> Result<Vec<u8>, Failure> {
        let pages: i64 = self
            .internal(|conn| conn.query_row("PRAGMA page_count", [], |row| row.get(0)))
            .map_err(|e| self.failure_of(e))?;
        if pages == 0 {
            return Ok(Vec::new());
        }
        // The engine asks the database its size through a pragma of its own.
        self.internal(|conn| conn.serialize(MAIN_DB).map(|data| data.to_vec()))
            .map_err(|e| self.failure_of(e))
    }

    /// Runs one statement of the program's. `reads_only` refuses one that could write.
    pub fn run(&mut self, sql: &str, params: &[Scalar], limits: Limits, reads_only: bool) -> Reply {
        let mut pages = Vec::new();
        self.paged(sql, params, limits, reads_only, |page| {
            pages.push(page);
            // One page holding every row the limits allow, then the statement's end.
            (pages.len() == 1).then_some(i64::MAX)
        });
        let mut pages = pages.into_iter();
        let opened = pages.next().unwrap_or_default();
        match pages.next() {
            Some(page) if opened.failure.is_none() => Reply {
                columns: opened.columns,
                ..page
            },
            _ => opened,
        }
    }

    /// Runs one statement a page at a time. `pager` is handed what opening it answered, the
    /// columns or a failure, and then each page; it answers how many rows the next page may hold,
    /// or `None` to end the statement. A page shorter than was asked for is the last with rows.
    /// `limits.rows` bounds the rows of every page together and `limits.bytes` each page.
    pub fn paged(
        &mut self,
        sql: &str,
        params: &[Scalar],
        limits: Limits,
        reads_only: bool,
        mut pager: impl FnMut(Reply) -> Option<i64>,
    ) {
        if let Err(failure) = self.admit(sql, params, limits) {
            pager(Reply::failed(failure));
            return;
        }
        self.guard.take_refusal();
        let mut stmt = match self.conn.prepare_cached(sql) {
            Ok(stmt) => stmt,
            Err(e) => {
                pager(Reply::failed(self.failure_of(e)));
                return;
            }
        };
        let wrote = !stmt.readonly();
        let bound = (|| {
            if reads_only && wrote {
                return Err(Failure::new(
                    "writes",
                    "",
                    "this statement writes, and a query only reads: run it with `execute`",
                ));
            }
            if self.read_only && wrote {
                return Err(Failure::new(
                    "read_only",
                    "",
                    "this statement writes, and the connection was opened to read",
                ));
            }
            if stmt.parameter_count() != params.len() {
                return Err(Failure::refused(format!(
                    "the statement has {} placeholder(s) and was given {} parameter(s)",
                    stmt.parameter_count(),
                    params.len()
                )));
            }
            for (i, param) in params.iter().enumerate() {
                stmt.raw_bind_parameter(i + 1, param)
                    .map_err(|e| self.failure_of(e))?;
            }
            Ok(())
        })();
        if let Err(failure) = bound {
            pager(Reply::failed(failure));
            return;
        }
        let columns: Vec<String> = stmt
            .column_names()
            .into_iter()
            .map(str::to_string)
            .collect();
        // A statement that writes runs inside a savepoint, so one refused after it ran, for what
        // it answered or what it left, leaves nothing behind.
        let guarded = wrote;
        if guarded
            && let Err(e) = self.internal(|conn| conn.execute_batch("SAVEPOINT ply_statement"))
        {
            pager(Reply::failed(self.failure_of(e)));
            return;
        }
        let Some(mut ask) = pager(Reply {
            columns,
            wrote,
            ..Reply::default()
        }) else {
            drop(stmt);
            self.settle(guarded, false);
            return;
        };
        stmt.reset_status(StatementStatus::VmStep);
        self.guard.steps.store(limits.steps, Ordering::Relaxed);
        self.guard.spent.store(false, Ordering::Relaxed);
        self.guard.last_row_id_taken.store(false, Ordering::Relaxed);
        // SAFETY: the handle is this connection's, open for as long as `self.conn` is.
        unsafe { ffi::sqlite3_set_last_insert_rowid(self.conn.handle(), 0) };
        let changes_before = self.conn.total_changes();
        let width = stmt.column_count();
        let mut rows = stmt.raw_query();
        let mut seen = 0i64;
        // The page the statement ends in: its rows, or why it stopped.
        let last = loop {
            let mut page = Vec::new();
            let mut bytes = 0i64;
            let mut ended = None;
            while ended.is_none() && (page.len() as i64) < ask {
                match rows.next() {
                    Ok(None) => ended = Some(Ok(())),
                    Err(e) => ended = Some(Err(self.failure_of(e))),
                    Ok(Some(_)) if seen >= limits.rows => {
                        ended = Some(Err(Failure::too_large("rows", limits.rows)));
                    }
                    Ok(Some(row)) => match read_row(row, width) {
                        Err(refusal) => ended = Some(Err(refusal)),
                        Ok(values) => {
                            seen += 1;
                            bytes += values.iter().map(Scalar::bytes).sum::<i64>();
                            if bytes > limits.bytes {
                                ended = Some(Err(Failure::too_large("bytes", limits.bytes)));
                            } else {
                                page.push(values);
                            }
                        }
                    },
                }
            }
            match ended {
                Some(Ok(())) => break Ok(page),
                Some(Err(failure)) => break Err(failure),
                None => {}
            }
            let asked = pager(Reply {
                rows: page,
                wrote,
                ..Reply::default()
            });
            match asked {
                Some(next) => ask = next,
                None => {
                    drop(rows);
                    drop(stmt);
                    self.settle(guarded, false);
                    return;
                }
            }
        };
        drop(rows);
        drop(stmt);
        let mut reply = Reply {
            wrote,
            ended: true,
            ..Reply::default()
        };
        match last {
            Ok(page) => {
                reply.rows = page;
                reply.failure = self.left_behind(sql);
            }
            Err(failure) => reply.failure = Some(failure),
        }
        if let Some(failure) = self.settle(guarded, reply.failure.is_none()) {
            reply.failure.get_or_insert(failure);
        }
        if reply.failure.is_some() {
            reply.rows = Vec::new();
        } else {
            reply.changed = if self.conn.total_changes() == changes_before {
                0
            } else {
                i64::try_from(self.conn.changes()).unwrap_or(i64::MAX)
            };
            reply.last_row_id = self.conn.last_insert_rowid();
        }
        pager(reply);
    }

    /// One step of the connection's transaction. A `Begin` inside a transaction is a savepoint,
    /// and a `Commit` or `Rollback` closes the innermost scope whatever the engine answers.
    pub fn control(&mut self, step: Control) -> Reply {
        if self.depth > 0 && self.conn.is_autocommit() {
            // The engine rolled the whole transaction back under a failed statement.
            let scope = self.depth;
            self.depth = match step {
                Control::Begin | Control::BeginRead => self.depth,
                Control::Commit | Control::Rollback => self.depth - 1,
            };
            return match step {
                Control::Rollback => Reply::default(),
                _ => Reply::failed(aborted(scope)),
            };
        }
        let sql = match (step, self.depth) {
            (Control::Begin | Control::BeginRead, depth) if depth >= MAX_DEPTH => {
                return Reply::failed(Failure::refused(format!(
                    "a connection nests {MAX_DEPTH} transactions at most"
                )));
            }
            (Control::Begin, 0) => "BEGIN IMMEDIATE".to_string(),
            (Control::BeginRead, 0) => "BEGIN".to_string(),
            (Control::Begin | Control::BeginRead, depth) => format!("SAVEPOINT ply_{depth}"),
            (Control::Commit | Control::Rollback, 0) => {
                return Reply::failed(Failure::refused("no transaction is open"));
            }
            (Control::Commit, 1) => "COMMIT".to_string(),
            (Control::Commit, depth) => format!("RELEASE ply_{}", depth - 1),
            (Control::Rollback, 1) => "ROLLBACK".to_string(),
            (Control::Rollback, depth) => {
                format!("ROLLBACK TO ply_{0}; RELEASE ply_{0}", depth - 1)
            }
        };
        let done = self.internal(|conn| conn.execute_batch(&sql));
        match step {
            Control::Begin | Control::BeginRead => {
                if done.is_ok() {
                    self.depth += 1;
                }
            }
            Control::Commit | Control::Rollback => {
                self.depth -= 1;
                // A commit the engine refused leaves its transaction open, and this scope is
                // closed whatever it answered.
                if done.is_err() && self.depth == 0 && !self.conn.is_autocommit() {
                    let _ = self.internal(|conn| conn.execute_batch("ROLLBACK"));
                }
            }
        }
        match done {
            Ok(()) => Reply::default(),
            Err(e) => Reply::failed(self.failure_of(e)),
        }
    }

    /// A compacted copy of the database written to `to`, which its caller has confined and
    /// which holds nothing yet.
    pub fn backup(&mut self, to: &Path) -> Reply {
        let Some(to) = to.to_str() else {
            return Reply::failed(Failure::refused("the path is not UTF-8"));
        };
        let done = self.internal(|conn| {
            // `VACUUM INTO` attaches the file it writes, and a connection attaches none otherwise.
            conn.set_limit(Limit::SQLITE_LIMIT_ATTACHED, 1)?;
            let done = conn.execute("VACUUM INTO ?1", [to]);
            conn.set_limit(Limit::SQLITE_LIMIT_ATTACHED, 0)?;
            done
        });
        match done {
            Ok(_) => Reply::default(),
            Err(e) => Reply::failed(self.failure_of(e)),
        }
    }

    fn admit(&self, sql: &str, params: &[Scalar], limits: Limits) -> Result<(), Failure> {
        if self.depth > 0 && self.conn.is_autocommit() {
            return Err(aborted(self.depth));
        }
        if sql.len() as i64 > limits.sql_bytes {
            return Err(Failure::too_large("sql_bytes", limits.sql_bytes));
        }
        if params.len() as i64 > limits.params {
            return Err(Failure::too_large("params", limits.params));
        }
        // The one statement that writes another file and asks the authorizer nothing first.
        if first_word_is(sql, "vacuum") {
            return Err(Failure::refused(
                "a connection reaches its own database and no other file: `VACUUM` is refused, and a backup is how a copy is made",
            ));
        }
        Ok(())
    }

    /// What a statement that ran may not leave: a table that is not `STRICT` where the
    /// connection requires it, or a row at the largest row id.
    fn left_behind(&self, sql: &str) -> Option<Failure> {
        if self.guard.last_row_id_taken.load(Ordering::Relaxed) {
            return Some(Failure::refused(format!(
                "a row id of {} is refused: past it the engine draws row ids at random",
                i64::MAX
            )));
        }
        if !self.strict || !first_word_is(sql, "create") {
            return None;
        }
        let loose: rusqlite::Result<Option<String>> = self.internal(|conn| {
            let mut stmt = conn.prepare(
                "SELECT name FROM pragma_table_list WHERE schema = 'main' AND type = 'table' \
                 AND strict = 0 AND name NOT LIKE 'sqlite_%' ORDER BY name LIMIT 1",
            )?;
            let mut rows = stmt.query([])?;
            match rows.next()? {
                Some(row) => row.get(0).map(Some),
                None => Ok(None),
            }
        });
        match loose {
            Ok(None) => None,
            Ok(Some(table)) => Some(Failure::refused(format!(
                "the table `{table}` is not `STRICT`, and this connection requires every table to be"
            ))),
            Err(e) => Some(self.failure_of(e)),
        }
    }

    /// Closes the savepoint a statement ran in: kept, or rolled back to.
    fn settle(&self, guarded: bool, kept: bool) -> Option<Failure> {
        if !guarded || self.conn.is_autocommit() {
            return None;
        }
        let sql = if kept {
            "RELEASE ply_statement"
        } else {
            "ROLLBACK TO ply_statement; RELEASE ply_statement"
        };
        match self.internal(|conn| conn.execute_batch(sql)) {
            Ok(()) => None,
            Err(e) => {
                let failure = self.failure_of(e);
                // A release the engine refused is a commit that did not happen.
                if self.depth == 0 && !self.conn.is_autocommit() {
                    let _ = self.internal(|conn| conn.execute_batch("ROLLBACK"));
                }
                Some(failure)
            }
        }
    }

    fn internal<T>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<T> {
        self.guard.internal.store(true, Ordering::Relaxed);
        let out = f(&self.conn);
        self.guard.internal.store(false, Ordering::Relaxed);
        out
    }

    fn failure_of(&self, e: rusqlite::Error) -> Failure {
        if let Some(why) = self.guard.take_refusal() {
            return Failure::refused(why);
        }
        let spent = self.guard.spent.swap(false, Ordering::Relaxed);
        match e {
            rusqlite::Error::MultipleStatement => {
                Failure::refused("one statement at a time: this text holds another after its `;`")
            }
            rusqlite::Error::NulError(_) => {
                Failure::refused("the statement's text holds a NUL byte")
            }
            rusqlite::Error::SqlInputError {
                error, msg, offset, ..
            } => {
                let at = if offset >= 0 {
                    format!(" at byte {offset}")
                } else {
                    String::new()
                };
                by_code(error, format!("{msg}{at}"), spent)
            }
            rusqlite::Error::SqliteFailure(error, msg) => {
                by_code(error, msg.unwrap_or_default(), spent)
            }
            other => Failure::new("engine", "", other.to_string()),
        }
    }
}

fn aborted(depth: u32) -> Failure {
    Failure::new(
        "aborted",
        "",
        format!(
            "the engine rolled the transaction back under an earlier failure, and {depth} scope(s) of it are still open: nothing more runs in it"
        ),
    )
}

/// Why a database did not open, in the engine's own words.
pub fn unopened(e: rusqlite::Error) -> Failure {
    let detail = match &e {
        rusqlite::Error::SqliteFailure(_, Some(msg)) => msg.clone(),
        other => other.to_string(),
    };
    Failure::new("unopened", "", detail)
}

/// The engine's result code as a failure `std.sqlite` types.
fn by_code(error: ffi::Error, msg: String, spent: bool) -> Failure {
    let extended = error.extended_code;
    match extended & 0xff {
        ffi::SQLITE_BUSY | ffi::SQLITE_LOCKED => Failure::new("busy", "", msg),
        ffi::SQLITE_CONSTRAINT => {
            let kind = match extended {
                ffi::SQLITE_CONSTRAINT_UNIQUE
                | ffi::SQLITE_CONSTRAINT_PRIMARYKEY
                | ffi::SQLITE_CONSTRAINT_ROWID => "unique",
                ffi::SQLITE_CONSTRAINT_FOREIGNKEY => "foreign_key",
                ffi::SQLITE_CONSTRAINT_NOTNULL => "not_null",
                ffi::SQLITE_CONSTRAINT_CHECK => "check",
                ffi::SQLITE_CONSTRAINT_DATATYPE => "datatype",
                _ => "constraint",
            };
            Failure::new(kind, constraint_named(&msg), msg)
        }
        ffi::SQLITE_INTERRUPT if spent => Failure::new(
            "steps",
            "",
            "the statement ran past the steps its caller allowed",
        ),
        ffi::SQLITE_AUTH => Failure::refused(msg),
        ffi::SQLITE_READONLY => Failure::new("read_only", "", msg),
        ffi::SQLITE_TOOBIG => Failure::new("too_large", "value", msg),
        ffi::SQLITE_ERROR => match msg.strip_prefix(REFUSED) {
            Some(why) => Failure::refused(why),
            None => Failure::new("statement", "", msg),
        },
        ffi::SQLITE_RANGE | ffi::SQLITE_MISMATCH => Failure::new("statement", "", msg),
        code => Failure::new("engine", code_name(code), msg),
    }
}

fn code_name(code: i32) -> String {
    match code {
        ffi::SQLITE_CANTOPEN => "SQLITE_CANTOPEN".to_string(),
        ffi::SQLITE_CORRUPT => "SQLITE_CORRUPT".to_string(),
        ffi::SQLITE_FULL => "SQLITE_FULL".to_string(),
        ffi::SQLITE_INTERRUPT => "SQLITE_INTERRUPT".to_string(),
        ffi::SQLITE_IOERR => "SQLITE_IOERR".to_string(),
        ffi::SQLITE_NOMEM => "SQLITE_NOMEM".to_string(),
        ffi::SQLITE_NOTADB => "SQLITE_NOTADB".to_string(),
        ffi::SQLITE_PERM => "SQLITE_PERM".to_string(),
        ffi::SQLITE_SCHEMA => "SQLITE_SCHEMA".to_string(),
        other => format!("SQLITE_{other}"),
    }
}

/// What a constraint failure names: `t.a, t.b` of `UNIQUE constraint failed: t.a, t.b`, a check's
/// name or text, a strict column, and nothing for a foreign key, which the engine does not name.
fn constraint_named(msg: &str) -> String {
    msg.split_once("failed: ")
        .or_else(|| msg.split_once(" column "))
        .map(|(_, name)| name.to_string())
        .unwrap_or_default()
}

/// Whether `sql` opens with the keyword `word`, past its leading blanks and comments.
fn first_word_is(sql: &str, word: &str) -> bool {
    let mut rest = sql;
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix("--") {
            rest = after.split_once('\n').map_or("", |(_, tail)| tail);
        } else if let Some(after) = rest.strip_prefix("/*") {
            rest = after.split_once("*/").map_or("", |(_, tail)| tail);
        } else {
            break;
        }
    }
    rest.get(..word.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(word))
        && !rest[word.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn read_row(row: &rusqlite::Row<'_>, width: usize) -> Result<Vec<Scalar>, Failure> {
    (0..width)
        .map(|i| {
            Ok(match row.get_ref(i).map_err(unreadable)? {
                ValueRef::Null => Scalar::Null,
                ValueRef::Integer(n) => Scalar::Int(n),
                ValueRef::Real(x) => Scalar::Real(x),
                ValueRef::Text(bytes) => match std::str::from_utf8(bytes) {
                    Ok(text) => Scalar::Text(text.to_string()),
                    Err(_) => {
                        return Err(Failure::new(
                            "text",
                            "",
                            format!("column {i} holds text that is not UTF-8: read it with `cast(.. as blob)`"),
                        ));
                    }
                },
                ValueRef::Blob(bytes) => Scalar::Blob(bytes.to_vec()),
            })
        })
        .collect()
}

fn unreadable(e: rusqlite::Error) -> Failure {
    Failure::new("engine", "", e.to_string())
}

/// The settings every connection holds, which no statement changes: the authorizer refuses the
/// pragmas that would.
fn configure(conn: &Connection, guard: &Arc<Guard>, statements: usize) -> rusqlite::Result<()> {
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_FKEY, true)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DQS_DML, false)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DQS_DDL, false)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_ATTACH_CREATE, false)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_ATTACH_WRITE, false)?;
    conn.set_limit(Limit::SQLITE_LIMIT_ATTACHED, 0)?;
    conn.set_limit(Limit::SQLITE_LIMIT_LENGTH, MAX_VALUE_BYTES)?;
    conn.set_limit(Limit::SQLITE_LIMIT_SQL_LENGTH, 1 << 20)?;
    conn.set_limit(Limit::SQLITE_LIMIT_COLUMN, 500)?;
    conn.set_limit(Limit::SQLITE_LIMIT_EXPR_DEPTH, 200)?;
    conn.set_limit(Limit::SQLITE_LIMIT_COMPOUND_SELECT, 100)?;
    conn.set_limit(Limit::SQLITE_LIMIT_FUNCTION_ARG, 64)?;
    conn.set_limit(Limit::SQLITE_LIMIT_LIKE_PATTERN_LENGTH, 4096)?;
    conn.set_limit(Limit::SQLITE_LIMIT_TRIGGER_DEPTH, 32)?;
    conn.set_limit(Limit::SQLITE_LIMIT_WORKER_THREADS, 0)?;
    conn.set_prepared_statement_cache_capacity(statements);
    for &name in CLOCKED {
        // Innocuous, as the functions they stand in for are: a view or a trigger may call one
        // though the schema is not trusted.
        conn.create_scalar_function(
            name,
            -1,
            FunctionFlags::SQLITE_UTF8
                | FunctionFlags::SQLITE_DETERMINISTIC
                | FunctionFlags::SQLITE_INNOCUOUS,
            move |ctx| clocked(name, ctx),
        )?;
    }
    let authorizing = Arc::clone(guard);
    conn.authorizer(Some(move |ctx: AuthContext<'_>| {
        if authorizing.internal.load(Ordering::Relaxed) {
            return Authorization::Allow;
        }
        match refusal(ctx.action) {
            Some(why) => authorizing.refuse(why),
            None => Authorization::Allow,
        }
    }))?;
    let counting = Arc::clone(guard);
    conn.progress_handler(
        STEP_GRAIN,
        Some(move || {
            if counting.internal.load(Ordering::Relaxed) {
                return false;
            }
            let left = counting.steps.fetch_sub(1, Ordering::Relaxed) - 1;
            if left < 0 {
                counting.spent.store(true, Ordering::Relaxed);
            }
            left < 0
        }),
    )?;
    let watching = Arc::clone(guard);
    conn.update_hook(Some(
        move |action: Action, _: &str, _: &str, row_id: i64| {
            if row_id == i64::MAX && action != Action::SQLITE_DELETE {
                watching.last_row_id_taken.store(true, Ordering::Relaxed);
            }
        },
    ))?;
    Ok(())
}

/// Why the authorizer refuses `action`, or `None` for one a statement may take.
fn refusal(action: AuthAction<'_>) -> Option<String> {
    match action {
        AuthAction::Attach { .. } | AuthAction::Detach { .. } => Some(
            "a connection reaches its own database and no other file: `ATTACH`, `DETACH` and `VACUUM` are refused"
                .to_string(),
        ),
        AuthAction::Pragma {
            pragma_name,
            pragma_value,
        } => {
            let name = pragma_name.to_ascii_lowercase();
            match PRAGMAS.iter().find(|(admitted, _)| *admitted == name) {
                None => Some(format!(
                    "the pragma `{name}` is not one a statement may run: a connection's settings are fixed when it opens"
                )),
                Some((_, false)) if pragma_value.is_some() => Some(format!(
                    "the pragma `{name}` may be read and not set: a connection's settings are fixed when it opens"
                )),
                Some(_) => None,
            }
        }
        AuthAction::Function { function_name } => {
            let name = function_name.to_ascii_lowercase();
            if FUNCTIONS.contains(&name.as_str()) || INTERNAL_FUNCTIONS.contains(&name.as_str()) {
                None
            } else {
                Some(format!(
                    "the function `{name}` is not one a statement may call: what it answers is not a function of the database and the values bound"
                ))
            }
        }
        AuthAction::Transaction { .. } | AuthAction::Savepoint { .. } => Some(
            "a transaction is opened and closed by `transaction`, never by a statement".to_string(),
        ),
        AuthAction::CreateTempTable { .. }
        | AuthAction::CreateTempIndex { .. }
        | AuthAction::CreateTempTrigger { .. }
        | AuthAction::CreateTempView { .. }
        | AuthAction::DropTempTable { .. }
        | AuthAction::DropTempIndex { .. }
        | AuthAction::DropTempTrigger { .. }
        | AuthAction::DropTempView { .. } => Some(
            "temporary tables, indexes, triggers and views are refused: they belong to a connection, not to the database"
                .to_string(),
        ),
        AuthAction::CreateVtable { module_name, .. } | AuthAction::DropVtable { module_name, .. } => {
            Some(format!("virtual tables are refused, `{module_name}`'s among them"))
        }
        AuthAction::Unknown { code, .. } => Some(format!(
            "the engine asked leave for an action this build does not know (code {code})"
        )),
        _ => None,
    }
}

thread_local! {
    /// The engine's own date and time functions, on a connection that holds nothing: where a
    /// guard sends the calls it lets through.
    static UNGUARDED: Option<Connection> = Connection::open_in_memory().ok();
}

/// A date or time function, refused where it would read the clock or the machine's zone: with no
/// time given, with the time `now`, or with the modifier `localtime` or `utc`. Which of those a
/// call does is decided by its values, so it is judged as it runs.
fn clocked(name: &'static str, ctx: &Context<'_>) -> rusqlite::Result<rusqlite::types::Value> {
    let times = match name {
        "strftime" => 1..2,
        "timediff" => 0..2,
        _ => 0..1,
    };
    let refuse = |why: String| Err(rusqlite::Error::UserFunctionError(why.into()));
    if ctx.len() < times.end {
        return refuse(format!(
            "{REFUSED}`{name}` with no time reads the clock: bind the time as a parameter"
        ));
    }
    let args: Vec<Scalar> = (0..ctx.len())
        .map(|i| match ctx.get_raw(i) {
            ValueRef::Null => Scalar::Null,
            ValueRef::Integer(n) => Scalar::Int(n),
            ValueRef::Real(x) => Scalar::Real(x),
            ValueRef::Text(bytes) => Scalar::Text(String::from_utf8_lossy(bytes).into_owned()),
            ValueRef::Blob(bytes) => Scalar::Blob(bytes.to_vec()),
        })
        .collect();
    for arg in &args {
        if let Scalar::Text(text) = arg {
            let word = text.trim().to_ascii_lowercase();
            if word == "now" {
                return refuse(format!(
                    "{REFUSED}`{name}` of `now` reads the clock: bind the time as a parameter"
                ));
            }
            if word == "localtime" || word == "utc" {
                return refuse(format!(
                    "{REFUSED}`{name}` with `{word}` reads the machine's time zone: convert with `std.tz` and bind the result"
                ));
            }
        }
    }
    UNGUARDED.with(|unguarded| {
        let Some(conn) = unguarded else {
            return refuse(format!("{REFUSED}`{name}` could not be answered"));
        };
        let holes: Vec<String> = (1..=args.len()).map(|i| format!("?{i}")).collect();
        let mut stmt = conn.prepare_cached(&format!("SELECT {name}({})", holes.join(", ")))?;
        stmt.query_row(rusqlite::params_from_iter(args.iter()), |row| row.get(0))
    })
}

/// `sqlite_run(image, sql, params, reads_only, strict, limits)`.
pub fn run_image(args: &[Value], span: Span) -> Result<Value, Diagnostic> {
    let what = "`sqlite_run`";
    let image = args[0].as_bytes(span, what)?;
    let sql = args[1].as_str(span, what)?;
    let params = scalars_of(&args[2], span, what)?;
    let reads_only = args[3].as_bool(span, what)?;
    let strict = args[4].as_bool(span, what)?;
    let limits = Limits::of(&args[5], span, what)?;
    let (reply, left) = match Engine::memory(image, strict) {
        Err(failure) => (Reply::failed(failure), None),
        Ok(mut engine) => {
            let reply = engine.run(sql, &params, limits, reads_only);
            if reply.failure.is_some() || !reply.wrote {
                (reply, None)
            } else {
                match engine.image() {
                    Ok(left) => (reply, Some(left)),
                    Err(failure) => (Reply::failed(failure), None),
                }
            }
        }
    };
    let image = match left {
        Some(left) => Value::bytes(left),
        None => args[0].clone(),
    };
    let mut fields: Vec<(Symbol, Value)> = reply
        .fields()
        .into_iter()
        .map(|(name, value)| (Symbol::new(name), value))
        .collect();
    fields.push((Symbol::new("image"), image));
    Ok(Value::Record(Arc::new(Fields::from_unsorted(fields))))
}

/// `sqlite_functions()`.
pub fn functions() -> Value {
    Value::list(FUNCTIONS.iter().map(Value::str).collect())
}
