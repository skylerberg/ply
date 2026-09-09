use clap::{CommandFactory, Parser};

/// A command runs on a thread with room for a deep program: the front end and the emitter
/// recurse once per node on the native stack, and the default thread is not enough for an
/// expression a few thousand deep, which the harness's workers already allow for.
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
