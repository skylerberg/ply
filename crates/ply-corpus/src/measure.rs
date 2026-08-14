//! The four numbers ADR 0005 spends and never priced.
//!
//! [`crate::bench`] reports a whole run's phase split, which answers "did the
//! milestone cost anything" and not "where". These measure the machine's own
//! claims one at a time:
//!
//! - **throughput** — the two engines on one corpus, with per-worker setup
//!   separated from evaluation. `ply-test` builds a worker per rayon thread per
//!   concurrency group, so a setup cost proportional to the program is charged
//!   many times over and would otherwise be read as interpreter speed.
//! - **fork** — `World::fork` against rebuilding the same fixture, which is the
//!   ratio "build a fixture once, fork per test" is a claim about.
//! - **multi-shot** — resuming a continuation zero, one, two and four times,
//!   beside direct measurements of `Stack::capture` and `Stack::resume` at
//!   growing pending-frame counts. The end-to-end number cannot separate the
//!   residual computation from the splice; the microbenchmarks can.
//! - **scheduling** — how much of a corpus is trivially parallel, and what the
//!   colouring of the remainder costs in groups.

use crate::pipeline::{Front, front};
use anyhow::{Context, Result, bail};
use ply_core::Footprint;
use ply_eval::cont::{Frame, Prompt, Stack};
use ply_eval::{CellId, Engine, Env, Evaluator, Interp, Machine, Value, World};
use ply_span::{SourceId, SourceMap, Span};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::parse_program;
use ply_syntax::resolve::{Resolved, resolve};
use serde::Serialize;
use std::hint::black_box;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// The fastest of several attempts. A slower one only ever means the machine
/// did something else as well, so a minimum is the least noisy estimator
/// available without a statistics dependency.
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

// ---------------------------------------------------------------- throughput

#[derive(Clone, Debug, Serialize)]
pub struct EnginePass {
    pub engine: String,
    /// Constructing one worker: what `ply-test` pays per rayon thread per
    /// concurrency group.
    pub worker_setup_millis: f64,
    /// Every test once on a freshly built worker — setup, plus whatever the
    /// engine defers to first call.
    pub first_pass_millis: f64,
    /// Every test again on the same worker. Nothing is left to warm, so this is
    /// the interpreter alone — and it is the same worker's life as
    /// `first_pass_millis`, so the two are comparable.
    pub steady_pass_millis: f64,
    pub tests: usize,
    /// Atoms performed in one pass, from the tracer. The two engines must agree
    /// on it or they were not running the same program.
    pub performs: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Throughput {
    pub root: String,
    pub definitions: usize,
    pub tests: usize,
    pub engines: Vec<EnginePass>,
    /// `lower` over every test body once. `Machine::eval_test` lowers the body
    /// it is about to run on every call and caches nothing, so this is a floor
    /// on the machine's pass that no amount of interpreter speed removes.
    pub lower_test_bodies_millis: f64,
    /// Machine over tree-walker on the steady pass: the interpreter ratio with
    /// per-worker setup excluded. `None` when only one engine was asked for.
    pub steady_ratio: Option<f64>,
    /// Machine over tree-walker on a fresh worker, which is what a real run
    /// charges.
    pub first_pass_ratio: Option<f64>,
}

pub fn throughput(root: &Path, engines: &[Engine], repeats: usize) -> Result<Throughput> {
    let front = front(root)?;
    let mut passes = Vec::new();
    for &engine in engines {
        passes.push(one_engine(&front, engine, repeats)?);
    }

    let by = |e: Engine| passes.iter().find(|p| p.engine == e.as_str());
    let (tree, machine) = (by(Engine::Treewalk), by(Engine::Machine));
    if let (Some(tree), Some(machine)) = (tree, machine)
        && tree.performs != machine.performs
    {
        bail!(
            "the engines performed {} and {} atoms, so they did not run the same program",
            tree.performs,
            machine.performs
        );
    }
    let ratio = |f: fn(&EnginePass) -> f64| match (tree, machine) {
        (Some(t), Some(m)) if f(t) > 0.0 => Some(f(m) / f(t)),
        _ => None,
    };

    Ok(Throughput {
        root: root.display().to_string(),
        definitions: front.check.defs.len(),
        tests: front.check.tests.len(),
        lower_test_bodies_millis: millis(lower_every_test_body(&front, repeats)),
        steady_ratio: ratio(|p| p.steady_pass_millis),
        first_pass_ratio: ratio(|p| p.first_pass_millis),
        engines: passes,
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

fn one_engine(front: &Front, engine: Engine, repeats: usize) -> Result<EnginePass> {
    fn build<'a>(front: &'a Front, engine: Engine) -> Box<dyn Evaluator + 'a> {
        match engine {
            Engine::Treewalk => {
                Box::new(Interp::new(&front.program, &front.resolved, &front.check))
            }
            Engine::Machine => {
                Box::new(Machine::new(&front.program, &front.resolved, &front.check))
            }
        }
    }

    let setup = best_of(repeats, || {
        let started = Instant::now();
        black_box(build(front, engine));
        started.elapsed()
    });

    // Both passes come from the least disturbed attempt, as a pair. Minimizing
    // them apart reports one worker's first pass against another's steady pass,
    // which is a ratio no worker ever produced.
    let mut performs = 0u64;
    let mut best: Option<(Duration, Duration)> = None;
    for _ in 0..repeats.max(1) {
        let mut worker = build(front, engine);
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

    Ok(EnginePass {
        engine: engine.as_str().to_string(),
        worker_setup_millis: millis(setup),
        first_pass_millis: millis(first),
        steady_pass_millis: millis(steady),
        tests: front.check.tests.len(),
        performs,
    })
}

/// Every test once, returning the atoms performed across the pass. A failure is
/// an error rather than a number, because a corpus that stopped passing is not
/// a corpus whose speed means anything.
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

// ---------------------------------------------------------------------- fork

#[derive(Clone, Debug, Serialize)]
pub struct ForkPoint {
    pub cells: usize,
    pub fork_nanos: f64,
    /// A fork a test then writes to, which is the path that pays for the
    /// persistent map's copy-on-write.
    pub fork_and_write_nanos: f64,
    /// Building the same world by running the setup again — the alternative to
    /// forking, and the denominator of the claim.
    pub rebuild_nanos: f64,
    pub rebuild_over_fork: f64,
    pub rebuild_over_fork_and_write: f64,
}

/// Records rather than integers, so a copy is a copy of something.
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

pub fn fork_cost(sizes: &[usize], repeats: usize) -> Vec<ForkPoint> {
    sizes
        .iter()
        .map(|&cells| {
            let (world, ids) = seeded(cells);
            // Enough iterations that a nanosecond-scale operation is not being
            // read off the clock's own resolution.
            let iterations = 100_000;

            let fork = best_of(repeats, || {
                let started = Instant::now();
                for _ in 0..iterations {
                    black_box(black_box(&world).fork());
                }
                started.elapsed() / iterations
            });

            let fork_write = best_of(repeats, || {
                let started = Instant::now();
                for i in 0..iterations {
                    let mut forked = black_box(&world).fork();
                    forked.set(ids[i as usize % ids.len()], Value::Int(-1));
                    black_box(forked);
                }
                started.elapsed() / iterations
            });

            let rebuild = best_of(repeats, || {
                let started = Instant::now();
                black_box(seeded(cells));
                started.elapsed()
            });

            ForkPoint {
                cells,
                fork_nanos: nanos(fork),
                fork_and_write_nanos: nanos(fork_write),
                rebuild_nanos: nanos(rebuild),
                rebuild_over_fork: rebuild.as_secs_f64() / fork.as_secs_f64(),
                rebuild_over_fork_and_write: rebuild.as_secs_f64() / fork_write.as_secs_f64(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------- multi-shot

#[derive(Clone, Debug, Serialize)]
pub struct Resumptions {
    pub resumptions: usize,
    pub micros: f64,
    /// What this resumption added over the previous count. Constant marginal
    /// cost is the whole claim: a splice that copied the captured frames would
    /// make each one dearer than the last.
    pub marginal_micros: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct StackPoint {
    pub pending_frames: usize,
    pub segments: usize,
    pub capture_nanos: f64,
    pub resume_nanos: f64,
    /// What `Continuation::frames` reports, to show the frames really were
    /// pending and the capture still did not walk them.
    pub captured_frames: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct MultiShot {
    pub resumptions: Vec<Resumptions>,
    pub stack: Vec<StackPoint>,
}

/// A handler resuming a fixed residual computation a varying number of times.
/// The residual is the same in every case, so the difference between two rows
/// is exactly one more resumption.
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
    let program = parse_program([(id, ModuleName::from_dotted(name), src)])
        .map_err(|ds| anyhow::anyhow!("the measurement program must parse: {ds:#?}"))?;
    let resolved =
        resolve(&program).map_err(|ds| anyhow::anyhow!("it must also resolve: {ds:#?}"))?;
    Ok((program, resolved))
}

pub fn multi_shot(repeats: usize) -> Result<MultiShot> {
    let (program, resolved) = load("multishot", MULTISHOT_SRC)?;
    let mut machine = Machine::for_program(&program, &resolved);

    let mut rows: Vec<Resumptions> = Vec::new();
    for (count, name) in [(0usize, "r0"), (1, "r1"), (2, "r2"), (4, "r4")] {
        let qualified = format!("multishot.{name}");
        // One call outside the clock, so lazy lowering is not charged to the
        // first row and read as a cost of resuming zero times.
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

/// `Stack::capture` and `Stack::resume` against the number of frames pending
/// inside the captured segment. Flat is the claim; growth would mean the
/// segment is being walked.
fn stack_cost(repeats: usize) -> Vec<StackPoint> {
    [8usize, 1_000, 100_000]
        .into_iter()
        .map(|pending| {
            let mut stack = Stack::new().push_prompt(empty_prompt());
            for _ in 0..pending {
                stack = stack.push(Frame::Call {
                    name: None,
                    call_site: Span::DUMMY,
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

// ---------------------------------------------------------------- scheduling

#[derive(Clone, Debug, Serialize)]
pub struct Scheduling {
    pub root: String,
    pub tests: usize,
    pub isolated: usize,
    pub shared: usize,
    /// Groups over every test, which is what a cold run schedules.
    pub groups: usize,
    /// Groups the shared tests need on their own. The two are equal, which is
    /// ADR 0005 §5's property stated as a measurement.
    pub shared_groups: usize,
    pub largest_group: usize,
    pub smallest_group: usize,
    /// Tests in the largest group over tests scheduled: the fraction that runs
    /// in one concurrent wave.
    pub largest_group_share: f64,
}

/// A cold run selects every test, so the schedule is a function of the corpus
/// alone and the cache need not be touched — which matters, because clearing it
/// to observe a cold schedule would destroy the state `store_open` measures.
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

// --------------------------------------------------------------- store::open

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

/// `Store::open` against a cache the corpus has already filled. ADR 0003's
/// budget is 5 ms at 10,000 definitions and it covers both caches, so the
/// measurement is only meaningful on a root a full run has been through.
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

// -------------------------------------------------------------------- report

#[derive(Clone, Debug, Serialize)]
pub struct Measurements {
    pub throughput: Option<Throughput>,
    pub scheduling: Option<Scheduling>,
    pub store_open: Option<StoreOpen>,
    pub fork: Vec<ForkPoint>,
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
            "  {:<10} {:>12} {:>12} {:>12} {:>12}\n",
            "engine", "setup ms", "1st pass", "steady", "performs"
        ));
        for e in &t.engines {
            s.push_str(&format!(
                "  {:<10} {:>12.2} {:>12.2} {:>12.2} {:>12}\n",
                e.engine,
                e.worker_setup_millis,
                e.first_pass_millis,
                e.steady_pass_millis,
                e.performs
            ));
        }
        s.push_str(&format!(
            "  lowering every test body once: {:.2} ms\n",
            t.lower_test_bodies_millis
        ));
        if let (Some(steady), Some(first)) = (t.steady_ratio, t.first_pass_ratio) {
            s.push_str(&format!(
                "  machine / treewalk: {steady:.2}x steady, {first:.2}x on a fresh worker\n"
            ));
        }
        s.push('\n');
    }

    if !m.fork.is_empty() {
        s.push_str("fork — one seeded world, per operation\n");
        s.push_str(&format!(
            "  {:>9} {:>12} {:>14} {:>14} {:>12}\n",
            "cells", "fork ns", "fork+write ns", "rebuild ns", "rebuild/fork"
        ));
        for p in &m.fork {
            s.push_str(&format!(
                "  {:>9} {:>12.1} {:>14.1} {:>14.1} {:>11.0}x\n",
                p.cells, p.fork_nanos, p.fork_and_write_nanos, p.rebuild_nanos, p.rebuild_over_fork
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
            "scheduling — {} tests: {} world-isolated, {} shared\n",
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
    fn both_engines_run_the_same_program_and_are_reported_separately() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        corpus_at(&root);

        let t = throughput(&root, &[Engine::Treewalk, Engine::Machine], 1).unwrap();
        assert_eq!(t.engines.len(), 2);
        // `throughput` refuses a mismatch, so reaching here already proves the
        // two agreed; asserting it keeps the reason visible.
        assert_eq!(t.engines[0].performs, t.engines[1].performs);
        assert!(t.engines.iter().all(|e| e.steady_pass_millis > 0.0));
        assert!(t.steady_ratio.is_some_and(|r| r > 0.0));
        assert!(t.lower_test_bodies_millis > 0.0);
    }

    /// A ratio between two engines is meaningless when only one ran, and a
    /// silent `1.0` would read as parity.
    #[test]
    fn one_engine_reports_no_ratio() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        corpus_at(&root);

        let t = throughput(&root, &[Engine::Machine], 1).unwrap();
        assert_eq!(t.engines.len(), 1);
        assert!(t.steady_ratio.is_none());
        assert!(t.first_pass_ratio.is_none());
    }

    /// The tree-walker deep-clones every body per worker and the machine lowers
    /// on first call, so setup must not be read as interpreter speed. This is
    /// the reason the two passes are separated at all.
    ///
    /// The tree-walker defers nothing to first call, so its two passes measure
    /// the same work and the bound below is what catches one of them measuring
    /// something else. Several repeats, because a pass here is under a
    /// millisecond and this test runs beside every other one in the crate.
    #[test]
    fn setup_is_reported_apart_from_evaluation() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        corpus_at(&root);

        let t = throughput(&root, &[Engine::Treewalk, Engine::Machine], 9).unwrap();
        for e in &t.engines {
            assert!(
                e.first_pass_millis >= e.steady_pass_millis * 0.5,
                "{} reported a first pass of {} ms against a steady {} ms",
                e.engine,
                e.first_pass_millis,
                e.steady_pass_millis
            );
        }
    }

    #[test]
    fn a_fork_does_not_get_dearer_as_the_world_grows() {
        let points = fork_cost(&[1, 10_000], 3);
        assert_eq!(points.len(), 2);
        assert!(
            points[1].fork_nanos < points[0].fork_nanos * 20.0 + 100.0,
            "forking a 10,000-cell world cost {} ns against {} ns for one cell",
            points[1].fork_nanos,
            points[0].fork_nanos
        );
        assert!(
            points[1].rebuild_over_fork > 1000.0,
            "rebuilding a 10,000-cell fixture was only {}x a fork",
            points[1].rebuild_over_fork
        );
    }

    /// If `capture` walked the segment it cut, the 100,000-frame row would cost
    /// four orders of magnitude more than the 8-frame one.
    #[test]
    fn capture_and_resume_are_flat_in_the_frames_they_move() {
        let points = stack_cost(3);
        let small = points.first().expect("a first row");
        let large = points.last().expect("a last row");
        assert_eq!(large.captured_frames, 100_000);
        assert_eq!(large.segments, 1);
        assert!(
            large.capture_nanos < small.capture_nanos * 4.0 + 50.0,
            "capturing {} frames cost {} ns against {} ns for {}",
            large.pending_frames,
            large.capture_nanos,
            small.capture_nanos,
            small.pending_frames
        );
        assert!(
            large.resume_nanos < small.resume_nanos * 4.0 + 50.0,
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
