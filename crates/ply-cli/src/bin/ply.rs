use clap::{CommandFactory, Parser};

/// The front end and emitter recurse once per node on the native stack.
const STACK: usize = 256 << 20;

fn main() {
    let command = std::thread::Builder::new()
        .name("ply".to_string())
        .stack_size(STACK)
        .spawn(run)
        .expect("a thread for the command");
    let code = command
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
    std::process::exit(code);
}

fn run() -> i32 {
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
    code
}
