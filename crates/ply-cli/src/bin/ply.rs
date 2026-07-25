use clap::Parser;

fn main() {
    let cli = ply_cli::cli::Cli::parse();
    std::process::exit(ply_cli::execute(cli));
}
