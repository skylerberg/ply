mod fields;
pub mod sink;
pub mod spans;

pub use fields::fields_read;
pub use sink::{
    Clock, DISCARD_PATH, Discard, Field, HostClock, Json, Outcome, OwnedRecord, Record, RecordKind,
    Sink, Text,
};
pub use spans::{Owner, Spans, Unbalanced};

use ply_core::host::MachineId;
use ply_core::{Decimal, Diagnostic, Resource, Symbol, TaskId};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

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

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Counts {
    pub events: u64,
    pub spans: u64,
    pub abandoned: u64,
    pub flushed: bool,
}

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

    pub fn with_clock(sink: Arc<dyn Sink>, clock: Arc<dyn Clock>) -> Trace {
        Trace {
            sink,
            clock,
            spans: Mutex::new(Spans::new()),
            events: AtomicU64::new(0),
            flushed: AtomicU64::new(0),
        }
    }

    pub fn sink_module_path(&self) -> &'static str {
        self.sink.path()
    }

    pub fn sink_destination_name(&self) -> &'static str {
        self.sink.destination()
    }

    pub fn wants(&self, level: Level) -> bool {
        self.sink.wants(level)
    }

    pub fn counts(&self) -> Counts {
        let spans = lock(&self.spans);
        Counts {
            events: self.events.load(Ordering::Relaxed),
            spans: spans.opened(),
            abandoned: spans.abandoned(),
            flushed: self.flushed.load(Ordering::Relaxed) > 0,
        }
    }

    pub fn open_spans(&self) -> usize {
        lock(&self.spans).total_open()
    }

    pub fn left_open(&self) -> u64 {
        lock(&self.spans).left_open()
    }

    pub fn end_entry_point(&self, machine: MachineId) -> Option<Diagnostic> {
        let (closings, abandoned) = lock(&self.spans).end_entry_point(machine);
        for closing in &closings {
            self.emit_close(closing);
        }
        (!abandoned.is_empty()).then(|| spans::warn_abandoned(&abandoned))
    }

    pub fn end_task(&self, machine: MachineId, task: TaskId) {
        let closings = lock(&self.spans).end_task(machine, task);
        for closing in &closings {
            self.emit_close(closing);
        }
    }

    pub fn span_owners(&self) -> usize {
        lock(&self.spans).owners()
    }

    pub fn flush(&self) {
        self.sink.flush();
        self.flushed.fetch_add(1, Ordering::Relaxed);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &self,
        owner: Owner,
        kind: RecordKind,
        level: Level,
        channel: &str,
        name: &str,
        amount: Option<i64>,
        value: Option<Decimal>,
        fields: &[(String, Field)],
    ) {
        if !self.sink.wants(level) {
            return;
        }
        let (span, parent) = lock(&self.spans).innermost(owner);
        let ts = self.clock.micros();
        self.events.fetch_add(1, Ordering::Relaxed);
        self.sink.write(&Record {
            ts,
            kind,
            level,
            channel,
            name,
            span,
            parent,
            outcome: &Outcome::Ok,
            amount,
            value,
            fields,
        });
    }

    pub fn enter(
        &self,
        owner: Owner,
        channel: &str,
        name: &str,
        fields: &[(String, Field)],
    ) -> i64 {
        let wanted = self.sink.wants(Level::Info);
        let started = wanted.then(|| self.clock.micros());
        let opened = lock(&self.spans).enter(owner, labelled(channel), name.into(), started);
        if wanted {
            self.events.fetch_add(1, Ordering::Relaxed);
            self.sink.write(&Record {
                ts: started.unwrap_or_default(),
                kind: RecordKind::Enter,
                level: Level::Info,
                channel,
                name: &opened.name,
                span: opened.id,
                parent: opened.parent,
                outcome: &Outcome::Ok,
                amount: None,
                value: None,
                fields,
            });
        }
        opened.id
    }

    pub fn exit(
        &self,
        owner: Owner,
        channel: &str,
        id: i64,
        outcome: Outcome,
    ) -> Result<(), Unbalanced> {
        let closings = lock(&self.spans).exit(owner, id, &labelled(channel), outcome)?;
        for closing in &closings {
            self.emit_close(closing);
        }
        Ok(())
    }

    fn emit_close(&self, closing: &spans::Closing) {
        if !self.sink.wants(Level::Info) {
            return;
        }
        let ts = self.clock.micros();
        self.events.fetch_add(1, Ordering::Relaxed);
        self.sink.write(&Record {
            ts,
            kind: RecordKind::Exit,
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

fn labelled(channel: &str) -> Resource {
    Resource::Named(Symbol::new(channel))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
