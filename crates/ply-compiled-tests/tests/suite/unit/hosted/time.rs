use super::hosted;
use crate::bare::context;
use ply_compiled::hosted::time::{rt_time_elapsed_ns, rt_time_now_ms, rt_time_sleep_ms};
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
    assert!(ctx.take_host_use().0.acted(), "a reading asks the machine");
}

#[test]
fn a_wait_parks_the_asking_thread_and_is_done_once() {
    let mut ctx = context();
    hosted(&mut ctx);
    let before = unsafe { rt_time_elapsed_ns(&mut ctx) }.0;
    unsafe { rt_time_sleep_ms(&mut ctx, I64(20)) };
    assert!(unsafe { rt_time_elapsed_ns(&mut ctx) }.0 >= before + 20_000_000);
    let (_, at_most_once) = ctx.take_host_use();
    assert_eq!(at_most_once, 1, "a wait crossed twice waits twice");
}
