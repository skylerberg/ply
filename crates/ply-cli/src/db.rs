//! How a run is told which database to talk to, and what it may say about it afterwards.

use ply_span::{Diagnostic, Symbol, codes};
use ply_ty::CheckOutput;
use ply_ty::ty::Type;
use std::collections::BTreeMap;
use std::fmt;

/// Path prefix of every postgres handler; how a listing row is recognised as one.
pub const HANDLER_PREFIX: &str = "ply_host::db::";

/// The SQL scanner `ply hosts` discloses: a parser inside the trusted computing base.
pub const SCANNER: &str = "ply_host::db::scan";

/// Statements the scanner accepts (else `E0432`); read from it so the listing cannot drift.
pub const ACCEPTED: &str = ply_host::db::scan::ACCEPTED;

/// Connection string when `--db` is absent; read only under `--host`, so it never binds.
pub const URL_ENV: &str = "PLY_DB_URL";

/// The password, kept out of `--db` so it never shows in `ps` or a shell history.
pub const PASSWORD_ENV: &str = "PLY_DB_PASSWORD";

/// Where a run's connection string came from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    Flag,
    Environment,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Flag => "--db",
            Source::Environment => URL_ENV,
        }
    }
}

/// A password that renders as `****` everywhere; only [`Secret::expose`] yields the bytes.
#[derive(Clone)]
pub struct Secret(String);

pub const REDACTED: &str = "****";

impl Secret {
    pub fn new(text: impl Into<String>) -> Secret {
        Secret(text.into())
    }

    /// Only the code that opens a connection may call this.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Secret({REDACTED})")
    }
}

impl PartialEq for Secret {
    fn eq(&self, other: &Secret) -> bool {
        self.0 == other.0
    }
}

impl Eq for Secret {}

/// `sslmode`, limited to modes that need no TLS; `require` and above are `E0431`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SslMode {
    #[default]
    Prefer,
    Disable,
}

impl SslMode {
    pub fn as_str(self) -> &'static str {
        match self {
            SslMode::Prefer => "prefer",
            SslMode::Disable => "disable",
        }
    }
}

/// A parsed `postgres://` connection string; `Display` and `Debug` redact the password.
#[derive(Clone, PartialEq, Eq)]
pub struct DbUrl {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
    password: Option<Secret>,
    pub sslmode: SslMode,
    /// Only `application_name`; any other parameter is refused rather than dropped.
    pub parameters: BTreeMap<String, String>,
}

pub const DEFAULT_PORT: u16 = 5432;

impl DbUrl {
    /// `postgres://user[:password]@host[:port]/database[?sslmode=…]`.
    pub fn parse(text: &str) -> Result<DbUrl, String> {
        let rest = text
            .strip_prefix("postgres://")
            .or_else(|| text.strip_prefix("postgresql://"))
            .ok_or_else(|| {
                if text.contains('=') && !text.contains("://") {
                    "this is libpq's keyword/value form; W4 reads the URI form, so write \
                     `postgres://user@host:5432/database`"
                        .to_string()
                } else {
                    "it does not begin with `postgres://`; write \
                     `postgres://user@host:5432/database`"
                        .to_string()
                }
            })?;

        let (authority, path_and_query) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i + 1..]),
            None => (rest, ""),
        };
        let (userinfo, hostport) = match authority.rfind('@') {
            Some(i) => (&authority[..i], &authority[i + 1..]),
            None => {
                return Err("there is no `user@` before the host; a run whose database \
                                user comes from the environment is a run whose identity \
                                depends on who invoked it"
                    .to_string());
            }
        };

        let (user, password) = match userinfo.split_once(':') {
            Some((user, password)) => (user, Some(decode(password)?)),
            None => (userinfo, None),
        };
        let user = decode(user)?;
        if user.is_empty() {
            return Err("the user is empty".to_string());
        }

        let (host, port) = match hostport.rsplit_once(':') {
            Some((host, port)) => {
                let port = port
                    .parse::<u16>()
                    .map_err(|_| format!("`{port}` is not a port"))?;
                if port == 0 {
                    return Err("port 0 is not a port a server listens on".to_string());
                }
                (host.to_string(), port)
            }
            None => (hostport.to_string(), DEFAULT_PORT),
        };
        if host.is_empty() {
            return Err(
                "there is no host; W4 connects over TCP, so a unix socket path \
                        is not configurable here"
                    .to_string(),
            );
        }

        let (database, query) = match path_and_query.split_once('?') {
            Some((database, query)) => (database, query),
            None => (path_and_query, ""),
        };
        let database = decode(database)?;
        if database.is_empty() {
            return Err("there is no database name after the host".to_string());
        }

        let mut sslmode = SslMode::default();
        let mut parameters = BTreeMap::new();
        for pair in query.split('&').filter(|p| !p.is_empty()) {
            let (key, value) = pair
                .split_once('=')
                .ok_or_else(|| format!("`{pair}` is not a `key=value` parameter"))?;
            let value = decode(value)?;
            match key {
                "sslmode" => {
                    sslmode = match value.as_str() {
                        "disable" => SslMode::Disable,
                        "prefer" => SslMode::Prefer,
                        other => {
                            return Err(format!(
                                "`sslmode={other}` is not configurable in W4: TLS to postgres \
                                 is not wired up, so only `disable` and `prefer` are accepted \
                                 and anything stronger would be a word that lies"
                            ));
                        }
                    }
                }
                "application_name" => {
                    parameters.insert(key.to_string(), value);
                }
                other => {
                    return Err(format!(
                        "`{other}` is not a parameter W4 reads; it accepts `sslmode` and \
                         `application_name`, and the timeouts are `--db-connect-ms`, \
                         `--db-statement-ms` and `--db-idle-txn-ms`"
                    ));
                }
            }
        }

        Ok(DbUrl {
            host,
            port,
            database,
            user,
            password: password.map(Secret::new),
            sslmode,
            parameters,
        })
    }

    pub fn password(&self) -> Option<&Secret> {
        self.password.as_ref()
    }

    pub fn has_password(&self) -> bool {
        self.password.is_some()
    }

    /// Callers refuse a second password rather than overwrite; see [`DbOptions::resolve_with`].
    fn set_password(&mut self, secret: Secret) {
        self.password = Some(secret);
    }

    /// The only rendering that carries the password, rebuilt from the validated fields.
    pub fn connection_string(&self) -> Secret {
        let mut out = String::from("postgres://");
        out.push_str(&encode(&self.user));
        if let Some(password) = &self.password {
            out.push(':');
            out.push_str(&encode(password.expose()));
        }
        out.push('@');
        out.push_str(&format!(
            "{}:{}/{}?sslmode={}",
            self.host,
            self.port,
            encode(&self.database),
            self.sslmode.as_str()
        ));
        for (key, value) in &self.parameters {
            out.push_str(&format!("&{key}={}", encode(value)));
        }
        Secret::new(out)
    }

    /// The form for diagnostics, listings and reports: the password is `****`.
    pub fn redacted(&self) -> String {
        let mut out = String::from("postgres://");
        out.push_str(&self.user);
        if self.password.is_some() {
            out.push(':');
            out.push_str(REDACTED);
        }
        out.push('@');
        out.push_str(&self.host);
        out.push_str(&format!(":{}/{}", self.port, self.database));
        out.push_str(&format!("?sslmode={}", self.sslmode.as_str()));
        for (key, value) in &self.parameters {
            out.push_str(&format!("&{key}={value}"));
        }
        out
    }
}

impl fmt::Display for DbUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.redacted())
    }
}

/// Redacted, so `{:?}` in a `dbg!` or an `expect` cannot leak the password.
impl fmt::Debug for DbUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DbUrl({})", self.redacted())
    }
}

/// Percent-encoding; the inverse of [`decode`].
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Percent-decoding, so a password containing `@` or `/` is not truncated at the delimiter.
fn decode(text: &str) -> Result<String, String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes
                .get(i + 1..i + 3)
                .ok_or_else(|| format!("`{text}` ends inside a percent-escape"))?;
            let hex = std::str::from_utf8(hex).map_err(|_| format!("`{text}` is not UTF-8"))?;
            out.push(
                u8::from_str_radix(hex, 16)
                    .map_err(|_| format!("`%{hex}` is not a percent-escape"))?,
            );
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| format!("`{text}` percent-decodes to something not UTF-8"))
}

/// Taken from the pool so the printed and enforced numbers cannot differ.
pub const DEFAULT_POOL: u32 = ply_host::db::pool::DEFAULT_POOL_SIZE as u32;
pub const DEFAULT_ACQUIRE_MS: u64 = ply_host::db::pool::DEFAULT_ACQUIRE_MS;
pub const DEFAULT_STATEMENT_MS: u64 = ply_host::db::pool::DEFAULT_STATEMENT_MS;
pub const DEFAULT_IDLE_TXN_MS: u64 = ply_host::db::pool::DEFAULT_IDLE_TXN_MS;
pub const DEFAULT_CONNECT_MS: u64 = ply_host::db::pool::DEFAULT_CONNECT_MS;
/// A per-connection setting, so not among the pool's defaults.
pub const DEFAULT_STATEMENT_CACHE: u32 = 256;

/// A run's validated database configuration; its pool numbers feed the `ply hosts` digest.
#[derive(Clone, Debug)]
pub struct DbConfig {
    pub url: DbUrl,
    pub source: Source,
    pub pool: u32,
    pub acquire_ms: u64,
    pub statement_ms: u64,
    pub idle_txn_ms: u64,
    pub connect_ms: u64,
    pub statement_cache: u32,
    /// `<module>.<fn>`, resolved against the program by [`schema::resolve`].
    pub schema: Option<String>,
}

impl DbConfig {
    /// Built from the same numbers the report and digest carry.
    pub fn pool_config(&self) -> (Secret, PoolBounds) {
        (
            self.url.connection_string(),
            PoolBounds {
                size: self.pool as usize,
                acquire: std::time::Duration::from_millis(self.acquire_ms),
                statement: std::time::Duration::from_millis(self.statement_ms),
                idle_txn: std::time::Duration::from_millis(self.idle_txn_ms),
                connect: std::time::Duration::from_millis(self.connect_ms),
                statements: self.statement_cache as usize,
            },
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PoolBounds {
    pub size: usize,
    pub acquire: std::time::Duration,
    pub statement: std::time::Duration,
    pub idle_txn: std::time::Duration,
    pub connect: std::time::Duration,
    pub statements: usize,
}

/// The database knobs, on every command that can bind a host handler.
#[derive(clap::Args, Clone, Debug, Default)]
pub struct DbOptions {
    /// The database URL; defaults to `PLY_DB_URL`, with the password from `PLY_DB_PASSWORD`.
    #[arg(long = "db", value_name = "URL", requires = "host")]
    pub url: Option<String>,

    /// Connections in the pool.
    #[arg(long = "db-pool", value_name = "N", requires = "host", value_parser = clap::value_parser!(u32).range(1..))]
    pub pool: Option<u32>,

    /// Milliseconds a `db` operation may wait for a connection before `E0437`.
    #[arg(long = "db-acquire-ms", value_name = "MS", requires = "host", value_parser = clap::value_parser!(u64).range(1..))]
    pub acquire_ms: Option<u64>,

    /// Server-side `statement_timeout`, set on every connection at checkout.
    #[arg(long = "db-statement-ms", value_name = "MS", requires = "host", value_parser = clap::value_parser!(u64).range(1..))]
    pub statement_ms: Option<u64>,

    /// Server-side `idle_in_transaction_session_timeout`.
    #[arg(long = "db-idle-txn-ms", value_name = "MS", requires = "host", value_parser = clap::value_parser!(u64).range(1..))]
    pub idle_txn_ms: Option<u64>,

    /// Milliseconds to establish a connection.
    #[arg(long = "db-connect-ms", value_name = "MS", requires = "host", value_parser = clap::value_parser!(u64).range(1..))]
    pub connect_ms: Option<u64>,

    /// Prepared statements kept per connection.
    #[arg(long = "db-statement-cache", value_name = "N", requires = "host", value_parser = clap::value_parser!(u32).range(1..))]
    pub statement_cache: Option<u32>,

    /// `<module>.<fn>`: a nullary pure function returning a `Schema` (not checked live).
    #[arg(long = "db-schema", value_name = "MODULE.FN", requires = "host")]
    pub schema: Option<String>,
}

impl DbOptions {
    /// `None` when the run named no database; [`missing`] makes that `E0431` if the driver binds.
    pub fn resolve(&self, host: bool) -> Result<Option<DbConfig>, Vec<Diagnostic>> {
        self.resolve_with(host, &|key| std::env::var(key).ok())
    }

    /// [`DbOptions::resolve`] against an explicit environment, since `std::env` is process-global.
    pub fn resolve_with(
        &self,
        host: bool,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Option<DbConfig>, Vec<Diagnostic>> {
        if !host {
            return Ok(None);
        }

        let from_environment = env(URL_ENV);
        let (text, source) = match (&self.url, &from_environment) {
            (Some(text), _) => (text.clone(), Source::Flag),
            (None, Some(text)) => (text.clone(), Source::Environment),
            (None, None) => return Ok(None),
        };
        if text.trim().is_empty() {
            return Err(vec![err_empty(source)]);
        }

        let mut url = DbUrl::parse(&text).map_err(|why| vec![err_malformed(source, &why)])?;

        if let Some(password) = env(PASSWORD_ENV) {
            if url.has_password() {
                return Err(vec![err_two_passwords(source)]);
            }
            url.set_password(Secret::new(password));
        }

        if let Some(name) = &self.schema {
            schema::check_shape(name).map_err(|d| vec![d])?;
        }

        Ok(Some(DbConfig {
            url,
            source,
            pool: self.pool.unwrap_or(DEFAULT_POOL),
            acquire_ms: self.acquire_ms.unwrap_or(DEFAULT_ACQUIRE_MS),
            statement_ms: self.statement_ms.unwrap_or(DEFAULT_STATEMENT_MS),
            idle_txn_ms: self.idle_txn_ms.unwrap_or(DEFAULT_IDLE_TXN_MS),
            connect_ms: self.connect_ms.unwrap_or(DEFAULT_CONNECT_MS),
            statement_cache: self.statement_cache.unwrap_or(DEFAULT_STATEMENT_CACHE),
            schema: self.schema.clone(),
        }))
    }
}

/// `E0431`: the program reaches the postgres driver and no database is configured.
pub fn missing(named: &[String]) -> Diagnostic {
    let mut diagnostic = Diagnostic::error(
        codes::DB_NOT_CONFIGURED,
        "this program performs `db` operations and the run named no database",
    )
    .note("pass `--db postgres://user@host:5432/database`, or set `PLY_DB_URL`")
    .note(format!(
        "put the password in `{PASSWORD_ENV}` rather than in the URL: an argument is \
         readable by every process on the machine"
    ));
    if !named.is_empty() {
        diagnostic = diagnostic.note(format!("the run reaches: {}", named.join(", ")));
    }
    diagnostic.note("without `--host` nothing binds and no database is needed")
}

fn err_empty(source: Source) -> Diagnostic {
    Diagnostic::error(
        codes::DB_NOT_CONFIGURED,
        format!("`{}` is set and empty", source.as_str()),
    )
    .note("an empty connection string is not the same as none: unset it to run without a database")
}

fn err_malformed(source: Source, why: &str) -> Diagnostic {
    Diagnostic::error(
        codes::DB_NOT_CONFIGURED,
        format!("`{}` is not a connection string: {why}", source.as_str()),
    )
    .note("the form is `postgres://user@host:5432/database?sslmode=disable`")
    .note(format!(
        "the string itself is not echoed here, because it may carry a password and a \
         diagnostic reaches the result cache; put the password in `{PASSWORD_ENV}`"
    ))
}

fn err_two_passwords(source: Source) -> Diagnostic {
    Diagnostic::error(
        codes::DB_NOT_CONFIGURED,
        format!(
            "`{}` carries a password and `{PASSWORD_ENV}` is also set",
            source.as_str()
        ),
    )
    .note("two answers to one question; picking one silently is how a deploy authenticates as the wrong user")
    .note(format!("remove the `:password` from the connection string, or unset `{PASSWORD_ENV}`"))
}

/// Resolving `--db-schema <module>.<fn>` against the program.
pub mod schema {
    use super::*;

    /// Matched on the name's tail so a project aliasing `std.db` still resolves.
    const SCHEMA_TYPE: &str = "Schema";

    /// The one field the pinned `Schema` record has.
    const TABLES: &str = "tables";

    #[derive(Clone, PartialEq, Eq, Debug)]
    pub struct SchemaView {
        pub name: String,
        /// `None` when not evaluated; printed as absent, never as zero tables.
        pub shape: Option<Shape>,
        pub state: State,
    }

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct Shape {
        pub tables: usize,
        pub columns: usize,
    }

    /// Read structurally, so Rust holds no second copy of `std.db`'s record layout.
    pub fn shape_of(value: &ply_eval::Value) -> Option<Shape> {
        use ply_eval::Value;
        let Value::Record(fields) = value else {
            return None;
        };
        let Some(Value::List(tables)) = fields.get(&Symbol::new("tables")) else {
            return None;
        };
        let mut columns = 0;
        for table in tables.iter() {
            let Value::Record(table) = table else {
                return None;
            };
            let Some(Value::List(cols)) = table.get(&Symbol::new("columns")) else {
                return None;
            };
            columns += cols.len();
        }
        Some(Shape {
            tables: tables.len(),
            columns,
        })
    }

    /// `Declared`: nothing compared the schema to a live server.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum State {
        Declared,
        Verified,
    }

    impl State {
        pub fn as_str(self) -> &'static str {
            match self {
                State::Declared => "declared",
                State::Verified => "verified",
            }
        }
    }

    /// `<module>.<fn>`, before any program is in hand.
    pub fn check_shape(name: &str) -> Result<(), Diagnostic> {
        let segments: Vec<&str> = name.split('.').collect();
        let well_formed = segments.len() >= 2
            && segments.iter().all(|s| {
                !s.is_empty()
                    && s.chars()
                        .next()
                        .is_some_and(|c| c.is_alphabetic() || c == '_')
                    && s.chars().all(|c| c.is_alphanumeric() || c == '_')
            });
        if well_formed {
            return Ok(());
        }
        Err(Diagnostic::error(
            codes::DB_NOT_CONFIGURED,
            format!("`--db-schema {name}` is not a `<module>.<fn>` name"),
        )
        .note("write the program-wide name of the function, as `ply hash` prints it"))
    }

    /// The definition `--db-schema` names, if it can build a schema; refusals list candidates.
    pub fn resolve<'a>(check: &'a CheckOutput, name: &str) -> Result<&'a Symbol, Diagnostic> {
        let Some((symbol, def)) = check.defs.iter().find(|(key, _)| key.as_str() == name) else {
            return Err(unknown(check, name));
        };
        let Type::Fn { params, ret, .. } = &def.scheme.ty else {
            return Err(not_a_schema_fn(
                name,
                "it is not a function, and a schema is materialised by calling one",
            ));
        };
        if !params.is_empty() {
            return Err(not_a_schema_fn(
                name,
                &format!(
                    "it takes {} argument{}, and the run has nothing to pass",
                    params.len(),
                    if params.len() == 1 { "" } else { "s" }
                ),
            ));
        }
        if !returns_schema(ret) {
            return Err(not_a_schema_fn(
                name,
                &format!("it returns `{ret}` rather than a `Schema`"),
            ));
        }
        if !def.footprint.is_empty() {
            return Err(not_a_schema_fn(
                name,
                &format!(
                    "its row is `{}`, and a schema is read before anything is bound, so it \
                     must be pure",
                    def.footprint
                ),
            ));
        }
        Ok(symbol)
    }

    /// Structural, since inference expands the `Schema` alias; must match what [`shape_of`] reads.
    fn returns_schema(ret: &Type) -> bool {
        match ret {
            Type::Con(name, args) if args.is_empty() => name
                .as_str()
                .rsplit('.')
                .next()
                .is_some_and(|tail| tail == SCHEMA_TYPE),
            Type::Record(fields) => {
                fields.len() == 1
                    && matches!(
                        fields.get(&Symbol::new(TABLES)),
                        Some(Type::Con(name, args)) if name.as_str() == "List" && args.len() == 1
                    )
            }
            _ => false,
        }
    }

    fn not_a_schema_fn(name: &str, why: &str) -> Diagnostic {
        Diagnostic::error(
            codes::DB_NOT_CONFIGURED,
            format!("`--db-schema {name}` does not name a schema: {why}"),
        )
        .note("it must be a nullary pure function returning `std.db.Schema` — a record `{tables: List<Table>}`")
    }

    fn unknown(check: &CheckOutput, name: &str) -> Diagnostic {
        let mut candidates: Vec<&str> = check
            .defs
            .iter()
            .filter(|(_, def)| match &def.scheme.ty {
                Type::Fn { params, ret, .. } => {
                    params.is_empty() && returns_schema(ret) && def.footprint.is_empty()
                }
                _ => false,
            })
            .map(|(key, _)| key.as_str())
            .collect();
        candidates.sort_unstable();

        let mut diagnostic = Diagnostic::error(
            codes::DB_NOT_CONFIGURED,
            format!("`--db-schema {name}` names no definition in this program"),
        );
        diagnostic = if candidates.is_empty() {
            diagnostic
                .note("this program declares no nullary function returning a `Schema`")
                .note("drop `--db-schema`: without it a mismatch is `E0433` at prepare time, later and per statement and still loud")
        } else {
            diagnostic.note(format!("this program has: {}", candidates.join(", ")))
        };
        diagnostic
    }
}

/// Facts from the connected server; absent (and printed so) until a connection is made.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ServerFacts {
    pub version: String,
    pub database: String,
    pub collation: String,
    pub encoding: String,
}

/// The `database` block of `ply hosts --host`, and its contribution to the digest.
#[derive(Clone, Debug)]
pub struct Database {
    /// `None` only in a hermetic `ply hosts`; otherwise that run is `E0431`.
    pub config: Option<DbConfig>,
    pub server: Option<ServerFacts>,
    pub schema: Option<schema::SchemaView>,
    pub operations: Vec<String>,
}

impl Database {
    /// `None` unless a postgres handler is reachable or a database was configured.
    pub fn of(
        operations: Vec<String>,
        config: Option<DbConfig>,
        server: Option<ServerFacts>,
        schema: Option<schema::SchemaView>,
    ) -> Option<Database> {
        if operations.is_empty() && config.is_none() {
            return None;
        }
        Some(Database {
            config,
            server,
            schema,
            operations,
        })
    }

    pub fn operations_of(listing: &ply_eval::host::HostListing) -> Vec<String> {
        listing
            .rows
            .iter()
            .filter(|row| row.path.starts_with(HANDLER_PREFIX))
            .map(|row| row.to_string())
            .collect()
    }

    /// `db.rollback` must be a Ply handler inside `transaction`; a bound one would silently commit.
    pub fn rollback_bound(listing: &ply_eval::host::HostListing) -> Option<Diagnostic> {
        let bound: Vec<String> = listing
            .rows
            .iter()
            .filter(|row| row.path.starts_with(HANDLER_PREFIX) && row.op.as_str() == "rollback")
            .map(|row| format!("{row} → {}", row.path))
            .collect();
        if bound.is_empty() {
            return None;
        }
        Some(
            Diagnostic::error(
                codes::INTERNAL_ERROR,
                "`db.rollback` resolved to a host handler",
            )
            .note("a rollback is a Ply handler clause that discards the continuation; a bound one would abort nothing")
            .note(format!("bound: {}", bound.join(", ")))
            .note("this is Ply's fault: report it with the program that produced it"),
        )
    }

    /// Whether the run actually reached a database, so a report is not read as hermetic.
    pub fn is_live(&self) -> bool {
        self.config.is_some() && !self.operations.is_empty()
    }

    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![String::new(), "database".to_string()];
        lines.push(format!("server     {}", self.server_line()));
        lines.push(format!("pool       {}", self.pool_line()));
        lines.push(format!("scanner    {SCANNER} · {ACCEPTED}"));
        lines.push(format!("schema     {}", self.schema_line()));
        lines
    }

    fn server_line(&self) -> String {
        match (&self.server, &self.config) {
            (Some(facts), _) => format!(
                "{} · database {} · collation {} · encoding {}",
                facts.version, facts.database, facts.collation, facts.encoding
            ),
            (None, Some(config)) => format!(
                "{} · not connected · configured by {}",
                config.url.redacted(),
                config.source.as_str()
            ),
            (None, None) => {
                "none — `--db` is unset, so a `db` operation is E0431 under `--host`".to_string()
            }
        }
    }

    fn pool_line(&self) -> String {
        match &self.config {
            Some(config) => format!(
                "{} connection{} · acquire {}ms · statement {}ms · idle-txn {}ms · connect {}ms · statements {}",
                config.pool,
                if config.pool == 1 { "" } else { "s" },
                config.acquire_ms,
                config.statement_ms,
                config.idle_txn_ms,
                config.connect_ms,
                config.statement_cache,
            ),
            None => "none".to_string(),
        }
    }

    fn schema_line(&self) -> String {
        let Some(view) = &self.schema else {
            return "none — without `--db-schema` a mismatch is E0433 at prepare time".to_string();
        };
        match view.shape {
            Some(shape) => format!(
                "{} · {} table{} · {} column{} · {}",
                view.name,
                shape.tables,
                if shape.tables == 1 { "" } else { "s" },
                shape.columns,
                if shape.columns == 1 { "" } else { "s" },
                view.state.as_str(),
            ),
            None => format!("{} · {}", view.name, view.state.as_str()),
        }
    }

    pub fn json(&self) -> serde_json::Value {
        use serde_json::json;
        json!({
            "live": self.is_live(),
            "operations": self.operations,
            "url": self.config.as_ref().map(|c| c.url.redacted()),
            "source": self.config.as_ref().map(|c| c.source.as_str()),
            "pool": self.config.as_ref().map(|c| json!({
                "connections": c.pool,
                "acquire_ms": c.acquire_ms,
                "statement_ms": c.statement_ms,
                "idle_txn_ms": c.idle_txn_ms,
                "connect_ms": c.connect_ms,
                "statement_cache": c.statement_cache,
            })),
            "scanner": json!({
                "handler": SCANNER,
                "accepts": ACCEPTED.split_whitespace().collect::<Vec<_>>(),
            }),
            "server": self.server.as_ref().map(|s| json!({
                "version": s.version,
                "database": s.database,
                "collation": s.collation,
                "encoding": s.encoding,
            })),
            "schema": self.schema.as_ref().map(|s| json!({
                "function": s.name,
                "tables": s.shape.map(|shape| shape.tables),
                "columns": s.shape.map(|shape| shape.columns),
                "state": s.state.as_str(),
            })),
        })
    }

    /// Pool numbers, scanner and schema name only: server facts change without the program.
    pub fn hash_into(&self, hasher: &mut blake3::Hasher) {
        fn write(hasher: &mut blake3::Hasher, text: &str) {
            hasher.update(&(text.len() as u64).to_le_bytes());
            hasher.update(text.as_bytes());
        }
        write(hasher, SCANNER);
        write(hasher, ACCEPTED);
        match &self.config {
            Some(config) => {
                write(hasher, "configured");
                for number in [
                    config.pool as u64,
                    config.acquire_ms,
                    config.statement_ms,
                    config.idle_txn_ms,
                    config.connect_ms,
                    config.statement_cache as u64,
                ] {
                    hasher.update(&number.to_le_bytes());
                }
                write(hasher, config.schema.as_deref().unwrap_or(""));
            }
            None => write(hasher, "unconfigured"),
        }
    }
}
