//! `ply test` — the runner for the `test` command of the program in `crates/ply-cli/ply`.

use super::shipped_program::{color, run};
use crate::artifact::Binds;
use crate::cli::TestArgs;
use crate::style::Style;
use std::path::Path;

pub fn execute(args: &TestArgs, style: Style) -> i32 {
    // One machine for the process: the front end, the compiled unit and the store it opens survive
    // every iteration of `--watch`, which is what makes a warm iteration cost nothing.
    let session = crate::test::Session::new(args);
    if !args.watch {
        return once(&session, args, style);
    }
    watch(&session, args, style)
}

/// Re-run whenever the tree moves. The loop stays here: it never returns, and a program that
/// polls for years inside its own entry would hold a frame per reading.
fn watch(session: &crate::test::Session, args: &TestArgs, style: Style) -> i32 {
    let root = crate::load::project_root(&args.path);
    // Stamped before the run, never after: a walk after it folds a save made while it ran into the
    // baseline, and that save is exactly the one the next iteration exists to notice.
    let mut baseline = crate::warm::tree_stamps(&root);
    once(session, args, style);
    loop {
        // Polling: the walk is owed anyway, so a watcher would add a dependency for no latency.
        std::thread::sleep(std::time::Duration::from_millis(120));
        let now = crate::warm::tree_stamps(&root);
        if now == baseline {
            continue;
        }
        // `now` was read before this run, so a save made during it still differs next time.
        baseline = now;
        if !args.json {
            println!();
        }
        once(session, args, style);
    }
}

fn once(session: &crate::test::Session, args: &TestArgs, style: Style) -> i32 {
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

/// What `process.args` answers: the command word, then the flags the program reads. Every other
/// flag a run takes configures the machine rather than the report, and is read where the machine
/// is built.
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
    if let Some(filter) = &args.filter {
        argv.push(format!("--filter={filter}"));
    }
    argv
}
