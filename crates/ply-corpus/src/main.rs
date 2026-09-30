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
struct W4Args {
    /// The repository root, where `examples/desk.ply` is read from for `crud`.
    repo: PathBuf,
    /// The database; its own `part` table is created and dropped, and `crud` needs the desk schema.
    db: String,
    ply: Option<PathBuf>,
    /// Concurrent tasks in the `ops` sweep.
    concurrency: Vec<u32>,
    /// Statements per point in the `ops` sweep.
    operations: u32,
    /// Table sizes the `sizes` section sweeps, in rows.
    rows: Vec<u32>,
    /// Pool sizes the `pool` section sweeps.
    pool_sizes: Vec<usize>,
    /// Connections the `ops` sweep's pool holds, constant across its rows.
    pool: usize,
    /// Repeats per point; the fastest is reported.
    repeats: usize,
    /// Client concurrencies in the `crud` section.
    load_concurrency: Vec<u32>,
    per_conn: u32,
    requests_per_point: u32,
    /// Sections to drop, for a run pointed at one question.
    no_ops: bool,
    no_sizes: bool,
    no_pool: bool,
    no_load: bool,
    json: bool,
}

fn w4(args: W4Args) -> Result<()> {
    use ply_corpus::w4;

    let mut out = w4::Measurements::default();
    if !args.no_ops {
        out.ops = w4::ops(
            &args.db,
            &args.concurrency,
            args.operations,
            args.pool,
            args.repeats,
        )?;
    }
    if !args.no_sizes {
        out.sizes = w4::sizes(&args.db, &args.rows, 200, args.repeats)?;
    }
    if !args.no_pool {
        out.pool = w4::pool(&args.db, &args.pool_sizes, 8, args.operations, args.repeats)?;
        // An acquire is a deadline, not a capacity check: a small pool queues until it expires.
        for (pool, concurrency, acquire) in [
            (1, 8, 5000),
            (1, 32, 5000),
            (1, 32, 1),
            (1, 32, 0),
            (8, 32, 0),
            (1, 8, 0),
        ] {
            out.exhaustion
                .push(w4::exhaustion(&args.db, pool, concurrency, acquire)?);
        }
    }
    if !args.no_load {
        let ply = match &args.ply {
            Some(path) => path.clone(),
            None => ply_corpus::ply_binary()?,
        };
        out.crud = w4::crud(
            &args.repo,
            &ply,
            &args.db,
            &[w4::Store::Twin, w4::Store::Postgres],
            &args.load_concurrency,
            args.per_conn,
            args.requests_per_point,
        )?;
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        print!("{}", w4::render(&out));
    }
    Ok(())
}

#[derive(Debug, serde::Deserialize)]
struct W5Args {
    /// The repository root, where `examples/desk.ply` is read from.
    repo: PathBuf,
    ply: Option<PathBuf>,
    /// The database the served sections run against. It must hold the desk's schema.
    db: Option<String>,
    /// Trace operations per point in the `events` table.
    operations: u32,
    /// Operations per twin point. Small because `Sink` appends are quadratic in records held.
    twin_operations: u32,
    repeats: usize,
    /// Client concurrencies in the `served` table.
    concurrency: Vec<u32>,
    per_conn: u32,
    requests_per_point: u32,
    /// Requests in flight when the signal arrives.
    in_flight: Vec<u32>,
    drain_ms: u64,
    /// How long a client holds its half-sent request before finishing it.
    hold_ms: u64,
    /// The credential the served desk is configured with.
    api_key: String,
    /// Serve the `served` table from the task-per-connection accept loop.
    concurrent: bool,
    /// Sections to drop, for a run pointed at one question.
    no_events: bool,
    no_served: bool,
    no_drain: bool,
    no_transaction: bool,
    no_deploy: bool,
    json: bool,
}

fn w5(args: W5Args) -> Result<()> {
    use ply_corpus::w5;

    let ply = match &args.ply {
        Some(path) => path.clone(),
        None => ply_corpus::ply_binary()?,
    };
    let mut out = w5::Measurements::default();
    if !args.no_events {
        let dir = tempfile::tempdir().context("a temp dir for the file sink")?;
        out.events = w5::events(
            args.operations,
            args.twin_operations,
            args.repeats,
            dir.path(),
        )?;
    }
    if !args.no_deploy {
        // Nothing reads this at start-up, so the second build differs in one leaf.
        out.deploy = Some(w5::deploy(
            &args.repo,
            &ply,
            (
                "fn store_reachable() -> db::Stmt = db::stmt(\"select count(*) from items\")",
                "fn store_reachable() -> db::Stmt = db::stmt(\"select count(*) from orders\")",
            ),
        )?);
    }
    if let Some(url) = &args.db {
        if !args.no_served {
            out.served = w5::tracing(
                &args.repo,
                &ply,
                url,
                &[w5::Stack::Twin, w5::Stack::Postgres, w5::Stack::PostgresTls],
                if args.concurrent {
                    ply_corpus::w4::Variant::TaskPerConn
                } else {
                    ply_corpus::w4::Variant::Sequential
                },
                &[
                    w5::Sinking::Off,
                    w5::Sinking::JsonNull,
                    w5::Sinking::JsonFile,
                ],
                &[
                    ("health (no db)", "/health"),
                    ("items (1 select)", "/items"),
                ],
                &args.concurrency,
                args.per_conn,
                args.requests_per_point,
                &args.api_key,
            )?;
        }
        if !args.no_drain {
            out.drain = w5::drain(
                &args.repo,
                &ply,
                url,
                &args.in_flight,
                args.drain_ms,
                args.hold_ms,
                &args.api_key,
            )?;
        }
        if !args.no_transaction {
            out.transaction = Some(w5::transaction_at_deadline(
                &args.repo,
                &ply,
                url,
                args.drain_ms,
                &args.api_key,
            )?);
        }
    } else if !(args.no_served && args.no_drain && args.no_transaction) {
        anyhow::bail!(
            "the `served`, `drain` and `transaction` sections need a database: pass `--db`, \
             or drop them with `--no-served --no-drain --no-transaction`"
        );
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        print!("{}", w5::render(&out));
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
        "w4" => w4(serde_json::from_value(args)?),
        "w5" => w5(serde_json::from_value(args)?),
        other => anyhow::bail!("the executor runs no `{other}` command"),
    }
}
