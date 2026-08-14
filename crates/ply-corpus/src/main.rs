use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use ply_corpus::bench;
use ply_corpus::build::generate;
use ply_corpus::measure;
use ply_corpus::spec::CorpusSpec;
use ply_corpus::write;
use ply_eval::{Engine, EngineChoice};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "ply-corpus",
    about = "Generate a scale corpus for Ply, and measure where a run's time goes"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Write a synthetic project, then compile and run it to prove it is real.
    Gen(GenArgs),
    /// Time parse, typecheck, hash, cache lookup and execution over a corpus.
    Bench(BenchArgs),
    /// Generate and benchmark at several sizes, for one comparison table.
    Sweep(SweepArgs),
    /// Price the control-stack machine's own claims: interpreter throughput
    /// against the tree-walker, fork against rebuild, resumption cost, and
    /// what isolation did to the schedule.
    Measure(MeasureArgs),
    /// Price the search: interleavings pruned against unpruned, seeds to the
    /// first failure against sampling, and seeds per second.
    Sim(SimArgs),
}

#[derive(Args, Debug, Clone)]
struct ShapeArgs {
    #[arg(long, default_value_t = 1)]
    seed: u64,
    #[arg(long, default_value_t = 20)]
    modules: usize,
    #[arg(long, default_value_t = 25)]
    defs_per_module: usize,
    #[arg(long, default_value_t = 200)]
    tests: usize,
    /// Layers in the module import DAG.
    #[arg(long, default_value_t = 4)]
    depth: usize,
    /// Distinct `db` resource labels, shared across the whole corpus.
    #[arg(long, default_value_t = 12)]
    tables: usize,
    /// Distinct `cache` resource labels.
    #[arg(long, default_value_t = 6)]
    regions: usize,
    #[arg(long, default_value_t = 0.35)]
    effect_fraction: f64,
    #[arg(long, default_value_t = 0.03)]
    nondet_fraction: f64,
    #[arg(long, default_value_t = 3)]
    hub_modules: usize,
    #[arg(long, default_value_t = 192)]
    max_weight: u32,
    /// `simulate` tests, on top of `--tests`.
    #[arg(long, default_value_t = 0)]
    concurrent_tests: usize,
    #[arg(long, default_value_t = 3)]
    tasks_per_test: usize,
    /// `counter.bump` calls per task, separated by a `task.yield()`.
    #[arg(long, default_value_t = 2)]
    steps_per_task: usize,
    /// 0.0 gives every task its own resource, so nothing conflicts and the
    /// search collapses; 1.0 puts every task on one, so nothing prunes.
    #[arg(long, default_value_t = 0.5)]
    conflict_density: f64,
}

impl From<ShapeArgs> for CorpusSpec {
    fn from(a: ShapeArgs) -> CorpusSpec {
        CorpusSpec {
            seed: a.seed,
            modules: a.modules,
            defs_per_module: a.defs_per_module,
            tests: a.tests,
            depth: a.depth,
            tables: a.tables,
            regions: a.regions,
            effect_fraction: a.effect_fraction,
            nondet_fraction: a.nondet_fraction,
            hub_modules: a.hub_modules,
            max_weight: a.max_weight,
            concurrent_tests: a.concurrent_tests,
            tasks_per_test: a.tasks_per_test,
            steps_per_task: a.steps_per_task,
            conflict_density: a.conflict_density,
        }
    }
}

#[derive(Args, Debug)]
struct GenArgs {
    /// Where to write it. Must be empty, or a corpus this tool already wrote.
    #[arg(long)]
    out: PathBuf,
    #[command(flatten)]
    shape: ShapeArgs,
    /// Write the corpus without compiling it. Only useful for inspecting output
    /// the compiler has already rejected.
    #[arg(long)]
    no_verify: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
struct BenchArgs {
    /// A directory a previous `gen` wrote.
    corpus: PathBuf,
    /// Repeats per scenario; the fastest run is reported.
    #[arg(long, default_value_t = 3)]
    repeats: usize,
    /// The evaluator whose wall clock the `execute` phase is measuring.
    #[arg(long, default_value = "machine", value_parser = parse_engine)]
    engine: EngineChoice,
    #[arg(long)]
    json: bool,
}

fn parse_engine(s: &str) -> Result<EngineChoice, String> {
    EngineChoice::parse(s).ok_or_else(|| format!("expected treewalk, machine or both, got `{s}`"))
}

#[derive(Args, Debug)]
struct SweepArgs {
    /// A directory to hold one sub-directory per size.
    #[arg(long)]
    out: PathBuf,
    /// Sizes to sweep, each `modules,defs_per_module,tests`.
    #[arg(long, value_name = "M,D,T", num_args = 1.., value_delimiter = ' ')]
    sizes: Vec<String>,
    #[arg(long, default_value_t = 1)]
    seed: u64,
    #[arg(long, default_value_t = 3)]
    repeats: usize,
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
struct MeasureArgs {
    /// A directory a previous `gen` wrote. Omit it for the measurements that
    /// need no corpus — fork cost and resumption cost.
    corpus: Option<PathBuf>,
    /// Repeats per measurement; the fastest is reported.
    #[arg(long, default_value_t = 3)]
    repeats: usize,
    /// Which engines the throughput table covers. One engine reports no ratio,
    /// and is what a profiler should be pointed at.
    #[arg(long, default_value = "both", value_parser = parse_engine)]
    engine: EngineChoice,
    /// World sizes for the fork comparison.
    #[arg(long, value_delimiter = ',', default_values_t = [1usize, 100, 1_000, 10_000, 100_000])]
    cells: Vec<usize>,
    /// Skip everything but the throughput table, so a profile is not dominated
    /// by measurements that are not being investigated.
    #[arg(long)]
    only_throughput: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
struct SimArgs {
    /// A `.ply` file, or a directory a previous `gen` wrote.
    corpus: PathBuf,
    /// Roots per strategy in the race-finding table. Zero drops that table,
    /// which is what to do on a corpus with no failing test.
    #[arg(long, default_value_t = 32)]
    trials: u32,
    /// Interleavings a search may run per root. The unpruned side gets the same
    /// one, so a spent budget is reported on both.
    #[arg(long, default_value_t = 4096)]
    budget: u32,
    /// Scheduling steps one interleaving may take.
    #[arg(long, default_value_t = ply_eval::sim::DEFAULT_STEPS)]
    steps: u32,
    /// Seeds the throughput table times. Sampled, so the rate is one whole test
    /// per seed with no search state between them.
    #[arg(long, default_value_t = 64)]
    rate_seeds: u32,
    /// Drop the reduction table, which is the expensive one.
    #[arg(long)]
    no_reduction: bool,
    #[arg(long)]
    json: bool,
}

fn simulate(args: SimArgs) -> Result<()> {
    let out = ply_corpus::simulate::SimMeasurements {
        root: args.corpus.display().to_string(),
        reduction: if args.no_reduction {
            Vec::new()
        } else {
            ply_corpus::simulate::reduction(&args.corpus, args.budget, args.steps)?
        },
        race: if args.trials == 0 {
            Vec::new()
        } else {
            ply_corpus::simulate::race_power(&args.corpus, args.trials, args.budget, args.steps)?
        },
        rate: if args.rate_seeds == 0 {
            Vec::new()
        } else {
            ply_corpus::simulate::seed_rate(&args.corpus, args.rate_seeds, args.steps)?
        },
    };
    if args.json {
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        print!("{}", ply_corpus::simulate::render(&out));
    }
    Ok(())
}

fn measure(args: MeasureArgs) -> Result<()> {
    let engines: Vec<Engine> = match args.engine {
        EngineChoice::Treewalk => vec![Engine::Treewalk],
        EngineChoice::Machine => vec![Engine::Machine],
        EngineChoice::Both => vec![Engine::Treewalk, Engine::Machine],
    };

    let mut out = measure::Measurements {
        throughput: None,
        scheduling: None,
        store_open: None,
        fork: Vec::new(),
        multi_shot: None,
    };
    if !args.only_throughput {
        out.fork = measure::fork_cost(&args.cells, args.repeats);
        out.multi_shot = Some(measure::multi_shot(args.repeats)?);
    }

    if let Some(root) = &args.corpus {
        out.throughput = Some(measure::throughput(root, &engines, args.repeats)?);
        if !args.only_throughput {
            // Scheduling clears the cache, so the store is timed before it
            // rather than over the empty one it leaves behind.
            out.store_open = Some(measure::store_open(root, args.repeats)?);
            out.scheduling = Some(measure::scheduling(root)?);
        }
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        print!("{}", measure::render(&out));
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("ply-corpus: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Gen(args) => generate_corpus(args),
        Command::Bench(args) => {
            let report = bench::run(
                &args.corpus,
                &bench::Options {
                    repeats: args.repeats,
                    engine: args.engine,
                },
            )?;
            emit_report(&report, args.json)
        }
        Command::Sweep(args) => sweep(args),
        Command::Measure(args) => measure(args),
        Command::Sim(args) => simulate(args),
    }
}

fn generate_corpus(args: GenArgs) -> Result<()> {
    let spec: CorpusSpec = args.shape.into();
    spec.validate()?;

    let corpus = generate(&spec);
    let written = write::write(&args.out, &spec, &corpus)?;
    let manifest = written.manifest;

    let verified = if args.no_verify {
        None
    } else {
        Some(ply_corpus::verify(&args.out).context("the generated corpus does not compile")?)
    };

    if args.json {
        let value = serde_json::json!({
            "root": args.out.display().to_string(),
            "manifest": manifest,
            "verified": verified.as_ref().map(|v| serde_json::json!({
                "definitions": v.definitions,
                "tests": v.tests,
                "passed": v.passed,
                "groups": v.groups,
                "largest_group": v.largest_group,
                "seeded": v.seeded,
            })),
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    println!("wrote {}", args.out.display());
    println!(
        "  {} modules · {} definitions ({} effectful) · {} tests ({} nondet)",
        manifest.modules,
        manifest.definitions,
        manifest.effectful_definitions,
        manifest.tests,
        manifest.nondet_tests
    );
    if manifest.concurrency.tests > 0 {
        let c = &manifest.concurrency;
        println!(
            "  {} concurrent tests · {} tasks × {} steps over {} shards · contention {:.2} (asked {:.2})",
            c.tests,
            c.tasks_per_test,
            c.steps_per_task,
            c.shards_per_test,
            c.contention,
            c.conflict_density
        );
    }
    println!(
        "  {} KiB of source · mean out-degree {:.2} · {} distinct resources",
        manifest.bytes / 1024,
        manifest.mean_out_degree,
        manifest.distinct_resources
    );
    match verified {
        Some(v) => println!(
            "  verified: {} tests passed in {} concurrency group(s), largest {}",
            v.passed, v.groups, v.largest_group
        ),
        None => println!("  not verified (--no-verify)"),
    }
    Ok(())
}

fn sweep(args: SweepArgs) -> Result<()> {
    let mut reports = Vec::new();
    for size in &args.sizes {
        let spec = parse_size(size, args.seed)?;
        spec.validate()?;
        let root = args.out.join(format!(
            "m{}_d{}_t{}",
            spec.modules, spec.defs_per_module, spec.tests
        ));
        write::write(&root, &spec, &generate(&spec))?;
        ply_corpus::verify(&root)
            .with_context(|| format!("the corpus for `{size}` does not compile"))?;
        reports.push(bench::run(
            &root,
            &bench::Options {
                repeats: args.repeats,
                engine: EngineChoice::default(),
            },
        )?);
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
        return Ok(());
    }
    for report in &reports {
        println!("{}", bench::render(report));
    }
    Ok(())
}

/// `modules,defs_per_module,tests` — the three numbers that move together when
/// a corpus is scaled, so a sweep names sizes rather than repeating ten flags.
fn parse_size(size: &str, seed: u64) -> Result<CorpusSpec> {
    let parts: Vec<&str> = size.split(',').collect();
    if parts.len() != 3 {
        anyhow::bail!("`{size}` is not `modules,defs_per_module,tests`");
    }
    let number = |s: &str| -> Result<usize> {
        s.trim()
            .parse()
            .with_context(|| format!("`{s}` is not a number"))
    };
    let modules = number(parts[0])?;
    Ok(CorpusSpec {
        seed,
        modules,
        defs_per_module: number(parts[1])?,
        tests: number(parts[2])?,
        depth: 4.min(modules.max(1)),
        ..CorpusSpec::default()
    })
}

fn emit_report(report: &bench::Report, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
    } else {
        print!("{}", bench::render(report));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_tree_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn a_size_is_three_numbers_and_nothing_else() {
        let spec = parse_size("10,20,30", 5).unwrap();
        assert_eq!(
            (spec.modules, spec.defs_per_module, spec.tests),
            (10, 20, 30)
        );
        assert_eq!(spec.seed, 5);
        assert!(parse_size("10,20", 1).is_err());
        assert!(parse_size("10,20,x", 1).is_err());
    }

    #[test]
    fn a_sweep_never_asks_for_more_layers_than_it_has_modules() {
        let spec = parse_size("2,5,5", 1).unwrap();
        spec.validate().unwrap();
        assert_eq!(spec.depth, 2);
    }
}
