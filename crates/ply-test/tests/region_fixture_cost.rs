//! What ADR 0017 §6's fixture costs, measured the way ADR 0005's fork was.
//!
//! The claim being replaced is `World::fork` at ~1 ns and 8,939× cheaper than
//! rebuilding a 10,000-cell fixture. The claim replacing it is "a region-scoped
//! fixture built once per group and mutated in place", and it has two prices a
//! group pays per test rather than one:
//!
//! - **open** — handing the region to a test. Under the persistent world this is
//!   the same pointer clone the fork was, so it is the same number.
//! - **close** — discarding what the test allocated while keeping what it wrote.
//!   This is the price the fork did not have. `World` is monotone and offers no
//!   way to drop a key, so a test that allocated is closed by rebuilding the
//!   cells below the mark: proportional to the *fixture*, not to the test.
//!
//! So the honest denominator is not the fork. It is what a group would pay
//! without a region at all — rebuilding the fixture for every test — and the
//! table below prints both that ratio and the absolute numbers it is made of,
//! because a ratio with no absolutes hides which side moved.
//!
//! **The finding, stated before anybody reads the numbers as good news.** The
//! ratio is about **1.6×**, not 8,939×. The 8,939× was the fork's and it does
//! not survive the fork: closing a region against a monotone persistent map is
//! re-inserting the cells below the mark, which is rebuilding the fixture minus
//! the cost of constructing its values. What would restore it is a `World` that
//! can drop a key — or ADR 0017's arena, where closing a region is a pointer
//! reset — and neither is `ply-test`'s to add. What *is* free here is the case
//! every corpus today is in: a group with no fixture closes in constant time.
//!
//! `cargo test --release -p ply-test --test region_fixture_cost -- --nocapture`
//! prints it. Release, because a debug build prices `rpds` rather than the
//! design.

use ply_eval::{CellId, Value, World};
use ply_test::GroupRegion;
use std::hint::black_box;
use std::time::{Duration, Instant};

const SIZES: [usize; 5] = [1, 10, 100, 1_000, 10_000];
/// What one test allocates in its own region. Small on purpose: the point of
/// the measurement is that closing costs the fixture's size and not this.
const TEST_CELLS: usize = 4;

/// Records rather than integers, so a copy is a copy of something — the same
/// shape `ply_corpus::measure::seeded` builds, so the two tables compare.
fn seeded(cells: usize) -> World {
    let mut world = World::new();
    for i in 0..cells {
        world.alloc(Value::list(vec![
            Value::Int(i as i64),
            Value::str(format!("row {i}")),
        ]));
    }
    world
}

fn best_of(repeats: usize, mut f: impl FnMut() -> Duration) -> Duration {
    (0..repeats).map(|_| f()).min().expect("one attempt runs")
}

fn nanos(d: Duration) -> f64 {
    d.as_secs_f64() * 1e9
}

struct Point {
    cells: usize,
    open_nanos: f64,
    /// A test that wrote the fixture and allocated nothing: the region it ends
    /// with *is* the group's, so closing is a pointer move.
    close_clean_nanos: f64,
    /// A test that allocated its own cells, which is every real test.
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
/// own resolution — and few enough that an unoptimized build, which is pricing
/// `rpds` rather than this design, still finishes in seconds.
fn iterations_for(cells: usize) -> u32 {
    match (cfg!(debug_assertions), cells) {
        (true, c) if c >= 1_000 => 20,
        (true, _) => 2_000,
        (false, c) if c >= 1_000 => 1_000,
        (false, _) => 100_000,
    }
}

fn measure(cells: usize, repeats: usize) -> Point {
    let region = GroupRegion::build(|| seeded(cells));
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
        let mut world = region.open();
        world.set(CellId(0), Value::Int(-1));
        let started = Instant::now();
        for _ in 0..iterations {
            black_box(&mut region).close(black_box(&world));
        }
        started.elapsed() / iterations
    });

    let close_dirty = best_of(repeats, || {
        let mut region = region.clone();
        let mut world = region.open();
        for i in 0..TEST_CELLS {
            world.alloc(Value::Int(i as i64));
        }
        world.set(CellId(0), Value::Int(-1));
        let started = Instant::now();
        for _ in 0..iterations {
            black_box(&mut region).close(black_box(&world));
        }
        started.elapsed() / iterations
    });

    let rebuild = best_of(repeats, || {
        let started = Instant::now();
        black_box(seeded(cells));
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
        "\nregion-scoped fixture — one group, per test\n{:>7} {:>10} {:>12} {:>12} {:>12} {:>14}\n",
        "cells", "open ns", "close ns", "close+alloc", "rebuild ns", "rebuild/region"
    );
    for p in points {
        s.push_str(&format!(
            "{:>7} {:>10.1} {:>12.1} {:>12.1} {:>12.1} {:>13.1}x\n",
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

/// The table, plus the three properties that have to hold for "cheap" to be a
/// fact rather than a slogan.
#[test]
fn a_region_scoped_fixture_is_cheaper_than_rebuilding_it_per_test() {
    let points: Vec<Point> = SIZES.iter().map(|&c| measure(c, 5)).collect();
    print!("{}", render(&points));

    let smallest = &points[0];
    let largest = points.last().expect("SIZES is not empty");

    // 1. Opening the region does not get dearer as the fixture grows. This is
    //    the half the fork already had and the half that must not regress.
    assert!(
        largest.open_nanos < smallest.open_nanos * 20.0 + 100.0,
        "opening a {}-cell region cost {:.1} ns against {:.1} ns for {} cells",
        largest.cells,
        largest.open_nanos,
        smallest.open_nanos,
        smallest.cells
    );

    // 2. A test that allocated nothing closes in constant time, whatever the
    //    fixture's size: the world it ends with is already the group's.
    assert!(
        largest.close_clean_nanos < smallest.close_clean_nanos * 20.0 + 100.0,
        "closing a clean {}-cell region cost {:.1} ns",
        largest.cells,
        largest.close_clean_nanos
    );

    // 3. And the whole per-test cost stays under what the group would pay to
    //    rebuild the fixture for that test, at every size. This is the number
    //    the design is for: 8,939× is gone with the fork, and what replaces it
    //    is a ratio that is still greater than one — but only just, which is why
    //    it is asserted on an optimized build only. Unoptimized, both sides are
    //    `rpds` with the inlining off and the margin is inside the noise.
    if !cfg!(debug_assertions) {
        for p in &points {
            assert!(
                p.rebuild_over_region() > 1.2,
                "at {} cells the region cost {:.1} ns against {:.1} ns to rebuild",
                p.cells,
                p.open_nanos + p.close_dirty_nanos,
                p.rebuild_nanos
            );
        }
    }
}

/// The amortization, stated as the thing a group actually does: build one
/// fixture and run *n* tests against it, against building one per test.
///
/// It is the build that the region saves, so the saving grows with the group —
/// and it **converges**, to `rebuild / (open + close)`, which is the ~1.6× the
/// module header names rather than anything like the fork's 8,939×. A group of
/// four thousand tests does not get a four-thousandfold saving, and the ceiling
/// is printed beside the ladder so that nobody reads the first rows as a trend.
#[test]
fn a_group_amortizes_the_build_up_to_a_ceiling_the_close_decides() {
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
}

/// The case every corpus in this repository is actually in: no fixture at all.
/// Opening and closing a region are both constant, so the region model costs a
/// suite with no fixture nothing per test — which is the half of the design that
/// has to be free before the half that is not can be argued about.
#[test]
fn a_group_with_no_fixture_opens_and_closes_in_constant_time() {
    let mut region = GroupRegion::empty();
    let iterations = iterations_for(0);

    let mut world = region.open();
    for i in 0..64 {
        world.alloc(Value::Int(i));
    }

    let per_test = best_of(5, || {
        let started = Instant::now();
        for _ in 0..iterations {
            black_box(black_box(&region).open());
            black_box(&mut region).close(black_box(&world));
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
