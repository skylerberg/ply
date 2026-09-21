//! `ply doc` — one definition or builtin as an agent reads it.

use super::shipped_program::{color, rooted, run};
use crate::cli::DocArgs;
use crate::style::Style;

pub fn execute(args: &DocArgs, style: Style) -> i32 {
    let (root, inside) = rooted(&args.path);
    let mut argv = vec!["doc".to_string(), color(style)];
    argv.push(format!("--root={}", root.display()));
    argv.push(format!("--query={}", args.query));
    if args.json {
        argv.push("--json".to_string());
    }
    argv.push(inside);
    run("doc", argv, &root, args.json, style)
}
