//! How a run is told which database to talk to, and what it may say about it afterwards.

use ply_core::CheckOutput;
use ply_core::ty::Type;
use ply_span::{Diagnostic, Symbol, codes};
use std::collections::BTreeMap;
use std::fmt;

// --- what the trusted computing base is called ------------------------------

/// The Rust path prefix every postgres handler is registered under, and
/// therefore how a listing row is recognised as one.
pub const HANDLER_PREFIX: &str = "ply_host::db::";

/// The SQL scanner `ply hosts` discloses. It is a parser inside the trusted
/// computing base, which the HTTP design's rule says is the line worth a human's
/// attention.
pub const SCANNER: &str = "ply_host::db::scan";

/// The statement shapes the scanner accounts for. Everything else is `E0432`,
/// so this is the whole of what a program may send.
///
/// Taken from the scanner rather than restated, because it is printed in the
/// listing and hashed into the digest: a second copy that drifted would make
/// `ply hosts` disclose a trusted computing base other than the one linked.
pub const ACCEPTED: &str = ply_host::db::scan::ACCEPTED;

// --- configuration from the environment -------------------------------------

/// The connection string, when `--db` did not carry one.
///
/// Consulted only under `--host`: the environment can say *which* database a
/// bound run uses and can never cause a binding, which is what keeps the host boundary contract's
/// "a reviewer reads `--host` in the command" true.
pub const URL_ENV: &str = "PLY_DB_URL";

/// The password, when the connection string does not carry one.
///
/// The reason this exists is `ps`: an argument is readable by every process on
/// the machine and lands in a shell history, so a `--db` that had to carry the
/// password would be a design that leaks it by default.
pub const PASSWORD_ENV: &str = "PLY_DB_PASSWORD";

/// Where a run's connection string came from, printed so an operator debugging
/// "it connected to the wrong database" is told rather than left to guess.
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

// --- the secret -------------------------------------------------------------

/// A password. Printing one is impossible by construction; reading one is
/// [`Secret::expose`] and nothing else.
///
/// No `Serialize`, no `Display` that reveals, no `Debug` that reveals, and no
/// `PartialEq` against a `&str` — every one of those is a way a value reaches a
/// log line, a `--json` object or a cached failure report by accident.
#[derive(Clone)]
pub struct Secret(String);

/// What every rendering of a password is.
pub const REDACTED: &str = "****";

impl Secret {
    pub fn new(text: impl Into<String>) -> Secret {
        Secret(text.into())
    }

    /// The one call that yields the bytes. Its only legitimate caller is the
    /// code that opens a connection.
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

// --- the connection string --------------------------------------------------

/// `sslmode`, restricted to what W4 configures.
///
/// `require` and above are `E0431` naming the trusted computing base listing: wiring rustls into
/// `tokio-postgres` is a real trusted-computing-base decision that belongs
/// beside W5's secrets, and accepting the word while not encrypting would be a
/// label that lies.
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

/// A parsed `postgres://` connection string, with the password held apart.
///
/// Every field the driver needs is here, so the driver builds its client
/// configuration from these rather than from the text. `Display` and `Debug` are
/// the redacted form, which is what makes it safe to interpolate one into a
/// diagnostic.
#[derive(Clone, PartialEq, Eq)]
pub struct DbUrl {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
    password: Option<Secret>,
    pub sslmode: SslMode,
    /// `application_name`, and nothing else today. Kept as a map because it is
    /// what the driver passes through, and refusing the rest is what keeps a
    /// mistyped parameter from being silently dropped.
    pub parameters: BTreeMap<String, String>,
}

/// Postgres's own default, so a string that omits the port means what libpq
/// would have meant by it.
pub const DEFAULT_PORT: u16 = 5432;

impl DbUrl {
    /// `postgres://user[:password]@host[:port]/database[?sslmode=…]`.
    ///
    /// The error is prose for an operator rather than a [`Diagnostic`], because
    /// the caller knows whether the text came from `--db` or from the
    /// environment and the message has to say which.
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

    /// Attach the password the environment supplied. Refused rather than
    /// overwritten when the string already carries one — see
    /// [`DbOptions::resolve_with`].
    fn set_password(&mut self, secret: Secret) {
        self.password = Some(secret);
    }

    /// The text the driver connects with — the **only** rendering that carries
    /// the password.
    ///
    /// It is rebuilt from the parsed fields rather than passed through, so what
    /// the driver opens is what this module validated and reported rather than
    /// whatever the operator typed. `Secret` is the return type so that the one
    /// dangerous string in the system cannot be printed, logged or serialized by
    /// accident: a caller has to write [`Secret::expose`], which is one word to
    /// grep the whole workspace for.
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

    /// The form that goes into a diagnostic, a listing, a `--json` object and a
    /// cached report: everything but the password, and `****` where that was.
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

/// Redacted, because `{:?}` in a `dbg!`, an `expect` or a derived `Debug` on
/// something holding one of these is the accident this type exists to prevent.
impl fmt::Debug for DbUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DbUrl({})", self.redacted())
    }
}

/// Percent-encoding, over every delimiter a userinfo or a path component could
/// otherwise terminate. The inverse of [`decode`], and the reason
/// [`DbUrl::connection_string`] can rebuild a string the driver reads back as
/// the same fields.
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

/// Percent-decoding, over the two components that need it: a password with an
/// `@` or a `/` in it is ordinary, and one that was silently truncated at the
/// delimiter is an authentication failure nobody can explain.
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

// --- the resolved configuration ---------------------------------------------

/// The pool's defaults, taken from the pool rather than restated. A number the
/// CLI prints and the driver acts on must be one number.
pub const DEFAULT_POOL: u32 = ply_host::db::pool::DEFAULT_POOL_SIZE as u32;
pub const DEFAULT_ACQUIRE_MS: u64 = ply_host::db::pool::DEFAULT_ACQUIRE_MS;
pub const DEFAULT_STATEMENT_MS: u64 = ply_host::db::pool::DEFAULT_STATEMENT_MS;
pub const DEFAULT_IDLE_TXN_MS: u64 = ply_host::db::pool::DEFAULT_IDLE_TXN_MS;
pub const DEFAULT_CONNECT_MS: u64 = ply_host::db::pool::DEFAULT_CONNECT_MS;
/// Statement preparation. Not in the pool's defaults: the statement cache is a property
/// of a connection rather than of the pool.
pub const DEFAULT_STATEMENT_CACHE: u32 = 256;

/// Everything a run was told about its database, validated.
///
/// Handed to the driver whole. The pool numbers are in [`Database::hash_into`]
/// and therefore in the `ply hosts` digest: a service whose pool silently
/// halved is a change to what the trusted computing base does under load, and
/// the digest is the line a CI check pins.
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
    /// `<module>.<fn>`, checked for shape here and resolved against the program
    /// by [`schema::resolve`].
    pub schema: Option<String>,
}

impl DbConfig {
    /// What the driver's pool is built from.
    ///
    /// The connection string is a [`Secret`] rather than a `String` — see
    /// [`DbUrl::connection_string`] — and the pool's numbers are the ones this
    /// run's report and digest carry, so what is printed and what is opened
    /// cannot disagree.
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

/// The pool's bounds as durations, which is the shape the driver wants and the
/// milliseconds are the shape a command line and a report want.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PoolBounds {
    pub size: usize,
    pub acquire: std::time::Duration,
    pub statement: std::time::Duration,
    pub idle_txn: std::time::Duration,
    pub connect: std::time::Duration,
    pub statements: usize,
}

// --- the flags --------------------------------------------------------------

/// The database knobs, on every command that can bind a host handler.
///
/// Every one carries `requires = "host"` for the reason `--tls` does: a flag
/// that would be silently ignored reads as a run that was configured and was
/// not. They are `Option` rather than `default_value_t` so that "the operator
/// chose 8" and "nobody said" stay distinguishable at the point the requirement
/// is checked.
#[derive(clap::Args, Clone, Debug, Default)]
pub struct DbOptions {
    /// The database: `--db postgres://ply@127.0.0.1:5432/desk`. Reads
    /// `PLY_DB_URL` when absent, and `PLY_DB_PASSWORD` for the password, which
    /// keeps the secret out of `ps` and out of a shell history.
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

    /// `<module>.<fn>` — a nullary pure function returning a `Schema`, evaluated
    /// at start-up for its table and column counts. Nothing compares it to the
    /// live database: a mismatch is `E0433` at prepare time, per statement, on
    /// first execution. `ply hosts` prints this `declared`, never `verified`.
    #[arg(long = "db-schema", value_name = "MODULE.FN", requires = "host")]
    pub schema: Option<String>,
}

impl DbOptions {
    /// The configuration this run has, or `None` when it named no database.
    ///
    /// `None` is not yet a failure: a program that performs no `db` operation
    /// binds no postgres handler and needs no database, and refusing here would
    /// make `--host` unusable for the HTTP-only services W3 shipped.
    /// [`missing`] is what turns it into `E0431`, once the binding says the
    /// driver is actually in the run.
    pub fn resolve(&self, host: bool) -> Result<Option<DbConfig>, Vec<Diagnostic>> {
        self.resolve_with(host, &|key| std::env::var(key).ok())
    }

    /// [`resolve`] against an explicit environment.
    ///
    /// Tests read this rather than `std::env`, which is process-global and
    /// therefore a race between two of them under one test binary.
    ///
    /// [`resolve`]: DbOptions::resolve
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

/// `E0431` for a run whose program reached the postgres driver with no database
/// configured.
///
/// `named` is what the run will actually reach — the entry point's own `db`
/// atoms, or the operations that bound when no entry point is known — so the
/// reader is told *why* a database is suddenly required by a command that did
/// not need one yesterday.
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

// --- `--db-schema` ----------------------------------------------------------

/// Resolving `--db-schema <module>.<fn>` against the program.
///
/// A schema is a value (migrations, out of scope): there is no migration tool, no version
/// table and no ordering across deploys. What W4 owns is the check that the
/// database the run is pointed at is the one the program describes, and the
/// first half of that check — that the program describes one at all — is here,
/// where the type checker's output is in hand and a mistake is a start-up
/// refusal rather than a start-up panic.
pub mod schema {
    use super::*;

    /// The type a `--db-schema` function must return, by simple name. Matched on
    /// the tail rather than on the whole program-wide name so that a project
    /// aliasing `std.db` still resolves.
    const SCHEMA_TYPE: &str = "Schema";

    /// The one field the pinned `Schema` record has.
    const TABLES: &str = "tables";

    /// What the run learned about the schema it named.
    #[derive(Clone, PartialEq, Eq, Debug)]
    pub struct SchemaView {
        pub name: String,
        /// `None` when the function was named and resolved but not evaluated —
        /// which is every command that does not need the numbers. An absent
        /// count is printed as absent rather than as zero, because "the schema
        /// declares no tables" is a different and much worse claim.
        pub shape: Option<Shape>,
        pub state: State,
    }

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct Shape {
        pub tables: usize,
        pub columns: usize,
    }

    /// The table and column counts of a materialised `Schema`, or `None` when
    /// the value is not one this reader recognises.
    ///
    /// Structural rather than typed: the type checker already accepted the
    /// return type, and reproducing `std.db`'s record layout in Rust would be a
    /// second definition of it that could drift from the first.
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

    /// How much is actually known about the live database's agreement with it.
    ///
    /// `Declared` is the honest word for "the program describes this and nothing
    /// compared it to a server". Printing `verified` there would be the green
    /// result over unexplored space this project audits for.
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

    /// The definition `--db-schema` names, checked to be one a schema can be
    /// materialised from.
    ///
    /// Every refusal lists the candidates, because the fix is a different
    /// argument rather than an edit to the program and an operator who mistyped
    /// a module prefix should not have to run a second command to find out what
    /// they meant.
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

    /// `Schema` is `{ tables: List<Table> }`, and a record type alias is
    /// **expanded** by inference — so by the time a signature is in hand there is
    /// no name left to match on and the check has to be structural. It is
    /// deliberately the same shape [`shape_of`] reads, so a return type this
    /// accepts is one the counts can be taken from.
    ///
    /// The nominal arm stays for a `Schema` that is an ADT or an opaque
    /// constructor rather than a record, which is what the type would become if
    /// `std.db` ever hid its representation.
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

// --- what `ply hosts` says about the database -------------------------------

/// What the driver learned from the server it connected to.
///
/// Absent until a connection is made, and its absence is printed rather than
/// papered over: "not connected" and "connected to a server whose collation is
/// C" are different facts and a reader deciding whether to trust the twin's
/// `ORDER BY` needs the second one.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ServerFacts {
    pub version: String,
    pub database: String,
    pub collation: String,
    pub encoding: String,
}

/// The `database` block of `ply hosts --host`, and its contribution to the
/// digest.
///
/// It exists for the same reason W3's `transport` block does: a fact the rows
/// cannot carry and a reviewer must not have to derive. The **collation** is
/// printed because what the twin does not model makes it the twin's largest silent divergence,
/// and the **scanner** because it is a parser inside the trusted computing base.
#[derive(Clone, Debug)]
pub struct Database {
    /// The configuration, redacted at every use. `None` is a program that
    /// performs `db` under a binding that never opened one — which is
    /// `E0431`, so a `Database` with no configuration only ever appears in a
    /// hermetic `ply hosts`.
    pub config: Option<DbConfig>,
    pub server: Option<ServerFacts>,
    pub schema: Option<schema::SchemaView>,
    /// The postgres rows of the listing, by triple, so the block can say how
    /// many operations it accounts for without the reader counting them.
    pub operations: Vec<String>,
}

impl Database {
    /// `Some` when this program can reach a postgres handler, or when the run
    /// was configured with a database.
    ///
    /// Absent otherwise, which is what keeps a program with no database in reach
    /// hashing and printing exactly what it did before W4.
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

    /// Every postgres operation the listing resolved, as `ply hosts` prints
    /// them.
    pub fn operations_of(listing: &ply_eval::host::HostListing) -> Vec<String> {
        listing
            .rows
            .iter()
            .filter(|row| row.path.starts_with(HANDLER_PREFIX))
            .map(|row| row.to_string())
            .collect()
    }

    /// `db.rollback` reaching the binding is a defect: transactions as handlers handles it
    /// in Ply, inside `transaction`, and a bound one would mean an abort that
    /// discarded no continuation.
    ///
    /// Checked here rather than trusted, because the failure is silent — the
    /// program would commit what it meant to roll back — and this is the one
    /// place in the CLI that reads the resolved rows.
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

    /// Whether a run actually reached a database, which is the fact a report has
    /// to carry so that "these tests passed" is not read as "these tests passed
    /// hermetically".
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

    /// What the digest covers: the pool numbers, the scanner's accepted
    /// statement set and the schema function's **name**.
    ///
    /// Deliberately **not** the server version, the database name, the host or
    /// the user. A CI check that broke on a minor server upgrade is a CI check
    /// people learn to ignore — W3's argument about a certificate fingerprint,
    /// and the same conclusion. The table and column counts are out for a
    /// sharper reason: they are a property of the *database*, and a digest that
    /// moved when someone else's migration ran would be pinning the wrong thing.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let map: BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key: &str| map.get(key).cloned()
    }

    fn options(url: Option<&str>) -> DbOptions {
        DbOptions {
            url: url.map(str::to_string),
            ..DbOptions::default()
        }
    }

    // --- parsing ------------------------------------------------------------

    #[test]
    fn a_full_url_parses_into_the_fields_the_driver_needs() {
        let url = DbUrl::parse("postgres://ply:hunter2@db.internal:5433/desk?sslmode=disable")
            .expect("a well-formed URL parses");
        assert_eq!(url.user, "ply");
        assert_eq!(url.host, "db.internal");
        assert_eq!(url.port, 5433);
        assert_eq!(url.database, "desk");
        assert_eq!(url.sslmode, SslMode::Disable);
        assert_eq!(url.password().map(Secret::expose), Some("hunter2"));
    }

    #[test]
    fn the_port_and_the_sslmode_default_to_what_postgres_means_by_omitting_them() {
        let url = DbUrl::parse("postgresql://ply@localhost/desk").unwrap();
        assert_eq!(url.port, DEFAULT_PORT);
        assert_eq!(url.sslmode, SslMode::Prefer);
        assert!(!url.has_password());
    }

    /// A password with an `@` or a `/` in it is ordinary, and one truncated at
    /// the delimiter is an authentication failure nobody can explain.
    #[test]
    fn a_percent_escaped_password_survives_the_delimiters_it_contains() {
        let url = DbUrl::parse("postgres://ply:p%40ss%2Fword@localhost:5432/desk").unwrap();
        assert_eq!(url.password().map(Secret::expose), Some("p@ss/word"));
        assert_eq!(url.host, "localhost");
    }

    #[test]
    fn every_malformed_string_is_refused_with_what_to_write() {
        for (text, expected) in [
            ("", "postgres://"),
            ("desk", "postgres://"),
            ("host=localhost dbname=desk", "keyword/value"),
            ("postgres://localhost/desk", "user@"),
            ("postgres://ply@/desk", "no host"),
            ("postgres://ply@localhost", "no database"),
            ("postgres://ply@localhost:abc/desk", "not a port"),
            ("postgres://ply@localhost:0/desk", "port 0"),
        ] {
            let why = DbUrl::parse(text).expect_err("`{text}` must be refused");
            assert!(
                why.contains(expected),
                "`{text}` was refused with `{why}`, which does not mention `{expected}`"
            );
        }
    }

    /// The trusted computing base listing: TLS to postgres is not wired up, so a word that promised
    /// encryption would be a word that lies.
    #[test]
    fn an_sslmode_stronger_than_prefer_is_refused_by_name() {
        for mode in ["require", "verify-ca", "verify-full", "allow"] {
            let why = DbUrl::parse(&format!("postgres://ply@h:5432/d?sslmode={mode}"))
                .expect_err("`{mode}` is not configurable");
            assert!(why.contains("a word that lies"), "{why}");
        }
        assert!(DbUrl::parse("postgres://ply@h:5432/d?sslmode=prefer").is_ok());
        assert!(DbUrl::parse("postgres://ply@h:5432/d?sslmode=disable").is_ok());
    }

    /// A mistyped parameter that was silently dropped is a timeout nobody set.
    #[test]
    fn an_unknown_parameter_is_refused_rather_than_ignored() {
        let why = DbUrl::parse("postgres://ply@h/d?connect_timeout=3").unwrap_err();
        assert!(why.contains("--db-connect-ms"), "{why}");
        assert!(DbUrl::parse("postgres://ply@h/d?application_name=desk").is_ok());
    }

    // --- redaction ----------------------------------------------------------

    /// The whole reason this module has its own types. A password reaching a
    /// diagnostic reaches the result cache, and the store is designed never to
    /// forget.
    #[test]
    fn no_rendering_of_a_url_or_a_secret_contains_the_password() {
        let url = DbUrl::parse("postgres://ply:hunter2@localhost:5432/desk").unwrap();
        let renderings = [
            url.to_string(),
            format!("{url:?}"),
            url.redacted(),
            url.password().unwrap().to_string(),
            format!("{:?}", url.password().unwrap()),
            format!(
                "{:?}",
                DbConfig {
                    url: url.clone(),
                    source: Source::Flag,
                    pool: 8,
                    acquire_ms: 1,
                    statement_ms: 1,
                    idle_txn_ms: 1,
                    connect_ms: 1,
                    statement_cache: 1,
                    schema: None,
                }
            ),
        ];
        for rendering in &renderings {
            assert!(
                !rendering.contains("hunter2"),
                "`{rendering}` carries the password"
            );
            assert!(rendering.contains(REDACTED), "`{rendering}` hid nothing");
        }
        assert_eq!(
            url.redacted(),
            "postgres://ply:****@localhost:5432/desk?sslmode=prefer"
        );
    }

    /// What the driver opens has to be what this module validated and reported,
    /// so the rebuilt string parses back to the same fields — including a
    /// password whose bytes are delimiters.
    #[test]
    fn the_string_the_driver_connects_with_round_trips_through_the_fields() {
        let url =
            DbUrl::parse("postgres://ply:p%40ss%2Fw%3Ard@db.internal:5433/desk?sslmode=disable")
                .unwrap();
        let rebuilt =
            DbUrl::parse(url.connection_string().expose()).expect("the rebuilt string parses");
        assert_eq!(rebuilt, url);
        assert_eq!(rebuilt.password().map(Secret::expose), Some("p@ss/w:rd"));
    }

    /// `connection_string` is the one rendering that carries the password, and
    /// it returns a `Secret` so a caller has to say `expose` to get at it. That
    /// word is what a reviewer greps the workspace for.
    #[test]
    fn the_only_rendering_that_carries_the_password_is_a_secret() {
        let url = DbUrl::parse("postgres://ply:hunter2@localhost:5432/desk").unwrap();
        let carrier = url.connection_string();
        assert!(carrier.expose().contains("hunter2"));
        assert_eq!(carrier.to_string(), REDACTED);
        assert!(!format!("{carrier:?}").contains("hunter2"));
    }

    /// Nothing about a diagnostic may echo the string it was handed, because the
    /// caller cannot know whether the operator put a password in it.
    #[test]
    fn a_malformed_url_diagnostic_never_echoes_what_it_was_given() {
        let options = options(Some("postgres://ply:hunter2@localhost:5432"));
        let diagnostics = options
            .resolve_with(true, &env_of(&[]))
            .expect_err("a URL with no database is refused");
        let rendered = format!("{:?}", diagnostics);
        assert!(!rendered.contains("hunter2"), "{rendered}");
        assert_eq!(diagnostics[0].code, codes::DB_NOT_CONFIGURED);
    }

    // --- resolution ---------------------------------------------------------

    /// The host boundary contract's rule is untouched: the environment says *which* database, and
    /// only `--host` decides that there is one.
    #[test]
    fn the_environment_supplies_the_url_and_never_the_binding() {
        let env = env_of(&[(URL_ENV, "postgres://ply@localhost:5432/desk")]);
        assert!(
            options(None).resolve_with(false, &env).unwrap().is_none(),
            "a hermetic run reads no database whatever the environment holds"
        );
        let config = options(None)
            .resolve_with(true, &env)
            .unwrap()
            .expect("under --host the environment is read");
        assert_eq!(config.source, Source::Environment);
        assert_eq!(config.url.database, "desk");
    }

    #[test]
    fn the_flag_wins_over_the_environment_and_the_report_says_which() {
        let env = env_of(&[(URL_ENV, "postgres://ply@localhost:5432/from_env")]);
        let config = options(Some("postgres://ply@localhost:5432/from_flag"))
            .resolve_with(true, &env)
            .unwrap()
            .unwrap();
        assert_eq!(config.url.database, "from_flag");
        assert_eq!(config.source, Source::Flag);
        assert_eq!(config.source.as_str(), "--db");
    }

    #[test]
    fn the_password_comes_out_of_the_environment_and_not_out_of_ps() {
        let env = env_of(&[(PASSWORD_ENV, "hunter2")]);
        let config = options(Some("postgres://ply@localhost:5432/desk"))
            .resolve_with(true, &env)
            .unwrap()
            .unwrap();
        assert_eq!(config.url.password().map(Secret::expose), Some("hunter2"));
        assert!(!config.url.redacted().contains("hunter2"));
    }

    /// Two answers to one question. Picking one silently is how a deploy
    /// authenticates as the wrong user.
    #[test]
    fn a_password_in_both_places_is_refused_rather_than_resolved() {
        let env = env_of(&[(PASSWORD_ENV, "fromenv")]);
        let diagnostics = options(Some("postgres://ply:fromurl@localhost:5432/desk"))
            .resolve_with(true, &env)
            .unwrap_err();
        assert_eq!(diagnostics[0].code, codes::DB_NOT_CONFIGURED);
        let rendered = format!("{diagnostics:?}");
        assert!(
            !rendered.contains("fromenv") && !rendered.contains("fromurl"),
            "{rendered}"
        );
    }

    #[test]
    fn an_empty_environment_value_is_a_refusal_rather_than_an_absence() {
        let diagnostics = options(None)
            .resolve_with(true, &env_of(&[(URL_ENV, "  ")]))
            .unwrap_err();
        assert_eq!(diagnostics[0].code, codes::DB_NOT_CONFIGURED);
        assert!(diagnostics[0].message.contains(URL_ENV));
    }

    /// Naming no database is not yet a failure: an HTTP-only service under
    /// `--host` binds no postgres handler and needs none.
    #[test]
    fn naming_no_database_is_not_an_error_until_the_driver_binds() {
        assert!(
            options(None)
                .resolve_with(true, &env_of(&[]))
                .unwrap()
                .is_none()
        );
        let diagnostic = missing(&["db.query[items]".to_string()]);
        assert_eq!(diagnostic.code, codes::DB_NOT_CONFIGURED);
        assert!(
            diagnostic
                .notes
                .iter()
                .any(|n| n.contains("db.query[items]"))
        );
        assert!(diagnostic.notes.iter().any(|n| n.contains(PASSWORD_ENV)));
    }

    #[test]
    fn the_pool_defaults_are_the_contract_s_and_are_reported_as_numbers_somebody_chose() {
        let config = options(Some("postgres://ply@localhost/desk"))
            .resolve_with(true, &env_of(&[]))
            .unwrap()
            .unwrap();
        assert_eq!(config.pool, DEFAULT_POOL);
        assert_eq!(config.acquire_ms, DEFAULT_ACQUIRE_MS);
        assert_eq!(config.statement_ms, DEFAULT_STATEMENT_MS);
        assert_eq!(config.idle_txn_ms, DEFAULT_IDLE_TXN_MS);
        assert_eq!(config.connect_ms, DEFAULT_CONNECT_MS);
        assert_eq!(config.statement_cache, DEFAULT_STATEMENT_CACHE);
    }

    #[test]
    fn a_schema_name_that_is_not_module_dot_fn_is_refused_at_resolution() {
        let mut options = options(Some("postgres://ply@localhost/desk"));
        options.schema = Some("schema".to_string());
        let diagnostics = options.resolve_with(true, &env_of(&[])).unwrap_err();
        assert_eq!(diagnostics[0].code, codes::DB_NOT_CONFIGURED);
        options.schema = Some("desk.schema".to_string());
        assert!(options.resolve_with(true, &env_of(&[])).is_ok());
    }

    // --- the `database` block -----------------------------------------------

    fn config() -> DbConfig {
        options(Some(
            "postgres://ply:secret@127.0.0.1:5433/desk?sslmode=disable",
        ))
        .resolve_with(true, &env_of(&[]))
        .unwrap()
        .unwrap()
    }

    #[test]
    fn a_program_with_no_database_in_reach_reports_no_block_at_all() {
        assert!(Database::of(Vec::new(), None, None, None).is_none());
    }

    #[test]
    fn the_block_names_the_scanner_the_pool_and_the_collation() {
        let database = Database::of(
            vec!["db.query[items]".to_string()],
            Some(config()),
            Some(ServerFacts {
                version: "PostgreSQL 18.3".to_string(),
                database: "desk".to_string(),
                collation: "C".to_string(),
                encoding: "UTF8".to_string(),
            }),
            Some(schema::SchemaView {
                name: "desk.schema".to_string(),
                shape: Some(schema::Shape {
                    tables: 2,
                    columns: 11,
                }),
                state: schema::State::Verified,
            }),
        )
        .unwrap();
        assert_eq!(
            database.lines(),
            [
                "",
                "database",
                "server     PostgreSQL 18.3 · database desk · collation C · encoding UTF8",
                "pool       8 connections · acquire 5000ms · statement 30000ms · idle-txn 30000ms · connect 5000ms · statements 256",
                "scanner    ply_host::db::scan · select insert update delete values with",
                "schema     desk.schema · 2 tables · 11 columns · verified",
            ]
        );
        assert!(database.is_live());
    }

    /// "connected" and "not connected" are different facts, and a reader
    /// deciding whether to trust the twin's `ORDER BY` needs the second one said
    /// out loud rather than implied by an absent line.
    #[test]
    fn an_unconnected_run_says_so_and_still_redacts() {
        let database = Database::of(
            vec!["db.query[items]".to_string()],
            Some(config()),
            None,
            None,
        )
        .unwrap();
        let text = database.lines().join("\n");
        assert!(text.contains("not connected"), "{text}");
        assert!(text.contains("configured by --db"), "{text}");
        assert!(!text.contains("secret"), "{text}");
        assert!(text.contains("E0433"), "{text}");
        assert_eq!(
            database.json()["url"],
            "postgres://ply:****@127.0.0.1:5433/desk?sslmode=disable"
        );
        assert_eq!(database.json()["server"], serde_json::Value::Null);
    }

    /// The digest is the line a CI check pins, so a halved pool must move it and
    /// a server upgrade must not.
    #[test]
    fn the_digest_covers_the_pool_and_the_schema_name_and_not_the_server() {
        let digest = |database: &Database| {
            let mut hasher = blake3::Hasher::new();
            database.hash_into(&mut hasher);
            hasher.finalize().to_hex().to_string()
        };
        let base =
            Database::of(vec!["db.query[items]".into()], Some(config()), None, None).unwrap();

        let mut halved = base.clone();
        halved.config.as_mut().unwrap().pool = 4;
        assert_ne!(digest(&base), digest(&halved), "a halved pool is a change");

        let mut upgraded = base.clone();
        upgraded.server = Some(ServerFacts {
            version: "PostgreSQL 19.0".to_string(),
            database: "other".to_string(),
            collation: "C".to_string(),
            encoding: "UTF8".to_string(),
        });
        assert_eq!(
            digest(&base),
            digest(&upgraded),
            "a CI check that broke on a minor server upgrade is one people learn to ignore"
        );

        let mut named = base.clone();
        named.config.as_mut().unwrap().schema = Some("desk.schema".to_string());
        assert_ne!(digest(&base), digest(&named));

        let mut migrated = named.clone();
        migrated.schema = Some(schema::SchemaView {
            name: "desk.schema".to_string(),
            shape: Some(schema::Shape {
                tables: 3,
                columns: 20,
            }),
            state: schema::State::Verified,
        });
        assert_eq!(
            digest(&named),
            digest(&migrated),
            "the table count is a property of the database, not of this binary"
        );
    }
}
