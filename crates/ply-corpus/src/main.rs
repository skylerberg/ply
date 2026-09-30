//! The corpus's transitional executor: the subcommands whose handlers are still Rust, run from the
//! JSON plan the corpus program (`crates/ply-corpus/ply`, whose `cmd.main` reads the line) hands it
//! as `ply-corpus --plan JSON`. It parses no command line of its own; `benches/corpus.sh` runs the
//! corpus.

use anyhow::{Context, Result};
use std::path::PathBuf;

#[derive(Debug, serde::Deserialize)]
struct SimArgs {
    /// A `.ply` file, or a directory a previous `gen` wrote.
    corpus: PathBuf,
    /// Roots per strategy in the race-finding table. Zero drops that table.
    trials: u32,
    /// Interleavings a search may run per root, pruned or not.
    budget: u32,
    /// Scheduling steps one interleaving may take.
    steps: u32,
    /// Seeds the throughput table times.
    rate_seeds: u32,
    /// Drop the reduction table, which is the expensive one.
    no_reduction: bool,
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

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let ran = match argv.as_slice() {
        [flag, plan] if flag == "--plan" => serde_json::from_str(plan)
            .context("the plan is not JSON")
            .and_then(run),
        _ => Err(anyhow::anyhow!(
            "this runs a plan the corpus program wrote, as `ply-corpus --plan JSON`; run the corpus \
             itself with `benches/corpus.sh`"
        )),
    };
    if let Err(e) = ran {
        eprintln!("ply-corpus: {e:#}");
        std::process::exit(1);
    }
}

/// Runs one plan: the subcommand it names, over the arguments it carries materialized.
fn run(plan: serde_json::Value) -> Result<()> {
    let command = plan["command"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("the plan carries no command"))?;
    let args = plan["args"].clone();
    match command {
        "sim" => simulate(serde_json::from_value(args)?),
        other => anyhow::bail!("the executor runs no `{other}` command"),
    }
}
