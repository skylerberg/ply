//! The `trace` effect and the sinks that serve it.

pub mod sink;
pub mod spans;
mod value;

pub use sink::{
    Clock, DISCARD_PATH, Discard, Field, HostClock, Json, Kept, Kind, Outcome, Record, Sink, Text,
};
pub use spans::{Owner, Spans};

use ply_core::ty::Resource;
use ply_eval::host::MachineId;
use ply_eval::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRegistry, HostRequest, HostResource,
    HostRuntime, Linearity,
};
use ply_span::{Diagnostic, Span, Symbol, codes};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// The Ply declaration the registrations below are checked against: the source of the module
/// `std.trace`, which ships with the compiler.
pub const DECLARATION: &str = ply_std::TRACE;

/// The module the declaration ships as, which is what qualifies [`EFFECT`].
pub const MODULE: &str = "std.trace";

/// The program-wide effect name.
pub const EFFECT: &str = "std.trace.trace";

/// How much of a record a sink admits.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }
}

/// The six operations `std.trace` declares.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    Event,
    Enter,
    Exit,
    Count,
    Gauge,
    Time,
}

impl Op {
    pub const ALL: [Op; 6] = [
        Op::Event,
        Op::Enter,
        Op::Exit,
        Op::Count,
        Op::Gauge,
        Op::Time,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Op::Event => "event",
            Op::Enter => "enter",
            Op::Exit => "exit",
            Op::Count => "count",
            Op::Gauge => "gauge",
            Op::Time => "time",
        }
    }

    /// How a diagnostic names it.
    pub fn what(self) -> &'static str {
        match self {
            Op::Event => "`trace.event`",
            Op::Enter => "`trace.enter`",
            Op::Exit => "`trace.exit`",
            Op::Count => "`trace.count`",
            Op::Gauge => "`trace.gauge`",
            Op::Time => "`trace.time`",
        }
    }

    pub fn arity(self) -> usize {
        match self {
            Op::Enter | Op::Exit => 2,
            Op::Event | Op::Count | Op::Gauge | Op::Time => 3,
        }
    }

    fn kind(self) -> Kind {
        match self {
            Op::Event => Kind::Event,
            Op::Enter => Kind::Enter,
            Op::Exit => Kind::Exit,
            Op::Count => Kind::Count,
            Op::Gauge => Kind::Gauge,
            Op::Time => Kind::Time,
        }
    }

    /// The registration.
    fn declaration(self, path: &'static str) -> HostOp {
        HostOp {
            effect: Symbol::new(EFFECT),
            op: Symbol::new(self.name()),
            // Whichever channels the program uses.
            resource: HostResource::Any,
            determinism: Determinism::Nondeterministic,
            linearity: Linearity::AtMostOnce,
            blocking: false,
            // A `Field` has no constructor over a `Secret`, so nothing of that type can reach a
            // record.
            secrets: false,
            path,
        }
    }
}

/// What the run's own summary says about tracing.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Counts {
    pub events: u64,
    pub spans: u64,
    pub abandoned: u64,
    pub flushed: bool,
}

/// The sink, the clock, and every span every entry point has open.
pub struct Trace {
    sink: Arc<dyn Sink>,
    clock: Arc<dyn Clock>,
    spans: Mutex<Spans>,
    events: AtomicU64,
    flushed: AtomicU64,
}

impl Default for Trace {
    fn default() -> Trace {
        Trace::new(Arc::new(Discard))
    }
}

impl Trace {
    pub fn new(sink: Arc<dyn Sink>) -> Trace {
        Trace::with_clock(sink, Arc::new(HostClock))
    }

    /// The same, over a clock a test supplies — which is how the counting harness asserts that a
    /// discarded event reads the clock zero times, and how a golden test over a JSON line gets a
    /// stamp that does not move.
    pub fn with_clock(sink: Arc<dyn Sink>, clock: Arc<dyn Clock>) -> Trace {
        Trace {
            sink,
            clock,
            spans: Mutex::new(Spans::new()),
            events: AtomicU64::new(0),
            flushed: AtomicU64::new(0),
        }
    }

    /// The sink's reviewable identity and where it writes, for the `observability` block of `ply
    /// hosts` and for the digest that block is in.
    pub fn sink_path(&self) -> &'static str {
        self.sink.path()
    }

    pub fn sink_destination(&self) -> &'static str {
        self.sink.destination()
    }

    /// What the shutdown banner prints.
    pub fn counts(&self) -> Counts {
        let spans = lock(&self.spans);
        Counts {
            events: self.events.load(Ordering::Relaxed),
            spans: spans.opened(),
            abandoned: spans.abandoned(),
            flushed: self.flushed.load(Ordering::Relaxed) > 0,
        }
    }

    /// How many spans are open across every entry point.
    pub fn open_spans(&self) -> usize {
        lock(&self.spans).total_open()
    }

    /// Closes every span this machine still has open, `Abandoned`, innermost first per task.
    pub fn end_entry_point(&self, machine: MachineId) -> Option<Diagnostic> {
        let closings = lock(&self.spans).end_entry_point(machine);
        if closings.is_empty() {
            return None;
        }
        for closing in &closings {
            self.emit_close(closing);
        }
        Some(spans::warn_abandoned(&closings))
    }

    /// Flushes the sink.
    pub fn flush(&self) {
        self.sink.flush();
        self.flushed.fetch_add(1, Ordering::Relaxed);
    }

    fn emit_close(&self, closing: &spans::Closing) {
        if !self.sink.wants(Level::Info) {
            return;
        }
        let ts = self.clock.micros();
        self.events.fetch_add(1, Ordering::Relaxed);
        self.sink.write(&Record {
            ts,
            kind: Kind::Exit,
            level: Level::Info,
            channel: spans::label(&closing.open.channel),
            name: &closing.open.name,
            span: closing.open.id,
            parent: closing.open.parent,
            outcome: &closing.outcome,
            amount: closing.open.started.map(|started| ts - started),
            value: None,
            fields: &[],
        });
    }
}

/// Register every operation of `trace` against a sink.
pub fn register(registry: &mut HostRegistry, trace: Arc<Trace>) {
    let path = trace.sink.path();
    for op in Op::ALL {
        registry.register(op.declaration(path), handler(op, Arc::clone(&trace)));
    }
}

/// One operation's handler, outside a registry.
pub fn handler(op: Op, trace: Arc<Trace>) -> Arc<dyn HostHandler> {
    Arc::new(Operation { op, trace })
}

/// A registry serving `trace` and nothing else.
pub fn registry(trace: Arc<Trace>) -> HostRegistry {
    let mut registry = HostRegistry::new();
    register(&mut registry, trace);
    registry
}

struct Operation {
    op: Op,
    trace: Arc<Trace>,
}

impl HostHandler for Operation {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        if req.args.len() != self.op.arity() {
            return Err(arity(self.op, req.args.len(), span));
        }
        // The resolved atom's resource, never one the handler re-derives: the registry already
        // decided which channel this perform named.
        let channel = &req.atom.resource;
        let owner: Owner = (req.machine, req.task);
        match self.op {
            // Two operations keep state whatever the sink does, because `E0445` is a statement
            // about the program and a program whose verdict moved with `--trace off` would be a
            // program nobody could debug.
            Op::Enter => self.enter(req, channel, owner),
            Op::Exit => self.exit(req, channel, owner, span),
            // The other four have nothing to keep, so a sink that wants nothing is answered before
            // a name is decoded, a field is built or a clock is read.
            Op::Event => {
                let level = value::level(&req.args[0], span)?;
                self.simple(req, channel, owner, level, 1)
            }
            Op::Count | Op::Gauge | Op::Time => self.simple(req, channel, owner, Level::Info, 0),
        }
    }
}

impl Operation {
    /// `event`, `count`, `gauge` and `time`: one record, no state.
    fn simple(
        &self,
        req: &HostRequest<'_>,
        channel: &Resource,
        owner: Owner,
        level: Level,
        name_at: usize,
    ) -> Result<HostAnswer, Diagnostic> {
        if !self.trace.sink.wants(level) {
            return Ok(HostAnswer::Value(ply_eval::Value::Unit));
        }
        let span = req.span;
        let name = req.args[name_at].as_str(span, "a record's name")?;
        let (amount, value) = match self.op {
            Op::Count | Op::Time => (Some(req.args[1].as_int(span, "an amount")?), None),
            Op::Gauge => (None, Some(req.args[1].as_decimal(span, "a gauge's value")?)),
            _ => (None, None),
        };
        let fields = value::fields(&req.args[2], span)?;
        let (in_span, parent) = self.enclosing(owner);
        let ts = self.trace.clock.micros();
        self.trace.events.fetch_add(1, Ordering::Relaxed);
        self.trace.sink.write(&Record {
            ts,
            kind: self.op.kind(),
            level,
            channel: spans::label(channel),
            name,
            span: in_span,
            parent,
            outcome: &Outcome::Ok,
            amount,
            value,
            fields: &fields,
        });
        Ok(HostAnswer::Value(ply_eval::Value::Unit))
    }

    fn enter(
        &self,
        req: &HostRequest<'_>,
        channel: &Resource,
        owner: Owner,
    ) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        let wanted = self.trace.sink.wants(Level::Info);
        // The `Arc<str>` the argument already holds, so keeping the name costs no allocation even
        // when nothing is collecting — which is what lets `W0609` name the innermost span under
        // `--trace off`.
        let name = match &req.args[0] {
            ply_eval::Value::Str(s) => Arc::clone(s),
            other => return Err(value_error(span, "a span's name", other)),
        };
        let started = wanted.then(|| self.trace.clock.micros());
        let opened = lock(&self.trace.spans).enter(owner, channel.clone(), name, started);
        if wanted {
            let fields = value::fields(&req.args[1], span)?;
            self.trace.events.fetch_add(1, Ordering::Relaxed);
            self.trace.sink.write(&Record {
                ts: started.unwrap_or_default(),
                kind: Kind::Enter,
                level: Level::Info,
                channel: spans::label(channel),
                name: &opened.name,
                span: opened.id,
                parent: opened.parent,
                outcome: &Outcome::Ok,
                amount: None,
                value: None,
                fields: &fields,
            });
        }
        Ok(HostAnswer::Value(span_value(opened.id, channel)))
    }

    fn exit(
        &self,
        req: &HostRequest<'_>,
        channel: &Resource,
        owner: Owner,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        let id = value::span_id(&req.args[0], span)?;
        let outcome = value::outcome(&req.args[1], span)?;
        let closings = match lock(&self.trace.spans).exit(owner, id, channel, outcome) {
            Ok(closings) => closings,
            Err(why) => {
                return Err(spans::err_unbalanced(span, self.op.what(), id, &why));
            }
        };
        for closing in &closings {
            self.trace.emit_close(closing);
        }
        Ok(HostAnswer::Value(ply_eval::Value::Unit))
    }

    /// The span an event or a metric was recorded in, and that span's parent.
    fn enclosing(&self, owner: Owner) -> (i64, i64) {
        lock(&self.trace.spans).innermost(owner)
    }
}

/// The two field names of a `Span`, interned once.
pub(crate) static ID: std::sync::LazyLock<Symbol> = std::sync::LazyLock::new(|| Symbol::new("id"));
static CHANNEL: std::sync::LazyLock<Symbol> = std::sync::LazyLock::new(|| Symbol::new("channel"));

/// `{ id: Int, channel: String }`, as `trace.enter` answers it.
fn span_value(id: i64, channel: &Resource) -> ply_eval::Value {
    let mut fields = BTreeMap::new();
    fields.insert(ID.clone(), ply_eval::Value::Int(id));
    fields.insert(CHANNEL.clone(), ply_eval::Value::str(spans::label(channel)));
    ply_eval::Value::Record(Arc::new(fields.into_iter().collect()))
}

/// A poisoned lock here holds a span table whose invariant is "innermost last", which a thread that
/// panicked mid-push cannot have broken: the push is one `Vec::push`.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
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
    .primary(span, "this perform reached the trace sink")
    .note("inference checks a perform's arity, so reaching this means the evaluator was handed a module that was never checked")
}

#[cold]
fn value_error(span: Span, what: &str, got: &ply_eval::Value) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("{what} expects String, but got {}", got.type_name()),
    )
    .primary(span, "this perform reached the trace sink")
    .note("inference checks a perform's argument types, so reaching this means the evaluator was handed a module that was never checked")
}

#[cfg(test)]
mod tests;
