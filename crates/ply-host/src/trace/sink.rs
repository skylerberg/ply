//! Where a record goes, and the three answers W5 ships.

use super::Level;
use ply_eval::Decimal;
use std::fmt::Write as _;
use std::io::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// What a run's wall clock is read through.
pub trait Clock: Send + Sync {
    /// Epoch microseconds.
    fn micros(&self) -> i64;
}

/// The host clock, and the only thing in this crate that reads it.
#[derive(Default)]
pub struct HostClock;

impl Clock for HostClock {
    fn micros(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_micros() as i64)
            // Before the epoch is a machine whose clock is set wrongly, which is a fact about the
            // deployment rather than a reason to refuse a log line.
            .unwrap_or(0)
    }
}

/// One structured value a program attached to a record.
#[derive(Clone, PartialEq, Debug)]
pub enum Field {
    Int(i64),
    Bool(bool),
    Text(String),
    Float(f64),
    Decimal(Decimal),
    Bytes(Vec<u8>),
    Json(String),
}

/// Which of the six operations produced a record.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Event,
    Enter,
    Exit,
    Count,
    Gauge,
    Time,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Event => "event",
            Kind::Enter => "enter",
            Kind::Exit => "exit",
            Kind::Count => "count",
            Kind::Gauge => "gauge",
            Kind::Time => "time",
        }
    }
}

/// How a span ended.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Outcome {
    Ok,
    Failed(String),
    /// Nothing ran the program's `exit`: a `db.rollback` discarded the continuation, a raise
    /// propagated past, or the entry point ended with the span open.
    Abandoned,
}

impl Outcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::Failed(_) => "failed",
            Outcome::Abandoned => "abandoned",
        }
    }
}

/// One line, before anything has rendered it.
pub struct Record<'a> {
    /// Epoch microseconds, stamped by the driver from the [`Clock`] — never by the call site, which
    /// is what keeps `clock.read` out of every tracing function's row.
    pub ts: i64,
    pub kind: Kind,
    pub level: Level,
    pub channel: &'a str,
    pub name: &'a str,
    /// The span this record belongs to, `0` outside any.
    pub span: i64,
    /// That span's parent, `0` at depth zero.
    pub parent: i64,
    pub outcome: &'a Outcome,
    /// `count`'s delta, `time`'s micros, and a closing span's own duration when the sink wanted its
    /// `enter` and therefore has a start stamp.
    pub amount: Option<i64>,
    /// `gauge`'s value.
    pub value: Option<Decimal>,
    pub fields: &'a [(String, Field)],
}

/// Where records go.
pub trait Sink: Send + Sync {
    /// The Rust path `ply hosts` prints — the reviewable identity of a member of the trusted
    /// computing base.
    fn path(&self) -> &'static str;

    /// Where the records go, in the words `ply hosts` prints.
    fn destination(&self) -> &'static str;

    /// Whether a record at this level will be written.
    fn wants(&self, level: Level) -> bool;

    fn write(&self, record: &Record<'_>);

    /// Called once at teardown, before the connection pool closes, so that a trace naming a
    /// rolled-back transaction is written before the connection that rolled it back is gone.
    fn flush(&self);
}

/// What [`Discard::path`] answers, for the one caller that has to tell the discarding sink from a
/// writing one without downcasting.
pub const DISCARD_PATH: &str = "ply_host::trace::discard";

/// `--trace off`.
#[derive(Default)]
pub struct Discard;

impl Sink for Discard {
    fn path(&self) -> &'static str {
        DISCARD_PATH
    }

    fn destination(&self) -> &'static str {
        "nothing"
    }

    fn wants(&self, _: Level) -> bool {
        false
    }

    fn write(&self, _: &Record<'_>) {}

    fn flush(&self) {}
}

/// `--trace json`: one JSON object per line, on stderr.
pub struct Json {
    level: Level,
    /// One `write_all` per line under one lock, so two tasks cannot interleave one line.
    out: Mutex<()>,
}

impl Json {
    pub fn new(level: Level) -> Json {
        Json {
            level,
            out: Mutex::new(()),
        }
    }
}

impl Sink for Json {
    fn path(&self) -> &'static str {
        "ply_host::trace::json"
    }

    fn destination(&self) -> &'static str {
        "stderr"
    }

    fn wants(&self, level: Level) -> bool {
        level >= self.level
    }

    fn write(&self, record: &Record<'_>) {
        let mut line = String::with_capacity(160);
        write_json(&mut line, record);
        line.push('\n');
        let _guard = lock(&self.out);
        // A log line that cannot be written is not a reason to end a run, and there is nowhere to
        // report it to that is not the thing that failed.
        let _ = std::io::stderr().write_all(line.as_bytes());
    }

    fn flush(&self) {
        let _guard = lock(&self.out);
        let _ = std::io::stderr().flush();
    }
}

/// `--trace text`: one human line per record, on stderr, stamped relative to the run's start.
pub struct Text {
    level: Level,
    started: Instant,
    out: Mutex<()>,
}

impl Text {
    pub fn new(level: Level) -> Text {
        Text {
            level,
            started: Instant::now(),
            out: Mutex::new(()),
        }
    }
}

impl Sink for Text {
    fn path(&self) -> &'static str {
        "ply_host::trace::text"
    }

    fn destination(&self) -> &'static str {
        "stderr"
    }

    fn wants(&self, level: Level) -> bool {
        level >= self.level
    }

    fn write(&self, record: &Record<'_>) {
        let mut line = String::with_capacity(120);
        let elapsed = self.started.elapsed().as_micros() as i64;
        let _ = write!(
            line,
            "+{}.{:03}ms {:<5} {} {} {}",
            elapsed / 1000,
            elapsed % 1000,
            record.level.as_str(),
            record.channel,
            record.kind.as_str(),
            record.name
        );
        if record.span != 0 {
            let _ = write!(line, " span={}", record.span);
        }
        if record.parent != 0 {
            let _ = write!(line, " parent={}", record.parent);
        }
        if record.kind == Kind::Exit {
            let _ = write!(line, " {}", record.outcome.as_str());
            if let Outcome::Failed(why) = record.outcome {
                let _ = write!(line, "({why})");
            }
        }
        if let Some(amount) = record.amount {
            let _ = write!(line, " {amount}");
        }
        if let Some(value) = record.value {
            let _ = write!(line, " {value}");
        }
        for (key, field) in record.fields {
            let _ = write!(line, " {key}=");
            write_text_field(&mut line, field);
        }
        line.push('\n');
        let _guard = lock(&self.out);
        let _ = std::io::stderr().write_all(line.as_bytes());
    }

    fn flush(&self) {
        let _guard = lock(&self.out);
        let _ = std::io::stderr().flush();
    }
}

/// One record a [`Recording`] kept, owned so that nothing a `Value` holds crosses a thread.
#[derive(Clone, PartialEq, Debug)]
pub struct Kept {
    pub ts: i64,
    pub kind: Kind,
    pub level: Level,
    pub channel: String,
    pub name: String,
    pub span: i64,
    pub parent: i64,
    pub outcome: Outcome,
    pub amount: Option<i64>,
    pub value: Option<Decimal>,
    pub fields: Vec<(String, Field)>,
}

/// A sink that keeps what it was given.
pub struct Recording {
    level: Level,
    kept: Mutex<Vec<Kept>>,
    flushes: AtomicU64,
}

impl Default for Recording {
    fn default() -> Recording {
        Recording::new(Level::Debug)
    }
}

impl Recording {
    pub fn new(level: Level) -> Recording {
        Recording {
            level,
            kept: Mutex::new(Vec::new()),
            flushes: AtomicU64::new(0),
        }
    }

    pub fn records(&self) -> Vec<Kept> {
        lock(&self.kept).clone()
    }

    pub fn flushes(&self) -> u64 {
        self.flushes.load(Ordering::Relaxed)
    }

    /// The same lines [`Json`] would have written, for a test that asserts on the wire format
    /// without capturing a file descriptor.
    pub fn lines(&self) -> Vec<String> {
        lock(&self.kept)
            .iter()
            .map(|kept| {
                let mut line = String::new();
                write_json(
                    &mut line,
                    &Record {
                        ts: kept.ts,
                        kind: kept.kind,
                        level: kept.level,
                        channel: &kept.channel,
                        name: &kept.name,
                        span: kept.span,
                        parent: kept.parent,
                        outcome: &kept.outcome,
                        amount: kept.amount,
                        value: kept.value,
                        fields: &kept.fields,
                    },
                );
                line
            })
            .collect()
    }
}

impl Sink for Recording {
    fn path(&self) -> &'static str {
        "ply_host::trace::recording"
    }

    fn destination(&self) -> &'static str {
        "a value"
    }

    fn wants(&self, level: Level) -> bool {
        level >= self.level
    }

    fn write(&self, record: &Record<'_>) {
        lock(&self.kept).push(Kept {
            ts: record.ts,
            kind: record.kind,
            level: record.level,
            channel: record.channel.to_string(),
            name: record.name.to_string(),
            span: record.span,
            parent: record.parent,
            outcome: record.outcome.clone(),
            amount: record.amount,
            value: record.value,
            fields: record.fields.to_vec(),
        });
    }

    fn flush(&self) {
        self.flushes.fetch_add(1, Ordering::Relaxed);
    }
}

/// A poisoned lock here holds a `Vec` of records, which has no invariant a panicking thread can
/// break, and a log that stopped working because a *different* thread failed would be the worst
/// time to lose one.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// The envelope, with the program's fields nested under `fields` **always**, even when empty.
pub fn write_json(out: &mut String, record: &Record<'_>) {
    let _ = write!(out, "{{\"ts\":{}", record.ts);
    let _ = write!(out, ",\"level\":\"{}\"", record.level.as_str());
    out.push_str(",\"channel\":");
    write_string(out, record.channel);
    let _ = write!(out, ",\"kind\":\"{}\"", record.kind.as_str());
    out.push_str(",\"name\":");
    write_string(out, record.name);
    let _ = write!(
        out,
        ",\"span\":{},\"parent\":{}",
        record.span, record.parent
    );
    if record.kind == Kind::Exit {
        let _ = write!(out, ",\"outcome\":\"{}\"", record.outcome.as_str());
        if let Outcome::Failed(why) = record.outcome {
            out.push_str(",\"reason\":");
            write_string(out, why);
        }
    }
    if let Some(amount) = record.amount {
        let key = match record.kind {
            Kind::Count => "delta",
            _ => "micros",
        };
        let _ = write!(out, ",\"{key}\":{amount}");
    }
    if let Some(value) = record.value {
        // A string, because a `Decimal`'s scale is a digit count the value carries and a JSON
        // number consumer would round it away.
        out.push_str(",\"value\":");
        write_string(out, &value.to_string());
    }
    out.push_str(",\"fields\":{");
    for (i, (key, field)) in record.fields.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_string(out, key);
        out.push(':');
        write_json_field(out, field);
    }
    out.push_str("}}");
}

fn write_json_field(out: &mut String, field: &Field) {
    match field {
        Field::Int(n) => {
            let _ = write!(out, "{n}");
        }
        Field::Bool(b) => {
            let _ = write!(out, "{b}");
        }
        Field::Text(s) => write_string(out, s),
        // JSON has no `NaN` and no infinity, and a writer that emitted one would produce a document
        // no parser accepts.
        Field::Float(f) if f.is_finite() => {
            let _ = write!(out, "{}", ply_syntax::ast::render_float(*f));
        }
        Field::Float(f) => write_string(out, &ply_syntax::ast::render_float(*f)),
        Field::Decimal(d) => write_string(out, &d.to_string()),
        // Lowercase hex.
        Field::Bytes(bytes) => {
            out.push('"');
            for byte in bytes {
                let _ = write!(out, "{byte:02x}");
            }
            out.push('"');
        }
        Field::Json(already) => out.push_str(already),
    }
}

fn write_text_field(out: &mut String, field: &Field) {
    match field {
        Field::Text(s) => out.push_str(s),
        other => write_json_field(out, other),
    }
}

/// A JSON string, with the escapes RFC 8259 requires and no others.
pub fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}
