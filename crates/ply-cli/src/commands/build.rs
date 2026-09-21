//! `ply build` — the runner for the `build` command of the program in `crates/ply-cli/ply`.

use super::shipped_program::{color, run};
use crate::artifact::Binds;
use crate::cli::BuildArgs;
use crate::style::Style;
use std::path::Path;

pub fn execute(args: &BuildArgs, style: Style) -> i32 {
    // The load, the emitter and BLAKE3 stay here, behind the effect the program performs; the
    // program is lent what each step did and reaches no tree of its own.
    let binds = Binds {
        lent: crate::build::lent(args),
        ..Binds::default()
    };
    run(
        "build",
        argv(args, style),
        Path::new("."),
        binds,
        args.json,
        style,
    )
}

/// What `process.args` answers: the command word, then the flags the program reads.
fn argv(args: &BuildArgs, style: Style) -> Vec<String> {
    let mut argv = vec!["build".to_string(), color(style)];
    if args.json {
        argv.push("--json".to_string());
    }
    if args.digest {
        argv.push("--digest".to_string());
    }
    if let Some(entry) = &args.entry {
        argv.push(format!("--entry={entry}"));
    }
    if let Some(named) = &args.config_schema {
        argv.push(format!("--config-schema={named}"));
    }
    if let Some(named) = &args.db_schema {
        argv.push(format!("--db-schema={named}"));
    }
    if let Some(out) = &args.output {
        argv.push(format!("--out={}", out.display()));
    }
    if let Some(old) = &args.diff {
        argv.push(format!("--diff={}", old.display()));
    }
    argv
}
