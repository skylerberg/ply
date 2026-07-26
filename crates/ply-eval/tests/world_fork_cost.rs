//! What a fork costs, as a number rather than a slogan.
//!
//! The milestone's claim is that forking a seeded world is cheap enough that
//! building a fixture once and forking it per test stops being a design
//! decision. The claim is checked two ways: by counting the heap allocations a
//! fork performs, which is machine-independent and exact, and by timing it at
//! two world sizes, which is what would notice a persistent map quietly turning
//! into a copy.
//!
//! The counting allocator is why this is its own test binary: a
//! `#[global_allocator]` is a whole-binary decision and has no business in the
//! crate's unit tests.

use ply_eval::{CellId, Fixture, Value, World};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::BTreeMap;
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
fn seeded(cells: usize) -> (World, Vec<CellId>) {
    let mut world = World::new();
    let ids = (0..cells)
        .map(|i| {
            world.alloc(Value::list(vec![
                Value::Int(i as i64),
                Value::str(format!("row {i}")),
            ]))
        })
        .collect();
    (world, ids)
}

/// The fastest of a few repeats: a slower one only ever means the machine did
/// something else too.
fn best_of(repeats: usize, mut run: impl FnMut() -> Duration) -> Duration {
    (0..repeats).map(|_| run()).min().expect("at least one run")
}

fn per_fork(world: &World, iterations: usize) -> Duration {
    best_of(3, || {
        let start = Instant::now();
        for _ in 0..iterations {
            black_box(black_box(world).fork());
        }
        start.elapsed() / iterations as u32
    })
}

#[test]
fn a_fork_allocates_nothing_whatever_the_world_holds() {
    for size in [1usize, 1_000, 100_000] {
        let (world, _) = seeded(size);
        let (forked, cost) = cost_of(|| world.fork());

        assert_eq!(forked.len(), size);
        assert_eq!(
            cost.allocs, 0,
            "forking a {size}-cell world allocated {} times",
            cost.allocs
        );
        assert_eq!(cost.bytes, 0);
    }
}

/// The comparison that makes the zero above mean something: the same state
/// copied into an owned map, which is what a world without structural sharing
/// would cost per test.
#[test]
fn forking_is_orders_of_magnitude_below_copying_the_same_state() {
    let (world, _) = seeded(10_000);

    let (_, fork) = cost_of(|| world.fork());
    let (copy_out, copy) = cost_of(|| world.cells().collect::<BTreeMap<_, _>>());

    assert_eq!(copy_out.len(), 10_000);
    assert_eq!(fork.allocs, 0);
    assert!(
        copy.allocs > 100,
        "the copy baseline allocated only {} times, so it is not a baseline",
        copy.allocs
    );
    println!(
        "10,000 cells: fork = {} allocations / {} bytes; copied into an owned map = {} allocations / {} bytes",
        fork.allocs, fork.bytes, copy.allocs, copy.bytes
    );
}

#[test]
fn a_fork_costs_the_same_at_one_cell_and_at_a_hundred_thousand() {
    let (small, _) = seeded(1);
    let (large, _) = seeded(100_000);

    let small_ns = per_fork(&small, 200_000);
    let large_ns = per_fork(&large, 200_000);
    println!(
        "fork: {:?} at 1 cell, {:?} at 100,000 cells",
        small_ns, large_ns
    );

    assert!(
        large_ns < Duration::from_micros(1),
        "forking a 100,000-cell world took {large_ns:?}, which is not O(1)"
    );
    assert!(
        large_ns <= small_ns * 20 + Duration::from_nanos(100),
        "forking a 100,000-cell world took {large_ns:?} against {small_ns:?} for one cell"
    );
}

/// The milestone's claim, run as the loop `ply-test` will run: one fixture,
/// forked per test, written to by each.
#[test]
fn a_seeded_fixture_forks_per_test_in_microseconds() {
    const TESTS: usize = 10_000;

    let fixture = Fixture::build(|world| {
        let mut handles = Vec::new();
        for i in 0..10_000 {
            handles.push(Value::Cell(world.alloc(Value::Int(i))));
        }
        Value::list(handles)
    });
    let ids: Vec<CellId> = match fixture.handle() {
        Value::List(items) => items
            .iter()
            .map(|v| match v {
                Value::Cell(id) => *id,
                other => panic!("expected a cell, found {other:?}"),
            })
            .collect(),
        other => panic!("expected the handle list, found {other:?}"),
    };

    let elapsed = best_of(3, || {
        let start = Instant::now();
        for i in 0..TESTS {
            let (mut world, handle) = fixture.fork();
            assert!(world.set(ids[i % ids.len()], Value::Int(-1)));
            black_box((world, handle));
        }
        start.elapsed()
    });
    let each = elapsed / TESTS as u32;
    println!(
        "fixture of 10,000 cells: fork + one write per test = {each:?} ({TESTS} tests in {elapsed:?})"
    );

    assert!(
        each < Duration::from_micros(50),
        "forking a 10,000-cell fixture per test cost {each:?}"
    );
    // Nothing any of those tests wrote reached the fixture they forked from.
    for (i, (_, value)) in fixture.world().cells().enumerate() {
        assert!(matches!(value, Value::Int(n) if *n == i as i64));
    }
}
