//! A window counts the whole process it runs in, so what the two modes count is compared in a
//! binary with no other test allocating beside it.

/// The counting allocator is a whole-binary decision, so this test binary installs it too.
#[global_allocator]
static ALLOCATOR: ply_launcher::count::Counting = ply_launcher::count::Counting;

/// Asking to see *where* an allocation came from is not a change to what the window counts: the
/// walk that names a site and the `realloc` growth of the map holding the names are the
/// instrument's, not the program's. A window that counted them would answer a different number
/// with attribution on than with it off.
#[test]
fn a_window_counts_the_same_work_whether_or_not_it_is_attributed() {
    let work = || {
        let v: Vec<u64> = (0..64u64).map(|n| n * 2).collect();
        v.len()
    };
    let plain = ply_launcher::count::window(work, false).1;
    let attributed = ply_launcher::count::window(work, true).1;
    assert!(plain.allocations > 0, "the window counted nothing");
    assert_eq!(
        plain, attributed,
        "the same work counted differently when it was attributed"
    );
}
