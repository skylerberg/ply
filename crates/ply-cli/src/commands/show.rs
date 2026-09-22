//! `ply show` — the runner for the `show` command of the program in `crates/ply-cli/ply`.

use super::shipped_program::{color, rooted, run};
use crate::artifact::Binds;
use crate::cli::ShowArgs;
use crate::style::Style;

pub fn execute(args: &ShowArgs, style: Style) -> i32 {
    let (root, inside) = rooted(&args.path);
    let mut argv = vec!["show".to_string(), color(style)];
    argv.push(format!("--root={}", root.display()));
    argv.push(format!("--query={}", args.query));
    if args.json {
        argv.push("--json".to_string());
    }
    argv.push(inside);
    run("show", argv, &root, Binds::default(), args.json, style)
}
