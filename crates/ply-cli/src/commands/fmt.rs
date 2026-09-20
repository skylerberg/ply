//! `ply fmt` — every `.ply` file under the paths rewritten in the canonical layout.

use super::common::emit_json;
use crate::cli::FmtArgs;
use crate::style::Style;
use crate::{EXIT_COMPILE_ERROR, EXIT_FAILED, EXIT_OK};
use serde_json::json;
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: u32 = 1;

pub fn execute(args: &FmtArgs, style: Style) -> i32 {
    let mut files = Vec::new();
    let mut errors: Vec<(PathBuf, String)> = Vec::new();
    for path in &args.paths {
        match collect(path, &mut files) {
            Ok(()) => {}
            Err(e) => errors.push((path.clone(), e.to_string())),
        }
    }
    files.sort();
    files.dedup();
    let mut changed = Vec::new();
    for path in &files {
        match format_file(path, args.check) {
            Ok(true) => changed.push(path.clone()),
            Ok(false) => {}
            Err(e) => errors.push((path.clone(), e)),
        }
    }
    let exit_code = if !errors.is_empty() {
        EXIT_COMPILE_ERROR
    } else if args.check && !changed.is_empty() {
        EXIT_FAILED
    } else {
        EXIT_OK
    };
    if args.json {
        emit_json(&json!({
            "command": "fmt",
            "schema_version": SCHEMA_VERSION,
            "ok": exit_code == EXIT_OK,
            "exit_code": exit_code,
            "files": files.iter().map(|f| json!({
                "path": f.display().to_string(),
                "changed": changed.contains(f),
            })).collect::<Vec<_>>(),
            "errors": errors.iter().map(|(path, error)| json!({
                "path": path.display().to_string(),
                "error": error,
            })).collect::<Vec<_>>(),
        }));
    } else {
        let verb = if args.check {
            "would format"
        } else {
            "formatted"
        };
        for path in &changed {
            println!("{verb} {}", path.display());
        }
        for (path, error) in &errors {
            eprintln!("{}: {}: {error}", style.red("error"), path.display());
        }
    }
    exit_code
}

/// Whether the file's text differs from its formatted form; written back unless `check`.
fn format_file(path: &Path, check: bool) -> Result<bool, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let formatted = ply_codegen::c::producer::fmt_source(&text)
        .map_err(|e| format!("the formatter could not run: {e:#}"))??;
    if formatted == text {
        return Ok(false);
    }
    if !check {
        std::fs::write(path, formatted).map_err(|e| e.to_string())?;
    }
    Ok(true)
}

/// A file as given; a directory's `*.ply` files, hidden directories and `target` skipped.
fn collect(path: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let meta = std::fs::metadata(path)?;
    if meta.is_file() {
        out.push(crate::load::tidy(path));
        return Ok(());
    }
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let child = crate::load::tidy(&entry.path());
        let file_type = entry.file_type()?;
        let name = child.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if file_type.is_dir() {
            if !name.starts_with('.') && name != "target" {
                collect(&child, out)?;
            }
        } else if file_type.is_file() && child.extension().is_some_and(|e| e == "ply") {
            out.push(child);
        }
    }
    Ok(())
}
