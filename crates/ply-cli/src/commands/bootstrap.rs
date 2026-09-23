//! `ply bootstrap` — the runner for the `bootstrap` command of the program in
//! `crates/ply-cli/ply`.

use super::shipped_program::{color, run};
use crate::artifact::Binds;
use crate::cli::BootstrapArgs;
use crate::style::Style;
use std::path::Path;

pub fn execute(args: &BootstrapArgs, style: Style) -> i32 {
    // The emitter is worked here, behind the effect the program performs; the program is lent
    // what it came to and reaches no tree of its own.
    let binds = Binds {
        lent: crate::bootstrap::lent(&ply_machine::bootstrap::BootstrapOptions {
            path: args.path.clone(),
            out: args.out.clone(),
            verify: args.verify,
            profile: args.profile.clone(),
        }),
        ..Binds::default()
    };
    run(
        "bootstrap",
        argv(args, style),
        Path::new("."),
        binds,
        args.json,
        style,
    )
}

/// What `process.args` answers: the command word, then the flags the program reads.
fn argv(args: &BootstrapArgs, style: Style) -> Vec<String> {
    let mut argv = vec![
        "bootstrap".to_string(),
        color(style),
        format!("--action={}", if args.verify { "verify" } else { "write" }),
    ];
    if args.json {
        argv.push("--json".to_string());
    }
    argv
}
