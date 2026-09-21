//! `ply fmt` — the runner for the `fmt` command of the program in `crates/ply-cli/ply`.

use super::shipped_program::{color, run};
use crate::artifact::Binds;
use crate::cli::FmtArgs;
use crate::style::Style;
use std::path::Path;

pub fn execute(args: &FmtArgs, style: Style) -> i32 {
    run(
        "fmt",
        argv(args, style),
        Path::new("."),
        Binds::default(),
        args.json,
        style,
    )
}

/// What `process.args` answers: the command word, the flags the program reads, then the paths.
fn argv(args: &FmtArgs, style: Style) -> Vec<String> {
    let mut argv = vec!["fmt".to_string(), color(style)];
    if args.check {
        argv.push("--check".to_string());
    }
    if args.json {
        argv.push("--json".to_string());
    }
    argv.extend(args.paths.iter().map(|p| p.display().to_string()));
    argv
}
