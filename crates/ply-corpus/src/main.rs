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

#[derive(Debug, serde::Deserialize)]
struct W6LadderArgs {
    /// The repository root, where `examples/desk.ply` is read from.
    repo: PathBuf,
    /// The `ply` binary the served rungs drive. Defaults to this binary's sibling.
    ply: Option<PathBuf>,
    /// The database the served rungs run against. It must hold the desk's schema.
    db: String,
    /// Requests per in-process point: over `SimNet`, a real listener, and the Rust floor.
    requests: u32,
    /// Iterations of the in-Ply loop rungs 2, 3 and 4 are read off.
    iterations: u32,
    repeats: usize,
    /// Client concurrencies the served sweep takes; the total uses the fastest.
    concurrency: Vec<u32>,
    per_conn: u32,
    requests_per_point: u32,
    /// The credential the served desk is configured with.
    api_key: String,
    /// The machine the numbers were taken on, for the provenance line.
    machine: String,
    /// The postgres version, for the same line.
    postgres: Option<String>,
    /// Drop the served half, for a run pointed at the in-process rungs.
    no_served: bool,
    /// Run one phase alone for a profiler: `sim`, `socket`, `routed`, `endpoint` or `items`.
    only: Option<String>,
    /// Rounds of `--only`, each of `--requests` requests.
    rounds: usize,
    /// Which accept loop the ladder is read off. Spawning disables the constant memo.
    accept: String,
    /// Repeats of the sweep on the loop the ladder is not read off.
    other_repeats: usize,
    /// Repeats of the whole served sweep, so rung differences have a width.
    served_repeats: usize,
    /// Skip the constant memo's end-to-end pricing.
    no_levers: bool,
    /// Where to write the report. Defaults to stdout.
    out: Option<PathBuf>,
    /// Where to write the raw rows the report is read off.
    detail: Option<PathBuf>,
    /// Write the constant memo's control program into this directory, and stop.
    write_control: Option<PathBuf>,
}

fn w6_ladder(args: W6LadderArgs) -> Result<()> {
    use ply_corpus::w6_run;
    let ply = w6_run::ply_binary(args.ply.clone())?;
    if let Some(dir) = &args.write_control {
        let shipped = std::fs::read_to_string(args.repo.join("examples/desk.ply"))?;
        std::fs::create_dir_all(dir.join("examples"))?;
        let path = dir.join("examples/desk.ply");
        std::fs::write(&path, w6_run::without_constants(&shipped))?;
        println!("wrote {}", path.display());
        return Ok(());
    }
    if let Some(phase) = &args.only {
        let per = w6_run::only(&args.repo, phase, args.requests, args.rounds)?;
        println!(
            "{phase}: {per:.3}us per request over {} requests",
            args.requests
        );
        return Ok(());
    }
    let stack = w6_run::in_process(&args.repo, args.requests, args.iterations, args.repeats)?;
    if args.no_served {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "in_process": stack,
            }))?
        );
        return Ok(());
    }
    let (variant, other) = match args.accept.as_str() {
        "sequential" => (w6_run::Variant::Sequential, w6_run::Variant::TaskPerConn),
        "task-per-conn" => (w6_run::Variant::TaskPerConn, w6_run::Variant::Sequential),
        other => anyhow::bail!("`--accept {other}`: the loops are sequential and task-per-conn"),
    };
    let served = w6_run::served(
        &args.repo,
        &ply,
        &args.db,
        variant,
        &args.concurrency,
        args.per_conn,
        args.requests_per_point,
        &args.api_key,
        args.served_repeats,
    )?;
    // The loop the ladder is not read off, reported as its own labelled rows.
    let other_rows = w6_run::served(
        &args.repo,
        &ply,
        &args.db,
        other,
        &args.concurrency,
        args.per_conn,
        args.requests_per_point,
        &args.api_key,
        args.other_repeats,
    )?;
    let mut levers = if args.no_levers {
        w6_run::Levers::default()
    } else {
        let concurrency = w6_run::best(
            &served,
            w6_run::Stack::PostgresTls.label(),
            w6_run::Sinking::JsonNull.label(),
            "items (1 select)",
        )
        .map(|point| point.concurrency)
        .unwrap_or(1);
        w6_run::memo_lever(
            &args.repo,
            &ply,
            &args.db,
            variant,
            other,
            concurrency,
            args.per_conn,
            args.requests_per_point,
            &args.api_key,
            args.served_repeats,
        )?
    };
    levers.allocations = w6_run::allocations(&args.repo, args.ply.clone())?;

    let report = w6_run::report(
        args.machine.clone(),
        args.postgres.clone(),
        &stack,
        &served,
        &other_rows,
        &levers,
    )?;
    let text = serde_json::to_string_pretty(&report)?;
    match &args.out {
        Some(path) => std::fs::write(path, format!("{text}\n"))
            .with_context(|| format!("writing `{}`", path.display()))?,
        None => println!("{text}"),
    }
    if let Some(path) = &args.detail {
        std::fs::write(
            path,
            serde_json::to_string_pretty(&serde_json::json!({
                "in_process": stack,
                "served": served,
                "other_accept_loop": other_rows,
                "levers": levers,
            }))?,
        )
        .with_context(|| format!("writing `{}`", path.display()))?;
    }
    Ok(())
}

#[derive(Debug, serde::Deserialize)]
struct W6Args {
    /// Measurement files, each a `ply_corpus::w6::Report` fragment; later files win per field.
    reports: Vec<PathBuf>,
    /// Exit non-zero when the report is incomplete.
    strict: bool,
    json: bool,
}

fn w6(args: W6Args) -> Result<()> {
    let mut merged = serde_json::Map::new();
    for path in &args.reports {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading `{}`", path.display()))?;
        let value: serde_json::Value = serde_json::from_str(&text)
            .with_context(|| format!("`{}` is not JSON", path.display()))?;
        let serde_json::Value::Object(fields) = value else {
            anyhow::bail!(
                "`{}` is not a W6 report object; each file holds the fields it measured",
                path.display()
            );
        };
        merged.extend(fields);
    }
    let report: ply_corpus::w6::Report = serde_json::from_value(serde_json::Value::Object(merged))
        .context("the merged measurements are not a W6 report")?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&ply_corpus::w6::rendered(&report)?)?
        );
    } else {
        print!("{}", ply_corpus::w6::render(&report));
    }
    let findings = report.audit();
    if args.strict && !findings.is_empty() {
        anyhow::bail!(
            "{} section(s) of the W6 report are missing; --strict refuses an incomplete one",
            findings.len()
        );
    }
    Ok(())
}

#[derive(Debug, serde::Deserialize)]
struct ProveArgs {
    /// `.ply` files or directories, each reported on its own row.
    projects: Vec<PathBuf>,
    cases: u32,
    prove_budget: u32,
    json: bool,
}

fn prove(args: ProveArgs) -> Result<()> {
    let plan = ply_prove::ProvePlan {
        cases: args.cases,
        prove_budget: args.prove_budget,
        ..ply_prove::ProvePlan::default()
    }
    .normalized();
    let runs: Vec<ply_corpus::discharge::Discharged> = args
        .projects
        .iter()
        .map(|p| ply_corpus::discharge::discharge(p, &plan))
        .collect::<Result<_>>()?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&runs)?);
    } else {
        print!("{}", ply_corpus::discharge::render(&runs));
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
        "prove" => prove(serde_json::from_value(args)?),
        "w6" => w6(serde_json::from_value(args)?),
        "w6-ladder" => w6_ladder(serde_json::from_value(args)?),
        other => anyhow::bail!("the executor runs no `{other}` command"),
    }
}
