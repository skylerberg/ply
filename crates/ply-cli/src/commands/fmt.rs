//! `ply fmt` — the runner for the program in `crates/ply-cli/ply/fmt.ply`. The walk, the rewrite
//! and both reports are that program's; this builds the command line, binds the working directory
//! as the root `cwd`, and answers with the code the program asked to exit with.

use super::common::{emit_json, print_diagnostics};
use crate::EXIT_FAILED;
use crate::cli::FmtArgs;
use crate::style::Style;
use ply_span::{Diagnostic, SourceMap};
use serde_json::json;
use std::path::PathBuf;

pub fn execute(args: &FmtArgs, style: Style) -> i32 {
    match run(argv(args)) {
        Ok(code) => code,
        Err(diagnostic) => refuse(args, &diagnostic, style),
    }
}

/// What `process.args` answers: the flags the program reads, then the paths as written.
fn argv(args: &FmtArgs) -> Vec<String> {
    let mut argv = Vec::with_capacity(args.paths.len() + 2);
    if args.check {
        argv.push("--check".to_string());
    }
    if args.json {
        argv.push("--json".to_string());
    }
    argv.extend(args.paths.iter().map(|p| p.display().to_string()));
    argv
}

fn run(argv: Vec<String>) -> Result<i32, Diagnostic> {
    let bytes = crate::shipped::program()?;
    let path = PathBuf::from(crate::shipped::ARTIFACT);
    let (artifact, _) = crate::artifact::decode(&bytes, &path)?;
    let opened = crate::artifact::open(&artifact, &path).map_err(first_of)?;
    crate::artifact::enter(
        &artifact,
        &opened,
        argv,
        &[ply_host::fs::RootSpec {
            name: "cwd".to_string(),
            path: PathBuf::from("."),
        }],
    )
}

fn first_of(diagnostics: Vec<Diagnostic>) -> Diagnostic {
    diagnostics.into_iter().next().unwrap_or_else(|| {
        Diagnostic::error(
            ply_span::codes::INTERNAL_ERROR,
            "the `ply fmt` program did not open, and nothing said why",
        )
    })
}

/// Only `ply` itself can fail here: the program's own refusals are lines it wrote and exited on.
fn refuse(args: &FmtArgs, diagnostic: &Diagnostic, style: Style) -> i32 {
    let empty = SourceMap::new();
    if args.json {
        emit_json(&json!({
            "command": "fmt",
            "ok": false,
            "exit_code": EXIT_FAILED,
            "files": [],
            "errors": [],
            "diagnostics": super::common::diagnostics_json(std::slice::from_ref(diagnostic), &empty),
        }));
    } else {
        print_diagnostics(std::slice::from_ref(diagnostic), &empty, style);
    }
    EXIT_FAILED
}
