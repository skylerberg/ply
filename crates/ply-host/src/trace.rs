//! The `trace` effect and the sinks that serve it.
//!
//! [`DECLARATION`] is the Ply source this registers against — the module
//! `std.trace`, which ships with the compiler — and `HostRegistry::bind` is what
//! checks the two still agree: an operation renamed on either side is `E0421`
//! before anything runs.
//!
//! Four properties of the registration are what `ply hosts` prints, and each is
//! one line of [`Op::declaration`]:
//!
//! - **The resource is a channel, and the call site writes it.** `trace.write[c]`
//!   per channel, never one singleton `trace.write`. A singleton would put every
//!   test that records anything into one concurrency group, and it would make a
//!   row say "records" and nothing more.
//! - **Nondeterministic.** A production sink stamps a wall-clock timestamp and
//!   mints a span id, and neither is a function of the program's state. So a
//!   `det` test that reaches an unhandled `trace` operation is `E0412` at compile
//!   time, whether or not `--host` was passed, and the only way to make such a
//!   test compile is to install a collecting handler.
//! - **At most once.** Replaying a continuation across an event writes the event
//!   twice, and a duplicated span in a log is a wrong answer about what happened
//!   rather than a missing one.
//! - **Not blocking.** A record is formatted and written inline; nothing here
//!   waits on a peer, so nothing answers `Pending`.
//!
//! ## What a span costs when nothing is collecting
//!
//! There is no configuration under which a trace operation is not performed — a
//! row cannot be conditional on a flag — so `--trace off` binds
//! [`sink::Discard`], a real, listed member of the trusted computing base whose
//! clause answers `Unit`. What that costs is exactly:
//!
//! 1. the `Fields` map the call site built, which is the program's;
//! 2. one perform: a failed `Stack::find_handler` walk, a binding resolution, and
//!    the `call` below;
//! 3. nothing else.
//!
//! "Nothing else" is designed rather than observed, and it is one `if` per
//! operation: [`Sink::wants`] is consulted **before** a name is decoded, a field
//! list is built or a clock is read. `crates/ply-host/tests/trace_cost.rs` is
//! the counting harness that asserts it — zero handler-side allocations, zero
//! clock reads, zero formatted strings for a discarded event — and it is a
//! counting harness rather than a stopwatch because a stopwatch cannot say
//! *what* was paid for.
//!
//! Level filtering is the sink's for the same reason, and it therefore saves
//! (1) nothing and (3) everything: `--trace-level warn` does not make a `Debug`
//! event free, it makes it cost one perform and one map. Saying otherwise would
//! be the misleading claim, because the only way to make it free is a row that
//! lies.
//!
//! Spans are the one thing kept whatever the sink. [`spans::Spans`] pushes and
//! pops under `discard` exactly as it does under `json`, because `E0445` is a
//! statement about the *program* and a program whose verdict changed with
//! `--trace off` would be a program nobody could debug.

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

/// The Ply declaration the registrations below are checked against: the source
/// of the module `std.trace`, which ships with the compiler.
pub const DECLARATION: &str = ply_std::TRACE;

/// The module the declaration ships as, which is what qualifies [`EFFECT`].
pub const MODULE: &str = "std.trace";

/// The program-wide effect name. Effect names are qualified, so the `trace`
/// declared by `std.trace` is `std.trace.trace`, and a program that declares its
/// own `trace` instead is `E0421` rather than silently acquiring a real sink.
pub const EFFECT: &str = "std.trace.trace";

/// How much of a record a sink admits.
///
/// Ordered `Debug < Info < Warn < Error`, which is what `--trace-level` filters
/// against. A span and a metric are `Info`: they are the shape of a thing that
/// happened rather than a thing that went wrong.
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

    /// The registration. Everything a reviewer reads in `ply hosts` is decided
    /// here; the only column an implementation gets a say in is the path, which
    /// must name the sink that actually serves the run rather than the effect.
    fn declaration(self, path: &'static str) -> HostOp {
        HostOp {
            effect: Symbol::new(EFFECT),
            op: Symbol::new(self.name()),
            // Whichever channels the program uses. `bind` expands this against
            // the program's own atoms and `ply hosts` prints one row per
            // expansion, so a sink that serves every channel still has to list
            // the channels it got — the difference between "this handler claims
            // everything" and "this handler claims these four channels".
            resource: HostResource::Any,
            determinism: Determinism::Nondeterministic,
            linearity: Linearity::AtMostOnce,
            blocking: false,
            // A `Field` has no constructor over a `Secret`, so nothing of that
            // type can reach a record. The column is the boundary's own account
            // of itself and this row's answer is no.
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
///
/// One per run, shared by `Arc`. The span table is what makes this stateful and
/// therefore what makes it a member of §7's accounting: the atom is
/// `trace.write[c]`, a **write** per channel, so two tests recording on one
/// channel conflict and are serialised by the existing conflict graph, and a
/// test that installs the `std.trace` twin discharges the atom entirely and is
/// coupled to nothing.
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

    /// The same, over a clock a test supplies — which is how the counting
    /// harness asserts that a discarded event reads the clock zero times, and
    /// how a golden test over a JSON line gets a stamp that does not move.
    pub fn with_clock(sink: Arc<dyn Sink>, clock: Arc<dyn Clock>) -> Trace {
        Trace {
            sink,
            clock,
            spans: Mutex::new(Spans::new()),
            events: AtomicU64::new(0),
            flushed: AtomicU64::new(0),
        }
    }

    /// The sink's reviewable identity and where it writes, for the
    /// `observability` block of `ply hosts` and for the digest that block is in.
    /// Read from the sink rather than from the flag that chose it, so the
    /// listing and the handler column cannot disagree.
    pub fn sink_path(&self) -> &'static str {
        self.sink.path()
    }

    pub fn sink_destination(&self) -> &'static str {
        self.sink.destination()
    }

    /// What the shutdown banner prints. Every number is one the run already
    /// held; nothing here is computed for the banner.
    pub fn counts(&self) -> Counts {
        let spans = lock(&self.spans);
        Counts {
            events: self.events.load(Ordering::Relaxed),
            spans: spans.opened(),
            abandoned: spans.abandoned(),
            flushed: self.flushed.load(Ordering::Relaxed) > 0,
        }
    }

    /// How many spans are open across every entry point. For the "0 spans open"
    /// line a stopping service prints, and for a test that asserts a teardown
    /// left nothing behind.
    pub fn open_spans(&self) -> usize {
        lock(&self.spans).total_open()
    }

    /// Closes every span this machine still has open, `Abandoned`, innermost
    /// first per task.
    ///
    /// Called from `HostRuntime::end_entry_point` on **every** exit path — a
    /// value, a diagnostic, or a spent budget — which is the whole of its value:
    /// three of the four ways a computation leaves a span never run another line
    /// of it.
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

    /// Flushes the sink. Runs after `end_entry_point` and **before** the
    /// connection pool closes, so a trace naming a rolled-back transaction is
    /// written before the connection that rolled it back is gone.
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
///
/// For the cost harness, which has to call the same handler a bound run calls
/// and must not be measuring a registry lookup a bound run does once. A second
/// handler written for the benchmark would be a benchmark of something else.
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
        // The resolved atom's resource, never one the handler re-derives: the
        // registry already decided which channel this perform named.
        let channel = &req.atom.resource;
        let owner: Owner = (req.machine, req.task);
        match self.op {
            // Two operations keep state whatever the sink does, because `E0445`
            // is a statement about the program and a program whose verdict moved
            // with `--trace off` would be a program nobody could debug.
            Op::Enter => self.enter(req, channel, owner),
            Op::Exit => self.exit(req, channel, owner, span),
            // The other four have nothing to keep, so a sink that wants nothing
            // is answered before a name is decoded, a field is built or a clock
            // is read.
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
    ///
    /// All four take their `Fields` last and their amount, if they have one, in
    /// the middle; `name_at` is the one position that moves, because an `event`
    /// carries a leading `level` and a metric does not.
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
        // The `Arc<str>` the argument already holds, so keeping the name costs
        // no allocation even when nothing is collecting — which is what lets
        // `W0609` name the innermost span under `--trace off`.
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
    /// `(0, 0)` outside any span.
    fn enclosing(&self, owner: Owner) -> (i64, i64) {
        lock(&self.trace.spans).innermost(owner)
    }
}

/// The two field names of a `Span`, interned once.
///
/// A `Symbol` is an `Arc<str>`, so minting one per `enter` and one per `exit`
/// would be three allocations per span for two constants — which is a third of
/// what a disabled span costs, spent on strings that never change.
pub(crate) static ID: std::sync::LazyLock<Symbol> = std::sync::LazyLock::new(|| Symbol::new("id"));
static CHANNEL: std::sync::LazyLock<Symbol> = std::sync::LazyLock::new(|| Symbol::new("channel"));

/// `{ id: Int, channel: String }`, as `trace.enter` answers it.
///
/// Two scalars in a record, which is the whole of what a call site allocates for
/// a span. The record is the sink's, and a discarding sink builds none.
fn span_value(id: i64, channel: &Resource) -> ply_eval::Value {
    let mut fields = BTreeMap::new();
    fields.insert(ID.clone(), ply_eval::Value::Int(id));
    fields.insert(CHANNEL.clone(), ply_eval::Value::str(spans::label(channel)));
    ply_eval::Value::Record(Arc::new(fields))
}

/// A poisoned lock here holds a span table whose invariant is "innermost last",
/// which a thread that panicked mid-push cannot have broken: the push is one
/// `Vec::push`.
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
