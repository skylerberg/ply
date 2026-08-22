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
use ply_codegen_spike::entry::{Declines, admissible, enterable, refusals_over};
use ply_codegen_spike::jit::{Opts, node_count};
use ply_codegen_spike::measure::Harness;
use ply_codegen_spike::program::Loaded;
use ply_eval::{Interp, Value, compare_answers, lower};
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
    /// `agreement` stops after the census, the agreement corpus and the entry
    /// counts — everything deterministic — and takes no wall clock at all.
    /// Correctness before timing is the house rule, and a mode that cannot time
    /// is how it is kept on a loaded machine.
    only: Option<String>,
}

fn parse_args() -> Result<Args> {
    let mut a = Args {
        dir: PathBuf::from("benches/kernel"),
        iterations: 200,
        repeats: 7,
        inner: 3,
        out: None,
        probe: None,
        only: None,
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
            "--only" => a.only = argv.next(),
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
    /// Accepted **and** offered to the machine. A function the fragment compiles
    /// whose signature is not `Int`/`Bool` throughout stays compiled — a native
    /// body may call it, which is what makes the set closed — and is never
    /// entered, because the boundary carries no other kind.
    enterable: bool,
    refused_because: Option<String>,
}

/// What the fragment accepts, taken over the whole module **as one unit**.
///
/// Before R5 this compiled each function on its own and a call to an uncompiled
/// function became a trampoline, so `mcts.search` counted as accepted while
/// every iteration of it left the fragment. It cannot now: the compiled set is
/// closed under calls, so a function is accepted only if everything it can reach
/// is too, and the census is a census of what can actually run natively.
fn census(loaded: &'static Loaded, module: &str) -> Result<(Vec<CensusRow>, Vec<String>)> {
    let all = loaded.functions_in(module);
    let accepted = admissible(loaded, &all)?;
    let entered = enterable(loaded, &accepted);
    let refusals = refusals_over(loaded, &all)?;
    let mut rows = Vec::new();
    for name in &all {
        let (def, _) = loaded.definition(name).expect("it was just listed");
        let nodes = node_count(&lower(&def.body));
        // A function can be refused twice — once on its own construct, once
        // again after a callee is dropped. The first refusal is the cause.
        let why = refusals
            .iter()
            .find(|(f, _)| f == name)
            .map(|(_, r)| r.clone());
        rows.push(CensusRow {
            function: name.clone(),
            nodes,
            accepted: accepted.contains(name),
            enterable: entered.contains(name),
            refused_because: why,
        });
    }
    Ok((rows, accepted))
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
    "work.zero",
    "work.ucb_if_sqrt_were_free",
];

/// The functions whose last argument names a number of *iterations*, with the
/// cap a corpus may draw there.
///
/// A wide draw in that position is not a wide input; it is a search that does
/// not finish inside a corpus run. Stated here rather than left to [`Rng::int`]
/// because the alternative is a hostile `i64::MAX` that costs the rest of the
/// corpus its run and tests nothing [`deep_recursion`] does not test
/// deliberately.
///
/// `mcts.playouts` is deliberately absent: its count is the one that reaches
/// the machine's nested-call bound, and that case is wanted.
const COUNTED: &[(&str, i64)] = &[
    ("mcts.plan", 16),
    ("mcts.plan_753", 16),
    ("work.work_753", 12),
    ("work.size_753", 12),
];

/// Functions that cost milliseconds a case on four evaluators, so they get
/// fewer generated cases. Named rather than inferred from a timing, so that a
/// function which becomes cheap leaves this list because somebody decided it
/// had.
const HEAVY: &[&str] = &[
    "mcts.plan",
    "mcts.plan_753",
    "mcts.playouts",
    "mcts.root",
    "work.work_753",
    "work.size_753",
    "work.plies",
];

/// Every kernel function a corpus can call directly: all parameters `Int` or
/// `Bool`, whatever it answers.
///
/// > **Widened by the R5 audit, 2026-08-21.** This used to be the *enterable*
/// > set — the functions the fragment compiles and the boundary will offer,
/// > which is 19 of the kernel's 34 — and that leaves the claim untested exactly
/// > where it is most interesting. `mcts.plan` and `mcts.plan_753` are refused
/// > by the fragment and are the functions whose *callees* get entered; running
/// > generated cases through them is what puts a corpus on the
/// > interpreter-drives-and-drops-in path rather than only on the leaves.
/// > Functions answering a `Node` or a `Tree` are here for the same reason: the
/// > answer crosses no boundary, so any difference in one is a difference the
/// > interpreter made while a backend was attached.
fn subjects(loaded: &'static Loaded) -> Vec<String> {
    let mut out = Vec::new();
    for module in ["mcts", "work"] {
        for name in loaded.functions_in(module) {
            if scalar_params(loaded, &name).is_some() {
                out.push(name);
            }
        }
    }
    out
}

#[derive(Serialize)]
struct Agreement {
    functions: usize,
    /// The subjects the fragment compiles, of [`Agreement::functions`]. The rest
    /// are the callers it refuses, and they are in the corpus on purpose.
    compiled_functions: usize,
    cases: usize,
    /// Cases whose arguments the boundary refuses on sight — a `Float`, a
    /// `Str`, a `List`, a `Map`, a record, a `Decimal`, a `Secret`, `Unit`. The
    /// hook must never be offered these, in any argument position, and the
    /// interpreter must answer exactly what it answers with no backend at all.
    non_scalar_cases: usize,
    /// Cases where the machine raised. Before R5 these were scored as agreement
    /// on both sides raising and never compared at all, which is the weaker
    /// check `ply_eval::differential` forbids by name.
    raising_cases: usize,
    whole_kernel_cases: usize,
    /// Cases where the fragment failed and the machine did not — a decline, not
    /// a disagreement, but counted because a fragment that declines everything
    /// agrees with everything.
    fragment_declined: usize,
    /// Native entries the hybrid machine took over the whole corpus.
    entries: u64,
    /// Distinct compiled functions the interpreter actually dropped into. A
    /// corpus that entered one function a hundred thousand times has covered one
    /// function.
    entered_functions: usize,
    /// Entries the corpus declined for running out of fuel — i.e. generated
    /// cases that reached the machine's own bound on nested calls, before
    /// [`deep_recursion`] adds the deliberate ones. Recorded because "the corpus
    /// reached the bound" is a fact about coverage rather than about the run.
    corpus_out_of_fuel: u64,
    disagreements: Vec<String>,
}

/// Every argument shape the boundary must refuse, one of each kind.
///
/// `Secret` is here because ADR 0019 §0.1 arms `argv`'s pool against exactly a
/// table that keeps one past the call that carried it, and the fragment's
/// `Ctx::slots` is such a table.
fn non_scalar_arguments() -> Vec<(&'static str, Value)> {
    vec![
        ("Float", Value::Float(1.5)),
        ("Str", Value::str("7")),
        ("Bytes", Value::bytes(b"7")),
        ("Unit", Value::Unit),
        ("List", Value::list(vec![Value::Int(1)])),
        ("Map", Value::Map(ply_eval::Map::new())),
        (
            "Record",
            Value::Record(std::sync::Arc::new(std::collections::BTreeMap::new())),
        ),
        ("Decimal", Value::Decimal(ply_eval::Decimal::new(7, 0))),
        (
            "Ctor",
            Value::ctor(ply_span::Symbol::new("None"), Vec::new()),
        ),
        ("Secret", Value::secret(Value::Int(7))),
    ]
}

/// The values that make an arithmetic body fail rather than answer: an overflow
/// in `next_seed`'s multiply, a division by zero in `below`, the extremes of the
/// representation.
const HOSTILE: &[i64] = &[
    0,
    -1,
    1,
    i64::MAX,
    i64::MIN,
    i64::MAX / 2,
    -9_223_372_036_854_775_000,
];

/// One generated call, and whether the boundary is allowed to carry it.
struct Case {
    args: Vec<Value>,
    /// False when some argument is a kind the boundary refuses. Such a case must
    /// not move *this function's* entry count and must not make a compiled body
    /// run and fail, which is a tighter claim than "the totals did not move":
    /// `mcts.plan_753(1.5)` legitimately enters `mcts.pack` on its way to
    /// raising.
    scalar: bool,
}

/// The whole case list for one function: generated, hostile everywhere, hostile
/// in one position at a time, and every refused kind in every position.
///
/// > **Widened by the R5 audit.** The refused-kind sweep used to write the shape
/// > into argument 0 only, so `ucb(3, "7", 3)` had never been offered to
/// > anything. Position now varies, which is where a boundary that checked only
/// > its first argument would show up.
fn cases_for(
    rng: &mut Rng,
    name: &str,
    kinds: &[&'static str],
    cases: usize,
    shapes: &[(&'static str, Value)],
) -> Vec<Case> {
    let cap = COUNTED
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, cap)| *cap);
    let clamp = |args: &mut Vec<Value>| {
        if let Some(cap) = cap
            && let Some(Value::Int(n)) = args.last_mut()
        {
            *n = n.rem_euclid(cap + 1);
        }
    };
    let benign = |kinds: &[&'static str]| -> Vec<Value> {
        kinds
            .iter()
            .map(|k| match *k {
                "Bool" => Value::Bool(true),
                _ => Value::Int(3),
            })
            .collect()
    };

    let mut out: Vec<Case> = Vec::new();
    if kinds.is_empty() {
        // A nullary definition takes the constant memo path, and one case is the
        // whole of its input domain. Two, so the memo is consulted as well as
        // written.
        out.push(Case {
            args: Vec::new(),
            scalar: true,
        });
        out.push(Case {
            args: Vec::new(),
            scalar: true,
        });
        return out;
    }

    let draws = if HEAVY.contains(&name) {
        cases / 8
    } else {
        cases
    };
    for case in 0..draws.max(4) {
        let big = LARGE_SAFE.contains(&name) && case % 2 == 1;
        let mut args: Vec<Value> = kinds
            .iter()
            .map(|k| match *k {
                "Bool" => Value::Bool(rng.next().is_multiple_of(2)),
                _ => Value::Int(if big { rng.large() } else { rng.int() }),
            })
            .collect();
        clamp(&mut args);
        out.push(Case { args, scalar: true });
    }
    for hostile in HOSTILE {
        let mut args: Vec<Value> = kinds
            .iter()
            .map(|k| match *k {
                "Bool" => Value::Bool(hostile % 2 == 0),
                _ => Value::Int(*hostile),
            })
            .collect();
        clamp(&mut args);
        out.push(Case { args, scalar: true });
        for position in 0..kinds.len() {
            let mut args = benign(kinds);
            if kinds[position] != "Bool" {
                args[position] = Value::Int(*hostile);
            }
            clamp(&mut args);
            out.push(Case { args, scalar: true });
        }
    }
    for position in 0..kinds.len() {
        for (_, shape) in shapes {
            let mut args = benign(kinds);
            args[position] = shape.clone();
            out.push(Case {
                args,
                scalar: false,
            });
        }
    }
    out
}

/// Agreement, before anything is timed, and against every evaluator there is.
///
/// Three claims, checked separately because they fail separately:
///
///   1. **The hybrid machine answers what the machine answers.** This is R5's
///      whole claim and it is checked with `differential::compare_answers`, so a
///      difference in the diagnostic's code, message, labels, spans or notes, in
///      the observed footprint, or in the cell arena is a disagreement — not
///      only a difference in the value. The two sides are the same `Machine` over
///      the same program; the backend is the only difference there is.
///   2. **The tree-walker agrees too**, so the audit is against an independent
///      implementation rather than against the engine under test.
///   3. **The fragment, called directly, answers what the machine answers where
///      it answers at all.** A fragment that fails where the machine succeeds is
///      a decline and is counted; a fragment that *succeeds where the machine
///      raises* is a wrong answer and is a disagreement.
///
/// A failing input is compared rather than waved through. `(Err(_), Err(_))`
/// counted as agreement until R5, and lines 1–3 of `ply_eval::differential` say
/// why that is the one comparison that hides what it exists to catch.
fn verify(
    loaded: &'static Loaded,
    harness: &mut Harness,
    seed: u64,
    cases: usize,
) -> Result<Agreement> {
    let mut rng = Rng(seed);
    let mut bad = Vec::new();
    let mut n_fn = 0;
    let mut n_compiled = 0;
    let mut n_case = 0;
    let mut n_nonscalar = 0;
    let mut n_raising = 0;
    let mut n_declined = 0;
    let shapes = non_scalar_arguments();

    for name in subjects(loaded) {
        let Some(kinds) = scalar_params(loaded, &name) else {
            continue;
        };
        let compiled = harness.compiled().entry(&name).is_some();
        n_fn += 1;
        if compiled {
            n_compiled += 1;
        }

        for (case, Case { args, scalar }) in cases_for(&mut rng, &name, &kinds, cases, &shapes)
            .into_iter()
            .enumerate()
        {
            n_case += 1;
            if !scalar {
                n_nonscalar += 1;
            }
            let before = (entries_for(harness, &name), harness.bodies.declines());
            let expected = harness.interpret_outcome(&name, &args);
            let hybrid = harness.hybrid_outcome(&name, &args);
            let after = (entries_for(harness, &name), harness.bodies.declines());
            if expected.is_err() {
                n_raising += 1;
            }
            // A refused kind must not be carried into `name`'s own body, and must
            // not make any compiled body run and fail — the two ways a boundary
            // that checked one argument would show up. The totals are *not* the
            // check: `mcts.plan_753(1.5)` enters `mcts.pack` legitimately before
            // it raises.
            if !scalar && (after.0 != before.0 || after.1.failed != before.1.failed) {
                bad.push(format!(
                    "{name} case {case}: the boundary carried an argument kind it refuses \
                     ({} entries and {} failed bodies became {} and {})",
                    before.0, before.1.failed, after.0, after.1.failed
                ));
            }
            if let Some(d) =
                compare_answers(&harness.machine, &harness.hybrid, &name, &expected, &hybrid)
            {
                bad.push(format!("{name} case {case}: with a backend attached, {d}"));
            }

            let mut interp = Interp::new(&loaded.ast, &loaded.resolved, &loaded.check);
            let walked = interp.call(&name, args.clone(), Span::DUMMY);
            if let Some(d) = compare_answers(&interp, &harness.machine, &name, &walked, &expected) {
                bad.push(format!("{name} case {case}: the tree-walker {d}"));
            }

            // And the fragment on its own, which is the only place its answers
            // are visible at all: a decline is invisible through the machine by
            // design. Only for the functions it compiled — a name it refused has
            // no body to call and would report every case as a decline.
            if !compiled {
                continue;
            }
            let direct = harness.compiled_call(&name, &args);
            match (&expected, &direct) {
                (Ok(a), Ok(b)) => {
                    if !ply_eval::values_equal(a, b, Span::DUMMY).unwrap_or(false) {
                        bad.push(format!(
                            "{name} case {case}: the machine answered {} and the fragment \
                             answered {}",
                            a.render(),
                            b.render()
                        ));
                    }
                }
                (Ok(_), Err(_)) => n_declined += 1,
                (Err(_), Ok(v)) => bad.push(format!(
                    "{name} case {case}: the machine raised and the fragment answered {}",
                    v.render()
                )),
                (Err(_), Err(_)) => {}
            }
        }
    }

    // And the whole kernel, which is the claim that actually matters: a search
    // the interpreter drives, dropping into compiled code at the leaves, answers
    // the move both interpreters answer.
    let mut whole = 0;
    for case in 0..24 {
        let a = 1 + (rng.next() % 8) as i64;
        let b = (rng.next() % 8) as i64;
        let c = (rng.next() % 8) as i64;
        let state = a + b * 16 + c * 256;
        let s = 1 + (rng.next() % 1_000_000) as i64;
        let n = 1 + (rng.next() % 40) as i64;
        let args = vec![Value::Int(state), Value::Int(s), Value::Int(n)];
        whole += 1;
        let expected = harness.interpret_outcome("mcts.plan", &args);
        let hybrid = harness.hybrid_outcome("mcts.plan", &args);
        if let Some(d) = compare_answers(
            &harness.machine,
            &harness.hybrid,
            "mcts.plan",
            &expected,
            &hybrid,
        ) {
            bad.push(format!(
                "mcts.plan case {case} (state {state}, seed {s}, {n} iterations): {d}"
            ));
        }
        let mut interp = Interp::new(&loaded.ast, &loaded.resolved, &loaded.check);
        let walked = interp.call("mcts.plan", args, Span::DUMMY);
        if let Some(d) = compare_answers(&interp, &harness.machine, "mcts.plan", &walked, &expected)
        {
            bad.push(format!("mcts.plan case {case}: the tree-walker {d}"));
        }
    }

    let (entries, _) = harness.hybrid_counts();
    Ok(Agreement {
        functions: n_fn,
        compiled_functions: n_compiled,
        cases: n_case,
        non_scalar_cases: n_nonscalar,
        raising_cases: n_raising,
        whole_kernel_cases: whole,
        fragment_declined: n_declined,
        entries,
        entered_functions: harness.bodies.entries_by_name().len(),
        corpus_out_of_fuel: harness.bodies.declines().out_of_fuel,
        disagreements: bad,
    })
}

/// Native entries into one function, which is what a refused-kind case must not
/// move. The totals cannot serve: a call that raises on a refused kind may
/// legitimately have entered *other* functions first.
fn entries_for(harness: &Harness, name: &str) -> u64 {
    harness
        .bodies
        .entries_by_name()
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, count)| count)
        .unwrap_or(0)
}

/// Two deep recursions, both compared field by field rather than described.
///
/// ADR 0019 §5 item 6 records the nested-call bound as closed and the R5 audit
/// asks to *see* it: a program the machine answers with `recursion limit of
/// 10000 nested calls exceeded` must answer the same thing with a backend
/// attached, down to the labels and the spans, and must not become a `SIGABRT`.
///
/// The two are different shapes and only the first reaches the bound:
///
///   * **`mcts.playouts` from a terminal position** recurses once per playout
///     and plays no game, so what it exercises is the nesting. It is itself
///     compiled, so the machine offers it, the native body burns its whole fuel,
///     declines, and the machine evaluates one level and offers it again — the
///     O(`max_calls`²) cost `entry::Declines::out_of_fuel` records. Two more
///     cases in the corpus above reach the bound the same way, from
///     `mcts.playouts(3, 3, i64::MAX)` and from `i64::MAX / 2`, which together
///     with this one are the run's 30,000 fuel declines.
///   * **`work.plies`** is *not* compiled, so the recursion is the machine's own
///     and it drops into compiled `mcts.next_seed`/`mcts.below`/`mcts.nth_move`
///     at every depth — the half the audit calls unobserved, since before R5 a
///     crossing went through `Machine::call`, which resets `stack.calls()`. It
///     does **not** reach the bound: a playout ends when the position is
///     terminal, so the fuel argument is not what stops it. The version that
///     does reach the bound with an entry at every depth is
///     `tests/hazards.rs::an_interpreted_recursion_entering_compiled_code_at_every_depth_is_bounded`,
///     over a fixture whose recursion no game terminates.
fn deep_recursion(loaded: &'static Loaded, harness: &mut Harness) -> Vec<String> {
    let mut bad = Vec::new();
    let deep: [(&str, Vec<Value>); 2] = [
        (
            "mcts.playouts",
            vec![Value::Int(0), Value::Int(1), Value::Int(5_000_000)],
        ),
        (
            "work.plies",
            vec![
                Value::Int(7 + 5 * 16 + 3 * 256),
                Value::Int(20260821),
                Value::Int(5_000_000),
            ],
        ),
    ];
    for (name, args) in deep {
        let expected = harness.interpret_outcome(name, &args);
        let hybrid = harness.hybrid_outcome(name, &args);
        if let Some(d) =
            compare_answers(&harness.machine, &harness.hybrid, name, &expected, &hybrid)
        {
            bad.push(format!("{name} at the nested-call bound: {d}"));
        }
        let mut interp = Interp::new(&loaded.ast, &loaded.resolved, &loaded.check);
        let walked = interp.call(name, args.clone(), Span::DUMMY);
        if let Some(d) = compare_answers(&interp, &harness.machine, name, &walked, &expected) {
            bad.push(format!(
                "{name} at the nested-call bound: the tree-walker {d}"
            ));
        }
    }
    bad
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
    /// Native bodies entered during one call of the whole kernel. **The number
    /// R5 exists to move.** In the hybrid ADR 0018 §0 reports, compiled code was
    /// reached by exactly three functions and 102 crossings; a rung whose
    /// interpreter never enters anything is a null result whatever its ratio
    /// says, and this is the field that says so.
    entries_per_call: f64,
    declines_per_call: f64,
    entries_by_function: Vec<(String, u64)>,
}

/// The compiled sets the ladder walks, each a superset of the one above.
///
/// They are *leaves* the interpreter drops into rather than drivers it is
/// entered from. Before R5 a rung named the outermost function the harness could
/// enter at, and the twenty arithmetic functions under it were compiled and
/// never run — which is the whole of what ADR 0018 §0 measured as 0.998x.
fn ladder_groups<'a>(everything: &[&'a str]) -> Vec<(&'static str, &'static str, Vec<&'a str>)> {
    let exploration: Vec<&str> = vec!["mcts.ilog2", "mcts.isqrt", "mcts.isqrt_step", "mcts.ucb"];
    let playout: Vec<&str> = {
        let mut v = exploration.clone();
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
    vec![
        (
            "control: nothing enterable",
            "a backend attached with nothing in it but the nullary entry probe, so every \
             offered call declines — this rung is what the hook itself costs",
            Vec::new(),
        ),
        (
            "the exploration term",
            "`ucb` and its whole call graph: `ilog2`, `isqrt`, `isqrt_step`. ADR 0018 \
             attributes 62.6% of the kernel to it, and its only caller is refused",
            exploration,
        ),
        (
            "+ the playout",
            "the same, plus `rollout` and the position arithmetic under it",
            playout,
        ),
        (
            "everything the fragment accepts",
            "every function of the kernel the fragment did not refuse",
            everything.to_vec(),
        ),
    ]
}

/// Native entries the interpreter takes over one call, and where they went.
fn entries(
    harness: &mut Harness,
    name: &str,
    args: &[Value],
) -> Result<(f64, f64, Vec<(String, u64)>)> {
    harness.bodies.reset_counts();
    let before = harness.hybrid_counts();
    harness.run_hybrid(name, args)?;
    let after = harness.hybrid_counts();
    Ok((
        (after.0 - before.0) as f64,
        (after.1 - before.1) as f64,
        harness.bodies.entries_by_name(),
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
    pure_compute_speedup: f64,
    harness_floor: Paired,
    pure_compute: Paired,
    micros_per_ucb: f64,
    micros_per_ucb_without_sqrt: f64,
    boundary_cost_by_tree_size: Vec<Crossing>,
    /// Which functions the interpreter dropped into, and how often, over one
    /// whole-kernel call with everything the fragment accepts compiled.
    entries_by_function: Vec<(String, u64)>,
    entries_per_call: f64,
    declines: Declines,
    recursion_bound_machine: String,
    recursion_bound_hybrid: String,
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
        match h.hybrid_outcome("mcts.playouts", &args) {
            Ok(v) => println!("the hybrid answered {}", v.render()),
            Err(d) => println!("the hybrid raised: {}", d.message),
        }
        // Which half did the work matters: a hybrid that was never offered the
        // call would answer the machine's diagnostic for the machine's reason,
        // and this probe would say nothing about the fragment's own bound.
        let d = h.bodies.declines();
        println!(
            "   the fragment was entered {} times and declined {} of them for running out of \
             fuel ({} for other reasons)",
            h.bodies.entered(),
            d.out_of_fuel,
            d.total() - d.out_of_fuel
        );
    } else {
        match h.interpret_outcome("mcts.playouts", &args) {
            Ok(v) => println!("machine answered {}", v.render()),
            Err(d) => println!("machine raised: {}", d.message),
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
    let (rows, accepted) = census(loaded, KERNEL)?;
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
            None if r.enterable => {
                println!("   enterable  {:<24} {:>4} nodes", r.function, r.nodes)
            }
            None => println!(
                "   compiled   {:<24} {:>4} nodes   (reachable natively, never entered)",
                r.function, r.nodes
            ),
            Some(why) => println!(
                "   refused    {:<24} {:>4} nodes   {why}",
                r.function, r.nodes
            ),
        }
    }
    println!(
        "\n   {} of {} functions, {} of {} lowered nodes, are inside the fragment; {} of them \
         can be entered from the interpreter",
        accepted.len(),
        rows.len(),
        accepted_nodes,
        total_nodes,
        rows.iter().filter(|r| r.enterable).count()
    );
    println!("\n== what is outside, ranked by the nodes it takes with it ==");
    for (why, n, nodes) in &refusals {
        println!("   {nodes:>4} nodes  {n:>2} function(s)  {why}");
    }

    // -------------------------------------------- 2. compile and verify --
    let names: Vec<&str> = accepted.iter().map(|s| s.as_str()).collect();
    let mut harness = Harness::over(loaded, &names, Opts::default(), Some(ENTRY))?;
    let mut agreement = verify(loaded, &mut harness, 0x9E3779B97F4A7C15, 64)?;
    agreement
        .disagreements
        .extend(deep_recursion(loaded, &mut harness));
    println!(
        "\n== agreement, before anything is timed ==\n   \
         {} functions ({} of them compiled) × generated inputs = {} cases, plus {} whole-kernel \
         searches\n   and 2 deep recursions, against the machine and the tree-walker",
        agreement.functions,
        agreement.compiled_functions,
        agreement.cases,
        agreement.whole_kernel_cases
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
    println!(
        "   {} of those cases were arguments the boundary refuses on sight, {} raised in the \
         machine,\n   {} were declined by the fragment and answered by the interpreter instead",
        agreement.non_scalar_cases, agreement.raising_cases, agreement.fragment_declined
    );
    println!(
        "\n== the number R5 exists to move ==\n   the interpreter entered compiled code \
         {} times over the agreement corpus.\n   Before R5 it could not enter compiled code at \
         all: the hybrid reached three functions, and\n   every `ucb`, `isqrt` and `rollout` \
         under them ran in the machine (ADR 0018 §0).",
        agreement.entries
    );
    println!(
        "   {} distinct compiled functions were entered: {:?}",
        agreement.entered_functions,
        harness.bodies.entries_by_name()
    );
    println!(
        "   {} of the generated cases reached the machine's own bound on nested calls before the \
         two deliberate ones",
        agreement.corpus_out_of_fuel
    );
    let declines = harness.bodies.declines();
    println!(
        "   declines: {} not compiled, {} arity, {} the body failed, {} out of fuel, {} re-entered, \
         {} touched a cell",
        declines.not_compiled,
        declines.arity,
        declines.failed,
        declines.out_of_fuel,
        declines.reentered,
        declines.touched_cells
    );
    if harness.hybrid.compiled_refusals() != 0 {
        bail!(
            "the machine refused {} answer(s) at the boundary: the backend returned a value this \
             boundary does not carry, which is a backend bug rather than a fragment limit",
            harness.hybrid.compiled_refusals()
        );
    }
    if a.only.as_deref() == Some("entries") {
        println!(
            "\n== native entries per `{WHOLE}({})` call, by compiled set ==\n   \
             deterministic: one position, one seed, a fixed iteration count. Nothing timed.",
            a.iterations
        );
        let arg = Value::Int(a.iterations);
        for (name, _, group) in ladder_groups(&names) {
            let mut h = Harness::over(loaded, &group, Opts::default(), Some(ENTRY))?;
            let (taken, declined, by_function) =
                entries(&mut h, WHOLE, std::slice::from_ref(&arg))?;
            println!("   {name:<32} {taken:>8.0} entries, {declined:>8.0} declines");
            for (f, c) in &by_function {
                println!("      {c:>8}  {f}");
            }
            if by_function.is_empty() {
                println!("      nothing entered");
            }
        }
        return Ok(());
    }
    if a.only.as_deref() == Some("agreement") {
        println!(
            "\n--only agreement: nothing was timed. Correctness first; a ratio taken on a loaded \
             machine is a ratio between two machines."
        );
        return Ok(());
    }

    // --------------------------------------------------- 3. entry costs --
    let entry_interp = band(
        || {
            harness.interpret(ENTRY, &[])?;
            Ok(())
        },
        2000,
        a.repeats,
    )?;
    let entry_compiled = band(
        || {
            harness.compiled_call(ENTRY, &[])?;
            Ok(())
        },
        2000,
        a.repeats,
    )?;
    let n = Value::Int(a.iterations);
    let n_probe = n.clone();

    // The same search on the two `Machine`s this harness holds, with the backend
    // attached to neither: nothing is entered on either side. It exists so that
    // a rung's ratio can be read against what the harness costs rather than
    // against 1.00x assumed.
    let floor = {
        let cell = std::cell::RefCell::new(&mut harness);
        paired(
            || {
                cell.borrow_mut()
                    .interpret(WHOLE, std::slice::from_ref(&n_probe))?;
                Ok(())
            },
            || {
                cell.borrow_mut()
                    .interpret(WHOLE, std::slice::from_ref(&n_probe))?;
                Ok(())
            },
            a.inner,
            a.repeats,
        )?
    };

    // ------------------------------------------------- 4. the timing ladder --
    let mut rungs = Vec::new();

    let ladder = ladder_groups(&names);

    let mut whole_entries_by_function = Vec::new();
    let mut whole_declines = Declines::default();
    for (name, what, group) in &ladder {
        let mut h = Harness::over(loaded, group, Opts::default(), Some(ENTRY))?;
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
                        .run_hybrid(WHOLE, std::slice::from_ref(&arg))?;
                    Ok(())
                },
                a.inner,
                a.repeats,
            )?
        };
        let interp = p.a;
        let hybrid = p.b;
        let (taken, declined, by_function) = entries(&mut h, WHOLE, std::slice::from_ref(&n))?;
        if *name == "everything the fragment accepts" {
            whole_entries_by_function = by_function.clone();
            whole_declines = h.bodies.declines();
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
            entries_per_call: taken,
            declines_per_call: declined,
            entries_by_function: by_function,
        });
    }

    // ------------------------------- 5. the same fragment with no tree at all --
    let pure_state = Value::Int(7 + 5 * 16 + 3 * 256);
    let pure_args = vec![
        pure_state.clone(),
        Value::Int(20260821),
        Value::Int(a.iterations),
    ];
    let pure = {
        let cell = std::cell::RefCell::new(&mut harness);
        paired(
            || {
                cell.borrow_mut().interpret(PURE, &pure_args)?;
                Ok(())
            },
            || {
                cell.borrow_mut().run_hybrid(PURE, &pure_args)?;
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

    let last = rungs.last().expect("four rungs");
    let end_to_end = last.speedup;
    let entries_per_call = last.entries_per_call;

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
        "   the same search timed against itself, no backend on either side:\n   \
         {:.1} µs and {:.1} µs — {:.3}x [{:.3}–{:.3}], which is what this method's own noise\n   \
         looks like, and what a rung's ratio is read against",
        floor.a.best, floor.b.best, floor.median, floor.low, floor.high
    );
    for r in &rungs {
        println!(
            "   {:<32} interpreter {:>9.1} µs   hybrid {:>9.1} µs   {:.3}x [{:.3}–{:.3}]   \
             {:.0} entries, {:.0} declines",
            r.name,
            r.interpreter.best,
            r.hybrid.best,
            r.speedup,
            r.low,
            r.high,
            r.entries_per_call,
            r.declines_per_call
        );
    }
    println!("\n   end to end, everything the fragment accepts: {end_to_end:.3}x");
    if last.entries_per_call == 0.0 {
        println!(
            "   NULL RESULT: the interpreter entered compiled code zero times, so the ratio \n   \
             above is a measurement of noise. R4's 0.998x was exactly this and nothing said so."
        );
    }

    println!("\n== the same fragment with the tree removed: `{PURE}` ==");
    println!(
        "   interpreter {:>9.1} µs   compiled {:>9.1} µs   {pure_speedup:.2}x [{:.2}–{:.2}], \
         entered from the interpreter",
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

    println!("\n== where the interpreter dropped into compiled code, per whole-kernel call ==");
    for (name, count) in &whole_entries_by_function {
        println!("   {count:>8}  {name}");
    }
    if whole_entries_by_function.is_empty() {
        println!("   nothing. Every number above is a null result.");
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
        pure_compute_speedup: pure_speedup,
        harness_floor: floor.clone(),
        pure_compute: pure.clone(),
        micros_per_ucb: t_ucb,
        micros_per_ucb_without_sqrt: t_ucb_free,
        boundary_cost_by_tree_size: boundary,
        entries_by_function: whole_entries_by_function,
        entries_per_call,
        declines: whole_declines,
        recursion_bound_machine: machine_says,
        recursion_bound_hybrid: compiled_says,
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
