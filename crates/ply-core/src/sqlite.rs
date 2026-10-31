use rusqlite::config::DbConfig;
use rusqlite::functions::{Context, FunctionFlags};
use rusqlite::hooks::{Action, AuthAction, AuthContext, Authorization};
use rusqlite::limits::Limit;
use rusqlite::types::{ToSqlOutput, ValueRef};
use rusqlite::{Connection, MAIN_DB, OpenFlags, StatementStatus, ffi};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

pub const MAX_VALUE_BYTES: i32 = 64 * 1024 * 1024;

pub const MAX_DEPTH: u32 = 32;

const PROGRESS_CHECK_INTERVAL: i32 = 1000;

const REJECTED: &str = "refused: ";

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

const CLOCK_READING_FUNCTIONS: &[&str] = &[
    "date",
    "time",
    "datetime",
    "julianday",
    "unixepoch",
    "strftime",
    "timediff",
];

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub sql_bytes: i64,
    pub params: i64,
    pub rows: i64,
    pub bytes: i64,
    pub steps: i64,
}

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

    fn rejected(detail: impl Into<String>) -> Failure {
        Failure::new("refused", "", detail)
    }

    fn too_large(limit: &'static str, bound: i64) -> Failure {
        Failure::new(
            "too_large",
            limit,
            format!("more than the {bound} its caller allowed"),
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reply {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Scalar>>,
    pub changed: i64,
    pub last_row_id: i64,
    pub failure: Option<Failure>,
    pub wrote: bool,
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
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    ReadOnly,
    ReadWrite,
    Create,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub strict: bool,
    pub busy_ms: i64,
    pub statements: i64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Control {
    Begin,
    BeginRead,
    Commit,
    Rollback,
}

#[derive(Default)]
struct SandboxState {
    internal: AtomicBool,
    rejected: Mutex<Option<String>>,
    steps: AtomicI64,
    spent: AtomicBool,
    left_row_at_max_row_id: AtomicBool,
}

impl SandboxState {
    fn refuse(&self, reason: String) -> Authorization {
        *self.rejected.lock().unwrap_or_else(|e| e.into_inner()) = Some(reason);
        Authorization::Deny
    }

    fn take_rejection_reason(&self) -> Option<String> {
        self.rejected
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }
}

pub struct Engine {
    conn: Connection,
    guard: Arc<SandboxState>,
    depth: u32,
    strict: bool,
    read_only: bool,
}

impl Engine {
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
        let guard = Arc::new(SandboxState::default());
        configure(&conn, &guard, 0).map_err(unopened)?;
        Ok(Engine {
            conn,
            guard,
            depth: 0,
            strict,
            read_only: false,
        })
    }

    pub fn file(conn: Connection, mode: Mode, options: Options) -> Result<Engine, Failure> {
        let guard = Arc::new(SandboxState::default());
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
                    let _ = conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()));
                }
                conn.execute_batch("PRAGMA cell_size_check=ON")?;
                conn.query_row("SELECT count(*) FROM sqlite_schema", [], |_| Ok(()))
            })
            .map_err(unopened)?;
        Ok(engine)
    }

    pub fn image(&self) -> Result<Vec<u8>, Failure> {
        let pages: i64 = self
            .internal(|conn| conn.query_row("PRAGMA page_count", [], |row| row.get(0)))
            .map_err(|e| self.failure_of(e))?;
        if pages == 0 {
            return Ok(Vec::new());
        }
        self.internal(|conn| conn.serialize(MAIN_DB).map(|data| data.to_vec()))
            .map_err(|e| self.failure_of(e))
    }

    pub fn run(&mut self, sql: &str, params: &[Scalar], limits: Limits, reads_only: bool) -> Reply {
        let mut pages = Vec::new();
        self.run_in_pages(sql, params, limits, reads_only, |page| {
            pages.push(page);
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

    pub fn run_in_pages(
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
        self.guard.take_rejection_reason();
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
                return Err(Failure::rejected(format!(
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
            self.close_savepoint(guarded, false);
            return;
        };
        stmt.reset_status(StatementStatus::VmStep);
        self.guard.steps.store(limits.steps, Ordering::Relaxed);
        self.guard.spent.store(false, Ordering::Relaxed);
        self.guard
            .left_row_at_max_row_id
            .store(false, Ordering::Relaxed);
        unsafe { ffi::sqlite3_set_last_insert_rowid(self.conn.handle(), 0) };
        let changes_before = self.conn.total_changes();
        let width = stmt.column_count();
        let mut rows = stmt.raw_query();
        let mut seen = 0i64;
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
                        Err(rejection) => ended = Some(Err(rejection)),
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
                    self.close_savepoint(guarded, false);
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
                reply.failure = self.check_schema_and_row_ids(sql);
            }
            Err(failure) => reply.failure = Some(failure),
        }
        if let Some(failure) = self.close_savepoint(guarded, reply.failure.is_none()) {
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

    pub fn control(&mut self, step: Control) -> Reply {
        if self.depth > 0 && self.conn.is_autocommit() {
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
                return Reply::failed(Failure::rejected(format!(
                    "a connection nests {MAX_DEPTH} transactions at most"
                )));
            }
            (Control::Begin, 0) => "BEGIN IMMEDIATE".to_string(),
            (Control::BeginRead, 0) => "BEGIN".to_string(),
            (Control::Begin | Control::BeginRead, depth) => format!("SAVEPOINT ply_{depth}"),
            (Control::Commit | Control::Rollback, 0) => {
                return Reply::failed(Failure::rejected("no transaction is open"));
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

    pub fn backup(&mut self, to: &Path) -> Reply {
        let Some(to) = to.to_str() else {
            return Reply::failed(Failure::rejected("the path is not UTF-8"));
        };
        let done = self.internal(|conn| {
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
        if first_word_is(sql, "vacuum") {
            return Err(Failure::rejected(
                "a connection reaches its own database and no other file: `VACUUM` is rejected, and a backup is how a copy is made",
            ));
        }
        Ok(())
    }

    fn check_schema_and_row_ids(&self, sql: &str) -> Option<Failure> {
        if self.guard.left_row_at_max_row_id.load(Ordering::Relaxed) {
            return Some(Failure::rejected(format!(
                "a row id of {} is rejected: past it the engine draws row ids at random",
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
            Ok(Some(table)) => Some(Failure::rejected(format!(
                "the table `{table}` is not `STRICT`, and this connection requires every table to be"
            ))),
            Err(e) => Some(self.failure_of(e)),
        }
    }

    fn close_savepoint(&self, guarded: bool, kept: bool) -> Option<Failure> {
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
        if let Some(reason) = self.guard.take_rejection_reason() {
            return Failure::rejected(reason);
        }
        let spent = self.guard.spent.swap(false, Ordering::Relaxed);
        match e {
            rusqlite::Error::MultipleStatement => {
                Failure::rejected("one statement at a time: this text holds another after its `;`")
            }
            rusqlite::Error::NulError(_) => {
                Failure::rejected("the statement's text holds a NUL byte")
            }
            rusqlite::Error::SqlInputError {
                error, msg, offset, ..
            } => {
                let at = if offset >= 0 {
                    format!(" at byte {offset}")
                } else {
                    String::new()
                };
                failure_from_sqlite_code(error, format!("{msg}{at}"), spent)
            }
            rusqlite::Error::SqliteFailure(error, msg) => {
                failure_from_sqlite_code(error, msg.unwrap_or_default(), spent)
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

pub fn unopened(e: rusqlite::Error) -> Failure {
    let detail = match &e {
        rusqlite::Error::SqliteFailure(_, Some(msg)) => msg.clone(),
        other => other.to_string(),
    };
    Failure::new("unopened", "", detail)
}

fn failure_from_sqlite_code(error: ffi::Error, msg: String, spent: bool) -> Failure {
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
        ffi::SQLITE_AUTH => Failure::rejected(msg),
        ffi::SQLITE_READONLY => Failure::new("read_only", "", msg),
        ffi::SQLITE_TOOBIG => Failure::new("too_large", "value", msg),
        ffi::SQLITE_ERROR => match msg.strip_prefix(REJECTED) {
            Some(reason) => Failure::rejected(reason),
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

fn constraint_named(msg: &str) -> String {
    msg.split_once("failed: ")
        .or_else(|| msg.split_once(" column "))
        .map(|(_, name)| name.to_string())
        .unwrap_or_default()
}

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

fn configure(
    conn: &Connection,
    guard: &Arc<SandboxState>,
    statements: usize,
) -> rusqlite::Result<()> {
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
    for &name in CLOCK_READING_FUNCTIONS {
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
        match rejection(ctx.action) {
            Some(reason) => authorizing.refuse(reason),
            None => Authorization::Allow,
        }
    }))?;
    let counting = Arc::clone(guard);
    conn.progress_handler(
        PROGRESS_CHECK_INTERVAL,
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
                watching
                    .left_row_at_max_row_id
                    .store(true, Ordering::Relaxed);
            }
        },
    ))?;
    Ok(())
}

fn rejection(action: AuthAction<'_>) -> Option<String> {
    match action {
        AuthAction::Attach { .. } | AuthAction::Detach { .. } => Some(
            "a connection reaches its own database and no other file: `ATTACH`, `DETACH` and `VACUUM` are rejected"
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
                    "the function `{name}` is not one a statement may call: what it returns is not a function of the database and the values bound"
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
            "temporary tables, indexes, triggers and views are rejected: they belong to a connection, not to the database"
                .to_string(),
        ),
        AuthAction::CreateVtable { module_name, .. } | AuthAction::DropVtable { module_name, .. } => {
            Some(format!("virtual tables are rejected, `{module_name}`'s among them"))
        }
        AuthAction::Unknown { code, .. } => Some(format!(
            "the engine asked leave for an action this build does not know (code {code})"
        )),
        _ => None,
    }
}

thread_local! {
    static EMPTY_DATE_FUNCTION_CONNECTION: Option<Connection> = Connection::open_in_memory().ok();
}

fn clocked(name: &'static str, ctx: &Context<'_>) -> rusqlite::Result<rusqlite::types::Value> {
    let times = match name {
        "strftime" => 1..2,
        "timediff" => 0..2,
        _ => 0..1,
    };
    let refuse = |reason: String| Err(rusqlite::Error::UserFunctionError(reason.into()));
    if ctx.len() < times.end {
        return refuse(format!(
            "{REJECTED}`{name}` with no time reads the clock: bind the time as a parameter"
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
                    "{REJECTED}`{name}` of `now` reads the clock: bind the time as a parameter"
                ));
            }
            if word == "localtime" || word == "utc" {
                return refuse(format!(
                    "{REJECTED}`{name}` with `{word}` reads the machine's time zone: convert with `std.tz` and bind the result"
                ));
            }
        }
    }
    EMPTY_DATE_FUNCTION_CONNECTION.with(|unguarded| {
        let Some(conn) = unguarded else {
            return refuse(format!("{REJECTED}`{name}` could not be evaluated"));
        };
        let holes: Vec<String> = (1..=args.len()).map(|i| format!("?{i}")).collect();
        let mut stmt = conn.prepare_cached(&format!("SELECT {name}({})", holes.join(", ")))?;
        stmt.query_row(rusqlite::params_from_iter(args.iter()), |row| row.get(0))
    })
}

pub fn run_on_image(
    image: &[u8],
    sql: &str,
    params: &[Scalar],
    limits: Limits,
    reads_only: bool,
    strict: bool,
) -> (Reply, Option<Vec<u8>>) {
    match Engine::memory(image, strict) {
        Err(failure) => (Reply::failed(failure), None),
        Ok(mut engine) => {
            let reply = engine.run(sql, params, limits, reads_only);
            if reply.failure.is_some() || !reply.wrote {
                (reply, None)
            } else {
                match engine.image() {
                    Ok(left) => (reply, Some(left)),
                    Err(failure) => (Reply::failed(failure), None),
                }
            }
        }
    }
}
