//! The one program `ply` ships, entered once per invocation. `clap` keeps the subcommands and
//! their flags — it is the documented surface, and it refuses an unknown flag before the program
//! runs — and each command here turns its parsed arguments back into an argv the program reads.
//! The walk, the front end and both reports are the program's.

use super::common::{diagnostics_json, emit_json, print_diagnostics};
use crate::EXIT_FAILED;
use crate::artifact::Binds;
use crate::style::Style;
use ply_span::{Diagnostic, SourceMap};
use serde_json::json;
use std::path::{Path, PathBuf};

/// Colour is decided here and carried in a flag: a program has no terminal to ask.
pub fn color(style: Style) -> String {
    if style.is_styled() {
        "--color=always".to_string()
    } else {
        "--color=never".to_string()
    }
}

/// The directory a load is rooted at, the path inside it to walk, and the root as a report prints
/// it. The program sees the root as `cwd` and nothing above it, so an absolute argument is answered
/// for without the program ever naming one.
pub fn rooted(path: &Path) -> (PathBuf, String) {
    let root = crate::load::project_root(path);
    let inside = match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string()),
        _ => ".".to_string(),
    };
    (root, inside)
}

/// Runs `argv` with `root` bound as `cwd` and the shelf beside it, plus whatever `binds` lends on
/// top of those, and answers with the code the program asked to exit with. Only `ply` itself can
/// fail here: the program's own refusals are lines it wrote.
pub fn run(
    command: &str,
    argv: Vec<String>,
    root: &Path,
    binds: Binds,
    json: bool,
    style: Style,
) -> i32 {
    match enter(argv, root, binds) {
        Ok(code) => code,
        Err(diagnostic) => refuse(command, &diagnostic, json, style),
    }
}

fn enter(argv: Vec<String>, root: &Path, mut binds: Binds) -> Result<i32, Diagnostic> {
    let bytes = crate::shipped::program()?;
    let shelf = crate::shipped::shelf()?;
    let path = PathBuf::from(crate::shipped::ARTIFACT);
    let (artifact, _) = crate::artifact::decode(&bytes, &path)?;
    let opened = crate::artifact::open(&artifact, &path).map_err(first_of)?;
    let mut roots = vec![
        ply_host::fs::RootSpec {
            name: "cwd".to_string(),
            path: root.to_path_buf(),
        },
        ply_host::fs::RootSpec {
            name: "shelf".to_string(),
            path: shelf,
        },
    ];
    roots.append(&mut binds.roots);
    binds.roots = roots;
    // The `ply` program is the tool's own work rather than a program under test, so the budgets
    // a run gives a program are not its.
    ply_codegen::rt::unbounded(|| crate::artifact::enter(&artifact, &opened, argv, binds))
}

fn first_of(diagnostics: Vec<Diagnostic>) -> Diagnostic {
    diagnostics.into_iter().next().unwrap_or_else(|| {
        Diagnostic::error(
            ply_span::codes::INTERNAL_ERROR,
            "the `ply` program did not open, and nothing said why",
        )
    })
}

fn refuse(command: &str, diagnostic: &Diagnostic, json: bool, style: Style) -> i32 {
    let empty = SourceMap::new();
    if json {
        emit_json(&json!({
            "command": command,
            "ok": false,
            "exit_code": EXIT_FAILED,
            "diagnostics": diagnostics_json(std::slice::from_ref(diagnostic), &empty),
        }));
    } else {
        print_diagnostics(std::slice::from_ref(diagnostic), &empty, style);
    }
    EXIT_FAILED
}
