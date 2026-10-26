//! The run's clocks: the system's wall clock, and a monotonic one counting from when the process
//! started, which `std.time`'s natives read and a production region's sleeps are deadlines on.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The run's monotonic clock, counting from the process's start when it was marked, else from the
/// moment this host was built.
pub struct TimeHost {
    started: Instant,
}

static PROCESS: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// Marks when the process started, so a program's first reading of the clock says what came
/// before it. The first mark wins.
pub fn mark_start(at: Instant) {
    let _ = PROCESS.set(at);
}

/// Milliseconds since the Unix epoch on the system's clock; a clock set before it reads `0` rather
/// than refusing.
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

    /// Nanoseconds since this run's clock was started: what `std.time` and the language's `clock`
    /// read, and what a production region's sleeps are deadlines on.
    pub fn elapsed_ns(&self) -> i64 {
        i64::try_from(self.started.elapsed().as_nanos()).unwrap_or(i64::MAX)
    }

    /// Parks this thread for `nanos`; a negative span is no wait at all.
    pub fn sleep_ns(&self, nanos: i64) {
        std::thread::sleep(Duration::from_nanos(u64::try_from(nanos).unwrap_or(0)));
    }
}
