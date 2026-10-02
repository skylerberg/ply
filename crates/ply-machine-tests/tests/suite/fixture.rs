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

/// The front end the CLI would hand a machine: the file, or the project walked, the compiler run
/// once over it, and both marshalled into the record the effects take.
///
/// A test that drives an effect directly has no CLI to do this for it, and every effect that reads a
/// program now takes one. The fixture's module mirrors the package's, so what the machine names has
/// to be what this declares -- `replay`'s own check says so for the payload.
pub fn handed(path: &Path) -> ply_eval::Value {
    use ply_codegen::c::producer::{self, Packages};
    producer::ensure_default();
    let root = ply_machine::load::project_root(path);
    let mut paths = Vec::new();
    if path.is_file() {
        paths.push(path.to_path_buf());
    } else {
        collect(path, &mut paths);
    }
    paths.sort();
    let mut files: Vec<(String, String, String)> = Vec::new();
    for path in paths {
        let relative = path.strip_prefix(&root).unwrap_or(&path).to_path_buf();
        let module = ply_eval::ModuleName::from_relative_path(&relative)
            .expect("a fixture's file is a module");
        let text = std::fs::read_to_string(&path).expect("the fixture is read");
        files.push((path.display().to_string(), module.to_string(), text));
    }
    let own: Vec<(String, String)> = files
        .iter()
        .map(|(_, name, text)| (name.clone(), text.clone()))
        .collect();
    let packages = Packages {
        root: root.display().to_string(),
        manifest: None,
        supplied: Vec::new(),
    };
    let pulled =
        producer::front_pulling_std_with(&own, ply_machine::shelf::sources(), &packages, &[])
            .expect("the front end runs");
    for name in &pulled.modules {
        let module = ply_eval::ModuleName::from_dotted(name);
        if let Some(text) = ply_machine::shelf::source(&module) {
            files.push((
                ply_machine::shelf::pseudo_path(&module)
                    .display()
                    .to_string(),
                name.clone(),
                text.to_string(),
            ));
        }
    }
    let file = |(path, name, text): (String, String, String)| {
        ply_machine::payload::record(vec![
            ("path", ply_eval::Value::str(&path)),
            ("name", ply_eval::Value::str(&name)),
            ("text", ply_eval::Value::bytes(text.as_bytes())),
        ])
    };
    ply_machine::payload::record(vec![
        ("dump", pulled.dump),
        (
            "files",
            ply_eval::Value::list(files.into_iter().map(file).collect()),
        ),
        ("read_ms", ply_eval::Value::Int(0)),
        ("front_ms", ply_eval::Value::Int(0)),
        ("file_ms", ply_eval::Value::Int(0)),
        ("cached", ply_eval::Value::Bool(false)),
    ])
}

/// Every `.ply` file under `root`, a directory whose name starts with `.` passed over as a walk
/// passes it.
fn collect(root: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if !entry.file_name().to_string_lossy().starts_with('.') {
                collect(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "ply") {
            out.push(path);
        }
    }
}

/// The file or project at `path` loaded as the CLI hands one to a machine.
pub fn loaded(path: &Path) -> ply_machine::load::Loaded {
    let front = ply_machine::driver::handed_front_of(&handed(path), ply_eval::Span::DUMMY)
        .unwrap_or_else(|d| panic!("the front end is handed over: {}", d.message));
    ply_machine::driver::load_over_front(path, &front).unwrap_or_else(|e| {
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
