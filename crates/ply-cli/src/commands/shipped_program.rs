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

/// The directory a load is rooted at and the path inside it to walk. The program sees the root as
/// `cwd` and nothing above it, so an absolute argument is answered for without the program ever
/// naming one. A path that names nothing at all is rooted at the nearest directory above it, so
/// the program reports it rather than the root binding refusing to resolve.
pub fn rooted(path: &Path) -> (PathBuf, String) {
    let path = crate::load::tidy(path);
    match std::fs::metadata(&path) {
        Ok(meta) if meta.is_dir() => (path, ".".to_string()),
        Ok(_) => (crate::load::project_root(&path), named(&path)),
        Err(_) => {
            let root = nearest_directory(&path);
            let inside = path
                .strip_prefix(&root)
                .map(|rest| rest.display().to_string())
                .unwrap_or_else(|_| path.display().to_string());
            (root, inside)
        }
    }
}

fn named(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string())
}

fn nearest_directory(path: &Path) -> PathBuf {
    let mut at = path.parent();
    while let Some(candidate) = at {
        if candidate.as_os_str().is_empty() {
            break;
        }
        if candidate.is_dir() {
            return candidate.to_path_buf();
        }
        at = candidate.parent();
    }
    PathBuf::from(".")
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

fn enter(argv: Vec<String>, root: &Path, binds: Binds) -> Result<i32, Diagnostic> {
    let bytes = crate::shipped::program()?;
    let program = ply_launcher::Program {
        artifact: bytes,
        artifact_name: crate::shipped::ARTIFACT.to_string(),
        shelf: ply_machine::shelf::sources().to_vec(),
        stage: format!("cli-{}", crate::shipped::identity()),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };
    ply_launcher::run(&program, root, argv, binds)
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
