use crate::style::ColorChoice;
use clap::{Args, Parser, Subcommand};
use ply_eval::Seed;
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

    /// Colour and the ✓/✗ marks. `auto` uses them only when stdout is a
    /// terminal and NO_COLOR is unset.
    #[arg(long, value_enum, default_value_t = ColorChoice::Auto, global = true)]
    pub color: ColorChoice,
}

impl Cli {
    /// What `clap`'s own machinery cannot check. `None` when the command line is
    /// coherent.
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
    /// What changed, whether its specification changed, and whether its
    /// obligations still hold.
    Review(ReviewArgs),
    /// Evaluate `main`. A directory must hold exactly one.
    Run(RunArgs),
    /// List every host handler this binary can bind: the trusted computing base.
    Hosts(HostsArgs),
    /// Print the content hash of every definition.
    Hash(HashArgs),
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

/// Which evaluator runs. `both` runs each program on both and fails the run on
/// any disagreement, which is the only check that catches an engine drifting.
///
/// Mirrors `ply_eval::EngineChoice` rather than deriving `ValueEnum` on it,
/// because `ply-eval` does not depend on `clap` and should not start.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
#[value(rename_all = "lower")]
pub enum EngineArg {
    Treewalk,
    #[default]
    Machine,
    Both,
}

impl EngineArg {
    pub fn as_str(self) -> &'static str {
        ply_eval::EngineChoice::from(self).as_str()
    }
}

impl From<EngineArg> for ply_eval::EngineChoice {
    fn from(a: EngineArg) -> ply_eval::EngineChoice {
        match a {
            EngineArg::Treewalk => ply_eval::EngineChoice::Treewalk,
            EngineArg::Machine => ply_eval::EngineChoice::Machine,
            EngineArg::Both => ply_eval::EngineChoice::Both,
        }
    }
}

/// Which search a `simulate` region runs.
///
/// Mirrors `ply_eval::SimMode` rather than deriving `ValueEnum` on it, because
/// `ply-eval` does not depend on `clap` and should not start.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
#[value(rename_all = "lower")]
pub enum SimArg {
    /// One interleaving, the one the seed names. The replay path.
    Once,
    /// One interleaving per seed. No state, and the seeds are independent, which
    /// is what makes widening a seed set cost only the new seeds.
    Random,
    /// Footprint-guided partial-order reduction. Never runs two interleavings
    /// from one equivalence class, so a small state space finishes exhaustively
    /// — a proof rather than a sample.
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

/// What a run searches, and therefore half of what a simulated test is cached
/// under. Every field is in the key: a green run under one plan is not a green
/// run under another.
#[derive(Args, Clone, Debug)]
pub struct SimOptions {
    /// Replay exactly one interleaving: `7`, or `7:3.0.2` for a seed the search
    /// refined. Implies `--sim once`, and it is the whole of a repro.
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

    /// Seeds per simulated test. Defaults to 1 under `dpor`, which already
    /// enumerates equivalence classes, and 64 under `random`.
    #[arg(long = "seeds", alias = "sim-roots", value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub seeds: Option<u32>,

    /// Interleavings per seed. Only `dpor` searches more than one.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub sim_budget: Option<u32>,

    /// Scheduling steps one interleaving may take before the region is reported
    /// as making no progress.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub sim_steps: Option<u32>,

    /// Also run the search with the dependence relation forced to `true` and
    /// report what an unpruned one would have cost. Off by default: the claim is
    /// a benchmark, not something every run pays double for.
    #[arg(long)]
    pub measure_reduction: bool,
}

impl SimOptions {
    /// The one contradiction `conflicts_with` cannot express, because it is
    /// between a flag and a *value* of another flag.
    ///
    /// Refused rather than ignored: a `--sim-budget` that is silently dropped
    /// reads as a search that was widened and was not.
    pub fn conflict(&self) -> Option<String> {
        (self.sim == SimArg::Random && self.sim_budget.is_some()).then(|| {
            "`--sim-budget` has no meaning under `--sim random`, which runs one \
             interleaving per seed; widen the search with `--seeds N`, or use \
             `--sim dpor` to spend a budget per seed"
                .to_string()
        })
    }
}

/// A seed that parses loosely replays something other than what failed, so
/// every form that is not canonical is refused with the two that are.
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
    /// A `.ply` file, or a project root: every `*.ply` under it is a module
    /// named after its path relative to that root.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Print the inferred signature and footprint of every definition.
    #[arg(long)]
    pub types: bool,

    /// Emit one JSON object on stdout and nothing else.
    #[arg(long)]
    pub json: bool,

    /// Report which files were parsed and which definitions were rechecked,
    /// with the reason a skip was refused.
    #[arg(long)]
    pub explain: bool,

    /// Neither read nor write the front-end cache: parse every file and
    /// recheck every definition.
    #[arg(long)]
    pub no_incremental: bool,

    /// Which evaluator the program has to be runnable by. A handler clause
    /// that binds a continuation is refused by `treewalk`.
    #[arg(long, value_enum, default_value_t = EngineArg::default(), value_name = "ENGINE")]
    pub engine: EngineArg,
}

#[derive(Args, Debug)]
pub struct TestArgs {
    /// A `.ply` file, or a project root: every `*.ply` under it is a module
    /// named after its path relative to that root.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Emit one JSON object on stdout and nothing else.
    #[arg(long)]
    pub json: bool,

    /// Show why each test was selected or skipped, and how the concurrency
    /// groups were formed.
    #[arg(long)]
    pub explain: bool,

    /// Neither read nor write the result cache: every test runs, and nothing
    /// this run proves is remembered.
    #[arg(long)]
    pub no_cache: bool,

    /// Only consider tests whose `<module>.<label>` key contains this
    /// substring.
    #[arg(long, value_name = "SUBSTRING")]
    pub filter: Option<String>,

    /// Worker threads. Defaults to one per core.
    #[arg(long, short = 'j', value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub jobs: Option<u32>,

    /// Neither read nor write the front-end cache. The result cache is
    /// untouched; `--no-cache` is what disables both.
    #[arg(long)]
    pub no_incremental: bool,

    /// Attribute a failure to the change that caused it. `auto` bisects a
    /// failing det test that has passed before; `never` reports no culprit at
    /// all.
    #[arg(long, value_enum, default_value_t = When::Auto, value_name = "WHEN")]
    pub bisect: When,

    /// Hybrid programs a bisection may evaluate. Counted in evaluations rather
    /// than seconds, so two runs over the same failure agree.
    #[arg(long, default_value_t = 64, value_name = "N")]
    pub bisect_budget: usize,

    /// Record which definitions a failing test actually entered. `auto` traces
    /// the re-run of a failure; `always` traces the first execution too.
    #[arg(long, value_enum, default_value_t = When::Auto, value_name = "WHEN")]
    pub trace: When,

    /// Which evaluator runs. `both` runs each test on both and fails on any
    /// disagreement. Anything but the default neither reads nor writes the
    /// result cache.
    #[arg(long, value_enum, default_value_t = EngineArg::default(), value_name = "ENGINE")]
    pub engine: EngineArg,

    /// Bind the real host handlers. Off by default, and the default is the
    /// point: a suite that silently acquires a live dependency is the failure
    /// mode this language exists to prevent. A test that reaches a bound
    /// handler always runs and is never cached.
    #[arg(long)]
    pub host: bool,

    #[command(flatten)]
    pub simulation: SimOptions,
}

/// What a run searches, and therefore what everything weaker than a proof is
/// cached under. A proof is cached under none of it: it is a claim about every
/// input satisfying the guard, so widening any of these cannot re-open it.
#[derive(Args, Clone, Debug)]
pub struct ProveOptions {
    /// Candidate binder tuples drawn per root. Fewer than 25 *kept* can only
    /// ever report `example`.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub prove_cases: Option<u32>,

    /// Generator roots. Each draws its own case set, so widening this widens
    /// the search rather than repeating it.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub prove_roots: Option<u32>,

    /// Static inference steps per obligation. A spent budget is inconclusive,
    /// which reports `property` — never `proved` and never `refuted`.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub prove_budget: Option<u32>,

    /// Candidate *evaluations* a counterexample may be shrunk by — never
    /// seconds, so two runs over one failure agree. Deliberately not part of any
    /// cache key: it can only change a counterexample's minimality, and
    /// failures are never cached.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub shrink_budget: Option<u32>,
}

#[derive(Args, Debug)]
pub struct ProveArgs {
    /// A `.ply` file, or a project root: every `*.ply` under it is a module
    /// named after its path relative to that root.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Emit one JSON object on stdout and nothing else.
    #[arg(long)]
    pub json: bool,

    /// Show where each discharge came from: the cache, a proof, or a search.
    #[arg(long)]
    pub explain: bool,

    /// Neither read nor write the obligation cache: every obligation is
    /// discharged again, and nothing this run establishes is remembered.
    #[arg(long)]
    pub no_cache: bool,

    /// Only consider obligations whose owner contains this substring.
    #[arg(long, value_name = "SUBSTRING")]
    pub filter: Option<String>,

    /// Worker threads. Defaults to one per core. Every obligation is pure, so
    /// no two contend and this changes only the wall clock.
    #[arg(long, short = 'j', value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub jobs: Option<u32>,

    /// Neither read nor write the front-end cache.
    #[arg(long)]
    pub no_incremental: bool,

    #[command(flatten)]
    pub prove: ProveOptions,

    #[command(flatten)]
    pub simulation: SimOptions,
}

#[derive(Args, Debug)]
pub struct ReviewArgs {
    /// A `.ply` file, or a project root: every `*.ply` under it is a module
    /// named after its path relative to that root.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Report only what moved since the last accepted review. This is the
    /// default; naming it is how a script says what it meant.
    #[arg(long)]
    pub changed: bool,

    /// Record the current definitions and specifications as reviewed. Written
    /// per definition, keyed by name, so that renaming one loses its baseline
    /// and reports it as unreviewed rather than as unchanged.
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

    #[command(flatten)]
    pub prove: ProveOptions,

    #[command(flatten)]
    pub simulation: SimOptions,
}

#[derive(Args, Debug)]
pub struct RunArgs {
    /// A `.ply` file, or a project root: every `*.ply` under it is a module
    /// named after its path relative to that root.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Emit one JSON object on stdout and nothing else.
    #[arg(long)]
    pub json: bool,

    /// Which evaluator runs. `both` runs each test on both and fails on any
    /// disagreement. Anything but the default neither reads nor writes the
    /// result cache.
    #[arg(long, value_enum, default_value_t = EngineArg::default(), value_name = "ENGINE")]
    pub engine: EngineArg,

    /// The interleaving a `simulate` region takes: `7`, or `7:3.0.2`.
    ///
    /// `ply run` explores exactly one interleaving whatever this says —
    /// exploration is a test-time activity — so this chooses which one rather
    /// than how many.
    #[arg(long, value_name = "SEED", value_parser = parse_seed)]
    pub seed: Option<Seed>,

    /// Bind the real host handlers. Off by default: reaching the boundary with
    /// nothing bound is a diagnostic naming the handler that would have served
    /// it, never a silent syscall.
    #[arg(long)]
    pub host: bool,
}

#[derive(Args, Debug)]
pub struct HostsArgs {
    /// A `.ply` file, or a project root: every `*.ply` under it is a module
    /// named after its path relative to that root.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// List the handlers as bound rather than reporting that nothing is.
    /// Resolution — and therefore any registration error — happens either way.
    #[arg(long)]
    pub host: bool,

    /// Emit one JSON object on stdout and nothing else.
    #[arg(long, conflicts_with = "digest")]
    pub json: bool,

    /// Print `b3:...` and nothing else: the one line a CI check pins against
    /// the trusted computing base.
    #[arg(long)]
    pub digest: bool,
}

#[derive(Args, Debug)]
pub struct HashArgs {
    /// A `.ply` file, or a project root: every `*.ply` under it is a module
    /// named after its path relative to that root.
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
    /// The project whose `.ply-cache` is meant; a `.ply` file means its
    /// directory.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Emit one JSON object on stdout and nothing else.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct InspectArgs {
    /// A program-wide name (`store.orders.place`), a name as its module wrote
    /// it (`place`), or a hash prefix of at least four hex characters.
    #[arg(value_name = "DEF")]
    pub query: String,

    /// The project whose `.ply-cache` is meant; a `.ply` file means its
    /// directory.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Emit one JSON object on stdout and nothing else.
    #[arg(long)]
    pub json: bool,
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
    fn path_defaults_to_the_current_directory() {
        let cli = Cli::parse_from(["ply", "test"]);
        match cli.command {
            Command::Test(args) => assert_eq!(args.path, PathBuf::from(".")),
            other => panic!("expected `test`, got {other:?}"),
        }
    }

    #[test]
    fn color_is_accepted_after_the_subcommand() {
        let cli = Cli::parse_from(["ply", "test", "--color", "always"]);
        assert_eq!(cli.color, ColorChoice::Always);
    }

    #[test]
    fn zero_jobs_is_rejected_rather_than_silently_meaning_auto() {
        assert!(Cli::try_parse_from(["ply", "test", "--jobs", "0"]).is_err());
        let cli = Cli::parse_from(["ply", "test", "-j", "4"]);
        match cli.command {
            Command::Test(args) => assert_eq!(args.jobs, Some(4)),
            other => panic!("expected `test`, got {other:?}"),
        }
    }

    #[test]
    fn cache_requires_an_action() {
        assert!(Cli::try_parse_from(["ply", "cache"]).is_err());
        assert!(Cli::try_parse_from(["ply", "cache", "stats"]).is_ok());
        assert!(Cli::try_parse_from(["ply", "cache", "clear"]).is_ok());
        assert!(Cli::try_parse_from(["ply", "cache", "compact"]).is_ok());
    }

    #[test]
    fn inspect_needs_something_to_look_up() {
        assert!(Cli::try_parse_from(["ply", "cache", "inspect"]).is_err());
        let cli = Cli::parse_from(["ply", "cache", "inspect", "9f2c", "src"]);
        match cli.command {
            Command::Cache(args) => match args.action {
                CacheAction::Inspect(inspect) => {
                    assert_eq!(inspect.query, "9f2c");
                    assert_eq!(inspect.path, PathBuf::from("src"));
                }
                other => panic!("expected `inspect`, got {other:?}"),
            },
            other => panic!("expected `cache`, got {other:?}"),
        }
    }

    #[test]
    fn the_bisection_switches_default_to_auto_and_reject_a_fourth_word() {
        let cli = Cli::parse_from(["ply", "test"]);
        match cli.command {
            Command::Test(args) => {
                assert_eq!(args.bisect, When::Auto);
                assert_eq!(args.trace, When::Auto);
                assert_eq!(args.bisect_budget, 64);
            }
            other => panic!("expected `test`, got {other:?}"),
        }
        assert!(Cli::try_parse_from(["ply", "test", "--bisect", "sometimes"]).is_err());
        assert!(Cli::try_parse_from(["ply", "test", "--bisect", "never"]).is_ok());
    }
}
