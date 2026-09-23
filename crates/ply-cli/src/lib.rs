//! The `ply` binary.

pub mod cache;
pub mod claims;
pub mod cli;
pub mod commands;
pub mod engine;
pub mod obligations;
pub mod shipped;
pub mod signature;
pub mod style;

use cli::{Cli, Command};

// The incremental front end and its store moved to the runtime: they serve any tool that loads a
// program, not just this binary. Re-exported so the suite's unit tests read as they did.
pub use ply_machine::drive as run;
pub use ply_machine::tester as test;
pub use ply_machine::{
    artifact, config, costs, db, driver, hosts, load, migrate, payload, simulation, trace, warm,
};
pub use ply_machine::{bootstrap, builder as build, mutate};
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
