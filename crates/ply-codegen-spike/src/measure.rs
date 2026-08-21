//! The comparison, and the rules that decide whether it is evidence.
//!
//! Both sides answer the same `Value`s in the same process, and every input is
//! checked for agreement against **both** evaluators before anything is timed:
//! a faster wrong answer prices nothing.

use crate::jit::{Compiled, Entry, Jit, Opts};
use crate::program::Loaded;
use crate::rt::Ctx;
use anyhow::{Result, bail};
use ply_eval::{Interp, Machine, Value, values_equal};
use ply_span::Span;
use serde::Serialize;
use std::time::Instant;

/// The three functions the fragment covers, compiled as one unit. `read_line`
/// is the one under measurement; the other two are its whole call graph, and
/// compiling them is what keeps the ratio from being a trampoline's.
pub const GROUP: &[&str] = &[
    "std.http.read_line",
    "std.http.line_at",
    "std.http.line_stops",
];

/// The nullary function both entry costs are read off: its body is one literal,
/// so what it measures is the cost of arriving.
pub const ENTRY_FN: &str = "std.http.chunk_budget";

pub struct Input {
    pub name: String,
    pub args: Vec<Value>,
}

#[derive(Clone, Debug, Serialize)]
pub struct InputResult {
    pub name: String,
    pub interpreter_best_micros: f64,
    pub interpreter_worst_micros: f64,
    pub spike_best_micros: f64,
    pub spike_worst_micros: f64,
    pub agreed: bool,
}

pub struct Harness {
    pub loaded: &'static Loaded,
    pub compiled: Compiled,
    pub ctx: Ctx,
    pub machine: Machine<'static>,
}

impl Harness {
    /// `loaded` is leaked because a `Machine` borrows its program for as long as
    /// it lives and this binary makes exactly one.
    pub fn new(names: &[&str]) -> Result<Harness> {
        Harness::with(names, Opts::default())
    }

    pub fn with(names: &[&str], opts: Opts) -> Result<Harness> {
        let loaded: &'static Loaded = Box::leak(Box::new(Loaded::std_library()?));
        Harness::over(loaded, names, opts, Some(ENTRY_FN))
    }

    /// The same, over a program somebody else loaded, and with the entry-cost
    /// function named by the caller — a project that does not import
    /// `std.http` has no [`ENTRY_FN`] to add.
    pub fn over(
        loaded: &'static Loaded,
        names: &[&str],
        opts: Opts,
        entry: Option<&str>,
    ) -> Result<Harness> {
        let mut all: Vec<&str> = names.to_vec();
        if let Some(entry) = entry
            && !all.contains(&entry)
        {
            all.push(entry);
        }
        let compiled = Jit::compile_with(loaded, &all, opts)?;
        let mut ctx = compiled.context();
        ctx.machine = Some(Box::new(Machine::new(
            &loaded.ast,
            &loaded.resolved,
            &loaded.check,
        )));
        let machine = Machine::new(&loaded.ast, &loaded.resolved, &loaded.check);
        Ok(Harness {
            loaded,
            compiled,
            ctx,
            machine,
        })
    }

    pub fn interpret(&mut self, name: &str, args: &[Value]) -> Result<Value> {
        self.machine
            .call(name, args.to_vec(), Span::DUMMY)
            .map_err(|d| anyhow::anyhow!("`{name}` raised: {}", d.message))
    }

    /// The compiled call, with the arena reset and the arguments boxed — the
    /// whole of what a caller pays to enter compiled code here.
    pub fn compiled_call(&mut self, entry: Entry, args: &[Value]) -> Result<Value> {
        self.ctx.reset();
        let handles: Vec<i64> = args.iter().map(|a| self.ctx.push(a.clone())).collect();
        let out = unsafe { entry(&mut self.ctx as *mut Ctx, handles.as_ptr()) };
        if self.ctx.failed != 0 {
            let d = self.ctx.take_failure();
            bail!(
                "compiled code raised: {}",
                d.map(|d| d.message).unwrap_or_else(|| "no message".into())
            );
        }
        Ok(self.ctx.read(out).clone())
    }
}

fn micros(started: Instant, iterations: u32) -> f64 {
    started.elapsed().as_secs_f64() * 1e6 / f64::from(iterations)
}

/// Best and worst per-call microseconds over `repeats` runs of `iterations`
/// calls. Best-of because the quantity of interest is the cost of the work;
/// worst-of because a ratio taken between two best-of numbers is a claim the
/// noise has not been asked about.
pub struct Band {
    pub best: f64,
    pub worst: f64,
}

fn band(mut run: impl FnMut() -> Result<()>, iterations: u32, repeats: u32) -> Result<Band> {
    let mut best = f64::INFINITY;
    let mut worst: f64 = 0.0;
    for _ in 0..repeats {
        let started = Instant::now();
        for _ in 0..iterations {
            run()?;
        }
        let taken = micros(started, iterations);
        best = best.min(taken);
        worst = worst.max(taken);
    }
    Ok(Band { best, worst })
}

pub struct Measured {
    pub results: Vec<InputResult>,
}

/// Runs the comparison over every input, agreement first.
pub fn compare(
    harness: &mut Harness,
    function: &str,
    inputs: &[Input],
    iterations: u32,
    repeats: u32,
) -> Result<Measured> {
    let entry = harness
        .compiled
        .entry(function)
        .ok_or_else(|| anyhow::anyhow!("`{function}` was not compiled"))?;

    let mut results = Vec::new();
    for input in inputs {
        let expected = harness.interpret(function, &input.args)?;
        let actual = harness.compiled_call(entry, &input.args)?;
        let agreed = values_equal(&expected, &actual, Span::DUMMY).unwrap_or(false);

        let args = input.args.clone();
        let interpreter = {
            let h = &mut *harness;
            band(
                || {
                    h.interpret(function, &args)?;
                    Ok(())
                },
                iterations,
                repeats,
            )?
        };
        let args = input.args.clone();
        let spike = {
            let h = &mut *harness;
            band(
                || {
                    h.compiled_call(entry, &args)?;
                    Ok(())
                },
                iterations,
                repeats,
            )?
        };
        results.push(InputResult {
            name: input.name.clone(),
            interpreter_best_micros: interpreter.best,
            interpreter_worst_micros: interpreter.worst,
            spike_best_micros: spike.best,
            spike_worst_micros: spike.worst,
            agreed,
        });
    }
    Ok(Measured { results })
}

/// Agreement with the tree-walker as well, so the comparison is against an
/// evaluator that `--engine both` already polices (ADR 0016 §7 test 14).
pub fn agrees_with_treewalk(harness: &mut Harness, function: &str, input: &Input) -> Result<bool> {
    let mut interp = Interp::new(
        &harness.loaded.ast,
        &harness.loaded.resolved,
        &harness.loaded.check,
    );
    let expected = interp
        .call(function, input.args.clone(), Span::DUMMY)
        .map_err(|d| anyhow::anyhow!("the tree-walker raised: {}", d.message))?;
    let entry = harness
        .compiled
        .entry(function)
        .ok_or_else(|| anyhow::anyhow!("`{function}` was not compiled"))?;
    let actual = harness.compiled_call(entry, &input.args)?;
    Ok(values_equal(&expected, &actual, Span::DUMMY).unwrap_or(false))
}

/// The minimum conservative ratio — interpreter best over spike worst — across
/// every input, which is the number a decision may use.
pub fn speedup(results: &[InputResult]) -> f64 {
    results
        .iter()
        .map(|r| {
            if r.spike_worst_micros > 0.0 {
                r.interpreter_best_micros / r.spike_worst_micros
            } else {
                0.0
            }
        })
        .fold(f64::INFINITY, f64::min)
}
