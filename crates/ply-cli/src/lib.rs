//! The `ply` binary. Every command is two projections of the same run: lines for
//! a person and, under `--json`, exactly one object on stdout for an agent.

pub mod cli;
pub mod commands;
pub mod driver;
pub mod load;
pub mod migrate;
pub mod simulation;
pub mod style;

use cli::{CacheAction, Cli, Command};
use style::Style;

pub const EXIT_OK: i32 = 0;
/// At least one test failed, or `main` raised.
pub const EXIT_FAILED: i32 = 1;
/// The program did not get as far as running: a bad path, a syntax error, a
/// type error.
pub const EXIT_COMPILE_ERROR: i32 = 2;

pub fn execute(cli: Cli) -> i32 {
    let style = Style::detect(cli.color);
    match &cli.command {
        Command::Check(args) => commands::check::execute(args, style),
        Command::Test(args) => commands::test::execute(args, style),
        Command::Run(args) => commands::run::execute(args, style),
        Command::Hash(args) => commands::hash::execute(args, style),
        Command::Cache(args) => match &args.action {
            CacheAction::Clear(scope) => commands::cache::clear(scope, style),
            CacheAction::Stats(scope) => commands::cache::stats(scope, style),
            CacheAction::Compact(scope) => commands::cache::compact(scope, style),
            CacheAction::Inspect(inspect) => commands::cache::inspect(inspect, style),
        },
    }
}
