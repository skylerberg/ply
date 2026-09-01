//! Which sink a run writes its records to, and at what level.

use clap::{Args, ValueEnum};
use ply_host::trace::{Discard, Json, Level, Sink, Text, Trace};
use std::sync::Arc;

#[derive(Args, Clone, Debug, Default)]
pub struct TraceOptions {
    /// Where a `trace` operation's record goes. `json` is one object per line on
    /// stderr; `text` is a human line stamped from the run's start; `off` binds
    /// `ply_host::trace::discard`, which is a handler and not an absence.
    #[arg(
        id = "trace_sink",
        long = "trace",
        value_enum,
        default_value_t = SinkArg::Json,
        value_name = "SINK",
        requires = "host",
    )]
    pub sink: SinkArg,

    /// The lowest level the sink writes. Filtering happens in the sink, so this
    /// does not make a `Debug` event free — it makes it cost one perform and one
    /// map. A span and a metric are `info`.
    #[arg(
        id = "trace_level",
        long = "trace-level",
        value_enum,
        default_value_t = LevelArg::Info,
        value_name = "LEVEL",
        requires = "host",
    )]
    pub level: LevelArg,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, ValueEnum)]
pub enum SinkArg {
    #[default]
    Json,
    Text,
    Off,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, ValueEnum)]
pub enum LevelArg {
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

impl LevelArg {
    pub fn name(self) -> &'static str {
        match self {
            LevelArg::Debug => "debug",
            LevelArg::Info => "info",
            LevelArg::Warn => "warn",
            LevelArg::Error => "error",
        }
    }

    pub fn level(self) -> Level {
        match self {
            LevelArg::Debug => Level::Debug,
            LevelArg::Info => Level::Info,
            LevelArg::Warn => Level::Warn,
            LevelArg::Error => Level::Error,
        }
    }
}

impl TraceOptions {
    /// What `ply test` binds, with or without `--host`, and there is no flag
    /// that changes it.
    ///
    /// Two reasons, and the second is the one that decides it. A suite's records
    /// are asserted through `std.trace`'s twin — that is the substitution this
    /// milestone exists to make possible — so a test that wants to know what it
    /// recorded installs a collecting handler and never reaches a sink at all.
    /// And `ply test` already owns `--trace <auto|always|never>`, which selects
    /// M5's *definition* trace for a suspect set; a second `--trace` meaning
    /// something else on the same command would be a flag whose meaning depended
    /// on which milestone the reader had in mind.
    ///
    /// The cost is stated: a host-backed test's records cannot be read from
    /// stderr. `ply run --host --trace json` is where that is done.
    pub fn silent() -> TraceOptions {
        TraceOptions {
            sink: SinkArg::Off,
            level: LevelArg::Info,
        }
    }

    /// What the `observability` block prints beside the sink's path.
    pub fn level_name(&self) -> &'static str {
        self.level.name()
    }

    /// The driver a run binds: the sink, the host clock, and the span table.
    ///
    /// Built here rather than inside `Hosts::open` so that the one place a sink
    /// is chosen is the one place the flags are read.
    pub fn open(&self) -> Arc<Trace> {
        let sink: Arc<dyn Sink> = match self.sink {
            SinkArg::Json => Arc::new(Json::new(self.level.level())),
            SinkArg::Text => Arc::new(Text::new(self.level.level())),
            // A level on a discarding sink would be a distinction with no consequence, and printing
            // one would invite a reader to believe `--trace off --trace-level debug` writes
            // something.
            SinkArg::Off => Arc::new(Discard),
        };
        Arc::new(Trace::new(sink))
    }
}
