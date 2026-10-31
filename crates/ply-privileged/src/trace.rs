use ply_host::trace::{Discard, Json, Level, Sink, Text, Trace};
use std::sync::Arc;

#[derive(Clone, Debug, Default)]
pub struct TraceOptions {
    pub sink: SinkArg,

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
    pub fn of_grants(grants: &ply_core::host::Grants) -> TraceOptions {
        let Some([sink, level]) = grants.named("trace").next() else {
            return TraceOptions::default();
        };
        TraceOptions {
            sink: match sink.as_str() {
                "text" => SinkArg::Text,
                "off" => SinkArg::Off,
                _ => SinkArg::Json,
            },
            level: match level.as_str() {
                "debug" => LevelArg::Debug,
                "warn" => LevelArg::Warn,
                "error" => LevelArg::Error,
                _ => LevelArg::Info,
            },
        }
    }

    pub fn level_name(&self) -> &'static str {
        self.level.name()
    }

    pub fn open(&self) -> Arc<Trace> {
        let sink: Arc<dyn Sink> = match self.sink {
            SinkArg::Json => Arc::new(Json::new(self.level.level())),
            SinkArg::Text => Arc::new(Text::new(self.level.level())),
            SinkArg::Off => Arc::new(Discard),
        };
        Arc::new(Trace::new(sink))
    }
}
