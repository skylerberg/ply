//! The `ply` binary: the launcher enters the program, which reads the line itself.

use ply_launcher::Program;

/// Counts what the entry allocates; nothing but the entry is counted, and only when `--count-allocs`
/// asked for it.
#[global_allocator]
static ALLOCATOR: ply_launcher::count::Counting = ply_launcher::count::Counting;

fn main() {
    let program = match ply_launcher::shipped::program() {
        Ok(bytes) => Program {
            artifact: bytes,
            artifact_name: ply_launcher::shipped::ARTIFACT.to_string(),
            shelf: ply_machine::shelf::sources().to_vec(),
            stage: format!("cli-{}", ply_launcher::shipped::identity()),
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            std::process::exit(1);
        }
    };
    let mut argv: Vec<String> = std::env::args().skip(1).collect();
    // The launcher's own flags, taken out before the program parses the line.
    let count = match ply_launcher::count::flag(&mut argv) {
        Ok(count) => count,
        Err(why) => {
            eprintln!("ply: {why}");
            std::process::exit(2);
        }
    };
    let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    // Every command family's ops are lent unconfigured to the `ply` program, which is this
    // binary's own; the program configures what it drives.
    let binds = ply_machine::artifact::Binds {
        lent: ply_machine::policy::all(),
        trust: ply_launcher::trust(),
        ..ply_machine::artifact::Binds::default()
    };
    let (answer, warnings) = ply_launcher::run(&program, &root, argv, binds, count).into_parts();
    for warning in warnings {
        eprintln!("{warning}");
    }
    let code = match answer {
        Ok(code) => code,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            1
        }
    };
    std::process::exit(code);
}
