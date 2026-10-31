use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub struct TimeHost {
    started: Instant,
}

static PROCESS: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

pub fn mark_start(at: Instant) {
    let _ = PROCESS.set(at);
}

pub fn wall_clock_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

impl Default for TimeHost {
    fn default() -> TimeHost {
        TimeHost::new()
    }
}

impl TimeHost {
    pub fn new() -> TimeHost {
        TimeHost {
            started: PROCESS.get().copied().unwrap_or_else(Instant::now),
        }
    }

    pub fn elapsed_ns(&self) -> i64 {
        i64::try_from(self.started.elapsed().as_nanos()).unwrap_or(i64::MAX)
    }
}
