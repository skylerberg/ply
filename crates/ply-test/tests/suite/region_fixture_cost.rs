//! What ADR 0017 §6's fixture costs, measured the way ADR 0005's fork was.
//!
//! The claim being replaced is `World::fork` at ~1 ns and 8,939× cheaper than
//! rebuilding a 10,000-cell fixture. The claim replacing it is "a region-scoped
//! fixture built once per worker and mutated in place", and it has two prices a
//! group pays per test rather than one:
//!
//! - **open** — handing a test its own region stack. A stack is not a persistent
//!   value, so this replays the fixture's slots into a fresh arena rather than
//!   cloning a pointer: linear in the fixture where the fork was constant.
//! - **close** — keeping what the test wrote to the fixture while discarding
//!   what it allocated. Also linear in the fixture, and *free* in the discard:
//!   the test's own slots go back to the bump pointer at the next entry point,
//!   which is the pointer reset the persistent world could not do at all.
//!
//! So the honest denominator is not the fork. It is what a group would pay
//! without a region at all — rebuilding the fixture for every test — and the
//! table below prints both that ratio and the absolute numbers it is made of,
//! because a ratio with no absolutes hides which side moved.
//!
//! **The findings, stated before anybody reads the numbers as good news.**
//!
//! 1. The ratio is **under 2×**, not 8,939×, and it is small because both sides
//!    are now linear in the fixture. The 8,939× was the fork's and it does not
//!    survive the fork.
//! 2. **At a one-cell fixture the region model is worse than rebuilding it per
//!    test** — around 0.7× rather than a saving. Opening and closing each
//!    construct a whole region stack, and at that size the fixed cost of doing
//!    so is the whole cost. The region only pays for itself once the fixture is
//!    big enough to dominate it, from around ten cells. Written down rather than
//!    rounded away, and the gate below is set to it.
//! 3. Opening a 10,000-cell fixture costs hundreds of microseconds where
//!    `World::fork` cost ~1 ns. That is the price ADR 0017 §6 names, at five
//!    orders of magnitude, and the only reason it does not show up on any run
//!    today is that no corpus has a fixture.
//!
//! **What *is* free is the case every corpus in this repository is in.** A group
//! with no fixture has nothing to seed and nothing to carry, so `ply_test`'s
//! runner skips both operations outright and the per-test region cost is zero —
//! not "one pointer clone", zero. Under the forkable world it was an `rpds`
//! clone per test per engine. That is the half of the design that has to be free
//! before the half that is not can be argued about, and it is now freer than
//! what it replaced.
//!
//! `cargo test --release -p ply-test --test region_fixture_cost -- --nocapture`
//! prints it. Release, because a debug build prices the arena's bounds checks
//! rather than the design.

use ply_eval::{TaskRegions, Value};
use ply_test::GroupRegion;
use std::hint::black_box;
use std::time::{Duration, Instant};

const SIZES: [usize; 5] = [1, 10, 100, 1_000, 10_000];
/// What one test allocates in its own region. Small on purpose: the point of
/// the measurement is that discarding it costs nothing at all.
const TEST_CELLS: usize = 4;

/// Records rather than integers, so a copy is a copy of something — the same
/// shape `ply_corpus::measure::seeded` builds, so the two tables compare.
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
        Value::List(items) => match items[0] {
            Value::Cell(slot) => slot,
            ref other => panic!("expected a cell, found {other:?}"),
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
    cells: usize,
    open_nanos: f64,
    /// A test that wrote the fixture and allocated nothing.
    close_clean_nanos: f64,
    /// A test that allocated its own cells, which is every real test. It must
    /// not be dearer than the clean close: the test's own slots are discarded by
    /// a bump-pointer reset and not by anything this pays for.
    close_dirty_nanos: f64,
    rebuild_nanos: f64,
}

impl Point {
    /// One test's whole region cost against rebuilding the fixture for it,
    /// which is what a group with no region would pay.
    fn rebuild_over_region(&self) -> f64 {
        self.rebuild_nanos / (self.open_nanos + self.close_dirty_nanos)
    }
}

/// Enough that a nanosecond-scale operation is not being read off the clock's
/// own resolution — and few enough that an unoptimized build still finishes in
/// seconds.
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

    let close_clean = best_of(repeats, || {
        let mut region = region.clone();
        let (mut stack, handle) = region.open();
        stack.set(first_cell(&handle), Value::Int(-1));
        let started = Instant::now();
        for _ in 0..iterations {
            black_box(&mut region).close(black_box(&stack));
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
        cells,
        open_nanos: nanos(open),
        close_clean_nanos: nanos(close_clean),
        close_dirty_nanos: nanos(close_dirty),
        rebuild_nanos: nanos(rebuild),
    }
}

fn render(points: &[Point]) -> String {
    let mut s = format!(
        "\nregion-scoped fixture — one worker, per test\n{:>7} {:>10} {:>12} {:>12} {:>12} {:>14}\n",
        "cells", "open ns", "close ns", "close+alloc", "rebuild ns", "rebuild/region"
    );
    for p in points {
        s.push_str(&format!(
            "{:>7} {:>10.1} {:>12.1} {:>12.1} {:>12.1} {:>13.2}x\n",
            p.cells,
            p.open_nanos,
            p.close_clean_nanos,
            p.close_dirty_nanos,
            p.rebuild_nanos,
            p.rebuild_over_region(),
        ));
    }
    s
}

/// The table, plus the properties that have to hold for the design's cost story
/// to be a fact rather than a slogan.
#[test]
fn a_region_scoped_fixture_costs_the_fixture_and_never_the_test() {
    let points: Vec<Point> = SIZES.iter().map(|&c| measure(c, 5)).collect();
    print!("{}", render(&points));

    let smallest = &points[0];
    let largest = points.last().expect("SIZES is not empty");

    // 1. Opening is linear in the fixture and no worse. The fork was constant
    //    here and this is not, which is the price ADR 0017 §6 names — but a
    //    superlinear open would be a defect rather than a price.
    let growth = largest.open_nanos / smallest.open_nanos.max(1.0);
    let sizes = largest.cells as f64 / smallest.cells as f64;
    assert!(
        growth < sizes * 4.0 + 100.0,
        "opening a {}-cell region cost {:.1} ns against {:.1} ns for {} cells",
        largest.cells,
        largest.open_nanos,
        smallest.open_nanos,
        smallest.cells
    );

    // 2. And the whole per-test cost against what the group would pay to rebuild
    //    the fixture for that test. Two thresholds rather than one, because the
    //    measurement says two things: the region wins where a fixture is big
    //    enough for the choice to matter, and at one cell it *loses* — both
    //    sides are a few hundred nanoseconds and the region's is the fixed cost
    //    of constructing a stack twice. A single `> 1` gate would be a claim the
    //    numbers do not support; a gate that only checked the large sizes would
    //    let the small case rot unnoticed.
    //
    //    Asserted on an optimized build only: unoptimized, both sides are the
    //    arena with the inlining off and the margin is inside the noise.
    if !cfg!(debug_assertions) {
        for p in &points {
            let ratio = p.rebuild_over_region();
            let floor = if p.cells >= 100 { 1.5 } else { 0.5 };
            assert!(
                ratio > floor,
                "at {} cells the region cost {:.1} ns against {:.1} ns to rebuild ({ratio:.2}x, \
                 floor {floor})",
                p.cells,
                p.open_nanos + p.close_dirty_nanos,
                p.rebuild_nanos
            );
        }
    }
}

/// The half the persistent world could not do at all: discarding what the test
/// allocated.
///
/// `World` was monotone and offered no way to drop a key, so a test that
/// allocated was closed by rebuilding the fixture *and* the close was charged
/// for the test's own cells. Here the test's slots go back to the bump pointer
/// at the next entry point and the close reads the fixture's prefix, so a test
/// that allocated a thousand cells closes as fast as one that allocated four.
///
/// Varying the test's allocations rather than timing one size against another is
/// what makes this robust: a thousandfold difference in the input has to show up
/// as a thousandfold difference in the output if the claim is false, which no
/// amount of machine noise produces.
#[test]
fn discarding_a_tests_own_cells_costs_nothing() {
    const CELLS: usize = 1_000;
    let build = seed(CELLS);
    let region = GroupRegion::build(&build);
    let iterations = iterations_for(CELLS);

    let close_over = |allocated: usize| {
        best_of(9, || {
            let mut region = region.clone();
            let (mut stack, handle) = region.open();
            for i in 0..allocated {
                stack.alloc_cell(Value::Int(i as i64));
            }
            stack.set(first_cell(&handle), Value::Int(-1));
            let started = Instant::now();
            for _ in 0..iterations {
                black_box(&mut region).close(black_box(&stack));
            }
            nanos(started.elapsed() / iterations)
        })
    };

    let few = close_over(TEST_CELLS);
    let many = close_over(TEST_CELLS * 1_000);
    println!(
        "\nclose over a {CELLS}-cell fixture: {} test cells {few:.1} ns · {} test cells {many:.1} ns",
        TEST_CELLS,
        TEST_CELLS * 1_000
    );

    assert!(
        many < few * 4.0 + 1_000.0,
        "a test that allocated {} cells closed in {many:.1} ns against {few:.1} ns for {}; \
         the close is being charged for the test's own region",
        TEST_CELLS * 1_000,
        TEST_CELLS
    );
}

/// The amortization, stated as the thing a group actually does: build one
/// fixture and run *n* tests against it, against building one per test.
///
/// It is the build that the region saves, so the saving grows with the group —
/// and it **converges**, to `rebuild / (open + close)`, which is single digits
/// rather than anything like the fork's 8,939×. A group of four thousand tests
/// does not get a four-thousandfold saving, and the ceiling is printed beside
/// the ladder so that nobody reads the first rows as a trend.
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
    let (_, biggest) = ratios.last().expect("the ladder is not empty");
    assert!(
        *biggest > 1.0 || cfg!(debug_assertions),
        "a group of 4096 saved nothing at all: {biggest:.2}x"
    );
    // And the first row is below one: a group of a single test pays for a build
    // *and* an open and a close, where rebuilding pays for the build alone. The
    // region is a bet on the group being longer than one test.
    let (_, smallest) = ratios.first().expect("the ladder is not empty");
    assert!(
        *smallest < *biggest,
        "a one-test group must not look like a saving: {smallest:.2}x"
    );
}

/// The case every corpus in this repository is actually in: no fixture at all.
///
/// The runner does not call either operation for an empty region — an engine
/// resets its own stack to its fixture at every entry point, so there is nothing
/// to install and nothing to carry — and this is the measurement that says the
/// operations are free even when a caller does call them. Under the forkable
/// world the same case cost an `rpds` clone per test per engine.
#[test]
fn a_group_with_no_fixture_opens_and_closes_in_constant_time() {
    let mut region = GroupRegion::empty();
    let iterations = iterations_for(0);

    let (mut stack, _) = region.open();
    for i in 0..64 {
        stack.alloc_cell(Value::Int(i));
    }

    let per_test = best_of(5, || {
        let started = Instant::now();
        for _ in 0..iterations {
            black_box(black_box(&region).open());
            black_box(&mut region).close(black_box(&stack));
        }
        started.elapsed() / iterations
    });
    println!("\nno fixture — open + close: {:.1} ns", nanos(per_test));

    assert!(region.is_empty(), "the region must not have grown");
    assert_eq!(region.mark(), 0);
    let budget = if cfg!(debug_assertions) {
        5_000.0
    } else {
        500.0
    };
    assert!(
        nanos(per_test) < budget,
        "opening and closing an empty region cost {:.1} ns",
        nanos(per_test)
    );
}
