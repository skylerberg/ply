//! `ply hash` — every definition and test under the hash content addressing keys it by.

use super::shipped_program::{color, rooted, run};
use crate::cli::HashArgs;
use crate::style::Style;

pub fn execute(args: &HashArgs, style: Style) -> i32 {
    let (root, inside) = rooted(&args.path);
    let mut argv = vec!["hash".to_string(), color(style)];
    argv.push(format!("--root={}", root.display()));
    if args.deps {
        argv.push("--deps".to_string());
    }
    if args.json {
        argv.push("--json".to_string());
    }
    argv.push(inside);
    run("hash", argv, &root, args.json, style)
}
