//! `ply std` — the runner for the `std` command of the program in `crates/ply-cli/ply`. The
//! modules themselves are read off the shelf the run binds; the digest over them is this binary's
//! own fact and travels in a flag, as colour does.

use super::shipped_program::{color, run};
use crate::artifact::Binds;
use crate::cli::StdArgs;
use crate::style::Style;
use std::path::Path;

pub fn execute(args: &StdArgs, style: Style) -> i32 {
    run(
        "std",
        argv(args, style),
        Path::new("."),
        Binds::default(),
        args.json,
        style,
    )
}

/// What `process.args` answers: the command word, then the flags the program reads.
fn argv(args: &StdArgs, style: Style) -> Vec<String> {
    let mut argv = vec![
        "std".to_string(),
        color(style),
        format!("--shipped-digest={}", ply_std::digest_short()),
    ];
    if args.digest {
        argv.push("--digest".to_string());
    }
    if args.json {
        argv.push("--json".to_string());
    }
    if let Some(module) = &args.show {
        argv.push(format!("--query={module}"));
    }
    argv
}
