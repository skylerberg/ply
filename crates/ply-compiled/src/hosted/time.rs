use super::{Nondeterministic, if_host_allowed};
use crate::c::{MachineNative, runtime_native};
use crate::native::I64;
use crate::rt::Context;
use std::time::Duration;

pub(super) fn natives() -> Vec<MachineNative> {
    vec![
        runtime_native(
            "time_now_ms",
            rt_time_now_ms as unsafe extern "C" fn(_) -> _,
        ),
        runtime_native(
            "time_elapsed_ns",
            rt_time_elapsed_ns as unsafe extern "C" fn(_) -> _,
        ),
        runtime_native(
            "time_sleep_ms",
            rt_time_sleep_ms as unsafe extern "C" fn(_, _),
        ),
        runtime_native("clock_now", rt_clock_now as unsafe extern "C" fn(_) -> _),
        runtime_native("clock_sleep", rt_clock_sleep as unsafe extern "C" fn(_, _)),
    ]
}

fn monotonic(c: &mut Context) -> I64 {
    match c.host_runtime().now() {
        Ok(nanos) => I64(nanos),
        Err(d) => {
            c.fail(d);
            I64(0)
        }
    }
}

pub unsafe extern "C" fn rt_time_now_ms(context: *mut Context) -> I64 {
    if_host_allowed(context, "time_now_ms", Nondeterministic, I64(0), |_| {
        I64(ply_host::time::wall_clock_ms())
    })
}

pub unsafe extern "C" fn rt_time_elapsed_ns(context: *mut Context) -> I64 {
    if_host_allowed(
        context,
        "time_elapsed_ns",
        Nondeterministic,
        I64(0),
        monotonic,
    )
}

pub unsafe extern "C" fn rt_time_sleep_ms(context: *mut Context, ms: I64) {
    if_host_allowed(context, "time_sleep_ms", Nondeterministic, (), |_| {
        std::thread::sleep(Duration::from_millis(u64::try_from(ms.0).unwrap_or(0)));
    });
}

pub unsafe extern "C" fn rt_clock_now(context: *mut Context) -> I64 {
    if_host_allowed(context, "clock_now", Nondeterministic, I64(0), monotonic)
}

pub unsafe extern "C" fn rt_clock_sleep(context: *mut Context, nanos: I64) {
    if_host_allowed(context, "clock_sleep", Nondeterministic, (), |c| {
        if crate::tasks::waited(c, crate::tasks::REQUEST_SLEEP, nanos.0).is_none() {
            std::thread::sleep(Duration::from_nanos(u64::try_from(nanos.0).unwrap_or(0)));
        }
    });
}
