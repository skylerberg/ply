use ply_host::time::{TimeHost, wall_clock_ms};
use std::time::Duration;

#[test]
fn the_wall_clock_reads_milliseconds_since_the_epoch() {
    let now = wall_clock_ms();
    assert!(now > 1_577_836_800_000, "{now}");
    assert!(now < 4_102_444_800_000, "{now}");
}

#[test]
fn the_monotonic_clock_counts_from_the_run_and_never_goes_back() {
    let time = TimeHost::new();
    let first = time.elapsed_ns();
    assert!(first >= 0, "{first}");
    std::thread::sleep(Duration::from_millis(5));
    let second = time.elapsed_ns();
    assert!(
        second >= first + 5_000_000,
        "{second} is not 5ms after {first}"
    );
}
