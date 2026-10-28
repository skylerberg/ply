//! `std.time`'s natives: the system's wall clock, the run's monotonic one, and a wait of the thread
//! that asks; and those of the language's `clock` outside `simulate`: the run's monotonic clock, and
//! a sleep that waits in the run's region where a task of it sleeps.

use super::{READS, WRITES, asked};
use crate::c::{MachineNative, machine_native};
use crate::native::I64;
use crate::rt::Ctx;
use std::time::Duration;

pub(super) fn natives() -> Vec<MachineNative> {
    vec![
        machine_native(
            "time_now_ms",
            rt_time_now_ms as unsafe extern "C" fn(_) -> _,
        ),
        machine_native(
            "time_elapsed_ns",
            rt_time_elapsed_ns as unsafe extern "C" fn(_) -> _,
        ),
        machine_native(
            "time_sleep_ms",
            rt_time_sleep_ms as unsafe extern "C" fn(_, _),
        ),
        machine_native("clock_now", rt_clock_now as unsafe extern "C" fn(_) -> _),
        machine_native("clock_sleep", rt_clock_sleep as unsafe extern "C" fn(_, _)),
    ]
}

/// Nanoseconds on the run's monotonic clock, since its host was built or the process started.
fn monotonic(c: &mut Ctx) -> I64 {
    match c.host_runtime().now() {
        Ok(nanos) => I64(nanos),
        Err(d) => {
            c.fail(d);
            I64(0)
        }
    }
}

/// Milliseconds since the Unix epoch on the system's clock; one set before it reads 0.
pub unsafe extern "C" fn rt_time_now_ms(ctx: *mut Ctx) -> I64 {
    asked(ctx, "time_now_ms", READS, I64(0), |_| {
        I64(ply_host::time::wall_clock_ms())
    })
}

/// Nanoseconds on the run's monotonic clock; 0, with the run failed, where no host lends one.
pub unsafe extern "C" fn rt_time_elapsed_ns(ctx: *mut Ctx) -> I64 {
    asked(ctx, "time_elapsed_ns", READS, I64(0), monotonic)
}

/// The asking thread parked for `ms` milliseconds, which the real handler hands only above 0.
pub unsafe extern "C" fn rt_time_sleep_ms(ctx: *mut Ctx, ms: I64) {
    asked(ctx, "time_sleep_ms", WRITES, (), |_| {
        std::thread::sleep(Duration::from_millis(u64::try_from(ms.0).unwrap_or(0)));
    });
}

/// `clock.now`: nanoseconds on the run's monotonic clock, the one `time_elapsed_ns` reads; 0, with
/// the run failed, where no host lends one.
pub unsafe extern "C" fn rt_clock_now(ctx: *mut Ctx) -> I64 {
    asked(ctx, "clock_now", READS, I64(0), monotonic)
}

/// `clock.sleep` of `nanos` nanoseconds: a task of the run's region waits there while the region's
/// other tasks run, and the run's clock reaching its deadline readies it; any other asker waits
/// its thread, not at all for a span at or below zero. A wait changes nothing outside the program,
/// so asking again is as asking once.
pub unsafe extern "C" fn rt_clock_sleep(ctx: *mut Ctx, nanos: I64) {
    asked(ctx, "clock_sleep", READS, (), |c| {
        if crate::tasks::waited(c, crate::tasks::ASK_SLEEP, nanos.0).is_none() {
            std::thread::sleep(Duration::from_nanos(u64::try_from(nanos.0).unwrap_or(0)));
        }
    });
}
