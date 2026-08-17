use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use ply_corpus::bench;
use ply_corpus::build::generate;
use ply_corpus::measure;
use ply_corpus::regions;
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
    /// Price the spec tier: where obligations land, why the ones that fell
    /// short did, and what shrinking bought.
    Prove(ProveArgs),
    /// Price a request: what one costs per layer, and what the endpoint
    /// sustains under load. The number W6's decision on M9 turns on.
    Serve(ServeArgs),
    /// Price what W2 put on the request path: a derived JSON codec, `Map`, and
    /// what derivation costs the front end and the cache.
    Payload(PayloadArgs),
    /// Price what W3 put on it: routing, real HTTP/1.1 framing, keep-alive and
    /// TLS — and re-take W2's field-proportional cost sweep against the result.
    W3(W3Args),
    /// Price what W4 put behind it: a statement through the effect boundary
    /// against the same statement with no Ply in the path, the pool, and a
    /// route that hits the database against one that does not.
    W4(W4Args),
    /// Price what W5 put around it: a trace operation against the same loop
    /// performing none, the service with only `--trace` moved, a drain with
    /// requests in flight, what the deadline does to an open transaction, and
    /// what a deploy ships.
    W5(W5Args),
    /// Assemble the W6 report and apply the M9 criteria to it. The thresholds
    /// live in `ply_corpus::w6::Criteria` and are not readable from the file,
    /// so a measurement cannot supply the bar it is about to clear.
    W6(W6Args),
    /// Take the W6 ladder: nine rungs, each a pair of absolutes differing in
    /// one substitution, plus the floor, the served total, the offering rows
    /// and the engine substitution. Writes the measurement half of a
    /// `ply_corpus::w6::Report`, which `w6` then judges.
    W6Ladder(W6LadderArgs),
    /// Price ADR 0017 §6 before it is built: colour the same test set with and
    /// without the world-backed exemption, and report what the second colouring
    /// costs in groups, in critical path and in wall clock.
    Regions(RegionsArgs),
}

#[derive(Args, Debug)]
struct RegionsArgs {
    /// Projects to analyse. Each is loaded the way `ply` loads one, so shipped
    /// modules resolve and `examples/` works.
    #[arg(required = false)]
    roots: Vec<PathBuf>,
    /// Workers the wall-clock columns are modelled at, and the pool the
    /// measured suite number is taken in.
    #[arg(long, default_value_t = 8)]
    jobs: usize,
    /// Hypothetical footprints, `cells:labels`, appended as their own rows.
    /// The measured corpora carry no `cell` atom at all, so this is how the
    /// risk is priced rather than asserted away.
    #[arg(long, value_delimiter = ',')]
    hypothetical: Vec<String>,
    /// Tests carrying a contending resource atom in each hypothetical row.
    #[arg(long, default_value_t = 10)]
    hypothetical_shared: usize,
    /// Pure tests in each hypothetical row.
    #[arg(long, default_value_t = 165)]
    hypothetical_pure: usize,
    /// Include shipped modules' tests, as `ply test --std` does.
    #[arg(long)]
    std: bool,
    #[arg(long)]
    json: bool,
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
    /// Fraction of generated definitions carrying a `requires`/`ensures` pair.
    /// This is the axis to vary when pricing discharge against definition count.
    #[arg(long, default_value_t = 0.0)]
    spec_fraction: f64,
    /// Definitions per module written for their obligation, so that the tier
    /// distribution spans the table instead of landing in one bucket. Each
    /// contributes a law as well.
    #[arg(long, default_value_t = 0)]
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

#[derive(Args, Debug)]
struct ServeArgs {
    /// The repository root, which is where `examples/hello.ply` is read from.
    /// The endpoint under measurement is the one W1 shipped, not a copy.
    #[arg(long, default_value = ".")]
    repo: PathBuf,
    /// The `ply` binary the load table drives. Defaults to this binary's
    /// sibling, so a release measurement never silently serves from a debug
    /// build.
    #[arg(long)]
    ply: Option<PathBuf>,
    /// Requests per ladder rung. Each is one connection.
    #[arg(long, default_value_t = 2000)]
    ladder_requests: u32,
    /// Repeats per rung; the fastest is reported.
    #[arg(long, default_value_t = 3)]
    repeats: usize,
    /// Requests per load point.
    #[arg(long, default_value_t = 2000)]
    requests: u32,
    /// Simultaneous client connections to sweep. `serve` recursion is charged
    /// against the call budget, so a point costs `requests` nested calls.
    #[arg(long, value_delimiter = ',', default_values_t = [1u32, 2, 4, 8, 16, 32, 64])]
    concurrency: Vec<u32>,
    /// Filler header lines the client's request carries, per load point. Zero is
    /// `curl`; eight is about what a browser sends, which is the length W1's
    /// scans were quadratic-feeling on.
    #[arg(long, value_delimiter = ',', default_values_t = [0usize])]
    load_headers: Vec<usize>,
    /// Drop the per-request ladder, which is the slow half.
    #[arg(long)]
    no_ladder: bool,
    /// Drop the load table, which is the half that needs a built `ply`.
    #[arg(long)]
    no_load: bool,
    /// Also measure the endpoint with W1's `fold`-based scans, so the byte
    /// builtins' before and after are one table taken on one machine.
    #[arg(long)]
    baseline: bool,
    #[arg(long)]
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
                // The sequential endpoint is `examples/hello.ply` as written,
                // and it serves one connection at a time however many arrive —
                // so it is reported at concurrency 1 alone. Sweeping it would
                // measure a queue.
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

#[derive(Args, Debug)]
struct W3Args {
    /// The repository root, which is where `examples/desk.ply` is read from.
    /// The service under measurement is the one W3 shipped, not a copy.
    #[arg(long, default_value = ".")]
    repo: PathBuf,
    /// The `ply` binary the load tables drive. Defaults to this binary's
    /// sibling, so a release measurement never silently serves from a debug
    /// build.
    #[arg(long)]
    ply: Option<PathBuf>,
    /// Simultaneous client connections to sweep.
    #[arg(long, value_delimiter = ',', default_values_t = [1u32, 2, 4, 8, 16, 32, 64])]
    concurrency: Vec<u32>,
    /// Requests one connection carries in the throughput sweep.
    #[arg(long, default_value_t = 32)]
    per_conn: u32,
    /// Requests per point in the throughput sweep, held about constant across
    /// concurrencies so a p99 at one rests on as many samples as at another.
    #[arg(long, default_value_t = 4000)]
    requests_per_point: u32,
    /// Requests per point in the keep-alive and TLS ladders, held constant
    /// while the requests-per-connection rung varies.
    #[arg(long, default_value_t = 3200)]
    ladder_requests: u32,
    /// Client threads in the keep-alive and TLS ladders.
    #[arg(long, default_value_t = 8)]
    ladder_concurrency: u32,
    /// Requests per in-process point, for the per-route and shape tables.
    #[arg(long, default_value_t = 2000)]
    requests: u32,
    /// Repeats per in-process point; the fastest is reported.
    #[arg(long, default_value_t = 3)]
    repeats: usize,
    /// Also serve the task-per-connection variant, which is the same service
    /// with a spawn in its accept loop.
    #[arg(long)]
    concurrent: bool,
    /// Also re-take W2's single-endpoint load number on this machine, so the
    /// comparison is one table rather than a figure quoted from a milestone ago.
    #[arg(long)]
    w2_baseline: bool,
    /// Sections to drop, for a run pointed at one question.
    #[arg(long)]
    no_load: bool,
    #[arg(long)]
    no_shape: bool,
    #[arg(long)]
    no_tls: bool,
    #[arg(long)]
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

#[derive(Args, Debug)]
struct W4Args {
    /// The repository root, which is where `examples/desk.ply` is read from for
    /// the `crud` section.
    #[arg(long, default_value = ".")]
    repo: PathBuf,
    /// The database every section runs against. This harness creates and drops
    /// its own `part` table in it and touches nothing else, and the `crud`
    /// section expects the desk's schema to be there already.
    #[arg(long)]
    db: String,
    #[arg(long)]
    ply: Option<PathBuf>,
    /// Concurrent tasks in the `ops` sweep.
    #[arg(long, value_delimiter = ',', default_values_t = [1u32, 2, 4, 8, 16])]
    concurrency: Vec<u32>,
    /// Statements per point in the `ops` sweep.
    #[arg(long, default_value_t = 400)]
    operations: u32,
    /// Table sizes the `sizes` section sweeps, in rows.
    #[arg(long, value_delimiter = ',', default_values_t = [8u32, 32, 128, 512])]
    rows: Vec<u32>,
    /// Pool sizes the `pool` section sweeps.
    #[arg(long, value_delimiter = ',', default_values_t = [1usize, 2, 4, 8, 16])]
    pool_sizes: Vec<usize>,
    /// Connections the `ops` sweep's pool holds, held constant so its rows
    /// differ in concurrency and not in two things at once.
    #[arg(long, default_value_t = 16)]
    pool: usize,
    /// Repeats per point; the fastest is reported.
    #[arg(long, default_value_t = 3)]
    repeats: usize,
    /// Client concurrencies in the `crud` section.
    #[arg(long, value_delimiter = ',', default_values_t = [1u32, 8, 32])]
    load_concurrency: Vec<u32>,
    #[arg(long, default_value_t = 32)]
    per_conn: u32,
    #[arg(long, default_value_t = 3000)]
    requests_per_point: u32,
    /// Sections to drop, for a run pointed at one question.
    #[arg(long)]
    no_ops: bool,
    #[arg(long)]
    no_sizes: bool,
    #[arg(long)]
    no_pool: bool,
    #[arg(long)]
    no_load: bool,
    #[arg(long)]
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
        // A pool acquire is a *deadline*, not a capacity check: a pool of one
        // with eight open scopes queues and completes if the deadline is
        // generous, and refuses if it is not. Both rows are here because the
        // first is the one a reader assumes is a failure and is not.
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

#[derive(Args, Debug)]
struct W5Args {
    /// The repository root, which is where `examples/desk.ply` is read from.
    #[arg(long, default_value = ".")]
    repo: PathBuf,
    #[arg(long)]
    ply: Option<PathBuf>,
    /// The database the served sections run against. It must already hold the
    /// desk's schema — `--db-schema desk.schema` refuses at bind time if not —
    /// and the transaction section writes one order into it and expects the
    /// teardown to take it away again.
    #[arg(long)]
    db: Option<String>,
    /// Trace operations per point in the `events` table.
    #[arg(long, default_value_t = 20_000)]
    operations: u32,
    /// Operations per twin point. Smaller because `std.trace`'s `Sink` appends
    /// with `push` and is therefore quadratic in the records it holds, and a
    /// twin lives inside one test holding tens of them.
    #[arg(long, default_value_t = 200)]
    twin_operations: u32,
    #[arg(long, default_value_t = 3)]
    repeats: usize,
    /// Client concurrencies in the `served` table.
    #[arg(long, value_delimiter = ',', default_values_t = [1u32, 8, 32])]
    concurrency: Vec<u32>,
    #[arg(long, default_value_t = 32)]
    per_conn: u32,
    #[arg(long, default_value_t = 3000)]
    requests_per_point: u32,
    /// Requests in flight when the signal arrives.
    #[arg(long, value_delimiter = ',', default_values_t = [1u32, 8, 32])]
    in_flight: Vec<u32>,
    #[arg(long, default_value_t = 5_000)]
    drain_ms: u64,
    /// How long a client holds its half-sent request before finishing it, in
    /// the points whose drain is meant to complete.
    #[arg(long, default_value_t = 500)]
    hold_ms: u64,
    /// The credential the served desk is configured with. A benchmark's key is
    /// a fixture credential and is not a credential.
    #[arg(long, default_value = "bench-key")]
    api_key: String,
    /// Serve the `served` table from the task-per-connection accept loop rather
    /// than `examples/desk.ply`'s sequential one. A sequential server answers
    /// one connection at a time, so a tail latency at concurrency 8 is a queue
    /// rather than a service.
    #[arg(long)]
    concurrent: bool,
    /// Sections to drop, for a run pointed at one question.
    #[arg(long)]
    no_events: bool,
    #[arg(long)]
    no_served: bool,
    #[arg(long)]
    no_drain: bool,
    #[arg(long)]
    no_transaction: bool,
    #[arg(long)]
    no_deploy: bool,
    #[arg(long)]
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
        // One definition's body, and one that nothing in `desk.ply` reads at
        // start-up, so the second build differs in exactly one leaf and its
        // dependents rather than in the shape of the program.
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

#[derive(Args, Debug)]
struct W6LadderArgs {
    /// The repository root, which is where `examples/desk.ply` is read from.
    /// The service under measurement is the one W5 shipped, not a copy.
    #[arg(long, default_value = ".")]
    repo: PathBuf,
    /// The `ply` binary the served rungs drive. Defaults to this binary's
    /// sibling, so a release measurement never serves from a debug build.
    #[arg(long)]
    ply: Option<PathBuf>,
    /// The database the served rungs run against. It must already hold the
    /// desk's schema: `--db-schema desk.schema` refuses at bind time if not.
    #[arg(long)]
    db: String,
    /// Requests per in-process point, for the rungs taken over `SimNet`, over a
    /// real listener and against the Rust floor.
    #[arg(long, default_value_t = 2000)]
    requests: u32,
    /// Iterations of the in-Ply loop rungs 2, 3 and 4 are read off. One
    /// `Machine::call` runs them all, so the twin's fixture is amortized rather
    /// than landing inside a layer.
    #[arg(long, default_value_t = 2000)]
    iterations: u32,
    #[arg(long, default_value_t = 3)]
    repeats: usize,
    /// Client concurrencies the served sweep takes. The total is read off
    /// whichever maximizes throughput.
    #[arg(long, num_args = 1.., default_values_t = [1u32, 2, 4, 8, 16, 32])]
    concurrency: Vec<u32>,
    #[arg(long, default_value_t = 32)]
    per_conn: u32,
    #[arg(long, default_value_t = 3000)]
    requests_per_point: u32,
    /// The credential the served desk is configured with. A benchmark's key is
    /// a fixture credential and is not a credential.
    #[arg(long, default_value = "bench-key")]
    api_key: String,
    /// The machine the numbers were taken on, for the provenance line.
    #[arg(long, default_value = "unnamed")]
    machine: String,
    /// The postgres version, for the same line.
    #[arg(long)]
    postgres: Option<String>,
    /// Drop the served half, for a run pointed at the in-process rungs.
    #[arg(long)]
    no_served: bool,
    /// Run one phase on its own and print what it cost per request, so a
    /// sampling profiler sees that phase and nothing else. One of `sim`,
    /// `socket`, `routed`, `endpoint`, `items`.
    #[arg(long)]
    only: Option<String>,
    /// Rounds of `--only`, each of `--requests` requests.
    #[arg(long, default_value_t = 1)]
    rounds: usize,
    /// Which accept loop the **ladder's** rungs and total are read off.
    ///
    /// ADR 0016 §1.6 pins `task-per-conn`, and the default here departs from it
    /// for a measured reason: `task.spawn` opens a production region for the
    /// life of the server, and `Machine::constant` refuses the constant memo
    /// inside any open region — so a spawning service memoizes nothing. The
    /// in-process rungs are `run_memory` over one connection at a time and do
    /// memoize, so a ladder whose lower rungs spanned one regime and whose
    /// total spanned the other would divide a memo-active numerator by a
    /// memo-inert denominator. Both loops are measured either way, and the one
    /// this is not set to becomes its own labelled offering rows.
    #[arg(long, default_value = "sequential")]
    accept: String,
    /// Repeats of the sweep on the loop the ladder is *not* read off. It
    /// contributes tail latency and throughput rows, which need no band.
    #[arg(long, default_value_t = 1)]
    other_repeats: usize,
    /// Repeats of the whole served sweep. The rungs above the socket are
    /// differences between served rows, and a difference between two numbers
    /// taken once has no width.
    #[arg(long, default_value_t = 3)]
    served_repeats: usize,
    /// Skip the constant memo's end-to-end pricing, which serves a second
    /// variant of the service and roughly doubles the run.
    #[arg(long)]
    no_levers: bool,
    /// Where to write the report. The default is stdout, so
    /// `w6-ladder .. > benches/w6-ladder.json` reproduces the shipped file.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Where to write the raw in-process and served rows the report is read
    /// off. Not part of the report, and not needed to re-take it.
    #[arg(long)]
    detail: Option<PathBuf>,
    /// Write the control the constant memo's price is measured against —
    /// `examples/desk.ply` with every nullary definition of its own given a
    /// dead parameter — into this directory, and stop. A ratio is only as
    /// checkable as the program on the other side of it.
    #[arg(long)]
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
    let engines = w6_run::engines(&args.repo, args.requests, args.repeats)?;
    if args.no_served {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "in_process": stack,
                "engines": engines,
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
    // The other loop, reported once, separately and labelled — which is what
    // §1.6 asks of whichever loop the ladder is not read off.
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
        engines,
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

#[derive(Args, Debug)]
struct W6Args {
    /// One or more measurement files. Each is a `ply_corpus::w6::Report`
    /// fragment; later files win field by field, which is how the ladder agent
    /// and the spike agent produce their halves independently.
    #[arg(required = true)]
    reports: Vec<PathBuf>,
    /// Exit non-zero when the report is incomplete. The default prints the
    /// audit and still renders, because a partial report is worth reading; CI
    /// wants the other behaviour.
    #[arg(long)]
    strict: bool,
    #[arg(long)]
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

#[derive(Args, Debug)]
struct PayloadArgs {
    /// Line items per JSON payload. Forty is about four kilobytes, which is the
    /// size a real order body arrives at.
    #[arg(long, value_delimiter = ',', default_values_t = [1usize, 10, 40, 200, 1000])]
    lines: Vec<usize>,
    /// Encodes and decodes per payload size.
    #[arg(long, default_value_t = 200)]
    iterations: u32,
    /// `lines:pad` pairs for the table that separates a decode's per-field cost
    /// from its per-byte one. The default holds the line count still and grows
    /// one string field, which is the only way the two come apart.
    #[arg(long, value_delimiter = ',', default_values_t = [
        String::from("40:0"),
        String::from("40:100"),
        String::from("40:400"),
        String::from("40:1600"),
    ])]
    shape: Vec<String>,
    /// Entries per `Map` measurement.
    #[arg(long, value_delimiter = ',', default_values_t = [16usize, 256, 4_096, 65_536])]
    entries: Vec<usize>,
    /// Type counts the derivation comparison is taken at.
    #[arg(long, value_delimiter = ',', default_values_t = [50usize, 200, 500])]
    types: Vec<usize>,
    /// Types per module in that comparison.
    #[arg(long, default_value_t = 10)]
    types_per_module: usize,
    /// Processes the `map_keys` order check spawns. Two is the minimum that can
    /// see a per-process hasher seed at all.
    #[arg(long, default_value_t = 4)]
    processes: usize,
    /// The `ply` binary the order check drives.
    #[arg(long)]
    ply: Option<PathBuf>,
    #[arg(long, default_value_t = 3)]
    repeats: usize,
    /// Drop the derivation comparison, which is the slow half: it compiles and
    /// runs six whole projects.
    #[arg(long)]
    no_derivation: bool,
    #[arg(long)]
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

/// `lines:pad` — the two numbers that have to move independently for the table
/// to say anything.
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

#[derive(Args, Debug)]
struct ProveArgs {
    /// One or more `.ply` files or directories. Each is reported on its own row
    /// so a generated corpus never averages with a written one.
    #[arg(required = true)]
    projects: Vec<PathBuf>,
    #[arg(long, default_value_t = ply_prove::DEFAULT_CASES)]
    cases: u32,
    #[arg(long, default_value_t = ply_prove::DEFAULT_PROVE_BUDGET)]
    prove_budget: u32,
    #[arg(long, default_value_t = ply_prove::DEFAULT_SHRINK_BUDGET)]
    shrink_budget: u32,
    #[arg(long)]
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
        Command::Prove(args) => prove(args),
        Command::Serve(args) => serve(args),
        Command::Payload(args) => payload(args),
        Command::W3(args) => w3(args),
        Command::W4(args) => w4(args),
        Command::W5(args) => w5(args),
        Command::W6(args) => w6(args),
        Command::W6Ladder(args) => w6_ladder(args),
        Command::Regions(args) => regions(args),
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
