//! What replaced the fork, as a number rather than a slogan.
//!
//! ADR 0005 §2's claim was that forking a seeded world costs one pointer clone
//! at any fixture size, so building a fixture once and forking it per test stops
//! being a design decision. ADR 0017 §6 takes the fork away and owes the
//! replacement's price in the same units: **this file is that measurement**, and
//! it is deliberately blunt about the half that got dearer.
//!
//! Three numbers:
//!
//! 1. **Opening a fixture** — O(the fixture), where a fork was O(1). Measured at
//!    three sizes so the slope is visible rather than asserted away.
//! 2. **Resetting to the fixture** — what an entry point actually does, which is
//!    the operation a fork was on the path of. It allocates nothing, whatever
//!    the run allocated, which is what a bump arena buys and a persistent map
//!    could not.
//! 3. **A fixture per test, with a write** — the loop `ply-test` runs.
//!
//! The counting allocator is why this is its own test binary: a
//! `#[global_allocator]` is a whole-binary decision and has no business in the
//! crate's unit tests.

use ply_eval::arena::Slot;
use ply_eval::{Fixture, Value};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use std::time::{Duration, Instant};

thread_local! {
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static BYTES: Cell<usize> = const { Cell::new(0) };
}

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        let _ = BYTES.try_with(|c| c.set(c.get() + layout.size()));
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

struct Cost {
    allocs: usize,
    bytes: usize,
}

fn cost_of<T>(f: impl FnOnce() -> T) -> (T, Cost) {
    ALLOCS.with(|c| c.set(0));
    BYTES.with(|c| c.set(0));
    let out = f();
    let cost = Cost {
        allocs: ALLOCS.with(Cell::get),
        bytes: BYTES.with(Cell::get),
    };
    (out, cost)
}

/// Records, so a copy of one is a copy of something rather than of an `i64`.
fn seeded(cells: usize) -> Fixture {
    Fixture::build(|regions| {
        Value::list(
            (0..cells)
                .map(|i| {
                    Value::Cell(regions.alloc_cell(Value::list(vec![
                        Value::Int(i as i64),
                        Value::str(format!("row {i}")),
                    ])))
                })
                .collect(),
        )
    })
}

fn slots_of(fixture: &Fixture) -> Vec<Slot> {
    match fixture.handle() {
        Value::List(items) => items
            .iter()
            .map(|v| match v {
                Value::Cell(slot) => *slot,
                other => panic!("expected a cell, found {other:?}"),
            })
            .collect(),
        other => panic!("expected the handle list, found {other:?}"),
    }
}

/// The fastest of a few repeats: a slower one only ever means the machine did
/// something else too.
fn best_of(repeats: usize, mut run: impl FnMut() -> Duration) -> Duration {
    (0..repeats).map(|_| run()).min().expect("at least one run")
}

/// The half that got dearer, stated rather than hidden. A fork allocated
/// nothing at every size; an open replays the fixture's allocations, so it
/// allocates in proportion to it.
#[test]
fn opening_a_fixture_costs_the_fixture_and_the_number_is_printed() {
    println!("\n  cells   open allocations   open bytes");
    let mut at = Vec::new();
    for size in [1usize, 1_000, 100_000] {
        let fixture = seeded(size);
        let ((regions, _), cost) = cost_of(|| fixture.open());

        assert_eq!(regions.live(), size);
        assert_eq!(regions.base_len(), size);
        println!("  {size:>5}   {:>16}   {:>10}", cost.allocs, cost.bytes);
        at.push((size, cost.allocs));
    }

    let (_, one) = at[0];
    let (_, many) = at[2];
    assert!(
        many > one,
        "opening a 100,000-cell fixture allocated {many} against {one} for one \
         cell, which would mean the replay is not happening"
    );
}

/// The half that got cheaper, and the one on the path a run actually takes.
///
/// A fork put a fresh persistent map in place and dropped the old one, freeing
/// a tree node per cell the run allocated. A reset truncates the bump pointer
/// and keeps the chunks, so a run of a given size charges the allocator once in
/// the machine's whole life rather than once per entry point.
#[test]
fn resetting_to_the_fixture_allocates_nothing_however_much_the_run_did() {
    let fixture = seeded(1_000);
    let (mut regions, _) = fixture.open();

    for round in 0..4 {
        for i in 0..50_000 {
            regions.alloc_cell(Value::Int(i));
        }
        let ((), cost) = cost_of(|| regions.reset());
        assert_eq!(
            (cost.allocs, cost.bytes),
            (0, 0),
            "round {round}: resetting after 50,000 cells allocated {} times",
            cost.allocs
        );
        assert_eq!(regions.live(), 1_000, "round {round}");
    }
}

/// The milestone's claim, run as the loop `ply-test` runs: one fixture, opened
/// per test, written to by each.
///
/// The bound is a ceiling on the replacement rather than a restatement of the
/// fork's. A fork of a 10,000-cell fixture plus a write was under 50 µs; an open
/// is a replay, so this asks only that a 10,000-cell fixture still opens in the
/// same order of magnitude as the tests it is opened for.
#[test]
fn a_seeded_fixture_opens_per_test_in_microseconds() {
    const TESTS: usize = 1_000;

    let fixture = Fixture::build(|regions| {
        Value::list(
            (0..10_000)
                .map(|i| Value::Cell(regions.alloc_cell(Value::Int(i))))
                .collect(),
        )
    });
    let slots = slots_of(&fixture);

    let elapsed = best_of(3, || {
        let start = Instant::now();
        for i in 0..TESTS {
            let (mut regions, handle) = fixture.open();
            assert!(regions.set(slots[i % slots.len()], Value::Int(-1)));
            black_box((regions, handle));
        }
        start.elapsed()
    });
    let each = elapsed / TESTS as u32;
    println!(
        "\n  fixture of 10,000 cells: open + one write per test = {each:?} \
         ({TESTS} tests in {elapsed:?})"
    );

    assert!(
        each < Duration::from_millis(2),
        "opening a 10,000-cell fixture per test cost {each:?}"
    );
    // Nothing any of those tests wrote reached the fixture they opened from.
    let (after, _) = fixture.open();
    for (i, (_, value)) in after.slots().enumerate() {
        assert!(matches!(value, Value::Int(n) if *n == i as i64));
    }
}

/// A fixture is a seed rather than a live parent, and the two stacks it hands
/// out are independent. That is what the fork's sibling isolation bought, kept.
#[test]
fn two_stacks_opened_from_one_fixture_share_no_storage() {
    let fixture = seeded(64);
    let slots = slots_of(&fixture);

    let (mut a, _) = fixture.open();
    let (b, _) = fixture.open();
    assert!(a.set(slots[7], Value::Int(-1)));

    assert!(matches!(a.get(slots[7]), Some(Value::Int(-1))));
    assert!(matches!(b.get(slots[7]), Some(Value::List(_))));
}
