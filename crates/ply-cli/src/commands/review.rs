//! `ply review` — the runner for the `review` command of the program in `crates/ply-cli/ply`.

use super::shipped_program::{color, run};
use crate::artifact::Binds;
use crate::claims::Job;
use crate::cli::ReviewArgs;
use crate::style::Style;
use std::path::Path;

pub fn execute(args: &ReviewArgs, style: Style) -> i32 {
    // The load, the store, the prover and the baseline stay behind the effect the program
    // performs; a review binds nothing, so a `law/host` is a gap here as under a hermetic run.
    let binds = Binds {
        lent: crate::claims::lent(job(args)),
        ..Binds::default()
    };
    run(
        "review",
        argv(args, style),
        Path::new("."),
        binds,
        args.json,
        style,
    )
}

fn job(args: &ReviewArgs) -> Job {
    Job {
        path: args.path.clone(),
        incremental: !args.no_incremental,
        use_cache: !args.no_cache,
        std: args.std,
        jobs: None,
        backend: args.backend.clone(),
        plan: ply_machine::simulation::prove_plan(
            &(&args.prove).into(),
            &(&args.simulation).into(),
        ),
        binding: None,
    }
}

/// What `process.args` answers: the command word, then the flags the program reads. `--changed` is
/// the default, so the action is always named rather than implied.
fn argv(args: &ReviewArgs, style: Style) -> Vec<String> {
    let mut argv = vec![
        "review".to_string(),
        color(style),
        format!(
            "--action={}",
            if args.accept { "accept" } else { "changed" }
        ),
    ];
    if args.json {
        argv.push("--json".to_string());
    }
    argv
}
