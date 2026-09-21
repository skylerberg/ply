//! `ply check` — the front end over a program, and as much of what it inferred as the flags ask for.

use super::shipped_program::{color, rooted, run};
use crate::cli::CheckArgs;
use crate::style::Style;

pub fn execute(args: &CheckArgs, style: Style) -> i32 {
    let (root, inside) = rooted(&args.path);
    let mut argv = vec!["check".to_string(), color(style)];
    argv.push(format!("--root={}", root.display()));
    if args.types {
        argv.push("--types".to_string());
    }
    if args.costs {
        argv.push("--costs".to_string());
    }
    if args.explain {
        argv.push("--explain".to_string());
    }
    if args.json {
        argv.push("--json".to_string());
    }
    argv.push(inside);
    run("check", argv, &root, args.json, style)
}
