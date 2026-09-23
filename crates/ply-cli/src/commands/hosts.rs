//! `ply hosts` — the runner for the `hosts` command of the program in `crates/ply-cli/ply`.

use super::shipped_program::{color, run};
use crate::artifact::Binds;
use crate::cli::HostsArgs;
use crate::style::Style;
use std::path::Path;

pub fn execute(args: &HostsArgs, style: Style) -> i32 {
    // The flags that name a host facility stay here, behind the effect the program performs; the
    // program is lent the binding they define and reaches no tree of its own.
    let options = ply_machine::hosts::HostsOptions {
        path: args.path.clone(),
        host: args.host,
        json: args.json,
        digest: args.digest,
        tls: (&args.tls).into(),
        fs: args.fs.fs.clone(),
        db: (&args.db).into(),
        config: (&args.config).into(),
        trace: (&args.trace).into(),
        shutdown: (&args.shutdown).into(),
    };
    let binds = Binds {
        lent: crate::hosts::lent(&options),
        ..Binds::default()
    };
    run(
        "hosts",
        argv(args, style),
        Path::new("."),
        binds,
        args.json,
        style,
    )
}

/// What `process.args` answers: the command word, then the flags the program reads.
fn argv(args: &HostsArgs, style: Style) -> Vec<String> {
    let mut argv = vec!["hosts".to_string(), color(style)];
    if args.json {
        argv.push("--json".to_string());
    }
    if args.digest {
        argv.push("--digest".to_string());
    }
    argv
}
