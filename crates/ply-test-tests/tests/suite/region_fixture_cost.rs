use ply_eval::{TaskRegions, Value};
use ply_test::GroupRegion;
use std::hint::black_box;
use std::time::{Duration, Instant};

const TEST_CELLS: usize = 4;

/// Records rather than integers, so a copy copies something; matches `ply_corpus::measure::seeded`.
fn seed(cells: usize) -> impl Fn(&mut TaskRegions) -> Value {
    move |regions: &mut TaskRegions| {
        Value::list(
            (0..cells)
                .map(|i| {
                    Value::Cell(regions.alloc_cell(Value::list(vec![
                        Value::Int(i as i64),
                        Value::str(format!("row {i}")),
                    ])))
                })
                .collect::<Vec<Value>>(),
        )
    }
}

fn first_cell(handle: &Value) -> ply_eval::arena::Slot {
    match handle {
        Value::List(items) => match items.first() {
            Some(Value::Cell(slot)) => *slot,
            other => panic!("expected a cell, found {other:?}"),
        },
        other => panic!("expected the handle list, found {other:?}"),
    }
}

fn best_of<T: PartialOrd>(repeats: usize, mut f: impl FnMut() -> T) -> T {
    (0..repeats)
        .map(|_| f())
        .reduce(|a, b| if b < a { b } else { a })
        .expect("one attempt runs")
}

fn nanos(d: Duration) -> f64 {
    d.as_secs_f64() * 1e9
}

struct Point {
    open_nanos: f64,
    /// A test that allocated its own cells, which is every real test.
    close_dirty_nanos: f64,
    rebuild_nanos: f64,
}

impl Point {
    /// Rebuilding the fixture per test, what a group with no region pays, over one test's region cost.
    fn rebuild_over_region(&self) -> f64 {
        self.rebuild_nanos / (self.open_nanos + self.close_dirty_nanos)
    }
}

/// Enough to rise above clock resolution, few enough for an unoptimized build.
fn iterations_for(cells: usize) -> u32 {
    match (cfg!(debug_assertions), cells) {
        (true, c) if c >= 1_000 => 20,
        (true, _) => 2_000,
        (false, c) if c >= 1_000 => 1_000,
        (false, _) => 100_000,
    }
}

fn measure(cells: usize, repeats: usize) -> Point {
    let build = seed(cells);
    let region = GroupRegion::build(&build);
    let iterations = iterations_for(cells);

    let open = best_of(repeats, || {
        let started = Instant::now();
        for _ in 0..iterations {
            black_box(black_box(&region).open());
        }
        started.elapsed() / iterations
    });

    let close_dirty = best_of(repeats, || {
        let mut region = region.clone();
        let (mut stack, handle) = region.open();
        for i in 0..TEST_CELLS {
            stack.alloc_cell(Value::Int(i as i64));
        }
        stack.set(first_cell(&handle), Value::Int(-1));
        let started = Instant::now();
        for _ in 0..iterations {
            black_box(&mut region).close(black_box(&stack));
        }
        started.elapsed() / iterations
    });

    let rebuild = best_of(repeats, || {
        let started = Instant::now();
        black_box(GroupRegion::build(&build));
        started.elapsed()
    });

    Point {
        open_nanos: nanos(open),
        close_dirty_nanos: nanos(close_dirty),
        rebuild_nanos: nanos(rebuild),
    }
}

/// Asserts only the ladder's shape, which is arithmetic over readings and so holds under any load.
#[test]
fn a_group_amortizes_the_build_up_to_a_ceiling_the_open_decides() {
    const CELLS: usize = 10_000;
    let point = measure(CELLS, 5);
    let ceiling = point.rebuild_over_region();
    let mut ratios = Vec::new();
    for tests in [1usize, 8, 64, 512, 4096] {
        let with_region =
            point.rebuild_nanos + tests as f64 * (point.open_nanos + point.close_dirty_nanos);
        let without = tests as f64 * point.rebuild_nanos;
        ratios.push((tests, without / with_region));
    }
    println!(
        "\ngroup of n against one fixture per test, {CELLS} cells (ceiling {ceiling:.2}x)\n{:>7} {:>10}",
        "tests", "speedup"
    );
    for (tests, ratio) in &ratios {
        println!("{tests:>7} {ratio:>9.2}x");
    }

    assert!(
        ratios.windows(2).all(|w| w[1].1 > w[0].1),
        "the saving must grow with the group: {ratios:?}"
    );
    for (tests, ratio) in &ratios {
        assert!(
            *ratio <= ceiling + 1e-9,
            "a group of {tests} claimed {ratio:.2}x against a ceiling of {ceiling:.2}x"
        );
    }
    // A one-test group pays build, open and close; rebuilding pays the build alone.
    let (_, biggest) = ratios.last().expect("the ladder is not empty");
    let (_, smallest) = ratios.first().expect("the ladder is not empty");
    assert!(
        *smallest < *biggest,
        "a one-test group must not look like a saving: {smallest:.2}x"
    );
}

#[test]
fn a_group_with_no_fixture_opens_and_closes_without_touching_the_arena() {
    let mut region = GroupRegion::empty();

    let (mut stack, _) = region.open();
    for i in 0..64 {
        stack.alloc_cell(Value::Int(i));
    }

    for _ in 0..iterations_for(0) {
        black_box(black_box(&region).open());
        black_box(&mut region).close(black_box(&stack));
    }

    assert!(region.is_empty(), "the region must not have grown");
    assert_eq!(region.mark(), 0);
}
