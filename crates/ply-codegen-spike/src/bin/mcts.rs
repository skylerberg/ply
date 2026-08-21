//! ADR 0018 §1 — re-pricing ADR 0016 §3's codegen spike against a compute kernel.
//!
//!     mcts [--dir benches/kernel] [--iterations N] [--repeats R] [--out mcts.json]
//!
//! ADR 0016 measured the spike on `std.http.read_line` and concluded 1.02–1.05×
//! end to end, because the fragment it compiles is 2–5% of an HTTP request. ADR
//! 0018 is ordered on the assumption that an MCTS inner loop is *almost
//! entirely* that fragment, and says in as many words that §1 exists to test
//! the assumption before anything is built.
//!
//! So this binary compiles a real MCTS kernel — `benches/kernel/mcts.ply`,
//! three-heap Nim, bit-packed positions, UCB1 in integer fixed point, bounded
//! playouts — and reports four things:
//!
//!   1. what fraction of the kernel the fragment accepts, statically and by
//!      executed work;
//!   2. what compiling the accepted part is worth **end to end**, not on the
//!      kernel;
//!   3. what is outside the fragment, ranked by the work it carries;
//!   4. that the compiled code answers what both interpreters answer, on
//!      generated inputs, before any of the above is timed.
//!
//! Nothing here reads a figure out of a document. Every number is taken in this
//! process, in this run.

use anyhow::{Result, bail};
use ply_codegen_spike::jit::{Jit, Opts, node_count};
use ply_codegen_spike::measure::Harness;
use ply_codegen_spike::program::Loaded;
use ply_eval::{Interp, Value, lower, values_equal};
use ply_span::Span;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

const KERNEL: &str = "mcts";
/// The nullary probe both sides' entry costs are read off.
const ENTRY: &str = "work.zero";
/// The whole kernel behind one name: build a tree, search it, answer a move.
const WHOLE: &str = "mcts.plan_753";
/// The same kernel with the tree removed — playouts and nothing else.
const PURE: &str = "mcts.playouts";
/// The instrumented walk that counts the work one search does.
const WORK: &str = "work.work_753";
/// The playout length of one (state, seed), so a per-call cost can be turned
/// into a per-ply cost and weighted by a real ply count.
const PLIES: &str = "work.plies";

struct Args {
    dir: PathBuf,
    iterations: i64,
    repeats: u32,
    inner: u32,
    out: Option<String>,
    probe: Option<String>,
}

fn parse_args() -> Result<Args> {
    let mut a = Args {
        dir: PathBuf::from("benches/kernel"),
        iterations: 200,
        repeats: 7,
        inner: 3,
        out: None,
        probe: None,
    };
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--dir" => a.dir = PathBuf::from(argv.next().unwrap_or_default()),
            "--iterations" => a.iterations = argv.next().unwrap_or_default().parse()?,
            "--repeats" => a.repeats = argv.next().unwrap_or_default().parse()?,
            "--inner" => a.inner = argv.next().unwrap_or_default().parse()?,
            "--out" => a.out = argv.next(),
            "--probe" => a.probe = argv.next(),
            other => bail!("unknown argument: {other}"),
        }
    }
    Ok(a)
}

/// Best and worst per-call microseconds over `repeats` runs of `inner` calls.
/// Best-of because the quantity of interest is the cost of the work; worst-of
/// because a ratio between two best-of numbers is a claim the noise has not
/// been asked about. The same rule `measure::band` follows.
#[derive(Clone, Copy, Serialize)]
struct Band {
    best: f64,
    worst: f64,
}

fn band(mut run: impl FnMut() -> Result<()>, inner: u32, repeats: u32) -> Result<Band> {
    let mut best = f64::INFINITY;
    let mut worst: f64 = 0.0;
    for _ in 0..repeats {
        let started = Instant::now();
        for _ in 0..inner {
            run()?;
        }
        let taken = started.elapsed().as_secs_f64() * 1e6 / f64::from(inner);
        best = best.min(taken);
        worst = worst.max(taken);
    }
    Ok(Band { best, worst })
}

/// A ratio taken **inside** one repeat rather than between two best-of numbers.
///
/// This machine is shared with other work and its load average moved between
/// 15 and 47 while these numbers were taken, so a ratio between two separately
/// timed loops is a ratio between two different machines. Timing both sides in
/// the same window and reporting the median of the per-window ratios is the
/// only form of this comparison that survives that.
#[derive(Clone, Serialize)]
struct Paired {
    a: Band,
    b: Band,
    ratios: Vec<f64>,
    median: f64,
    low: f64,
    high: f64,
}

fn paired(
    mut a: impl FnMut() -> Result<()>,
    mut b: impl FnMut() -> Result<()>,
    inner: u32,
    repeats: u32,
) -> Result<Paired> {
    let mut ratios = Vec::new();
    let (mut abest, mut aworst) = (f64::INFINITY, 0.0f64);
    let (mut bbest, mut bworst) = (f64::INFINITY, 0.0f64);
    for _ in 0..repeats {
        let t0 = Instant::now();
        for _ in 0..inner {
            a()?;
        }
        let ta = t0.elapsed().as_secs_f64() * 1e6 / f64::from(inner);
        let t1 = Instant::now();
        for _ in 0..inner {
            b()?;
        }
        let tb = t1.elapsed().as_secs_f64() * 1e6 / f64::from(inner);
        abest = abest.min(ta);
        aworst = aworst.max(ta);
        bbest = bbest.min(tb);
        bworst = bworst.max(tb);
        ratios.push(ta / tb);
    }
    let mut sorted = ratios.clone();
    sorted.sort_by(|x, y| x.partial_cmp(y).unwrap());
    Ok(Paired {
        a: Band {
            best: abest,
            worst: aworst,
        },
        b: Band {
            best: bbest,
            worst: bworst,
        },
        median: sorted[sorted.len() / 2],
        low: sorted[0],
        high: sorted[sorted.len() - 1],
        ratios,
    })
}

#[derive(Serialize)]
struct CensusRow {
    function: String,
    nodes: usize,
    accepted: bool,
    refused_because: Option<String>,
}

fn census(loaded: &'static Loaded, module: &str) -> Vec<CensusRow> {
    let mut rows = Vec::new();
    for name in loaded.functions_in(module) {
        let (def, _) = loaded.definition(&name).expect("it was just listed");
        let nodes = node_count(&lower(&def.body));
        let (accepted, why) = match Jit::compile(loaded, &[&name]) {
            Ok(_) => (true, None),
            Err(e) => {
                let text = e.to_string();
                let reason = text
                    .rsplit_once(": ")
                    .map(|(_, r)| r.to_string())
                    .unwrap_or(text);
                (false, Some(reason))
            }
        };
        rows.push(CensusRow {
            function: name,
            nodes,
            accepted,
            refused_because: why,
        });
    }
    rows
}

/// Whether every parameter is written `Int` or `Bool` — the shapes a generic
/// fuzz can supply without knowing what the function means.
fn scalar_params(loaded: &Loaded, name: &str) -> Option<Vec<&'static str>> {
    let (def, _) = loaded.definition(name)?;
    let mut out = Vec::new();
    for p in &def.params {
        let ply_syntax::ast::TypeExpr::Con { name, args, .. } = p.ty.as_ref()? else {
            return None;
        };
        if !args.is_empty() {
            return None;
        }
        match name.symbol().as_str() {
            "Int" => out.push("Int"),
            "Bool" => out.push("Bool"),
            _ => return None,
        }
    }
    Some(out)
}

/// A 64-bit xorshift, so the generated inputs are the same on every machine and
/// a disagreement is reproducible from the seed printed beside it.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// Small integers with the edges the kernel actually meets: zero, one, a
    /// heap size, a packed position, a short fuel, a negative.
    ///
    /// Bounded at 512 on purpose. Three of the kernel's functions recurse to a
    /// depth an *argument* names, and compiled code carries no equivalent of
    /// the machine's nested-call bound — see [`probe_recursion`], which is a
    /// finding this fuzz produced rather than a hazard it works around.
    fn int(&mut self) -> i64 {
        let r = self.next();
        match r % 8 {
            0 => 0,
            1 => 1,
            2 => (r >> 8) as i64 % 16,
            3 => (r >> 8) as i64 % 512,
            4 => (r >> 8) as i64 % 64,
            5 => 4096 + (r >> 8) as i64 % 16,
            6 => (r >> 8) as i64 % 256,
            _ => -((r >> 8) as i64 % 8),
        }
    }

    /// The magnitudes a seed and a visit count really reach, for the functions
    /// whose recursion depth no argument names.
    fn large(&mut self) -> i64 {
        let r = self.next();
        match r % 4 {
            0 => (r >> 8) as i64 % 2_147_483_648,
            1 => (r >> 8) as i64 % 1_000_000,
            2 => 2_147_483_647 - (r >> 8) as i64 % 4,
            _ => (r >> 8) as i64 % 100_000,
        }
    }
}

/// The functions whose recursion depth is bounded by the *value* of an
/// argument rather than by its magnitude, so a large draw is safe.
const LARGE_SAFE: &[&str] = &[
    "mcts.heap",
    "mcts.turn",
    "mcts.pack",
    "mcts.objects",
    "mcts.terminal",
    "mcts.winner",
    "mcts.move_count",
    "mcts.nth_move",
    "mcts.apply_move",
    "mcts.next_seed",
    "mcts.below",
    "mcts.ilog2",
    "mcts.isqrt",
    "mcts.isqrt_step",
    "mcts.ucb",
    "mcts.zero",
];

#[derive(Serialize)]
struct Agreement {
    functions: usize,
    cases: usize,
    whole_kernel_cases: usize,
    disagreements: Vec<String>,
}

/// Agreement against **both** evaluators, on generated inputs, before anything
/// is timed. A raise on both sides counts as agreement: the fragment is
/// supposed to reproduce the machine's failures too.
fn verify(
    loaded: &'static Loaded,
    harness: &mut Harness,
    accepted: &[String],
    seed: u64,
    cases: usize,
) -> Result<Agreement> {
    let mut rng = Rng(seed);
    let mut bad = Vec::new();
    let mut n_fn = 0;
    let mut n_case = 0;
    for name in accepted {
        let Some(kinds) = scalar_params(loaded, name) else {
            continue;
        };
        let Some(entry) = harness.compiled.entry(name) else {
            continue;
        };
        n_fn += 1;
        for case in 0..cases {
            let big = LARGE_SAFE.contains(&name.as_str()) && case % 2 == 1;
            let args: Vec<Value> = kinds
                .iter()
                .map(|k| match *k {
                    "Bool" => Value::Bool(rng.next().is_multiple_of(2)),
                    _ => Value::Int(if big { rng.large() } else { rng.int() }),
                })
                .collect();
            n_case += 1;
            let expected = harness.interpret(name, &args);
            let actual = harness.compiled_call(entry, &args);
            let same = match (&expected, &actual) {
                (Ok(a), Ok(b)) => values_equal(a, b, Span::DUMMY).unwrap_or(false),
                (Err(_), Err(_)) => true,
                _ => false,
            };
            if !same {
                bad.push(format!(
                    "{name} case {case}: the machine and the fragment differ"
                ));
                continue;
            }
            if expected.is_ok() {
                let mut interp = Interp::new(&loaded.ast, &loaded.resolved, &loaded.check);
                let walked = interp.call(name, args.clone(), Span::DUMMY);
                let agreed = match (&walked, &actual) {
                    (Ok(a), Ok(b)) => values_equal(a, b, Span::DUMMY).unwrap_or(false),
                    _ => false,
                };
                if !agreed {
                    bad.push(format!("{name} case {case}: the tree-walker disagreed"));
                }
            }
        }
    }

    // And the whole kernel, which is the claim that actually matters: a search
    // driven from compiled code answers the move both interpreters answer.
    let mut whole = 0;
    let plan_entry = harness.compiled.entry("mcts.plan");
    for case in 0..24 {
        let a = 1 + (rng.next() % 8) as i64;
        let b = (rng.next() % 8) as i64;
        let c = (rng.next() % 8) as i64;
        let state = a + b * 16 + c * 256;
        let s = 1 + (rng.next() % 1_000_000) as i64;
        let n = 1 + (rng.next() % 40) as i64;
        let args = vec![Value::Int(state), Value::Int(s), Value::Int(n)];
        let expected = harness.interpret("mcts.plan", &args)?;
        whole += 1;
        if let Some(entry) = plan_entry {
            let actual = harness.compiled_call(entry, &args)?;
            if !values_equal(&expected, &actual, Span::DUMMY).unwrap_or(false) {
                bad.push(format!(
                    "mcts.plan case {case} (state {state}, seed {s}, {n} iterations): \
                     the fragment answered {} and the machine answered {}",
                    actual.render(),
                    expected.render()
                ));
            }
        }
        let mut interp = Interp::new(&loaded.ast, &loaded.resolved, &loaded.check);
        let walked = interp.call("mcts.plan", args, Span::DUMMY);
        match walked {
            Ok(v) if values_equal(&expected, &v, Span::DUMMY).unwrap_or(false) => {}
            _ => bad.push(format!("mcts.plan case {case}: the tree-walker disagreed")),
        }
    }

    Ok(Agreement {
        functions: n_fn,
        cases: n_case,
        whole_kernel_cases: whole,
        disagreements: bad,
    })
}

#[derive(Serialize)]
struct Rung {
    name: String,
    what: String,
    compiled: Vec<String>,
    interpreter: Band,
    hybrid: Band,
    /// Interpreter best over hybrid worst — the conservative direction, the
    /// same one ADR 0016's spike reports.
    /// Median of the per-window ratios; `low` and `high` are the extremes.
    speedup: f64,
    low: f64,
    high: f64,
    crossings_per_call: f64,
    builtin_calls_per_call: f64,
}

/// Total machine calls, total builtin calls, and the machine calls split by the
/// function each one went to.
type Crossings = (f64, f64, Vec<(String, u64)>);

fn crossings(harness: &mut Harness, name: &str, args: &[Value]) -> Result<Crossings> {
    harness.ctx.reset_counts();
    let entry = harness
        .compiled
        .entry(name)
        .ok_or_else(|| anyhow::anyhow!("`{name}` was not compiled"))?;
    harness.compiled_call(entry, args)?;
    let by_target: Vec<(String, u64)> = harness
        .ctx
        .targets
        .iter()
        .cloned()
        .zip(harness.ctx.machine_calls_by_target.iter().copied())
        .filter(|(_, n)| *n > 0)
        .collect();
    Ok((
        harness.ctx.machine_calls as f64,
        harness.ctx.builtin_calls as f64,
        by_target,
    ))
}

fn field_int(v: &Value) -> i64 {
    match v {
        Value::Int(i) => *i,
        _ => 0,
    }
}

fn field(v: &Value, name: &str) -> i64 {
    match v {
        Value::Record(map) => match map.get(&ply_span::Symbol::new(name)) {
            Some(Value::Int(i)) => *i,
            _ => 0,
        },
        _ => 0,
    }
}

#[derive(Serialize)]
struct Crossing {
    iterations: i64,
    nodes: i64,
    micros: f64,
}

#[derive(Serialize)]
struct Counted {
    ucb_evaluations: i64,
    playout_plies: i64,
    backprop_steps: i64,
    nodes_expanded: i64,
    playouts: i64,
}

#[derive(Serialize)]
struct Cost {
    what: String,
    calls: f64,
    micros_per_call: f64,
    micros_total: f64,
    share_of_request: f64,
}

#[derive(Serialize)]
struct Report {
    provenance: BTreeMap<String, String>,
    kernel: String,
    iterations: i64,
    census: Vec<CensusRow>,
    accepted_functions: usize,
    total_functions: usize,
    accepted_nodes: usize,
    total_nodes: usize,
    refusals_ranked: Vec<(String, usize, usize)>,
    agreement: Agreement,
    entry_micros_interpreter: f64,
    entry_micros_compiled: f64,
    rungs: Vec<Rung>,
    counted: Counted,
    attribution: Vec<Cost>,
    fragment_share_measured: f64,
    share_ucb_paired: Paired,
    share_rollout_paired: Paired,
    ceiling_at_measured_ratio: f64,
    ceiling_at_infinite_ratio: f64,
    unattributed_share: f64,
    end_to_end: f64,
    end_to_end_without_trampoline_tax: f64,
    trampoline_tax_micros: f64,
    pure_compute_speedup: f64,
    harness_floor: Paired,
    pure_compute: Paired,
    micros_per_ucb: f64,
    micros_per_ucb_without_sqrt: f64,
    boundary_cost_by_tree_size: Vec<Crossing>,
    crossings_by_target: Vec<(String, u64)>,
    recursion_bound_machine: String,
    recursion_bound_compiled: String,
    not_measured: Vec<String>,
}

/// A depth the machine refuses and compiled code does not.
///
/// `ply_eval::limit` bounds nested calls at 10,000 in *both* shipped engines,
/// and reports the breach as a diagnostic rather than as a crash, precisely so
/// that a deeply-recursive program does not take an unrelated test's process
/// with it. ADR 0016 §3.2's fragment compiles a self-call to a native call and
/// carries no such bound, so the same program aborts the process. This is run
/// as a subprocess by the main report so the claim is observed rather than
/// asserted.
fn probe_recursion(dir: &std::path::Path, compiled: bool) -> Result<()> {
    let loaded: &'static Loaded = Box::leak(Box::new(Loaded::project(dir)?));
    let names = [
        "mcts.playouts",
        "mcts.rollout",
        "mcts.terminal",
        "mcts.winner",
        "mcts.turn",
        "mcts.objects",
        "mcts.heap",
        "mcts.move_count",
        "mcts.nth_move",
        "mcts.apply_move",
        "mcts.next_seed",
        "mcts.below",
    ];
    let mut h = Harness::over(loaded, &names, Opts::default(), Some(ENTRY))?;
    // A terminal position, so every playout is instant and what the recursion
    // measures is the nesting rather than the game.
    let args = vec![Value::Int(0), Value::Int(1), Value::Int(5_000_000)];
    if compiled {
        let entry = h.compiled.entry("mcts.playouts").expect("compiled");
        match h.compiled_call(entry, &args) {
            Ok(v) => println!("compiled answered {}", v.render()),
            Err(e) => println!("compiled raised: {e}"),
        }
    } else {
        match h.interpret("mcts.playouts", &args) {
            Ok(v) => println!("machine answered {}", v.render()),
            Err(e) => println!("machine raised: {e}"),
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    let a = parse_args()?;
    if let Some(which) = &a.probe {
        return probe_recursion(&a.dir, which == "compiled");
    }
    let loaded: &'static Loaded = Box::leak(Box::new(Loaded::project(&a.dir)?));

    // ---------------------------------------------------------- 1. census --
    let rows = census(loaded, KERNEL);
    let accepted: Vec<String> = rows
        .iter()
        .filter(|r| r.accepted)
        .map(|r| r.function.clone())
        .collect();
    let total_nodes: usize = rows.iter().map(|r| r.nodes).sum();
    let accepted_nodes: usize = rows.iter().filter(|r| r.accepted).map(|r| r.nodes).sum();
    let mut ranked: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for r in rows.iter().filter(|r| !r.accepted) {
        let e = ranked
            .entry(r.refused_because.clone().unwrap_or_default())
            .or_insert((0, 0));
        e.0 += 1;
        e.1 += r.nodes;
    }
    let mut refusals: Vec<(String, usize, usize)> =
        ranked.into_iter().map(|(k, (n, m))| (k, n, m)).collect();
    refusals.sort_by(|x, y| y.2.cmp(&x.2).then(y.1.cmp(&x.1)));

    println!("== the kernel, function by function ==");
    for r in &rows {
        match &r.refused_because {
            None => println!("   compiled   {:<24} {:>4} nodes", r.function, r.nodes),
            Some(why) => println!(
                "   refused    {:<24} {:>4} nodes   {why}",
                r.function, r.nodes
            ),
        }
    }
    println!(
        "\n   {} of {} functions, {} of {} lowered nodes, are inside the fragment",
        accepted.len(),
        rows.len(),
        accepted_nodes,
        total_nodes
    );
    println!("\n== what is outside, ranked by the nodes it takes with it ==");
    for (why, n, nodes) in &refusals {
        println!("   {nodes:>4} nodes  {n:>2} function(s)  {why}");
    }

    // -------------------------------------------- 2. compile and verify --
    let names: Vec<&str> = accepted.iter().map(|s| s.as_str()).collect();
    let mut harness = Harness::over(loaded, &names, Opts::default(), Some(ENTRY))?;
    let agreement = verify(loaded, &mut harness, &accepted, 0x9E3779B97F4A7C15, 64)?;
    println!(
        "\n== agreement, before anything is timed ==\n   \
         {} functions × generated inputs = {} cases, plus {} whole-kernel searches, \
         against the machine and the tree-walker",
        agreement.functions, agreement.cases, agreement.whole_kernel_cases
    );
    if !agreement.disagreements.is_empty() {
        for d in &agreement.disagreements {
            println!("   DISAGREEMENT  {d}");
        }
        bail!(
            "{} disagreement(s): a faster wrong answer prices nothing",
            agreement.disagreements.len()
        );
    }
    println!("   0 disagreements");

    // --------------------------------------------------- 3. entry costs --
    let entry_interp = band(
        || {
            harness.interpret(ENTRY, &[])?;
            Ok(())
        },
        2000,
        a.repeats,
    )?;
    let zero_entry = harness.compiled.entry(ENTRY).expect("compiled");
    let entry_compiled = band(
        || {
            harness.compiled_call(zero_entry, &[])?;
            Ok(())
        },
        2000,
        a.repeats,
    )?;
    let n = Value::Int(a.iterations);
    let n_probe = n.clone();

    // The same interpreter, reached through the *other* `Machine` — the one the
    // trampoline calls into. Nothing is compiled on this path and nothing
    // crosses; it exists so that a rung's ratio can be read against what the
    // harness costs rather than against 1.00x assumed.
    let floor = {
        let cell = std::cell::RefCell::new(&mut harness);
        paired(
            || {
                cell.borrow_mut()
                    .interpret(WHOLE, std::slice::from_ref(&n_probe))?;
                Ok(())
            },
            || {
                let mut h = cell.borrow_mut();
                let m = h.ctx.machine.as_mut().expect("installed");
                m.call(WHOLE, vec![n_probe.clone()], Span::DUMMY)
                    .map_err(|d| anyhow::anyhow!("{}", d.message))?;
                Ok(())
            },
            a.inner,
            a.repeats,
        )?
    };

    // ------------------------------------------------- 4. the timing ladder --
    let mut rungs = Vec::new();

    // The ladder's rungs are compiled sets, each a superset of the one above.
    let outer: Vec<&str> = vec!["mcts.plan_753", "mcts.plan", "mcts.search", "mcts.pack"];
    let rollout_group: Vec<&str> = {
        let mut v = outer.clone();
        v.extend([
            "mcts.rollout",
            "mcts.terminal",
            "mcts.winner",
            "mcts.turn",
            "mcts.objects",
            "mcts.heap",
            "mcts.move_count",
            "mcts.nth_move",
            "mcts.apply_move",
            "mcts.next_seed",
            "mcts.below",
        ]);
        v
    };
    let ladder: Vec<(&str, &str, Vec<&str>)> = vec![
        (
            "control: entry only",
            "only `plan_753`, so one crossing and no compiled work — this rung \
             is here to show what the harness costs when it compiles nothing that runs",
            vec!["mcts.plan_753", "mcts.pack"],
        ),
        (
            "outer loop only",
            "the search driver and nothing under it",
            outer.clone(),
        ),
        (
            "outer loop + playouts",
            "the driver plus the whole playout and its position arithmetic",
            rollout_group.clone(),
        ),
        (
            "everything the fragment accepts",
            "every function of the kernel the fragment did not refuse",
            names.clone(),
        ),
    ];

    let mut whole_by_target = Vec::new();
    for (name, what, group) in &ladder {
        let mut h = Harness::over(loaded, group, Opts::default(), Some(ENTRY))?;
        let entry = h
            .compiled
            .entry(WHOLE)
            .ok_or_else(|| anyhow::anyhow!("`{WHOLE}` is not in this rung's compiled set"))?;
        let p = {
            let arg = n.clone();
            let cell = std::cell::RefCell::new(&mut h);
            paired(
                || {
                    cell.borrow_mut()
                        .interpret(WHOLE, std::slice::from_ref(&arg))?;
                    Ok(())
                },
                || {
                    cell.borrow_mut()
                        .compiled_call(entry, std::slice::from_ref(&arg))?;
                    Ok(())
                },
                a.inner,
                a.repeats,
            )?
        };
        let interp = p.a;
        let hybrid = p.b;
        let (m, b, by_target) = crossings(&mut h, WHOLE, std::slice::from_ref(&n))?;
        if *name == "everything the fragment accepts" {
            whole_by_target = by_target;
        }
        rungs.push(Rung {
            name: (*name).to_string(),
            what: (*what).to_string(),
            compiled: group.iter().map(|s| (*s).to_string()).collect(),
            interpreter: interp,
            hybrid,
            speedup: p.median,
            low: p.low,
            high: p.high,
            crossings_per_call: m,
            builtin_calls_per_call: b,
        });
    }

    // ------------------------------- 5. the same fragment with no tree at all --
    let pure_state = Value::Int(7 + 5 * 16 + 3 * 256);
    let pure_args = vec![
        pure_state.clone(),
        Value::Int(20260821),
        Value::Int(a.iterations),
    ];
    let pure_entry = harness.compiled.entry(PURE).expect("playouts compiled");
    let pure = {
        let cell = std::cell::RefCell::new(&mut harness);
        paired(
            || {
                cell.borrow_mut().interpret(PURE, &pure_args)?;
                Ok(())
            },
            || {
                cell.borrow_mut().compiled_call(pure_entry, &pure_args)?;
                Ok(())
            },
            a.inner * 4,
            a.repeats,
        )?
    };
    let (pure_interp, pure_hybrid) = (pure.a, pure.b);
    let pure_speedup = pure.median;

    // ------------------------------------- 6. how much work a search does --
    let work = harness.interpret(WORK, std::slice::from_ref(&n))?;
    let counted = Counted {
        ucb_evaluations: field(&work, "ucb"),
        playout_plies: field(&work, "plies"),
        backprop_steps: field(&work, "steps"),
        nodes_expanded: field(&work, "nodes"),
        playouts: a.iterations,
    };

    // Per-call interpreter costs, on arguments the search actually produces.
    let e = entry_interp.best;
    /// The mean cost of one call, over argument sets the search actually
    /// produced rather than one set chosen by the person measuring. The
    /// entry cost is subtracted because a call from inside the kernel does not
    /// pay it.
    fn per_call(
        harness: &mut Harness,
        name: &str,
        sets: &[Vec<Value>],
        entry: f64,
        inner: u32,
        repeats: u32,
    ) -> Result<f64> {
        if sets.is_empty() {
            bail!("no argument sets were captured for `{name}`");
        }
        let mut i = 0usize;
        let b = band(
            || {
                harness.interpret(name, &sets[i % sets.len()])?;
                i += 1;
                Ok(())
            },
            inner,
            repeats,
        )?;
        Ok((b.best - entry).max(0.0))
    }

    /// Every `(wins, visits, parent_visits)` a descent of this tree could hand
    /// `ucb`, read out of the tree the search actually built.
    fn ucb_arguments(tree: &Value) -> Vec<Vec<Value>> {
        let Value::Record(root) = tree else {
            return Vec::new();
        };
        let Some(Value::Map(nodes)) = root.get(&ply_span::Symbol::new("nodes")) else {
            return Vec::new();
        };
        let visits_of = |id: &Value| -> Option<i64> { nodes.get(id).map(|n| field(n, "visits")) };
        let mut out = Vec::new();
        for (_, node) in nodes.iter() {
            let Value::Record(_) = node else { continue };
            let parent = field(node, "parent");
            if parent < 0 {
                continue;
            }
            let Some(pv) = visits_of(&Value::Int(parent)) else {
                continue;
            };
            out.push(vec![
                Value::Int(field(node, "wins")),
                Value::Int(field(node, "visits")),
                Value::Int(pv),
            ]);
        }
        out
    }

    /// The states playouts actually start from, with the seed each one is given.
    fn rollout_arguments(tree: &Value) -> Vec<Vec<Value>> {
        let Value::Record(root) = tree else {
            return Vec::new();
        };
        let Some(Value::Map(nodes)) = root.get(&ply_span::Symbol::new("nodes")) else {
            return Vec::new();
        };
        let seed = root
            .get(&ply_span::Symbol::new("seed"))
            .map(field_int)
            .unwrap_or(1);
        let mut out = Vec::new();
        for (id, node) in nodes.iter() {
            let Value::Int(i) = id else { continue };
            out.push(vec![
                Value::Int(field(node, "state")),
                Value::Int((seed / (1 + i.unsigned_abs() as i64 % 97)).max(1)),
                Value::Int(64),
            ]);
        }
        out
    }
    let root_state = Value::Int(7 + 5 * 16 + 3 * 256);
    let seed0 = Value::Int(20260821);
    let built = harness.interpret("mcts.root", &[root_state.clone(), seed0.clone()])?;
    let searched = harness.interpret("mcts.search", &[built.clone(), n.clone()])?;
    let ucb_sets = ucb_arguments(&searched);
    let rollout_sets = rollout_arguments(&searched);

    let t_ucb = per_call(&mut harness, "mcts.ucb", &ucb_sets, e, 400, a.repeats)?;
    let t_ucb_free = per_call(
        &mut harness,
        "work.ucb_if_sqrt_were_free",
        &ucb_sets,
        e,
        2000,
        a.repeats,
    )?;
    let t_turn = per_call(
        &mut harness,
        "mcts.turn",
        &[vec![root_state.clone()]],
        e,
        2000,
        a.repeats,
    )?;
    let t_move_count = per_call(
        &mut harness,
        "mcts.move_count",
        &[vec![root_state.clone()]],
        e,
        2000,
        a.repeats,
    )?;
    let t_nth_move = per_call(
        &mut harness,
        "mcts.nth_move",
        &[vec![root_state.clone(), Value::Int(3)]],
        e,
        2000,
        a.repeats,
    )?;
    let t_apply = per_call(
        &mut harness,
        "mcts.apply_move",
        &[vec![root_state.clone(), Value::Int(3)]],
        e,
        2000,
        a.repeats,
    )?;

    // A playout's cost per ply, over the states the search really rolls out
    // from, so that a per-call figure can be weighted by the ply count the
    // search really played rather than by an assumed playout length.
    let t_rollout = per_call(
        &mut harness,
        "mcts.rollout",
        &rollout_sets,
        e,
        200,
        a.repeats,
    )?;
    let mut sampled_plies = 0i64;
    for set in &rollout_sets {
        sampled_plies += field_int(&harness.interpret(PLIES, set)?);
    }
    let mean_plies = sampled_plies as f64 / rollout_sets.len() as f64;
    let t_ply = t_rollout / mean_plies.max(1.0);

    // What it costs to hand the machine a whole tree at the boundary. The body
    // of `work.touch` is one field read, so the difference between this and the
    // nullary probe is what the crossing itself charges for the value.
    let mut boundary = Vec::new();
    for k in [0i64, a.iterations / 4, a.iterations / 2, a.iterations] {
        let t = harness.interpret("mcts.search", &[built.clone(), Value::Int(k)])?;
        let size = field(&t, "size");
        let b = band(
            || {
                harness.interpret("work.touch", std::slice::from_ref(&t))?;
                Ok(())
            },
            500,
            a.repeats,
        )?;
        boundary.push(Crossing {
            iterations: k,
            nodes: size,
            micros: b.best,
        });
    }

    // The two big components, measured **against the whole search in the same
    // window**: one side runs the search, the other runs exactly the `ucb`
    // calls (or exactly the playout plies) that search makes, over the same
    // arguments. The ratio is the share, and it is a ratio of two numbers taken
    // on the same machine in the same second rather than of two bests taken
    // minutes apart.
    let ucb_cycles = (counted.ucb_evaluations as usize).div_ceil(ucb_sets.len().max(1));
    let ucb_calls = (ucb_cycles * ucb_sets.len()) as f64;
    let ucb_paired = {
        let cell = std::cell::RefCell::new(&mut harness);
        paired(
            || {
                cell.borrow_mut()
                    .interpret(WHOLE, std::slice::from_ref(&n))?;
                Ok(())
            },
            || {
                let mut h = cell.borrow_mut();
                for _ in 0..ucb_cycles {
                    for set in &ucb_sets {
                        h.interpret("mcts.ucb", set)?;
                    }
                }
                Ok(())
            },
            1,
            a.repeats,
        )?
    };
    let share_ucb = (1.0 / ucb_paired.median) * (counted.ucb_evaluations as f64 / ucb_calls);

    let rollout_cycles =
        ((counted.playout_plies as f64 / sampled_plies.max(1) as f64).round() as usize).max(1);
    let rollout_plies = (rollout_cycles as i64 * sampled_plies) as f64;
    let rollout_paired = {
        let cell = std::cell::RefCell::new(&mut harness);
        paired(
            || {
                cell.borrow_mut()
                    .interpret(WHOLE, std::slice::from_ref(&n))?;
                Ok(())
            },
            || {
                let mut h = cell.borrow_mut();
                for _ in 0..rollout_cycles {
                    for set in &rollout_sets {
                        h.interpret("mcts.rollout", set)?;
                    }
                }
                Ok(())
            },
            1,
            a.repeats,
        )?
    };
    let share_rollout =
        (1.0 / rollout_paired.median) * (counted.playout_plies as f64 / rollout_plies);

    let whole_interp = rungs.last().map(|r| r.interpreter.best).unwrap_or(f64::NAN);
    let mut attribution = vec![
        Cost {
            what: "playouts (`rollout` and the position arithmetic under it)".into(),
            calls: counted.playout_plies as f64,
            micros_per_call: t_ply,
            micros_total: share_rollout * whole_interp,
            share_of_request: share_rollout,
        },
        Cost {
            what: "`ucb` (with `isqrt` and `ilog2` under it)".into(),
            calls: counted.ucb_evaluations as f64,
            micros_per_call: t_ucb,
            micros_total: share_ucb * whole_interp,
            share_of_request: share_ucb,
        },
        Cost {
            what: "`turn`, once per backpropagation step".into(),
            calls: counted.backprop_steps as f64,
            micros_per_call: t_turn,
            micros_total: counted.backprop_steps as f64 * t_turn,
            share_of_request: counted.backprop_steps as f64 * t_turn / whole_interp,
        },
        Cost {
            what: "`move_count`, `nth_move` and `apply_move`, once per expansion".into(),
            calls: counted.nodes_expanded as f64,
            micros_per_call: t_move_count * 2.0 + t_nth_move + t_apply,
            micros_total: counted.nodes_expanded as f64
                * (t_move_count * 2.0 + t_nth_move + t_apply),
            share_of_request: counted.nodes_expanded as f64
                * (t_move_count * 2.0 + t_nth_move + t_apply)
                / whole_interp,
        },
    ];
    attribution.sort_by(|x, y| y.micros_total.partial_cmp(&x.micros_total).unwrap());
    let fragment_share: f64 = attribution.iter().map(|c| c.share_of_request).sum();

    let last = rungs.last().expect("three rungs");
    let end_to_end = last.speedup;
    let tax = last.crossings_per_call * entry_interp.best;
    let end_to_end_no_tax = last.interpreter.best / (last.hybrid.best - tax).max(1e-9);

    println!("\n== entry cost, the price of arriving on each side ==");
    println!("   interpreter  {:.3} µs per `{ENTRY}`", entry_interp.best);
    println!(
        "   compiled     {:.3} µs per `{ENTRY}`",
        entry_compiled.best
    );

    println!(
        "\n== the whole kernel, end to end: `{WHOLE}({})` ==",
        a.iterations
    );
    println!(
        "   the same search, no JIT involved, on the two `Machine`s this harness holds:\n   \
         {:.1} µs and {:.1} µs — {:.3}x [{:.3}–{:.3}], which is what this method's own noise\n   \
         looks like, and what a rung's ratio is read against",
        floor.a.best, floor.b.best, floor.median, floor.low, floor.high
    );
    for r in &rungs {
        println!(
            "   {:<32} interpreter {:>9.1} µs   hybrid {:>9.1} µs   {:.3}x [{:.3}–{:.3}]   \
             {:.0} crossings",
            r.name,
            r.interpreter.best,
            r.hybrid.best,
            r.speedup,
            r.low,
            r.high,
            r.crossings_per_call
        );
    }
    println!("\n   end to end, everything the fragment accepts: {end_to_end:.3}x");
    println!(
        "   the {:.0} trampoline crossings cost {tax:.1} µs at the machine's own entry price, \n   \
         which is {:.3}% of the run — so the boundary is not what is happening here. \n   \
         Charging it back gives {end_to_end_no_tax:.3}x.",
        last.crossings_per_call,
        100.0 * tax / last.interpreter.best
    );
    println!(
        "   Compiling the twenty arithmetic functions between the second rung and the fourth \n   \
         moved the whole-program time from {:.1} µs to {:.1} µs: the interpreter cannot enter \n   \
         compiled code, so a function the fragment accepts and whose callers it refuses is \n   \
         compiled and then never run.",
        rungs[1].hybrid.best, last.hybrid.best
    );

    println!("\n== the same fragment with the tree removed: `{PURE}` ==");
    println!(
        "   interpreter {:>9.1} µs   compiled {:>9.1} µs   {pure_speedup:.2}x [{:.2}–{:.2}], \
         zero crossings",
        pure_interp.best, pure_hybrid.best, pure.low, pure.high
    );

    println!(
        "\n== what one search of {} iterations actually does ==",
        a.iterations
    );
    println!("   {:>8} playouts", counted.playouts);
    println!("   {:>8} playout plies", counted.playout_plies);
    println!("   {:>8} `ucb` evaluations", counted.ucb_evaluations);
    println!("   {:>8} backpropagation steps", counted.backprop_steps);
    println!("   {:>8} nodes expanded", counted.nodes_expanded);

    println!("\n== where the interpreter's time goes, weighted by those counts ==");
    for c in &attribution {
        println!(
            "   {:>5.1}%  {:>10.0} calls × {:.4} µs = {:>9.1} µs   {}",
            c.share_of_request * 100.0,
            c.calls,
            c.micros_per_call,
            c.micros_total,
            c.what
        );
    }
    println!(
        "   -----\n   {:>5.1}%  is inside the fragment\n   {:>5.1}%  is the tree: `Map`, records, \
         lists, `Option` matching, and the machine's own dispatch",
        fragment_share * 100.0,
        (1.0 - fragment_share) * 100.0
    );

    let ucb_now = counted.ucb_evaluations as f64 * t_ucb;
    let ucb_free = counted.ucb_evaluations as f64 * t_ucb_free;
    // Amdahl over the two numbers this run measured: the share of the kernel
    // the fragment covers, and the ratio the fragment reaches where it runs.
    // Neither is assumed and neither is quoted.
    let ceiling_at_measured_ratio = 1.0 / ((1.0 - fragment_share) + fragment_share / pure_speedup);
    let ceiling_at_infinite_ratio = 1.0 / (1.0 - fragment_share);
    println!("\n== the ceiling, from the two numbers above ==");
    println!(
        "   {:.1}% of the kernel is inside the fragment and the fragment runs {pure_speedup:.1}x \
         faster there,\n   so a backend that could be *entered* from interpreted code would \
         reach {ceiling_at_measured_ratio:.2}x end to end.",
        fragment_share * 100.0
    );
    println!(
        "   At an infinitely fast fragment it would reach {ceiling_at_infinite_ratio:.2}x, and \
         no execution-strategy\n   change can pass that while the other {:.1}% is `Map`, records \
         and lists.",
        (1.0 - fragment_share) * 100.0
    );

    println!("\n== what the two missing numeric builtins cost ==");
    println!(
        "   `ucb` costs {t_ucb:.2} µs a call as written and {t_ucb_free:.2} µs with the square \n            root and the logarithm removed, so a `sqrt` and an `ln` in the prelude would take \n            {:.0} µs off a {:.0} µs search: {:.2}x, with no compiler work at all",
        ucb_now - ucb_free,
        whole_interp,
        whole_interp / (whole_interp - (ucb_now - ucb_free)).max(1e-9)
    );

    println!("\n== what the boundary charges for the value crossing it ==");
    println!(
        "   the machine's entry with no argument at all: {:.3} µs",
        entry_interp.best
    );
    for c in &boundary {
        println!(
            "   a tree of {:>4} nodes ({:>4} iterations): {:>8.3} µs to enter `work.touch`, \
             whose body is one field read",
            c.nodes, c.iterations, c.micros
        );
    }

    println!("\n== crossings, by the function the fragment had to leave to ==");
    for (name, count) in &whole_by_target {
        println!("   {count:>8}  {name}");
    }

    println!("\n== the bound compiled code does not carry ==");
    let exe = std::env::current_exe()?;
    let machine_side = std::process::Command::new(&exe)
        .args(["--dir", &a.dir.display().to_string(), "--probe", "machine"])
        .output()?;
    let compiled_side = std::process::Command::new(&exe)
        .args(["--dir", &a.dir.display().to_string(), "--probe", "compiled"])
        .output()?;
    let machine_says = String::from_utf8_lossy(&machine_side.stdout)
        .trim()
        .to_string();
    let compiled_says = if compiled_side.status.success() {
        String::from_utf8_lossy(&compiled_side.stdout)
            .trim()
            .to_string()
    } else {
        format!(
            "the process died: {} — {}",
            compiled_side.status,
            String::from_utf8_lossy(&compiled_side.stderr).trim()
        )
    };
    println!("   `mcts.playouts(0, 1, 5000000)`, a recursion 5,000,000 calls deep:");
    println!("   machine:   {machine_says}");
    println!("   compiled:  {compiled_says}");

    let mut provenance = BTreeMap::new();
    provenance.insert(
        "rustc".into(),
        std::process::Command::new("rustc")
            .arg("--version")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "not read".into()),
    );
    provenance.insert("cranelift".into(), "0.134.3".to_string());
    provenance.insert("profile".into(), "release".to_string());
    provenance.insert(
        "command".into(),
        std::env::args().collect::<Vec<_>>().join(" "),
    );
    provenance.insert(
        "taken".into(),
        format!(
            "each ratio is the median of {} per-window ratios, both sides timed in the same \
             window, {} calls a side; the µs beside it are best-of over the same windows",
            a.repeats, a.inner
        ),
    );
    provenance.insert("kernel_dir".into(), a.dir.display().to_string());
    provenance.insert(
        "load_average".into(),
        std::process::Command::new("uptime")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "not read".into()),
    );

    let report = Report {
        provenance,
        kernel: KERNEL.into(),
        iterations: a.iterations,
        census: rows,
        accepted_functions: accepted.len(),
        total_functions: accepted.len() + refusals.iter().map(|(_, n, _)| n).sum::<usize>(),
        accepted_nodes,
        total_nodes,
        refusals_ranked: refusals,
        agreement,
        entry_micros_interpreter: entry_interp.best,
        entry_micros_compiled: entry_compiled.best,
        rungs,
        counted,
        attribution,
        fragment_share_measured: fragment_share,
        share_ucb_paired: ucb_paired,
        share_rollout_paired: rollout_paired,
        ceiling_at_measured_ratio,
        ceiling_at_infinite_ratio,
        unattributed_share: 1.0 - fragment_share,
        end_to_end,
        end_to_end_without_trampoline_tax: end_to_end_no_tax,
        trampoline_tax_micros: tax,
        pure_compute_speedup: pure_speedup,
        harness_floor: floor.clone(),
        pure_compute: pure.clone(),
        micros_per_ucb: t_ucb,
        micros_per_ucb_without_sqrt: t_ucb_free,
        boundary_cost_by_tree_size: boundary,
        crossings_by_target: whole_by_target,
        recursion_bound_machine: machine_says,
        recursion_bound_compiled: compiled_says,
        not_measured: vec![
            "the interpreter cannot call compiled code, so a function the fragment accepts \
             but whose only callers it refuses is compiled and never entered — every `ucb`, \
             `isqrt` and `ilog2` call in the hybrid runs in the machine"
                .into(),
            "`rt_call_machine` is a whole `Machine::call` entry point rather than a frame \
             push, because `Machine::apply` is not public; the tax line above prices that at \
             the machine's own measured entry cost, which is a lower bound on it"
                .into(),
            "allocation is not counted here at all; this is wall clock only".into(),
            "one machine, one run, best-of; no cross-machine check".into(),
        ],
    };
    if let Some(path) = &a.out {
        std::fs::write(path, serde_json::to_string_pretty(&report)?)?;
        println!("\nwrote {path}");
    }
    Ok(())
}
