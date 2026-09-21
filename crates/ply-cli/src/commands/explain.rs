//! `ply explain` — what a diagnostic code means. The table the program carries is held to
//! `ply_span::MEANINGS` by a test, so the registry the compiler raises from stays the Rust one.

use super::shipped_program::{color, run};
use crate::artifact::Binds;
use crate::cli::ExplainArgs;
use crate::style::Style;
use std::path::Path;

pub fn execute(args: &ExplainArgs, style: Style) -> i32 {
    let mut argv = vec!["explain".to_string(), color(style)];
    if args.all {
        argv.push("--all".to_string());
    }
    if args.json {
        argv.push("--json".to_string());
    }
    if let Some(code) = &args.code {
        argv.push(code.clone());
    }
    run(
        "explain",
        argv,
        Path::new("."),
        Binds::default(),
        args.json,
        style,
    )
}
