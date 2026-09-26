use anyhow::{Context, Result};
use ply_corpus::build::generate;
use ply_corpus::measure;
use ply_corpus::regions;
use ply_corpus::spec::CorpusSpec;
use ply_corpus::write;
use std::path::PathBuf;

#[derive(Debug, serde::Deserialize)]
struct RegionsArgs {
    /// Projects to analyse, each loaded the way `ply` loads one.
    roots: Vec<PathBuf>,
    /// Workers the wall-clock columns are modelled at and the suite is measured with.
    jobs: usize,
    /// Hypothetical footprints, `cells:labels`, appended as their own rows.
    hypothetical: Vec<String>,
    /// Tests carrying a contending resource atom in each hypothetical row.
    hypothetical_shared: usize,
    /// Pure tests in each hypothetical row.
    hypothetical_pure: usize,
    /// Include shipped modules' tests, as `ply test --std` does.
    std: bool,
    json: bool,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct ShapeArgs {
    seed: u64,
    modules: usize,
    defs_per_module: usize,
    tests: usize,
    /// Layers in the module import DAG.
    depth: usize,
    /// Distinct `db` resource labels, shared across the whole corpus.
    tables: usize,
    /// Distinct `cache` resource labels.
    regions: usize,
    effect_fraction: f64,
    nondet_fraction: f64,
    hub_modules: usize,
    max_weight: u32,
    /// `simulate` tests, on top of `--tests`.
    concurrent_tests: usize,
    tasks_per_test: usize,
    /// `counter.bump` calls per task, separated by a `task.yield()`.
    steps_per_task: usize,
    /// 0.0 gives every task its own resource; 1.0 puts every task on one.
    conflict_density: f64,
    /// Fraction of generated definitions carrying a `requires`/`ensures` pair.
    spec_fraction: f64,
    /// Definitions per module written for their obligation, each with a law.
    specimens_per_module: usize,
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
            spec_fraction: a.spec_fraction,
            specimens_per_module: a.specimens_per_module,
        }
    }
}

#[derive(Debug, serde::Deserialize)]
struct GenArgs {
    /// Where to write it. Must be empty, or a corpus this tool already wrote.
    out: PathBuf,
    #[serde(flatten)]
    shape: ShapeArgs,
    /// Write the corpus without compiling it.
    no_verify: bool,
    json: bool,
}

#[derive(Debug, serde::Deserialize)]
struct BenchArgs {
    /// A directory a previous `gen` wrote.
    corpus: PathBuf,
    /// Repeats per scenario; the fastest run is reported.
    repeats: usize,
    /// Attach a compiled backend, as `ply test --backend` spells it.
    backend: Option<String>,
    json: bool,
}

#[derive(Debug, serde::Deserialize)]
struct SweepArgs {
    /// A directory to hold one sub-directory per size.
    out: PathBuf,
    /// Sizes to sweep, each `modules,defs_per_module,tests`.
    sizes: Vec<String>,
    /// Attach a compiled backend to every size, as `ply test --backend` spells it.
    backend: Option<String>,
    seed: u64,
    repeats: usize,
    json: bool,
}

#[derive(Debug, serde::Deserialize)]
struct MeasureArgs {
    /// A directory a previous `gen` wrote. Omit it for fixture and resumption cost.
    corpus: Option<PathBuf>,
    /// Repeats per measurement; the fastest is reported.
    repeats: usize,
    /// Fixture sizes for the open-against-rebuild comparison.
    cells: Vec<usize>,
    /// Skip everything but the throughput table.
    only_throughput: bool,
    json: bool,
}

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
struct ServeArgs {
    /// The repository root, where `examples/hello.ply` is read from.
    repo: PathBuf,
    /// The `ply` binary the load table drives. Defaults to this binary's sibling.
    ply: Option<PathBuf>,
    /// Requests per ladder rung. Each is one connection.
    ladder_requests: u32,
    /// Repeats per rung; the fastest is reported.
    repeats: usize,
    /// Requests per load point.
    requests: u32,
    /// Simultaneous client connections to sweep.
    concurrency: Vec<u32>,
    /// Filler header lines the client's request carries, per load point.
    load_headers: Vec<usize>,
    /// Drop the per-request ladder, which is the slow half.
    no_ladder: bool,
    /// Drop the load table, which is the half that needs a built `ply`.
    no_load: bool,
    /// Also measure the endpoint with `fold`-based scans instead of the byte builtins.
    baseline: bool,
    json: bool,
}

fn serve(args: ServeArgs) -> Result<()> {
    let parsers: &[ply_corpus::serve::Parser] = if args.baseline {
        &[
            ply_corpus::serve::Parser::W1Folds,
            ply_corpus::serve::Parser::Native,
        ]
    } else {
        &[ply_corpus::serve::Parser::Native]
    };

    let mut ladders = Vec::new();
    let mut heads = Vec::new();
    if !args.no_ladder {
        for &parser in parsers {
            ladders.push(ply_corpus::serve::ladder(
                &args.repo,
                parser,
                args.ladder_requests,
                args.repeats,
            )?);
            heads.extend(ply_corpus::serve::head_sweep(
                &args.repo,
                parser,
                args.ladder_requests,
                args.repeats,
            )?);
        }
    }

    let mut load = Vec::new();
    if !args.no_load {
        let ply = match &args.ply {
            Some(path) => path.clone(),
            None => ply_corpus::serve::ply_binary()?,
        };
        for &headers in &args.load_headers {
            for &parser in parsers {
                // The sequential endpoint serves one connection at a time, so only concurrency 1.
                load.push(ply_corpus::serve::load(
                    &args.repo,
                    &ply,
                    ply_corpus::serve::Shape::Sequential,
                    parser,
                    headers,
                    1,
                    args.requests,
                )?);
                for &concurrency in &args.concurrency {
                    load.push(ply_corpus::serve::load(
                        &args.repo,
                        &ply,
                        ply_corpus::serve::Shape::Concurrent,
                        parser,
                        headers,
                        concurrency,
                        args.requests,
                    )?);
                }
            }
            for &concurrency in &args.concurrency {
                load.push(ply_corpus::serve::load_floor(
                    headers,
                    concurrency,
                    args.requests,
                )?);
            }
        }
    }

    let out = ply_corpus::serve::Measurements {
        ladders,
        heads,
        load,
    };
    if args.json {
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        print!("{}", ply_corpus::serve::render(&out));
    }
    Ok(())
}

#[derive(Debug, serde::Deserialize)]
struct W3Args {
    /// The repository root, where `examples/desk.ply` is read from.
    repo: PathBuf,
    /// The `ply` binary the load tables drive. Defaults to this binary's sibling.
    ply: Option<PathBuf>,
    /// Simultaneous client connections to sweep.
    concurrency: Vec<u32>,
    /// Requests one connection carries in the throughput sweep.
    per_conn: u32,
    /// Requests per point in the throughput sweep, held constant across concurrencies.
    requests_per_point: u32,
    /// Requests per point in the keep-alive and TLS ladders.
    ladder_requests: u32,
    /// Client threads in the keep-alive and TLS ladders.
    ladder_concurrency: u32,
    /// Requests per in-process point, for the per-route and shape tables.
    requests: u32,
    /// Repeats per in-process point; the fastest is reported.
    repeats: usize,
    /// Also serve the task-per-connection variant.
    concurrent: bool,
    /// Also re-take W2's single-endpoint load number on this machine.
    w2_baseline: bool,
    /// Sections to drop, for a run pointed at one question.
    no_load: bool,
    no_shape: bool,
    no_tls: bool,
    json: bool,
}

fn w3(args: W3Args) -> Result<()> {
    use ply_corpus::w3;

    let variant = if args.concurrent {
        w3::Variant::TaskPerConn
    } else {
        w3::Variant::Sequential
    };
    let mut out = w3::Measurements {
        aliases: Some(w3::aliases(&args.repo)?),
        ..w3::Measurements::default()
    };
    if !args.no_shape {
        out.stages = w3::stages(&args.repo, args.requests, args.repeats)?;
        out.per_route = w3::per_route(&args.repo, args.requests, args.repeats)?;
        out.shape = w3::shape(&args.repo, args.requests, args.repeats)?;
    }
    if !args.no_load {
        let ply = match &args.ply {
            Some(path) => path.clone(),
            None => ply_corpus::serve::ply_binary()?,
        };
        out.routes = w3::routes(
            &args.repo,
            &ply,
            variant,
            &args.concurrency,
            args.per_conn,
            args.requests_per_point,
        )?;
        out.keep_alive = w3::keep_alive(
            &args.repo,
            &ply,
            variant,
            args.ladder_concurrency,
            args.ladder_requests,
        )?;
        if !args.no_tls {
            out.tls = w3::tls(
                &args.repo,
                &ply,
                variant,
                args.ladder_concurrency,
                args.ladder_requests,
            )?;
        }
        if args.w2_baseline {
            for &concurrency in &args.concurrency {
                out.w2_baseline.push(ply_corpus::serve::load(
                    &args.repo,
                    &ply,
                    ply_corpus::serve::Shape::Concurrent,
                    ply_corpus::serve::Parser::Native,
                    0,
                    concurrency,
                    2000,
                )?);
            }
        }
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        print!("{}", w3::render(&out));
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
            None => ply_corpus::serve::ply_binary()?,
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
        None => ply_corpus::serve::ply_binary()?,
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
                    ply_corpus::w3::Variant::TaskPerConn
                } else {
                    ply_corpus::w3::Variant::Sequential
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
        "sequential" => (
            ply_corpus::w3::Variant::Sequential,
            ply_corpus::w3::Variant::TaskPerConn,
        ),
        "task-per-conn" => (
            ply_corpus::w3::Variant::TaskPerConn,
            ply_corpus::w3::Variant::Sequential,
        ),
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
            ply_corpus::w5::Stack::PostgresTls.label(),
            ply_corpus::w5::Sinking::JsonNull.label(),
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
struct PayloadArgs {
    /// Line items per JSON payload.
    lines: Vec<usize>,
    /// Encodes and decodes per payload size.
    iterations: u32,
    /// `lines:pad` pairs separating a decode's per-field cost from its per-byte one.
    shape: Vec<String>,
    /// Entries per `Map` measurement.
    entries: Vec<usize>,
    /// Type counts the derivation comparison is taken at.
    types: Vec<usize>,
    /// Types per module in that comparison.
    types_per_module: usize,
    /// Processes the `map_keys` order check spawns; two is the minimum to see a hasher seed.
    processes: usize,
    /// The `ply` binary the order check drives.
    ply: Option<PathBuf>,
    repeats: usize,
    /// Drop the derivation comparison, the slow half.
    no_derivation: bool,
    json: bool,
}

fn payload(args: PayloadArgs) -> Result<()> {
    let ply = match &args.ply {
        Some(path) => path.clone(),
        None => ply_corpus::payload::ply_binary()?,
    };
    let shape: Vec<(usize, usize)> = args
        .shape
        .iter()
        .map(|s| parse_shape(s))
        .collect::<Result<_>>()?;
    let out = ply_corpus::payload::Measurements {
        json: ply_corpus::payload::json_throughput(&args.lines, args.iterations, args.repeats)?,
        shape: ply_corpus::payload::json_shape(&shape, args.iterations, args.repeats)?,
        maps: ply_corpus::payload::map_ops(&args.entries, args.repeats)?,
        order: Some(ply_corpus::payload::map_order(&ply, args.processes)?),
        derivation: if args.no_derivation {
            Vec::new()
        } else {
            ply_corpus::payload::derivation_cost(&args.types, args.types_per_module, args.repeats)?
        },
    };
    if args.json {
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        print!("{}", ply_corpus::payload::render(&out));
    }
    Ok(())
}

/// Parses `lines:pad`.
fn parse_shape(point: &str) -> Result<(usize, usize)> {
    let (lines, pad) = point
        .split_once(':')
        .with_context(|| format!("`{point}` is not `lines:pad`"))?;
    Ok((
        lines
            .trim()
            .parse()
            .with_context(|| format!("`{lines}` is not a number"))?,
        pad.trim()
            .parse()
            .with_context(|| format!("`{pad}` is not a number"))?,
    ))
}

#[derive(Debug, serde::Deserialize)]
struct ProveArgs {
    /// `.ply` files or directories, each reported on its own row.
    projects: Vec<PathBuf>,
    cases: u32,
    prove_budget: u32,
    shrink_budget: u32,
    json: bool,
}

fn prove(args: ProveArgs) -> Result<()> {
    let plan = ply_prove::ProvePlan {
        cases: args.cases,
        prove_budget: args.prove_budget,
        shrink_budget: args.shrink_budget,
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

fn measure(args: MeasureArgs) -> Result<()> {
    let mut out = measure::Measurements {
        throughput: None,
        scheduling: None,
        store_open: None,
        fixture: Vec::new(),
        multi_shot: None,
    };
    if !args.only_throughput {
        out.fixture = measure::fixture_cost(&args.cells, args.repeats);
        out.multi_shot = Some(measure::multi_shot(args.repeats)?);
    }

    if let Some(root) = &args.corpus {
        out.throughput = Some(measure::throughput(root, args.repeats)?);
        if !args.only_throughput {
            // Scheduling clears the cache, so the store is timed before it.
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
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match ply_corpus::cmd::dispatch(&argv)? {
        ply_corpus::cmd::Outcome::Help(text) => println!("{text}"),
        ply_corpus::cmd::Outcome::Version(text) => println!("{text}"),
        ply_corpus::cmd::Outcome::Refused(why) => {
            eprint!("{why}");
            std::process::exit(2);
        }
        ply_corpus::cmd::Outcome::Run(plan) => run_plan(plan)?,
    }
    Ok(())
}

fn run_plan(plan: serde_json::Value) -> Result<()> {
    let command = plan["command"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("the plan carries no command"))?;
    let args = plan["args"].clone();
    match command {
        "gen" => generate_corpus(serde_json::from_value(args)?),
        "bench" => {
            let args: BenchArgs = serde_json::from_value(args)?;
            let report = bench_report(&args.corpus, args.repeats, args.backend.as_deref())?;
            emit_report(&report, args.json)
        }
        "sweep" => sweep(serde_json::from_value(args)?),
        "measure" => measure(serde_json::from_value(args)?),
        "sim" => simulate(serde_json::from_value(args)?),
        "prove" => prove(serde_json::from_value(args)?),
        "serve" => serve(serde_json::from_value(args)?),
        "payload" => payload(serde_json::from_value(args)?),
        "w3" => w3(serde_json::from_value(args)?),
        "w4" => w4(serde_json::from_value(args)?),
        "w5" => w5(serde_json::from_value(args)?),
        "w6" => w6(serde_json::from_value(args)?),
        "w6-ladder" => w6_ladder(serde_json::from_value(args)?),
        "regions" => regions(serde_json::from_value(args)?),
        other => anyhow::bail!("the corpus has no `{other}` command"),
    }
}

fn regions(args: RegionsArgs) -> Result<()> {
    let mut costs = Vec::new();
    for root in &args.roots {
        let corpus = regions::measure(root, args.jobs, args.std)
            .with_context(|| format!("measuring `{}`", root.display()))?;
        eprintln!(
            "{}: effects reaching a test footprint: {:?}",
            root.display(),
            regions::effects_present(&corpus.footprints)
        );
        costs.push(regions::analyse(&corpus, args.jobs));
    }
    for shape in &args.hypothetical {
        let (cells, labels) = shape
            .split_once(':')
            .context("`--hypothetical` takes `cells:labels`")?;
        let corpus = regions::hypothetical(regions::Hypothetical {
            cell_tests: cells.parse().context("`--hypothetical` cell count")?,
            labels: labels.parse().context("`--hypothetical` label count")?,
            shared_tests: args.hypothetical_shared,
            shared_labels: 3,
            pure_tests: args.hypothetical_pure,
            seed: 1,
        });
        costs.push(regions::analyse(&corpus, args.jobs));
    }
    if costs.is_empty() {
        anyhow::bail!("nothing to analyse: pass a project root or `--hypothetical cells:labels`");
    }
    if args.json {
        println!("{}", serde_json::to_string_pretty(&costs)?);
    } else {
        print!("{}", regions::render(&costs));
    }
    Ok(())
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
    if manifest.specs.obligations > 0 {
        let s = &manifest.specs;
        println!(
            "  {} definitions carry an obligation · {} do not",
            s.specified_definitions, s.unspecified_definitions
        );
        println!(
            "  {} obligations ({} laws) · built to be {} decided · {} sampled · {} gaps",
            s.obligations, s.laws, s.decided, s.sampled, s.gaps
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
        reports.push(bench_report(&root, args.repeats, args.backend.as_deref())?);
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
        return Ok(());
    }
    for report in &reports {
        print!(
            "{}",
            report["rendered"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("the bench's report carries no rendered text"))?
        );
    }
    Ok(())
}

/// The bench, run in the corpus package: the corpus drives the real `ply` and reads its
/// reports. Answers the report as decoded JSON; its `rendered` member is the text form.
fn bench_report(
    corpus: &std::path::Path,
    repeats: usize,
    backend: Option<&str>,
) -> Result<serde_json::Value> {
    let corpus = &corpus
        .canonicalize()
        .with_context(|| format!("`{}` does not exist", corpus.display()))?;
    let value = ply_corpus::cmd::run_ply_subcommand(
        "bench.run",
        vec![
            ply_eval::Value::str(corpus.to_string_lossy()),
            ply_eval::Value::Int(repeats as i64),
            ply_eval::Value::str(backend.unwrap_or("")),
        ],
        corpus,
        &std::path::PathBuf::from(ply_corpus::cmd::ply_binary()?),
    )?;
    let answered: String = match &value {
        ply_eval::Value::Ctor { name, args } if name.as_str() == "Ok" && args.len() == 1 => {
            match &args[0] {
                ply_eval::Value::Str(text) => Ok::<String, anyhow::Error>(text.to_string()),
                other => anyhow::bail!("`bench.run` answered {other}, not the report's text"),
            }
        }
        ply_eval::Value::Ctor { name, args } if name.as_str() == "Err" && args.len() == 1 => {
            match &args[0] {
                ply_eval::Value::Str(why) => anyhow::bail!("{why}"),
                other => anyhow::bail!("`bench.run` refused with {other}"),
            }
        }
        other => anyhow::bail!("`bench.run` answered {other}, not an `Ok` or an `Err`"),
    }?;
    serde_json::from_str(&answered).context("the bench's report is not JSON")
}

/// Parses `modules,defs_per_module,tests`.
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

fn emit_report(report: &serde_json::Value, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
    } else {
        print!(
            "{}",
            report["rendered"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("the bench's report carries no rendered text"))?
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
