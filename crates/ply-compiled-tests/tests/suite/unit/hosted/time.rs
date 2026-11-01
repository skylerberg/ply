use super::hosted;
use crate::bare::new_context;
use ply_compiled::hosted::time::{
    rt_clock_now, rt_clock_sleep, rt_time_elapsed_ns, rt_time_now_ms, rt_time_sleep_ms,
};
use ply_compiled::native::I64;

#[test]
fn the_clocks_read_the_system_and_the_run() {
    let mut context = new_context();
    hosted(&mut context);
    let now = unsafe { rt_time_now_ms(&mut context) }.0;
    assert!(now > 1_577_836_800_000, "{now}");
    let first = unsafe { rt_time_elapsed_ns(&mut context) }.0;
    let second = unsafe { rt_time_elapsed_ns(&mut context) }.0;
    assert!(second >= first, "{second} went back from {first}");
    assert!(context.take_failure().is_none());
    assert!(
        context.take_host_usage().used_nondeterminism(),
        "a reading asks the machine"
    );
}

#[test]
fn sleeping_blocks_the_calling_thread_for_at_least_the_requested_time() {
    let mut context = new_context();
    hosted(&mut context);
    let before = unsafe { rt_time_elapsed_ns(&mut context) }.0;
    unsafe { rt_time_sleep_ms(&mut context, I64(20)) };
    assert!(unsafe { rt_time_elapsed_ns(&mut context) }.0 >= before + 20_000_000);
    assert!(context.take_failure().is_none());
}

#[test]
fn the_clock_in_a_ply_program_moves_forward_by_the_sleep_time_and_a_negative_sleep_does_not_move_the_clock_backward()
 {
    let mut context = new_context();
    hosted(&mut context);
    let before = unsafe { rt_clock_now(&mut context) }.0;
    let elapsed = unsafe { rt_time_elapsed_ns(&mut context) }.0;
    assert!(
        elapsed >= before,
        "{elapsed} is behind the clock's {before}"
    );
    unsafe { rt_clock_sleep(&mut context, I64(20_000_000)) };
    let slept = unsafe { rt_clock_now(&mut context) }.0;
    assert!(slept >= before + 20_000_000, "{slept} after {before}");
    unsafe { rt_clock_sleep(&mut context, I64(-5_000_000_000)) };
    assert!(unsafe { rt_clock_now(&mut context) }.0 < slept + 5_000_000_000);
    assert!(context.take_failure().is_none());
    assert!(
        context.take_host_usage().used_nondeterminism(),
        "a reading asks the machine"
    );
}
