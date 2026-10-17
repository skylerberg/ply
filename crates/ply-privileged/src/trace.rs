//! Which sink a run writes its records to, and at what level.

use ply_host::trace::{Discard, Json, Level, Sink, Text, Trace};
use std::sync::Arc;

#[derive(Clone, Debug, Default)]
pub struct TraceOptions {
    /// Where a `trace` record goes: `json` lines or `text` lines on stderr, or `off`.
    pub sink: SinkArg,

    /// The lowest level the sink writes; spans and metrics are `info`.
    pub level: LevelArg,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SinkArg {
    #[default]
    Json,
    Text,
    Off,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
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
    /// What `ply test` always binds: its `--trace` flag already means the definition trace.
    pub fn silent() -> TraceOptions {
        TraceOptions {
            sink: SinkArg::Off,
            level: LevelArg::Info,
        }
    }

    pub fn level_name(&self) -> &'static str {
        self.level.name()
    }

    /// The driver a run binds: the sink, the host clock, and the span table.
    pub fn open(&self) -> Arc<Trace> {
        let sink: Arc<dyn Sink> = match self.sink {
            SinkArg::Json => Arc::new(Json::new(self.level.level())),
            SinkArg::Text => Arc::new(Text::new(self.level.level())),
            SinkArg::Off => Arc::new(Discard),
        };
        Arc::new(Trace::new(sink))
    }
}
