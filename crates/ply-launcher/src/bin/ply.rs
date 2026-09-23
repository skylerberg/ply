//! The `ply` binary: the launcher enters the program, which reads the line itself.

use ply_launcher::Program;

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
            eprint!(
                "{}",
                ply_span::render::to_terminal(&diagnostic, &ply_span::SourceMap::new(), false)
            );
            std::process::exit(1);
        }
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    // Every command family's ops are lent unconfigured; the program configures what it drives.
    let binds = ply_machine::artifact::Binds {
        lent: lent_all(),
        ..ply_machine::artifact::Binds::default()
    };
    let code = match ply_launcher::run(&program, &root, argv, binds) {
        Ok(code) => code,
        Err(diagnostic) => {
            eprint!(
                "{}",
                ply_span::render::to_terminal(&diagnostic, &ply_span::SourceMap::new(), false)
            );
            1
        }
    };
    if ply_eval::census::enabled() {
        eprint!("{}", ply_eval::census::report());
    }
    std::process::exit(code);
}

/// The ops any command may perform, each configured by the program itself.
fn lent_all() -> Vec<(
    ply_eval::host::HostOp,
    std::sync::Arc<dyn ply_eval::host::HostHandler>,
)> {
    let mut lent = ply_machine::registrations();
    lent.extend(
        ply_machine::tester::Session::new(&ply_machine::tester::TestOptions::default()).lent(),
    );
    lent.extend(ply_machine::claims::lent());
    lent.extend(ply_machine::builder::lent());
    lent.extend(ply_machine::cache::lent());
    lent.extend(ply_machine::bootstrap::lent());
    lent.extend(ply_machine::hosts::lent());
    lent.extend(ply_machine::edit::lent());
    lent
}
