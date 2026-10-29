use super::hosted;
use crate::bare::context;
use ply_compiled::hosted::time::{
    rt_clock_now, rt_clock_sleep, rt_time_elapsed_ns, rt_time_now_ms, rt_time_sleep_ms,
};
use ply_compiled::native::I64;

#[test]
fn the_clocks_read_the_system_and_the_run() {
    let mut ctx = context();
    hosted(&mut ctx);
    let now = unsafe { rt_time_now_ms(&mut ctx) }.0;
    assert!(now > 1_577_836_800_000, "{now}");
    let first = unsafe { rt_time_elapsed_ns(&mut ctx) }.0;
    let second = unsafe { rt_time_elapsed_ns(&mut ctx) }.0;
    assert!(second >= first, "{second} went back from {first}");
    assert!(ctx.take_failure().is_none());
    assert!(ctx.take_host_use().acted(), "a reading asks the machine");
}

#[test]
fn a_wait_parks_the_asking_thread() {
    let mut ctx = context();
    hosted(&mut ctx);
    let before = unsafe { rt_time_elapsed_ns(&mut ctx) }.0;
    unsafe { rt_time_sleep_ms(&mut ctx, I64(20)) };
    assert!(unsafe { rt_time_elapsed_ns(&mut ctx) }.0 >= before + 20_000_000);
    assert!(ctx.take_failure().is_none());
}

/// `clock` reads the run's monotonic clock, the one `std.time` reads, and outside a task of the
/// run's region its sleep waits the thread, a span at or below zero not at all.
#[test]
fn the_language_clock_reads_the_run_and_its_sleep_waits_the_thread_outside_a_task() {
    let mut ctx = context();
    hosted(&mut ctx);
    let before = unsafe { rt_clock_now(&mut ctx) }.0;
    let elapsed = unsafe { rt_time_elapsed_ns(&mut ctx) }.0;
    assert!(
        elapsed >= before,
        "{elapsed} is behind the clock's {before}"
    );
    unsafe { rt_clock_sleep(&mut ctx, I64(20_000_000)) };
    let slept = unsafe { rt_clock_now(&mut ctx) }.0;
    assert!(slept >= before + 20_000_000, "{slept} after {before}");
    unsafe { rt_clock_sleep(&mut ctx, I64(-5_000_000_000)) };
    assert!(unsafe { rt_clock_now(&mut ctx) }.0 < slept + 5_000_000_000);
    assert!(ctx.take_failure().is_none());
    assert!(ctx.take_host_use().acted(), "a reading asks the machine");
}
