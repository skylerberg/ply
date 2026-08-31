use clap::{CommandFactory, Parser};

fn main() {
    let cli = ply_cli::cli::Cli::parse();
    if let Some(conflict) = cli.conflict() {
        ply_cli::cli::Cli::command()
            .error(clap::error::ErrorKind::ArgumentConflict, conflict)
            .exit();
    }
    let code = ply_cli::execute(cli);
    if ply_eval::census::enabled() {
        eprint!("{}", ply_eval::census::report());
    }
    std::process::exit(code);
}
