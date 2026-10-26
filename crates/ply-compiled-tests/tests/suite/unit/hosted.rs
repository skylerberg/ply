use crate::bare::context;
use ply_compiled::hosted::rt_os_cpus;

#[test]
fn a_native_that_asks_the_machine_leaves_the_run_having_read_it() {
    let mut ctx = context();
    assert!(!ctx.take_host_use().0.acted());
    let cpus = unsafe { rt_os_cpus(&mut ctx) };
    assert!(cpus.0 >= 1);
    assert!(ctx.take_failure().is_none());
    let (used, at_most_once) = ctx.take_host_use();
    assert!(
        used.acted(),
        "a read of the machine left the run hermetic: {used:?}"
    );
    assert_eq!(at_most_once, 0);
}
