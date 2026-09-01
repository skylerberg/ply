//! The four numbers ADR 0005 spends and never priced.

use crate::pipeline::{Front, front};
use anyhow::{Context, Result, bail};
use ply_core::Footprint;
use ply_eval::arena::Slot;
use ply_eval::cont::{Frame, Prompt, Stack};
use ply_eval::{Env, Evaluator, Fixture, Machine, Value};
use ply_span::{SourceId, SourceMap, Span};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::parse_program;
use ply_syntax::resolve::{Resolved, resolve};
use serde::Serialize;
use std::hint::black_box;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// The fastest of several attempts.
fn best_of(repeats: usize, mut run: impl FnMut() -> Duration) -> Duration {
    (0..repeats.max(1))
        .map(|_| run())
        .min()
        .expect("at least one attempt always runs")
}

fn millis(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn nanos(d: Duration) -> f64 {
    d.as_secs_f64() * 1e9
}

#[derive(Clone, Debug, Serialize)]
pub struct Pass {
    /// Constructing one worker: what `ply-test` pays per rayon thread per concurrency group.
    pub worker_setup_millis: f64,
    /// Every test once on a freshly built worker — setup, plus whatever the engine defers to first
    /// call.
    pub first_pass_millis: f64,
    /// Every test again on the same worker.
    pub steady_pass_millis: f64,
    pub tests: usize,
    /// Atoms performed in one pass, from the tracer.
    pub performs: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Throughput {
    pub root: String,
    pub definitions: usize,
    pub tests: usize,
    pub pass: Pass,
    /// `lower` over every test body once.
    pub lower_test_bodies_millis: f64,
}

pub fn throughput(root: &Path, repeats: usize) -> Result<Throughput> {
    let front = front(root)?;
    Ok(Throughput {
        root: root.display().to_string(),
        definitions: front.check.defs.len(),
        tests: front.check.tests.len(),
        lower_test_bodies_millis: millis(lower_every_test_body(&front, repeats)),
        pass: one_pass(&front, repeats)?,
    })
}

/// What the machine pays before a test's first transition.
fn lower_every_test_body(front: &Front, repeats: usize) -> Duration {
    let bodies: Vec<&ply_syntax::ast::Expr> = front
        .program
        .modules
        .iter()
        .flat_map(|m| m.items.iter())
        .filter_map(|item| match item {
            ply_syntax::ast::Item::Test(t) => Some(&t.body),
            _ => None,
        })
        .collect();

    best_of(repeats, || {
        let started = Instant::now();
        for body in &bodies {
            black_box(ply_eval::lower(body));
        }
        started.elapsed()
    })
}

fn one_pass(front: &Front, repeats: usize) -> Result<Pass> {
    fn build<'a>(front: &'a Front) -> Box<dyn Evaluator + 'a> {
        let mut machine = Machine::new(&front.program, &front.resolved, &front.check);
        machine.share_region_kinds(front.shared_region_kinds());
        Box::new(machine)
    }

    let setup = best_of(repeats, || {
        let started = Instant::now();
        black_box(build(front));
        started.elapsed()
    });

    // Both passes come from the least disturbed attempt, as a pair.
    let mut performs = 0u64;
    let mut best: Option<(Duration, Duration)> = None;
    for _ in 0..repeats.max(1) {
        let mut worker = build(front);
        let started = Instant::now();
        performs = run_every_test(worker.as_mut())?;
        let first = started.elapsed();

        let started = Instant::now();
        run_every_test(worker.as_mut())?;
        let steady = started.elapsed();

        if best.is_none_or(|(f, s)| first + steady < f + s) {
            best = Some((first, steady));
        }
    }
    let (first, steady) = best.expect("at least one attempt always runs");

    Ok(Pass {
        worker_setup_millis: millis(setup),
        first_pass_millis: millis(first),
        steady_pass_millis: millis(steady),
        tests: front.check.tests.len(),
        performs,
    })
}

/// Every test once, returning the atoms performed across the pass.
fn run_every_test(worker: &mut dyn Evaluator) -> Result<u64> {
    let mut performs = 0u64;
    for index in 0..worker.test_count() {
        let name = worker.test_name(index).unwrap_or("?").to_string();
        worker.eval_test(index).map_err(|d| {
            anyhow::anyhow!("test `{name}` failed while being timed: {}", d.message)
        })?;
        performs += worker.observed_performs().unwrap_or_default();
    }
    Ok(performs)
}

/// What ADR 0017 §6's region-scoped fixture costs, measured the way `fork`'s 1 ns was.
#[derive(Clone, Debug, Serialize)]
pub struct FixturePoint {
    pub cells: usize,
    pub open_nanos: f64,
    /// An opened fixture a test then writes to — a slot write, where the persistent map paid for
    /// copy-on-write down a root-to-leaf path.
    pub open_and_write_nanos: f64,
    /// Building the same fixture by running the setup again — the alternative to opening one, and
    /// the denominator of the claim.
    pub rebuild_nanos: f64,
    pub rebuild_over_open: f64,
    pub rebuild_over_open_and_write: f64,
}

/// Records rather than integers, so a copy is a copy of something.
fn seeded(cells: usize) -> Fixture {
    Fixture::build(|regions| {
        let handles = (0..cells)
            .map(|i| {
                Value::Cell(regions.alloc_cell(Value::list(vec![
                    Value::Int(i as i64),
                    Value::str(format!("row {i}")),
                ])))
            })
            .collect();
        Value::list(handles)
    })
}

fn cells_of(handle: &Value) -> Vec<Slot> {
    match handle {
        Value::List(items) => items
            .iter()
            .filter_map(|v| match v {
                Value::Cell(slot) => Some(*slot),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

pub fn fixture_cost(sizes: &[usize], repeats: usize) -> Vec<FixturePoint> {
    sizes
        .iter()
        .map(|&cells| {
            let fixture = seeded(cells);
            let slots = cells_of(fixture.handle());
            // Enough iterations that a nanosecond-scale operation is not read off the clock's own
            // resolution, and few enough that an O(n) open of a 10,000-cell fixture still finishes:
            // the product is what is held roughly constant, not the count.
            let iterations = (1_000_000 / (cells as u32 + 1)).max(16);

            let open = best_of(repeats, || {
                let started = Instant::now();
                for _ in 0..iterations {
                    black_box(black_box(&fixture).open());
                }
                started.elapsed() / iterations
            });

            let open_write = best_of(repeats, || {
                let started = Instant::now();
                for i in 0..iterations {
                    let (mut regions, _) = black_box(&fixture).open();
                    if !slots.is_empty() {
                        regions.set(slots[i as usize % slots.len()], Value::Int(-1));
                    }
                    black_box(regions);
                }
                started.elapsed() / iterations
            });

            let rebuild = best_of(repeats, || {
                let started = Instant::now();
                black_box(seeded(cells));
                started.elapsed()
            });

            FixturePoint {
                cells,
                open_nanos: nanos(open),
                open_and_write_nanos: nanos(open_write),
                rebuild_nanos: nanos(rebuild),
                rebuild_over_open: rebuild.as_secs_f64() / open.as_secs_f64(),
                rebuild_over_open_and_write: rebuild.as_secs_f64() / open_write.as_secs_f64(),
            }
        })
        .collect()
}

#[derive(Clone, Debug, Serialize)]
pub struct Resumptions {
    pub resumptions: usize,
    pub micros: f64,
    /// What this resumption added over the previous count.
    pub marginal_micros: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct StackPoint {
    pub pending_frames: usize,
    pub segments: usize,
    pub capture_nanos: f64,
    pub resume_nanos: f64,
    /// What `Continuation::frames` reports, to show the frames really were pending and the capture
    /// still did not walk them.
    pub captured_frames: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct MultiShot {
    pub resumptions: Vec<Resumptions>,
    pub stack: Vec<StackPoint>,
}

/// A handler resuming a fixed residual computation a varying number of times.
const MULTISHOT_SRC: &str = r#"
effect amb {
  read pick[coin]() -> Int
}

fn residual(b: Int) -> Int = fold(range(0, 200), 0, |acc, x| acc + x + b)

fn r0() -> Int = handle { residual(amb.pick[coin]()) } with {
  amb.pick[coin]() resume k -> 7,
  return x -> x
}

fn r1() -> Int = handle { residual(amb.pick[coin]()) } with {
  amb.pick[coin]() resume k -> k(1),
  return x -> x
}

fn r2() -> Int = handle { residual(amb.pick[coin]()) } with {
  amb.pick[coin]() resume k -> k(1) + k(2),
  return x -> x
}

fn r4() -> Int = handle { residual(amb.pick[coin]()) } with {
  amb.pick[coin]() resume k -> k(1) + k(2) + k(3) + k(4),
  return x -> x
}

test "the shapes all evaluate" {
  assert_eq(r0(), 7);
  assert(r1() < r2())
}
"#;

fn load(name: &str, src: &str) -> Result<(Program, Resolved)> {
    let mut map = SourceMap::new();
    let id: SourceId = map.add(format!("{name}.ply"), src.to_string());
    let mut program = parse_program([(id, ModuleName::from_dotted(name), src)])
        .map_err(|ds| anyhow::anyhow!("the measurement program must parse: {ds:#?}"))?;
    let resolved =
        resolve(&mut program).map_err(|ds| anyhow::anyhow!("it must also resolve: {ds:#?}"))?;
    Ok((program, resolved))
}

pub fn multi_shot(repeats: usize) -> Result<MultiShot> {
    let (program, resolved) = load("multishot", MULTISHOT_SRC)?;
    let mut machine = Machine::for_program(&program, &resolved);

    let mut rows: Vec<Resumptions> = Vec::new();
    for (count, name) in [(0usize, "r0"), (1, "r1"), (2, "r2"), (4, "r4")] {
        let qualified = format!("multishot.{name}");
        // One call outside the clock, so lazy lowering is not charged to the first row and read as
        // a cost of resuming zero times.
        machine
            .call(&qualified, Vec::new(), Span::DUMMY)
            .map_err(|d| anyhow::anyhow!("`{qualified}` failed: {}", d.message))?;

        const CALLS: u32 = 200;
        let taken = best_of(repeats, || {
            let started = Instant::now();
            for _ in 0..CALLS {
                let _ = black_box(machine.call(&qualified, Vec::new(), Span::DUMMY));
            }
            started.elapsed() / CALLS
        });
        let micros = taken.as_secs_f64() * 1e6;
        let marginal = match rows.last() {
            Some(previous) => (micros - previous.micros) / (count - previous.resumptions) as f64,
            None => 0.0,
        };
        rows.push(Resumptions {
            resumptions: count,
            micros,
            marginal_micros: marginal,
        });
    }

    Ok(MultiShot {
        resumptions: rows,
        stack: stack_cost(repeats),
    })
}

fn empty_prompt() -> Rc<Prompt> {
    Rc::new(Prompt {
        clauses: Rc::new(Vec::new()),
        effects: Rc::new(Vec::new()),
        ret: None,
        env: Env::empty(),
        module: 0,
        span: Span::DUMMY,
    })
}

/// `Stack::capture` and `Stack::resume` against the number of frames pending inside the captured
/// segment.
fn stack_cost(repeats: usize) -> Vec<StackPoint> {
    [8usize, 1_000, 100_000]
        .into_iter()
        .map(|pending| {
            let mut stack = Stack::new().push_prompt(empty_prompt());
            for _ in 0..pending {
                stack = stack.push(Frame::Call {
                    name: None,
                    call_site: Span::DUMMY,
                    memo: false,
                });
            }
            let (k, below) = stack.capture(1, 0);

            let iterations = 100_000;
            let capture = best_of(repeats, || {
                let started = Instant::now();
                for _ in 0..iterations {
                    black_box(black_box(&stack).capture(1, 0));
                }
                started.elapsed() / iterations
            });
            let splice = best_of(repeats, || {
                let started = Instant::now();
                for _ in 0..iterations {
                    black_box(black_box(&below).resume(&k));
                }
                started.elapsed() / iterations
            });

            StackPoint {
                pending_frames: pending,
                segments: k.segments(),
                capture_nanos: nanos(capture),
                resume_nanos: nanos(splice),
                captured_frames: k.frames(),
            }
        })
        .collect()
}

#[derive(Clone, Debug, Serialize)]
pub struct Scheduling {
    pub root: String,
    pub tests: usize,
    pub isolated: usize,
    pub shared: usize,
    /// Groups over every test, which is what a cold run schedules.
    pub groups: usize,
    /// Groups the shared tests need on their own.
    pub shared_groups: usize,
    pub largest_group: usize,
    pub smallest_group: usize,
    /// Tests in the largest group over tests scheduled: the fraction that runs in one concurrent
    /// wave.
    pub largest_group_share: f64,
}

/// A cold run selects every test, so the schedule is a function of the corpus alone and the cache
/// need not be touched — which matters, because clearing it to observe a cold schedule would
/// destroy the state `store_open` measures.
pub fn scheduling(root: &Path) -> Result<Scheduling> {
    let front = front(root)?;
    let footprints: Vec<Footprint> = front
        .check
        .tests
        .iter()
        .map(|t| t.footprint.clone())
        .collect();
    let scheduled: Vec<(usize, Footprint)> = footprints.iter().cloned().enumerate().collect();

    let groups = ply_test::group_by_conflict(&scheduled);
    let parallelism = ply_test::parallelism(&footprints, &scheduled, &groups);
    let sizes: Vec<usize> = groups.iter().map(|g| g.len()).collect();

    Ok(Scheduling {
        root: root.display().to_string(),
        tests: parallelism.total,
        isolated: parallelism.isolated,
        shared: parallelism.shared,
        groups: parallelism.groups,
        shared_groups: parallelism.shared_groups,
        largest_group: sizes.iter().copied().max().unwrap_or(0),
        smallest_group: sizes.iter().copied().min().unwrap_or(0),
        largest_group_share: match scheduled.len() {
            0 => 0.0,
            n => sizes.iter().copied().max().unwrap_or(0) as f64 / n as f64,
        },
    })
}

#[derive(Clone, Debug, Serialize)]
pub struct StoreOpen {
    pub root: String,
    pub millis: f64,
    pub results: usize,
    pub definitions_seen: usize,
    pub index_bytes: u64,
    pub data_bytes: u64,
    pub results_bytes: u64,
}

/// `Store::open` against a cache the corpus has already filled.
pub fn store_open(root: &Path, repeats: usize) -> Result<StoreOpen> {
    let store = ply_store::Store::open(root).context("opening the cache")?;
    let stats = store.stats();
    if stats.results == 0 {
        bail!(
            "`{}` has an empty result cache; run the corpus before timing `Store::open`",
            root.display()
        );
    }
    drop(store);

    let taken = best_of(repeats, || {
        let started = Instant::now();
        black_box(ply_store::Store::open(root).ok());
        started.elapsed()
    });

    Ok(StoreOpen {
        root: root.display().to_string(),
        millis: millis(taken),
        results: stats.results,
        definitions_seen: stats.definitions_seen,
        index_bytes: stats.index_bytes,
        data_bytes: stats.data_bytes,
        results_bytes: stats.results_bytes,
    })
}

#[derive(Clone, Debug, Serialize)]
pub struct Measurements {
    pub throughput: Option<Throughput>,
    pub scheduling: Option<Scheduling>,
    pub store_open: Option<StoreOpen>,
    pub fixture: Vec<FixturePoint>,
    pub multi_shot: Option<MultiShot>,
}

pub fn render(m: &Measurements) -> String {
    let mut s = String::new();

    if let Some(t) = &m.throughput {
        s.push_str(&format!(
            "throughput — {} definitions, {} tests, one worker, one thread\n",
            t.definitions, t.tests
        ));
        s.push_str(&format!(
            "  {:>12} {:>12} {:>12} {:>12}\n",
            "setup ms", "1st pass", "steady", "performs"
        ));
        s.push_str(&format!(
            "  {:>12.2} {:>12.2} {:>12.2} {:>12}\n",
            t.pass.worker_setup_millis,
            t.pass.first_pass_millis,
            t.pass.steady_pass_millis,
            t.pass.performs
        ));
        s.push_str(&format!(
            "  lowering every test body once: {:.2} ms\n",
            t.lower_test_bodies_millis
        ));
        s.push('\n');
    }

    if !m.fixture.is_empty() {
        s.push_str("fixture — one seeded region stack, per operation\n");
        s.push_str(&format!(
            "  {:>9} {:>12} {:>14} {:>14} {:>12}\n",
            "cells", "open ns", "open+write ns", "rebuild ns", "rebuild/open"
        ));
        for p in &m.fixture {
            s.push_str(&format!(
                "  {:>9} {:>12.1} {:>14.1} {:>14.1} {:>11.0}x\n",
                p.cells, p.open_nanos, p.open_and_write_nanos, p.rebuild_nanos, p.rebuild_over_open
            ));
        }
        s.push('\n');
    }

    if let Some(ms) = &m.multi_shot {
        s.push_str("multi-shot — one handler, one residual computation\n");
        s.push_str(&format!(
            "  {:>12} {:>12} {:>16}\n",
            "resumptions", "us/call", "marginal us"
        ));
        for r in &ms.resumptions {
            s.push_str(&format!(
                "  {:>12} {:>12.2} {:>16.2}\n",
                r.resumptions, r.micros, r.marginal_micros
            ));
        }
        s.push_str(&format!(
            "  {:>12} {:>10} {:>14} {:>14}\n",
            "pending", "segments", "capture ns", "resume ns"
        ));
        for p in &ms.stack {
            s.push_str(&format!(
                "  {:>12} {:>10} {:>14.1} {:>14.1}\n",
                p.pending_frames, p.segments, p.capture_nanos, p.resume_nanos
            ));
        }
        s.push('\n');
    }

    if let Some(sc) = &m.scheduling {
        s.push_str(&format!(
            "scheduling — {} tests: {} region-isolated, {} shared\n",
            sc.tests, sc.isolated, sc.shared
        ));
        s.push_str(&format!(
            "  {} group(s); shared tests alone need {}; largest {} ({:.1}% of the run), smallest {}\n\n",
            sc.groups,
            sc.shared_groups,
            sc.largest_group,
            sc.largest_group_share * 100.0,
            sc.smallest_group
        ));
    }

    if let Some(so) = &m.store_open {
        s.push_str(&format!(
            "Store::open — {:.2} ms over {} results, {} definitions seen, {} KiB index + {} KiB data + {} KiB results\n\n",
            so.millis,
            so.results,
            so.definitions_seen,
            so.index_bytes / 1024,
            so.data_bytes / 1024,
            so.results_bytes / 1024
        ));
    }

    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::generate;
    use crate::spec::CorpusSpec;
    use crate::write::write;

    fn corpus_at(root: &Path) {
        let spec = CorpusSpec {
            seed: 4,
            modules: 5,
            defs_per_module: 6,
            tests: 10,
            depth: 2,
            ..CorpusSpec::default()
        };
        write(root, &spec, &generate(&spec)).unwrap();
    }

    #[test]
    fn a_pass_reports_what_it_ran() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        corpus_at(&root);

        let t = throughput(&root, 1).unwrap();
        assert!(t.pass.steady_pass_millis > 0.0);
        assert!(t.pass.performs > 0, "the corpus performed no atom");
        assert!(t.lower_test_bodies_millis > 0.0);
    }

    /// The machine lowers on first call, so setup must not be read as interpreter speed.
    #[test]
    fn setup_is_reported_apart_from_evaluation() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        corpus_at(&root);

        let t = throughput(&root, 9).unwrap();
        assert!(
            t.pass.first_pass_millis >= t.pass.steady_pass_millis * 0.5,
            "a first pass of {} ms against a steady {} ms",
            t.pass.first_pass_millis,
            t.pass.steady_pass_millis
        );
    }

    /// The forkable world's version of this asserted that a fork of a 10,000-cell fixture cost what
    /// a fork of a one-cell fixture did.
    #[test]
    fn opening_a_fixture_beats_rebuilding_it_once_the_fixture_is_real() {
        let points = fixture_cost(&[1, 10_000], 3);
        assert_eq!(points.len(), 2);
        for p in &points {
            println!(
                "  {:>6} cells: open {:>10.1} ns, rebuild {:>10.1} ns ({:.2}x)",
                p.cells, p.open_nanos, p.rebuild_nanos, p.rebuild_over_open
            );
        }
        assert!(
            points[1].rebuild_over_open > 1.0,
            "opening a 10,000-cell fixture cost {} ns against {} ns to rebuild it",
            points[1].open_nanos,
            points[1].rebuild_nanos
        );
        assert!(
            points[1].open_nanos > points[0].open_nanos,
            "an open is O(the fixture); a 10,000-cell one cost {} ns against {} ns for one cell",
            points[1].open_nanos,
            points[0].open_nanos
        );
    }

    /// If `capture` walked the segment it cut, the 100,000-frame row would cost four orders of
    /// magnitude more than the 8-frame one.
    #[test]
    fn capture_and_resume_are_flat_in_the_frames_they_move() {
        let points = stack_cost(3);
        let small = points.first().expect("a first row");
        let large = points.last().expect("a last row");
        assert_eq!(large.captured_frames, 100_000);
        assert_eq!(large.segments, 1);
        let flat = |slow: f64, fast: f64| slow < fast * 100.0 + 500.0;
        assert!(
            flat(large.capture_nanos, small.capture_nanos),
            "capturing {} frames cost {} ns against {} ns for {}",
            large.pending_frames,
            large.capture_nanos,
            small.capture_nanos,
            small.pending_frames
        );
        assert!(
            flat(large.resume_nanos, small.resume_nanos),
            "splicing {} frames cost {} ns against {} ns for {}",
            large.pending_frames,
            large.resume_nanos,
            small.resume_nanos,
            small.pending_frames
        );
    }

    #[test]
    fn every_resumption_costs_about_what_the_first_one_did() {
        let ms = multi_shot(3).unwrap();
        let one = ms
            .resumptions
            .iter()
            .find(|r| r.resumptions == 1)
            .expect("a one-resumption row");
        let four = ms
            .resumptions
            .iter()
            .find(|r| r.resumptions == 4)
            .expect("a four-resumption row");
        assert!(
            four.marginal_micros < one.micros * 2.0,
            "the fourth resumption cost {} us against {} us for a whole one-resumption call",
            four.marginal_micros,
            one.micros
        );
    }

    #[test]
    fn scheduling_reports_the_group_count_the_shared_tests_alone_need() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        corpus_at(&root);

        let s = scheduling(&root).unwrap();
        assert_eq!(s.isolated + s.shared, s.tests);
        assert_eq!(s.groups, s.shared_groups.max(1));
    }
}
