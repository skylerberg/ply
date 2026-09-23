//! `ply prove` — the runner for the `prove` command of the program in `crates/ply-cli/ply`.

use super::shipped_program::{color, run};
use crate::artifact::Binds;
use crate::claims::{Binding, Job};
use crate::cli::ProveArgs;
use crate::style::Style;
use std::path::Path;

pub fn execute(args: &ProveArgs, style: Style) -> i32 {
    // The load, the store, the binding and the prover stay behind the effect the program performs;
    // the program is lent the claims and what became of each, and discharges nothing of its own.
    let binds = Binds {
        lent: crate::claims::lent(job(args)),
        ..Binds::default()
    };
    run(
        "prove",
        argv(args, style),
        Path::new("."),
        binds,
        args.json,
        style,
    )
}

/// Every flag that configures the machine rather than the report, read where the machine is built.
fn job(args: &ProveArgs) -> Job {
    Job {
        path: args.path.clone(),
        incremental: !args.no_incremental,
        use_cache: !args.no_cache,
        std: args.std,
        jobs: args.jobs,
        backend: args.backend.clone(),
        plan: ply_machine::simulation::prove_plan(
            &(&args.prove).into(),
            &(&args.simulation).into(),
        ),
        binding: Some(Binding {
            host: args.host,
            tls: args.tls.clone(),
            fs: args.fs.clone(),
            db: args.db.clone(),
            config: args.config.clone(),
            trace: args.trace.clone(),
        }),
    }
}

/// What `process.args` answers: the command word, then the flags the program reads.
fn argv(args: &ProveArgs, style: Style) -> Vec<String> {
    let mut argv = vec!["prove".to_string(), color(style)];
    if args.json {
        argv.push("--json".to_string());
    }
    if args.explain {
        argv.push("--explain".to_string());
    }
    if let Some(filter) = &args.filter {
        argv.push(format!("--filter={filter}"));
    }
    argv
}
