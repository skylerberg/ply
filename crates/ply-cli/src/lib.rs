//! The `ply` binary.

pub mod artifact;
pub mod bootstrap;
pub mod build;
pub mod cache;
pub mod cli;
pub mod commands;
pub mod config;
pub mod costs;
pub mod db;
pub mod driver;
pub mod engine;
pub mod hosts;
pub mod load;
pub mod migrate;
pub mod obligations;
pub mod payload;
pub mod run;
pub mod shipped;
pub mod signature;
pub mod simulation;
pub mod style;
pub mod trace;
pub mod warm;

use cli::{Cli, Command};
use style::Style;

pub const EXIT_OK: i32 = 0;
/// At least one test failed, or `main` raised.
pub const EXIT_FAILED: i32 = 1;
/// The program did not get as far as running: a bad path, a syntax error, a type error.
pub const EXIT_COMPILE_ERROR: i32 = 2;
/// The drain deadline expired with requests still in flight.
pub const EXIT_DRAIN_INCOMPLETE: i32 = 3;

pub fn execute(cli: Cli) -> i32 {
    let style = Style::detect(cli.color);
    match &cli.command {
        Command::Check(args) => commands::check::execute(args, style),
        Command::Test(args) => commands::test::execute(args, style),
        Command::Prove(args) => commands::prove::execute(args, style),
        Command::Review(args) => commands::review::execute(args, style),
        Command::Run(args) => commands::run::execute(args, style),
        Command::Build(args) => commands::build::execute(args, style),
        Command::Hosts(args) => commands::hosts::execute(args, style),
        Command::Std(args) => commands::stdlib::execute(args, style),
        Command::Explain(args) => commands::explain::execute(args, style),
        Command::Doc(args) => commands::doc::execute(args, style),
        Command::Fmt(args) => commands::fmt::execute(args, style),
        Command::Show(args) => commands::show::execute(args, style),
        Command::Replace(args) => commands::replace::execute(args, style),
        Command::Hash(args) => commands::hash::execute(args, style),
        Command::Defs(args) => commands::defs::execute(args, style),
        Command::Callers(args) => commands::callers::execute(args, style),
        Command::Bootstrap(args) => commands::bootstrap::execute(args, style),
        Command::Cache(args) => commands::cache::execute(&args.action, style),
    }
}
