//! `ply run` — the runner for the `run` command of the program in `crates/ply-cli/ply`.

use super::shipped_program::{color, run};
use crate::artifact::Binds;
use crate::cli::RunArgs;
use crate::style::Style;
use std::path::Path;

pub fn execute(args: &RunArgs, style: Style) -> i32 {
    // The load, the artifact, the host binding and the entry itself stay behind the effect the
    // program performs; the program is lent what each step did and enters nothing of its own.
    let options = ply_machine::drive::RunOptions {
        argv: args.argv.clone(),
        json: args.json,
        steps: args.steps,
        timeout: args.timeout,
        seed: args.seed.clone(),
        host: args.host,
        tls: (&args.tls).into(),
        fs: args.fs.fs.clone(),
        exec: args.exec.exec.clone(),
        db: (&args.db).into(),
        config: (&args.config).into(),
        trace: (&args.trace).into(),
        shutdown: (&args.shutdown).into(),
        backend: args.backend.clone(),
        profile: args.profile.clone(),
        cache: false,
    };
    let binds = Binds {
        lent: ply_machine::registrations_with(options),
        ..Binds::default()
    };
    run(
        "run",
        argv(args, style),
        Path::new("."),
        binds,
        args.json,
        style,
    )
}

/// What `process.args` answers: the command word, then the flags the program reads. Every other
/// flag a run takes configures the machine rather than the report, and is read where the machine
/// is built.
fn argv(args: &RunArgs, style: Style) -> Vec<String> {
    let mut argv = vec!["run".to_string(), color(style)];
    if args.json {
        argv.push("--json".to_string());
    }
    // The program names the target to `machine.load` itself.
    argv.push(args.path.display().to_string());
    argv
}
