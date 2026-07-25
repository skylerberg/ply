use crate::style::ColorChoice;
use clap::{Args, Parser, Subcommand};
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

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Typecheck a file or directory.
    Check(CheckArgs),
    /// Select, schedule and run the tests.
    Test(TestArgs),
    /// Evaluate `main`. A directory must hold exactly one.
    Run(RunArgs),
    /// Print the content hash of every definition.
    Hash(HashArgs),
    /// Inspect or discard the result cache.
    Cache(CacheArgs),
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
    /// Report where the cache lives and how much it holds.
    Stats(CacheScope),
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
    }
}
