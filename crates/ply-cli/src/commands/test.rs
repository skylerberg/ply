//! `ply test` — the runner for the `test` command of the program in `crates/ply-cli/ply`.

use super::shipped_program::{color, run};
use crate::artifact::Binds;
use crate::cli::TestArgs;
use crate::style::Style;
use std::path::Path;

pub fn execute(args: &TestArgs, style: Style) -> i32 {
    // One machine for the process: the front end, the compiled unit and the store it opens survive
    // every report `--watch` asks for, which is what makes a warm one cost nothing.
    let session = crate::test::Session::new(args);
    // The load, the store, the selection, the binding and the worker pool stay behind the effect
    // the program performs; the program is lent what each step did and runs nothing of its own.
    let binds = Binds {
        lent: session.lent(),
        ..Binds::default()
    };
    run(
        "test",
        argv(args, style),
        Path::new("."),
        binds,
        args.json,
        style,
    )
}

/// What `process.args` answers: the command word, then the flags the program reads — what a report
/// says, and whether to write another one whenever the tree moves. Every other flag a run takes
/// configures the machine rather than the report, and is read where the machine is built.
fn argv(args: &TestArgs, style: Style) -> Vec<String> {
    let mut argv = vec!["test".to_string(), color(style)];
    if args.json {
        argv.push("--json".to_string());
    }
    if args.explain {
        argv.push("--explain".to_string());
    }
    if args.no_cache {
        argv.push("--no-cache".to_string());
    }
    if args.watch {
        argv.push("--watch".to_string());
    }
    if let Some(filter) = &args.filter {
        argv.push(format!("--filter={filter}"));
    }
    argv
}
