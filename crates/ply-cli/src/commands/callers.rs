//! `ply callers` — the runner for the `callers` command of the program in `crates/ply-cli/ply`.

use super::shipped_program::{color, rooted, run};
use crate::artifact::Binds;
use crate::cli::CallersArgs;
use crate::style::Style;

pub fn execute(args: &CallersArgs, style: Style) -> i32 {
    let (root, inside) = rooted(&args.path);
    let mut argv = vec!["callers".to_string(), color(style)];
    argv.push(format!("--root={}", root.display()));
    argv.push(format!("--query={}", args.query));
    if args.json {
        argv.push("--json".to_string());
    }
    argv.push(inside);
    run("callers", argv, &root, Binds::default(), args.json, style)
}
