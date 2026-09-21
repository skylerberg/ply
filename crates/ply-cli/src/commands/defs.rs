//! `ply defs` — every definition with its place, hash, signature and footprint.

use super::shipped_program::{color, rooted, run};
use crate::artifact::Binds;
use crate::cli::DefsArgs;
use crate::style::Style;

pub fn execute(args: &DefsArgs, style: Style) -> i32 {
    let (root, inside) = rooted(&args.path);
    let mut argv = vec!["defs".to_string(), color(style)];
    argv.push(format!("--root={}", root.display()));
    if let Some(filter) = &args.filter {
        argv.push(format!("--filter={filter}"));
    }
    if args.json {
        argv.push("--json".to_string());
    }
    argv.push(inside);
    run("defs", argv, &root, Binds::default(), args.json, style)
}
