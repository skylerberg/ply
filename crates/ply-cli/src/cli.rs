use crate::style::ColorChoice;
use clap::{Args, Parser, Subcommand, ValueEnum};
use ply_eval::Seed;
use ply_host::fs::RootSpec;
use ply_host::process::ExecSpec;
use ply_host::tls::CredentialSpec;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "ply",
    version,
    about = "Ply — a language where the verification loop collapses toward zero",
    propagate_version = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Colour and the ✓/✗ marks; `auto` uses them only on a terminal with NO_COLOR unset.
    #[arg(long, value_enum, default_value_t = ColorChoice::Auto, global = true)]
    pub color: ColorChoice,
}

impl Cli {
    /// A contradiction clap cannot check itself, or `None`.
    pub fn conflict(&self) -> Option<String> {
        match &self.command {
            Command::Test(args) => args.simulation.conflict(),
            Command::Prove(args) => args.simulation.conflict(),
            Command::Review(args) => args.simulation.conflict(),
            _ => None,
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Typecheck a file or directory.
    Check(CheckArgs),
    /// Select, schedule and run the tests.
    Test(TestArgs),
    /// Discharge every obligation and report the tier each was discharged at.
    Prove(ProveArgs),
    /// Report what changed and whether its specification and obligations still hold.
    Review(ReviewArgs),
    /// Evaluate `main` (a directory must hold exactly one), or run a built `.plyx`.
    Run(RunArgs),
    /// Write a deployable artifact: the transitive closure of one entry point.
    Build(BuildArgs),
    /// List every host handler this binary can bind: the trusted computing base.
    Hosts(HostsArgs),
    /// List the modules that ship with this compiler, and the digest over them.
    Std(StdArgs),
    /// What a diagnostic code means; `--all` lists every code.
    Explain(ExplainArgs),
    /// A definition or builtin: its signature with parameter names, the comment above it, its place.
    Doc(DocArgs),
    /// Rewrite `.ply` files in the canonical layout; a file that does not parse is left alone.
    Fmt(FmtArgs),
    /// One `fn` or `type` as its file holds it: the comment lines above it, `pub`, and the body.
    Show(ShowArgs),
    /// Rewrite one `fn` or `type` from a file or stdin, formatted, leaving every other byte of the file alone.
    Replace(ReplaceArgs),
    /// Print the content hash of every definition.
    Hash(HashArgs),
    /// Every definition with its place, hash, signature and footprint.
    Defs(DefsArgs),
    /// What mentions a definition: definitions, tests and laws, directly and through calls.
    Callers(CallersArgs),
    /// Write the front end out as the C that builds it, with its digests.
    Bootstrap(BootstrapArgs),
    /// Read, reclaim or discard what the caches hold.
    Cache(CacheArgs),
}

/// A three-way switch for work a run may do on its own behalf.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
#[value(rename_all = "lower")]
pub enum When {
    #[default]
    Auto,
    Always,
    Never,
}

impl When {
    pub fn as_str(self) -> &'static str {
        match self {
            When::Auto => "auto",
            When::Always => "always",
            When::Never => "never",
        }
    }
}

/// Which search a `simulate` region runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
#[value(rename_all = "lower")]
pub enum SimArg {
    /// One interleaving, the one the seed names.
    Once,
    /// One independent interleaving per seed.
    Random,
    /// Partial-order reduction; a small state space finishes exhaustively.
    #[default]
    Dpor,
}

impl SimArg {
    pub fn as_str(self) -> &'static str {
        ply_eval::SimMode::from(self).as_str()
    }
}

impl From<SimArg> for ply_eval::SimMode {
    fn from(a: SimArg) -> ply_eval::SimMode {
        match a {
            SimArg::Once => ply_eval::SimMode::Once,
            SimArg::Random => ply_eval::SimMode::Random,
            SimArg::Dpor => ply_eval::SimMode::Dpor,
        }
    }
}

/// What a run searches; every field is in a simulated test's cache key.
#[derive(Args, Clone, Debug)]
pub struct SimOptions {
    /// Replay exactly one interleaving: `7`, or `7:3.0.2`. Implies `--sim once`.
    #[arg(
        long,
        value_name = "SEED",
        value_parser = parse_seed,
        conflicts_with_all = ["sim", "seeds", "sim_budget"],
    )]
    pub seed: Option<Seed>,

    /// Which search each simulated test runs.
    #[arg(long, value_enum, default_value_t = SimArg::default(), value_name = "MODE")]
    pub sim: SimArg,

    /// Seeds per simulated test. Defaults to 1 under `dpor` and 64 under `random`.
    #[arg(long = "seeds", alias = "sim-roots", value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub seeds: Option<u32>,

    /// Interleavings per seed. Only `dpor` searches more than one.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub sim_budget: Option<u32>,

    /// Scheduling steps one interleaving may take before it reports no progress.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub sim_steps: Option<u32>,

    /// Also run an unpruned search and report what it would have cost.
    #[arg(long)]
    pub measure_reduction: bool,
}

impl SimOptions {
    /// The conflict `conflicts_with` cannot express: a flag against another flag's value.
    pub fn conflict(&self) -> Option<String> {
        (self.sim == SimArg::Random && self.sim_budget.is_some()).then(|| {
            "`--sim-budget` has no meaning under `--sim random`, which runs one \
             interleaving per seed; widen the search with `--seeds N`, or use \
             `--sim dpor` to spend a budget per seed"
                .to_string()
        })
    }
}

#[derive(Args, Clone, Debug, Default)]
pub struct TlsOptions {
    /// TLS credential: `--tls api=certs/api.pem,certs/api.key`. Repeatable, one per listener.
    #[arg(
        long = "tls",
        value_name = "NAME=CERT,KEY",
        value_parser = parse_credential,
        requires = "host",
    )]
    pub tls: Vec<CredentialSpec>,
    /// A certificate `net.connect_tls` trusts beside the built-in roots. Repeatable.
    #[arg(long = "trust", value_name = "CERT.pem", requires = "host")]
    pub trust: Vec<std::path::PathBuf>,
}

/// Directory roots per resource label; kept out of the program so no path enters a hash.
#[derive(Args, Clone, Debug, Default)]
pub struct FsOptions {
    /// Filesystem root: `--fs src=./crates`. Repeatable, one root per resource label.
    #[arg(
        long = "fs",
        value_name = "NAME=PATH",
        value_parser = parse_root,
        requires = "host",
    )]
    pub fs: Vec<RootSpec>,
}

/// The database knobs, on every command that can bind a host handler.
#[derive(clap::Args, Clone, Debug, Default)]
pub struct DbOptions {
    /// The database URL; defaults to `PLY_DB_URL`, with the password from `PLY_DB_PASSWORD`.
    #[arg(long = "db", value_name = "URL", requires = "host")]
    pub url: Option<String>,

    /// Connections in the pool.
    #[arg(long = "db-pool", value_name = "N", requires = "host", value_parser = clap::value_parser!(u32).range(1..))]
    pub pool: Option<u32>,

    /// Milliseconds a `db` operation may wait for a connection before `E0437`.
    #[arg(long = "db-acquire-ms", value_name = "MS", requires = "host", value_parser = clap::value_parser!(u64).range(1..))]
    pub acquire_ms: Option<u64>,

    /// Server-side `statement_timeout`, set on every connection at checkout.
    #[arg(long = "db-statement-ms", value_name = "MS", requires = "host", value_parser = clap::value_parser!(u64).range(1..))]
    pub statement_ms: Option<u64>,

    /// Server-side `idle_in_transaction_session_timeout`.
    #[arg(long = "db-idle-txn-ms", value_name = "MS", requires = "host", value_parser = clap::value_parser!(u64).range(1..))]
    pub idle_txn_ms: Option<u64>,

    /// Milliseconds to establish a connection.
    #[arg(long = "db-connect-ms", value_name = "MS", requires = "host", value_parser = clap::value_parser!(u64).range(1..))]
    pub connect_ms: Option<u64>,

    /// Prepared statements kept per connection.
    #[arg(long = "db-statement-cache", value_name = "N", requires = "host", value_parser = clap::value_parser!(u32).range(1..))]
    pub statement_cache: Option<u32>,

    /// `<module>.<fn>`: a nullary pure function returning a `Schema` (not checked live).
    #[arg(long = "db-schema", value_name = "MODULE.FN", requires = "host")]
    pub schema: Option<String>,
}

/// Configuration sources; the environment is read with no `PLY_` prefix or case translation.
#[derive(Args, Clone, Debug, Default)]
pub struct ConfigOptions {
    /// A configuration value: `--set DESK_REGION=eu`. Repeatable; highest precedence, last wins.
    #[arg(
        id = "config_set",
        long = "set",
        value_name = "KEY=VALUE",
        requires = "host"
    )]
    pub set: Vec<String>,

    /// A `KEY=VALUE` file, one pair per line, no quoting. Repeatable; a later file wins.
    #[arg(
        id = "config_files",
        long = "config",
        value_name = "PATH",
        requires = "host"
    )]
    pub files: Vec<PathBuf>,

    /// `<module>.<fn>`: a nullary pure function returning a `ConfigSpec`, checked at start-up.
    #[arg(
        id = "config_schema",
        long = "config-schema",
        value_name = "MODULE.FN",
        requires = "host"
    )]
    pub schema: Option<String>,
}

#[derive(Args, Clone, Debug, Default)]
pub struct TraceOptions {
    /// Where a `trace` record goes: `json` lines or `text` lines on stderr, or `off`.
    #[arg(
        id = "trace_sink",
        long = "trace",
        value_enum,
        default_value_t = SinkArg::Json,
        value_name = "SINK",
        requires = "host",
    )]
    pub sink: SinkArg,

    /// The lowest level the sink writes; spans and metrics are `info`.
    #[arg(
        id = "trace_level",
        long = "trace-level",
        value_enum,
        default_value_t = LevelArg::Info,
        value_name = "LEVEL",
        requires = "host",
    )]
    pub level: LevelArg,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, ValueEnum)]
pub enum SinkArg {
    #[default]
    Json,
    Text,
    Off,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, ValueEnum)]
pub enum LevelArg {
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

/// One program per resource label; only `ply run --host` starts a process, so only it binds one.
#[derive(Args, Clone, Debug, Default)]
pub struct ExecOptions {
    /// Program a `process.spawn` label may start: `--exec cc=/usr/bin/cc`. Repeatable.
    #[arg(
        long = "exec",
        value_name = "NAME=PATH",
        value_parser = parse_executable,
        requires = "host",
    )]
    pub exec: Vec<ExecSpec>,
}

/// What a `SIGINT` or a `SIGTERM` does to a serving run.
#[derive(Args, Clone, Debug)]
pub struct ShutdownOptions {
    /// How long in-flight requests have to finish after accepting stops (else `W0608`, exit 3).
    #[arg(
        long = "drain-ms",
        value_name = "MS",
        default_value_t = ply_host::signal::DEFAULT_DRAIN_MS,
        requires = "host",
    )]
    pub drain_ms: u64,

    /// How long accept keeps running after the signal, so a readiness route can answer `503`.
    #[arg(
        long = "drain-lead-ms",
        value_name = "MS",
        default_value_t = ply_host::signal::DEFAULT_LEAD_MS,
        requires = "host",
    )]
    pub drain_lead_ms: u64,
}

impl Default for ShutdownOptions {
    fn default() -> ShutdownOptions {
        ShutdownOptions {
            drain_ms: ply_host::signal::DEFAULT_DRAIN_MS,
            drain_lead_ms: ply_host::signal::DEFAULT_LEAD_MS,
        }
    }
}

impl ShutdownOptions {
    pub fn bounds(&self) -> ply_host::signal::Bounds {
        ply_host::signal::Bounds {
            lead: std::time::Duration::from_millis(self.drain_lead_ms),
            drain: std::time::Duration::from_millis(self.drain_ms),
        }
    }
}

/// A bad shape is a usage error; `E0430` is for a broken PEM.
fn parse_credential(text: &str) -> Result<CredentialSpec, String> {
    CredentialSpec::parse(text)
}

/// A bad shape is a usage error; `E0454` is for a root that does not resolve.
fn parse_root(text: &str) -> Result<RootSpec, String> {
    RootSpec::parse(text)
}

/// A bad shape is a usage error; `E0457` is for a program that cannot be executed.
fn parse_executable(text: &str) -> Result<ExecSpec, String> {
    ExecSpec::parse(text)
}

/// Refuses non-canonical forms: a loosely parsed seed would replay the wrong interleaving.
fn parse_seed(text: &str) -> Result<Seed, String> {
    Seed::parse(text).ok_or_else(|| {
        format!(
            "`{text}` is not a seed; write `7` for a whole search or `7:3.0.2` \
             for one interleaving, and copy the one the failure printed"
        )
    })
}

#[derive(Args, Debug)]
pub struct CheckArgs {
    /// A `.ply` file, or a project root whose `*.ply` files are modules named by path.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Print the inferred signature and footprint of every definition.
    #[arg(long)]
    pub types: bool,

    /// Print, for every `push`, whether it grows its list in place or copies it.
    #[arg(long)]
    pub costs: bool,

    /// Emit one JSON object on stdout and nothing else.
    #[arg(long)]
    pub json: bool,

    /// Report where the front end's time went, and where every row came from.
    #[arg(long)]
    pub explain: bool,
}

#[derive(Args, Clone, Debug)]
pub struct TestArgs {
    /// A `.ply` file, or a project root whose `*.ply` files are modules named by path.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Emit one JSON object on stdout and nothing else.
    #[arg(long)]
    pub json: bool,

    /// Show why each test was selected or skipped, and how concurrency groups formed.
    #[arg(long)]
    pub explain: bool,

    /// Neither read nor write the result cache: every test runs.
    #[arg(long)]
    pub no_cache: bool,

    /// Only consider tests whose `<module>.<label>` key contains this substring.
    #[arg(long, value_name = "SUBSTRING")]
    pub filter: Option<String>,

    /// Worker threads. Defaults to one per core.
    #[arg(long, short = 'j', value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub jobs: Option<u32>,

    /// Calls one test may make; past it the test fails with E0503. 0 is no bound.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(i64).range(0..))]
    #[arg(default_value_t = ply_eval::DEFAULT_STEP_BUDGET)]
    pub steps: i64,

    /// Wall clock per test, in milliseconds; past it the run is abandoned rather than judged,
    /// and nothing it did is recorded. 0 is no clock.
    #[arg(long, value_name = "MS", default_value_t = 60_000)]
    pub timeout: u64,

    /// Attribute a failure to its change; `auto` bisects only a det test that has passed before.
    #[arg(long, value_enum, default_value_t = When::Auto, value_name = "WHEN")]
    pub bisect: When,

    /// Hybrid programs a bisection may evaluate; counted, not timed, so runs agree.
    #[arg(long, default_value_t = 64, value_name = "N")]
    pub bisect_budget: usize,

    /// Which tests reach each definition, and the definitions none reaches.
    #[arg(long)]
    pub coverage: bool,

    /// After a green run, change each definition (or DEF) one operator or literal at a time and
    /// re-run the tests that reach it; a mutant every one of them passes is reported.
    #[arg(long, value_name = "DEF", num_args = 0..=1, default_missing_value = "*")]
    pub mutate: Option<String>,

    /// Mutants a run may judge; counted, not timed, so runs agree.
    #[arg(long, default_value_t = 64, value_name = "N")]
    pub mutate_budget: usize,

    /// Record which definitions a failing test entered; `always` also traces the first run.
    #[arg(long, value_enum, default_value_t = When::Auto, value_name = "WHEN")]
    pub trace: When,

    /// Attach a compiled backend: `c`.
    #[arg(long, value_name = "BACKEND")]
    pub backend: Option<String>,

    /// C toolchain: `development` (`tcc`, else `cc -O0`) or `release` (`cc -O2`).
    #[arg(
        long,
        value_name = "PROFILE",
        default_value = "development",
        requires = "backend"
    )]
    pub profile: String,

    /// Stay running: re-select and re-run whenever a `.ply` file under the path changes.
    #[arg(long)]
    pub watch: bool,

    /// Bind the real host handlers; a test that reaches one always runs and is never cached.
    #[arg(long)]
    pub host: bool,

    #[command(flatten)]
    pub tls: TlsOptions,

    #[command(flatten)]
    pub fs: FsOptions,

    #[command(flatten)]
    pub db: DbOptions,

    #[command(flatten)]
    pub config: ConfigOptions,

    /// Also select the tests declared by the modules that ship with the compiler.
    #[arg(long)]
    pub std: bool,

    #[command(flatten)]
    pub simulation: SimOptions,
}

/// Search bounds; they key every cached result weaker than a proof.
#[derive(Args, Clone, Debug)]
pub struct ProveOptions {
    /// Candidate binder tuples drawn per root; fewer than 25 kept can only report `example`.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub prove_cases: Option<u32>,

    /// Generator roots, each drawing its own case set.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub prove_roots: Option<u32>,

    /// Static inference steps per obligation; a spent budget reports `property`.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub prove_budget: Option<u32>,

    /// Evaluations a counterexample may be shrunk by; in no cache key, since failures never are.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub shrink_budget: Option<u32>,

    /// Calls per evaluation of a claim (default 1000000000); 0 is no bound. It keys the result.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(i64).range(0..))]
    pub prove_steps: Option<i64>,
}

#[derive(Args, Debug)]
pub struct ProveArgs {
    /// A `.ply` file, or a project root whose `*.ply` files are modules named by path.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Emit one JSON object on stdout and nothing else.
    #[arg(long)]
    pub json: bool,

    /// Show where each discharge came from: the cache, a proof, or a search.
    #[arg(long)]
    pub explain: bool,

    /// Neither read nor write the obligation cache: every obligation is discharged again.
    #[arg(long)]
    pub no_cache: bool,

    /// Only consider obligations whose owner contains this substring.
    #[arg(long, value_name = "SUBSTRING")]
    pub filter: Option<String>,

    /// Worker threads. Defaults to one per core.
    #[arg(long, short = 'j', value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub jobs: Option<u32>,

    /// Neither read nor write the front-end cache.
    #[arg(long)]
    pub no_incremental: bool,

    /// Also discharge the laws declared by the modules that ship with the compiler.
    #[arg(long)]
    pub std: bool,

    /// Bind the real host handlers, so that a `law/host` is attempted.
    #[arg(long)]
    pub host: bool,

    /// Attach a compiled backend, as `ply test --backend` does, for laws and contracts.
    #[arg(long, value_name = "BACKEND")]
    pub backend: Option<String>,

    #[command(flatten)]
    pub tls: TlsOptions,

    #[command(flatten)]
    pub fs: FsOptions,

    #[command(flatten)]
    pub db: DbOptions,

    #[command(flatten)]
    pub config: ConfigOptions,

    #[command(flatten)]
    pub trace: TraceOptions,

    #[command(flatten)]
    pub prove: ProveOptions,

    #[command(flatten)]
    pub simulation: SimOptions,
}

#[derive(Args, Debug)]
pub struct ReviewArgs {
    /// A `.ply` file, or a project root whose `*.ply` files are modules named by path.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Report only what moved since the last accepted review (the default).
    #[arg(long)]
    pub changed: bool,

    /// Record the current definitions and specifications as reviewed, keyed by name.
    #[arg(long, conflicts_with = "changed")]
    pub accept: bool,

    /// Emit one JSON object on stdout and nothing else.
    #[arg(long)]
    pub json: bool,

    /// Neither read nor write the obligation cache.
    #[arg(long)]
    pub no_cache: bool,

    /// Neither read nor write the front-end cache.
    #[arg(long)]
    pub no_incremental: bool,

    /// Also review the definitions the modules that ship with the compiler declare.
    #[arg(long)]
    pub std: bool,

    /// Attach a compiled backend, as `ply prove --backend` does.
    #[arg(long, value_name = "BACKEND")]
    pub backend: Option<String>,

    #[command(flatten)]
    pub prove: ProveOptions,

    #[command(flatten)]
    pub simulation: SimOptions,
}

#[derive(Args, Clone, Debug)]
pub struct RunArgs {
    /// A `.ply` file, or a project root whose `*.ply` files are modules named by path.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// What `process.args` answers: everything after `--`.
    #[arg(last = true, value_name = "ARGS")]
    pub argv: Vec<String>,

    /// Emit one JSON object on stdout and nothing else.
    #[arg(long)]
    pub json: bool,

    /// Calls the entry point may make; past it the run fails with E0503. 0 is no bound, which
    /// is the default: an entry that serves forever is a program, not a runaway.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(i64).range(0..))]
    #[arg(default_value_t = 0)]
    pub steps: i64,

    /// Wall clock for the entry point, in milliseconds; past it the run is abandoned rather than
    /// judged. 0 is no clock.
    #[arg(long, value_name = "MS", default_value_t = 0)]
    pub timeout: u64,

    /// The interleaving a `simulate` region takes: `7`, or `7:3.0.2`.
    #[arg(long, value_name = "SEED", value_parser = parse_seed)]
    pub seed: Option<Seed>,

    /// Bind the real host handlers; unbound, reaching the boundary is a diagnostic.
    #[arg(long)]
    pub host: bool,

    #[command(flatten)]
    pub tls: TlsOptions,

    #[command(flatten)]
    pub fs: FsOptions,

    #[command(flatten)]
    pub exec: ExecOptions,

    #[command(flatten)]
    pub db: DbOptions,

    #[command(flatten)]
    pub config: ConfigOptions,

    #[command(flatten)]
    pub trace: TraceOptions,

    #[command(flatten)]
    pub shutdown: ShutdownOptions,

    /// Attach a compiled backend, as `ply test --backend` does.
    #[arg(long, value_name = "BACKEND")]
    pub backend: Option<String>,

    /// C toolchain: `development` (`tcc`, else `cc -O0`) or `release` (`cc -O2`).
    #[arg(
        long,
        value_name = "PROFILE",
        default_value = "development",
        requires = "backend"
    )]
    pub profile: String,
}

#[derive(Args, Debug)]
pub struct BuildArgs {
    /// A `.ply` file, or a project root whose `*.ply` files are modules named by path.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Where to write the artifact. Defaults to `<entry module>.plyx` in the working directory.
    #[arg(long, short = 'o', value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// The definition whose closure is shipped: `app.serve` or a simple name. Defaults to `main`.
    #[arg(long, value_name = "NAME")]
    pub entry: Option<String>,

    /// Also ship this `--config-schema` function's closure, so the artifact takes the same flag.
    #[arg(long = "config-schema", value_name = "MODULE.FN")]
    pub config_schema: Option<String>,

    /// Also ship this `--db-schema` function's closure, so the artifact takes the same flag.
    #[arg(long = "db-schema", value_name = "MODULE.FN")]
    pub db_schema: Option<String>,

    /// Print `b3:...` and nothing else: the line a deployment pins. Writes no file.
    #[arg(long, conflicts_with_all = ["json", "diff", "output"])]
    pub digest: bool,

    /// Report what this build changes relative to a deployed artifact. Writes no file.
    #[arg(long, value_name = "OLD.plyx", conflicts_with = "output")]
    pub diff: Option<PathBuf>,

    /// Emit one JSON object on stdout and nothing else.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct HostsArgs {
    /// A `.ply` file, or a project root whose `*.ply` files are modules named by path.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// List the handlers as bound; registration errors are reported either way.
    #[arg(long)]
    pub host: bool,

    #[command(flatten)]
    pub tls: TlsOptions,

    #[command(flatten)]
    pub fs: FsOptions,

    #[command(flatten)]
    pub db: DbOptions,

    #[command(flatten)]
    pub config: ConfigOptions,

    #[command(flatten)]
    pub trace: TraceOptions,

    /// Accepted here as well as on `ply run` because the drain bounds are in the digest.
    #[command(flatten)]
    pub shutdown: ShutdownOptions,

    /// Emit one JSON object on stdout and nothing else.
    #[arg(long, conflicts_with = "digest")]
    pub json: bool,

    /// Print `b3:...` and nothing else: the line a CI check pins.
    #[arg(long)]
    pub digest: bool,
}

/// Needs no project: the modules are compiled into the binary.
#[derive(Args, Debug)]
pub struct StdArgs {
    /// Emit one JSON object on stdout and nothing else.
    #[arg(long, conflicts_with = "digest")]
    pub json: bool,

    /// Print `b3:...` and nothing else: the line a CI check pins.
    #[arg(long)]
    pub digest: bool,

    /// Print each module's source instead of listing it.
    #[arg(long, value_name = "MODULE")]
    pub show: Option<String>,
}

#[derive(Args, Debug)]
pub struct ExplainArgs {
    /// A code such as `E0302` or `W0611`.
    #[arg(required_unless_present = "all")]
    pub code: Option<String>,

    /// List every code with its meaning.
    #[arg(long)]
    pub all: bool,

    /// Emit one JSON object on stdout and nothing else.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct DocArgs {
    /// A program-wide name (`store.orders.place`), a simple name unique in the program, or a builtin.
    #[arg(value_name = "NAME")]
    pub query: String,

    /// A `.ply` file, or a project root whose `*.ply` files are modules named by path.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Emit one JSON object on stdout and nothing else.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct FmtArgs {
    /// `.ply` files, or directories whose `*.ply` files are formatted, under the working directory
    /// (a hidden directory, one named `target`, and a symlink are passed over).
    #[arg(default_value = ".")]
    pub paths: Vec<PathBuf>,

    /// Write nothing; list the files that would change and exit 1 if there are any.
    #[arg(long)]
    pub check: bool,

    /// Emit one JSON object on stdout and nothing else.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct ShowArgs {
    /// A program-wide name (`store.orders.place`) or a simple name unique in the program: a `fn` or a `type`.
    #[arg(value_name = "NAME")]
    pub query: String,

    /// A `.ply` file, or a project root whose `*.ply` files are modules named by path.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Emit one JSON object on stdout and nothing else.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct ReplaceArgs {
    /// A program-wide name (`store.orders.place`) or a simple name unique in the program: a `fn` or a `type`.
    #[arg(value_name = "NAME")]
    pub query: String,

    /// The file holding the new definition; stdin when absent.
    #[arg(long, value_name = "FILE")]
    pub with: Option<PathBuf>,

    /// A `.ply` file, or a project root whose `*.ply` files are modules named by path.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Write nothing; report whether the file would change.
    #[arg(long)]
    pub check: bool,

    /// Emit one JSON object on stdout and nothing else.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct DefsArgs {
    /// A `.ply` file, or a project root whose `*.ply` files are modules named by path.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Only definitions whose program-wide name contains this substring.
    #[arg(long, value_name = "SUBSTRING")]
    pub filter: Option<String>,

    /// Emit one JSON object on stdout and nothing else.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct CallersArgs {
    /// A program-wide name (`store.orders.place`) or a simple name unique in the program.
    #[arg(value_name = "DEF")]
    pub query: String,

    /// A `.ply` file, or a project root whose `*.ply` files are modules named by path.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Emit one JSON object on stdout and nothing else.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct HashArgs {
    /// A `.ply` file, or a project root whose `*.ply` files are modules named by path.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Also print each definition's direct references and transitive closure.
    #[arg(long)]
    pub deps: bool,

    /// Emit one JSON object on stdout and nothing else.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct CacheArgs {
    #[command(subcommand)]
    pub action: CacheAction,
}

#[derive(Subcommand, Debug)]
pub enum CacheAction {
    /// Discard every cached result, so the next run re-proves everything.
    Clear(CacheScope),
    /// Report where the cache lives, how much it holds, and what is reclaimable.
    Stats(CacheScope),
    /// Reclaim the space nothing points at any more.
    Compact(CacheScope),
    /// Print what the cache holds for one definition, resolved and readable.
    Inspect(InspectArgs),
}

#[derive(Args, Debug)]
pub struct CacheScope {
    /// The project whose `.ply-cache` is meant; a `.ply` file means its directory.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Emit one JSON object on stdout and nothing else.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct InspectArgs {
    /// A program-wide name, a name as its module wrote it, or a hash prefix of 4+ hex digits.
    #[arg(value_name = "DEF")]
    pub query: String,

    /// The project whose `.ply-cache` is meant; a `.ply` file means its directory.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Emit one JSON object on stdout and nothing else.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct BootstrapArgs {
    /// A `.ply` file, or a project root: the front end to write out.
    pub path: PathBuf,

    /// Where the archive goes: the C and its manifest.
    #[arg(long, value_name = "DIR", default_value = "bootstrap")]
    pub out: PathBuf,

    /// Re-emit and compare against the existing archive, writing nothing; non-zero on mismatch.
    #[arg(long)]
    pub verify: bool,

    /// C toolchain for the emitter that writes the archive; `release` by default here.
    #[arg(long, value_name = "PROFILE", default_value = "release")]
    pub profile: String,

    /// Emit one JSON object on stdout and nothing else.
    #[arg(long)]
    pub json: bool,
}

// --- Conversions into the runtime's plain options -----------------------------------------------

impl From<&TlsOptions> for ply_machine::options::TlsOptions {
    fn from(args: &TlsOptions) -> Self {
        ply_machine::options::TlsOptions {
            tls: args.tls.clone(),
            trust: args.trust.clone(),
        }
    }
}

impl From<&ShutdownOptions> for ply_machine::options::ShutdownOptions {
    fn from(args: &ShutdownOptions) -> Self {
        ply_machine::options::ShutdownOptions {
            drain_ms: args.drain_ms,
            drain_lead_ms: args.drain_lead_ms,
        }
    }
}

impl From<&DbOptions> for ply_machine::db::DbOptions {
    fn from(args: &DbOptions) -> Self {
        ply_machine::db::DbOptions {
            url: args.url.clone(),
            pool: args.pool,
            acquire_ms: args.acquire_ms,
            statement_ms: args.statement_ms,
            idle_txn_ms: args.idle_txn_ms,
            connect_ms: args.connect_ms,
            statement_cache: args.statement_cache,
            schema: args.schema.clone(),
        }
    }
}

impl From<&ConfigOptions> for ply_machine::config::ConfigOptions {
    fn from(args: &ConfigOptions) -> Self {
        ply_machine::config::ConfigOptions {
            set: args.set.clone(),
            files: args.files.clone(),
            schema: args.schema.clone(),
        }
    }
}

impl From<&TraceOptions> for ply_machine::trace::TraceOptions {
    fn from(args: &TraceOptions) -> Self {
        ply_machine::trace::TraceOptions {
            sink: match args.sink {
                SinkArg::Json => ply_machine::trace::SinkArg::Json,
                SinkArg::Text => ply_machine::trace::SinkArg::Text,
                SinkArg::Off => ply_machine::trace::SinkArg::Off,
            },
            level: match args.level {
                LevelArg::Debug => ply_machine::trace::LevelArg::Debug,
                LevelArg::Info => ply_machine::trace::LevelArg::Info,
                LevelArg::Warn => ply_machine::trace::LevelArg::Warn,
                LevelArg::Error => ply_machine::trace::LevelArg::Error,
            },
        }
    }
}

impl From<&SimOptions> for ply_machine::simulation::SimOptions {
    fn from(args: &SimOptions) -> Self {
        ply_machine::simulation::SimOptions {
            seed: args.seed.clone(),
            sim: args.sim.into(),
            seeds: args.seeds,
            sim_budget: args.sim_budget,
            sim_steps: args.sim_steps,
            measure_reduction: args.measure_reduction,
        }
    }
}

impl From<&ProveOptions> for ply_machine::simulation::ProveOptions {
    fn from(args: &ProveOptions) -> Self {
        ply_machine::simulation::ProveOptions {
            prove_cases: args.prove_cases,
            prove_roots: args.prove_roots,
            prove_budget: args.prove_budget,
            shrink_budget: args.shrink_budget,
            prove_steps: args.prove_steps,
        }
    }
}
