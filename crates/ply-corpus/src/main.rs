use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use ply_corpus::bench;
use ply_corpus::build::generate;
use ply_corpus::spec::CorpusSpec;
use ply_corpus::write;
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
    #[arg(long)]
    json: bool,
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
                },
            )?;
            emit_report(&report, args.json)
        }
        Command::Sweep(args) => sweep(args),
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
