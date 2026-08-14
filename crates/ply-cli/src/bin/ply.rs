use clap::{CommandFactory, Parser};

fn main() {
    let cli = ply_cli::cli::Cli::parse();
    if let Some(conflict) = cli.conflict() {
        ply_cli::cli::Cli::command()
            .error(clap::error::ErrorKind::ArgumentConflict, conflict)
            .exit();
    }
    std::process::exit(ply_cli::execute(cli));
}
