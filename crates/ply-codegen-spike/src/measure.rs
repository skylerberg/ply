//! The comparison, and the rules that decide whether it is evidence.

use crate::entry::{SpikeBodies, enterable};
use crate::jit::{Compiled, Jit, Opts};
use crate::program::Loaded;
use anyhow::Result;
use ply_eval::{DEFAULT_MAX_CALLS, Interp, Machine, Value, values_equal};
use ply_span::{Diagnostic, Span};
use serde::Serialize;
use std::rc::Rc;
use std::time::Instant;

/// The three functions the fragment covers, compiled as one unit.
pub const GROUP: &[&str] = &[
    "std.http.read_line",
    "std.http.line_at",
    "std.http.line_stops",
];

/// The nullary function both entry costs are read off: its body is one literal, so what it measures
/// is the cost of arriving.
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

/// One program, compiled once, with the two machines a comparison needs.
pub struct Harness {
    pub loaded: &'static Loaded,
    pub bodies: Rc<SpikeBodies>,
    /// The interpreter as shipped.
    pub machine: Machine<'static>,
    /// The same interpreter, with compiled bodies it may enter.
    pub hybrid: Machine<'static>,
}

impl Harness {
    /// `loaded` is leaked because a `Machine` borrows its program for as long as it lives and this
    /// binary makes exactly one.
    pub fn new(names: &[&str]) -> Result<Harness> {
        Harness::with(names, Opts::default())
    }

    pub fn with(names: &[&str], opts: Opts) -> Result<Harness> {
        let loaded: &'static Loaded = Box::leak(Box::new(Loaded::std_library()?));
        Harness::over(loaded, names, opts, Some(ENTRY_FN))
    }

    /// The same, over a program somebody else loaded, and with the entry-cost function named by the
    /// caller — a project that does not import `std.http` has no [`ENTRY_FN`] to add.
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
        // Only the scalar-signature members are offered to the machine.
        let compiled_names: Vec<String> = all.iter().map(|n| (*n).to_string()).collect();
        let admitted = enterable(loaded, &compiled_names);
        let bodies = Rc::new(SpikeBodies::new(loaded, compiled, &admitted)?);
        let machine = Machine::new(&loaded.ast, &loaded.resolved, &loaded.check);
        let mut hybrid = Machine::new(&loaded.ast, &loaded.resolved, &loaded.check);
        hybrid.set_compiled(bodies.clone());
        Ok(Harness {
            loaded,
            bodies,
            machine,
            hybrid,
        })
    }

    pub fn compiled(&self) -> &Compiled {
        self.bodies.compiled()
    }

    /// Puts a different backend behind the hybrid machine, over the same program and the same
    /// compiled bodies.
    pub fn set_backend(&mut self, backend: Rc<dyn ply_eval::Compiled>) {
        self.hybrid.set_compiled(backend);
    }

    /// The shipped interpreter, with no backend.
    pub fn interpret(&mut self, name: &str, args: &[Value]) -> Result<Value> {
        self.machine
            .call(name, args.to_vec(), Span::DUMMY)
            .map_err(|d| anyhow::anyhow!("`{name}` raised: {}", d.message))
    }

    /// The same, keeping the diagnostic rather than its message — what a comparison against the
    /// tree-walker needs.
    pub fn interpret_outcome(&mut self, name: &str, args: &[Value]) -> Result<Value, Diagnostic> {
        self.machine.call(name, args.to_vec(), Span::DUMMY)
    }

    /// The interpreter with the backend attached: the R5 measurement.
    pub fn hybrid_outcome(&mut self, name: &str, args: &[Value]) -> Result<Value, Diagnostic> {
        self.hybrid.call(name, args.to_vec(), Span::DUMMY)
    }

    pub fn run_hybrid(&mut self, name: &str, args: &[Value]) -> Result<Value> {
        self.hybrid
            .call(name, args.to_vec(), Span::DUMMY)
            .map_err(|d| anyhow::anyhow!("`{name}` raised: {}", d.message))
    }

    /// Native entries taken and calls declined by the machine, straight off the machine rather than
    /// off the provider — so a report cannot quote a ratio without the count that says whether
    /// anything ran.
    pub fn hybrid_counts(&self) -> (u64, u64) {
        self.hybrid.compiled_counts()
    }

    /// A direct native call, outside any machine: ADR 0016's original path, and the only one that
    /// can report the fragment's own failure.
    pub fn compiled_call(&mut self, name: &str, args: &[Value]) -> Result<Value> {
        self.bodies
            .call_direct(name, args, DEFAULT_MAX_CALLS as i64)
    }
}

fn micros(started: Instant, iterations: u32) -> f64 {
    started.elapsed().as_secs_f64() * 1e6 / f64::from(iterations)
}

/// Best and worst per-call microseconds over `repeats` runs of `iterations` calls.
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
    let mut results = Vec::new();
    for input in inputs {
        let expected = harness.interpret(function, &input.args)?;
        let actual = harness.compiled_call(function, &input.args)?;
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
                    h.compiled_call(function, &args)?;
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

/// Agreement with the tree-walker as well, so the comparison is against an evaluator that `--engine
/// both` already polices (ADR 0016 §7 test 14).
pub fn agrees_with_treewalk(harness: &mut Harness, function: &str, input: &Input) -> Result<bool> {
    let mut interp = Interp::new(
        &harness.loaded.ast,
        &harness.loaded.resolved,
        &harness.loaded.check,
    );
    let expected = interp
        .call(function, input.args.clone(), Span::DUMMY)
        .map_err(|d| anyhow::anyhow!("the tree-walker raised: {}", d.message))?;
    let actual = harness.compiled_call(function, &input.args)?;
    Ok(values_equal(&expected, &actual, Span::DUMMY).unwrap_or(false))
}

/// The minimum conservative ratio — interpreter best over spike worst — across every input, which
/// is the number a decision may use.
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
