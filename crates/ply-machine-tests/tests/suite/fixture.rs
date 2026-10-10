//! The projects a test loads, the front end the CLI would hand over for each, and the world a test
//! of the claims effect spells out in place of the one `proof.world` builds.

use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// A scratch directory.
pub fn scratch() -> TempDir {
    TempDir::new().expect("a scratch directory")
}

/// A temporary project whose `m.ply` is `source`.
pub fn project(source: &str) -> TempDir {
    let dir = scratch();
    write(dir.path(), "m.ply", source);
    dir
}

/// Writes `text` to `dir/name`, making the directories above it.
pub fn write(dir: &Path, name: &str, text: &str) {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("the file's directory is made");
    }
    std::fs::write(path, text).expect("the fixture is written");
}

/// The file, or the project walked, at `path`, as the files the builder is handed.
fn files_at(path: &Path) -> Vec<(String, String)> {
    let root = ply_machine::load::project_root(path);
    let mut paths = Vec::new();
    if path.is_file() {
        paths.push(path.to_path_buf());
    } else {
        collect(path, &mut paths);
    }
    paths.sort();
    paths
        .iter()
        .map(|p| {
            (
                p.strip_prefix(&root).unwrap_or(p).display().to_string(),
                std::fs::read_to_string(p).expect("the fixture is read"),
            )
        })
        .collect()
}

/// What the builder makes of the file or the project at `path`: its front end's answer, a
/// refusal's included, and its unit, every definition offered.
fn program_at(path: &Path) -> ply_machine::runnable::Runnable {
    ply_machine::builds::answered_program(&files_at(path))
        .unwrap_or_else(|d| panic!("the builder answers: {d}"))
}

/// The front end the CLI would hand a machine for the file, or the project, at `path`, as the record
/// the effects take.
pub fn handed(path: &Path) -> ply_eval::Value {
    let bytes = ply_machine::builds::answered(&files_at(path))
        .unwrap_or_else(|d| panic!("the builder answers: {d}"));
    ply_machine::runnable::front_value(&bytes)
        .unwrap_or_else(|why| panic!("the front end's answer reads: {why}"))
}

/// Every file under `root`, a directory whose name starts with `.` passed over as a walk passes it.
fn collect(root: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// The file or project at `path` loaded as the CLI hands one to a machine.
pub fn loaded(path: &Path) -> ply_machine::load::Loaded {
    let front = ply_machine::driver::loaded_analysis_of(&handed(path), ply_eval::Span::DUMMY)
        .unwrap_or_else(|d| panic!("the front end is handed over: {}", d.message));
    ply_machine::driver::load_over_analysis(path, &front).unwrap_or_else(|e| {
        panic!(
            "`{}` did not compile: {:?}",
            path.display(),
            e.diagnostics
                .iter()
                .map(|d| format!("{} {}", d.code, d.message))
                .collect::<Vec<_>>()
        )
    })
}

/// The errors the front end refused the program at `path` with.
pub fn refusal(path: &Path) -> Vec<ply_eval::Diagnostic> {
    let front = program_at(path).front.answer;
    assert!(front.has_error(), "the program was not refused");
    front
        .diagnostics
        .into_iter()
        .filter(|d| d.severity == ply_eval::Severity::Error)
        .collect()
}

/// The C of the unit of the program at `path`, every definition offered: what the CLI's emitter
/// hands a machine.
pub fn unit_text(path: &Path) -> Vec<u8> {
    program_at(path).unit.into_bytes()
}

/// The C of the unit of the program at `path`, as the value a program hands `machine.load`.
pub fn unit(path: &Path) -> ply_eval::Value {
    ply_eval::Value::bytes(unit_text(path))
}

/// The C backend over `loaded`, from the C handed over as the CLI hands it.
pub fn backend(loaded: &ply_machine::load::Loaded) -> std::sync::Arc<dyn ply_eval::Provider> {
    ply_machine::support::unit_of(&loaded.front, &unit_text(&loaded.root))
        .unwrap_or_else(|d| panic!("the unit compiles: {}", d.message))
}

/// A world of no declared types holding one law per `(owner, binders)`, each over `Int` binders and
/// sampled once the static prover has not settled it, keyed `1`, `2`, ... in order: what
/// `proof.world` builds for such a law, spelled out for a test that drives the claims effect with no
/// program to build one.
pub fn int_laws(laws: &[(&str, &[&str])]) -> ply_eval::Value {
    use ply_eval::Value;
    use ply_machine::payload::{ctor, option, record};
    let int = || {
        ctor(
            "proof.domain",
            "Con",
            vec![Value::str("Int"), Value::list(Vec::new())],
        )
    };
    let world = |name: &str, args: Vec<Value>| ctor("proof.world", name, args);
    let law = |(index, (owner, binders)): (usize, &(&str, &[&str]))| {
        record(vec![
            (
                "key",
                Value::str(ply_eval::DefHash([index as u8 + 1; 32]).to_hex()),
            ),
            ("owner", Value::str(*owner)),
            ("kind", ctor("proof.obligation", "Law", vec![option(None)])),
            (
                "at",
                record(vec![
                    ("module", Value::Int(0)),
                    ("start", Value::Int(0)),
                    ("end", Value::Int(0)),
                ]),
            ),
            (
                "binders",
                Value::list(
                    binders
                        .iter()
                        .map(|name| {
                            record(vec![
                                ("name", Value::str(*name)),
                                ("ty", int()),
                                ("text", Value::str("Int")),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("result", option(None)),
            ("variables", Value::list(Vec::new())),
            ("guards", Value::list(Vec::new())),
            ("literals", Value::list(Vec::new())),
            ("host", Value::Bool(false)),
            ("footprint", option(None)),
            ("frame", ctor("proof.obligation", "Pure", Vec::new())),
            (
                "strategy",
                world(
                    "Static",
                    vec![world("Run", vec![world("Drawn", Vec::new())])],
                ),
            ),
        ])
    };
    record(vec![
        ("decls", Value::list(Vec::new())),
        ("signatures", Value::list(Vec::new())),
        (
            "obligations",
            Value::list(laws.iter().enumerate().map(law).collect()),
        ),
    ])
}

/// What the builder makes of `source`, the module `module` names.
#[track_caller]
pub fn answer_for(module: &str, source: &str) -> ply_machine::runnable::Runnable {
    let files = ply_machine::builds::module_files(&[(module, source)]);
    ply_machine::builds::answered_program(&files)
        .unwrap_or_else(|d| panic!("the builder answers: {d}"))
}

/// The front end's answer for `source`, which has to check, and the unit compiled from it.
#[track_caller]
pub fn built(
    module: &str,
    source: &str,
) -> (ply_eval::Analysis, std::sync::Arc<ply_codegen::Unit>) {
    let files = ply_machine::builds::module_files(&[(module, source)]);
    let program = ply_machine::builds::checked_program(&files)
        .unwrap_or_else(|d| panic!("the fixture checks: {d}"));
    let front = program.front.answer;
    let unit =
        ply_codegen::Unit::handed(&front, program.unit).expect("this host has a C toolchain");
    (front, unit)
}
