//! The projects a test loads, and the repository paths it reads them from.

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

/// The front end the CLI would hand a machine: the project walked, the compiler run once over it,
/// and both marshalled into the record the effects take.
///
/// A test that drives an effect directly has no CLI to do this for it, and every effect that reads a
/// program now takes one. The fixture's module mirrors the package's, so what the machine names has
/// to be what this declares -- `replay`'s own check says so for the payload.
pub fn handed(root: &Path) -> ply_eval::Value {
    use ply_codegen::c::producer::{self, Packages};
    producer::ensure_default();
    let mut paths = Vec::new();
    collect(root, &mut paths);
    paths.sort();
    let mut files: Vec<(String, String, String)> = Vec::new();
    for path in paths {
        let relative = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
        let module = ply_ty::ModuleName::from_relative_path(&relative)
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
        producer::front_pulling_std_with(&own, ply_machine::shelf::sources(), &[], &[], &packages)
            .expect("the front end runs");
    for name in &pulled.modules {
        let module = ply_ty::ModuleName::from_dotted(name);
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
        ("dump", ply_eval::Value::bytes(pulled.dump.as_bytes())),
        (
            "files",
            ply_eval::Value::list(files.into_iter().map(file).collect()),
        ),
        ("packages", ply_eval::Value::list(Vec::new())),
        ("read_ms", ply_eval::Value::Int(0)),
        ("front_ms", ply_eval::Value::Int(0)),
    ])
}

/// Every `.ply` file under `root`.
fn collect(root: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "ply") {
            out.push(path);
        }
    }
}

/// The repository root, canonical so it is comparable with a path the loader resolved.
pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the crate lives two levels below the repository root")
}

/// What a program measures for one obligation: each binder's cardinality, and the name the
/// runtime's own texts join to. In a real run the decision is Ply's — `prove.domain`'s
/// `size`/`finite`/`name_of` over the types `prover.typed` hands over — and these audits make the
/// same decision, so what they assert is about a *measured* domain rather than a sampled one. Past
/// the bound, or a product of no points, there is no domain to walk and the obligation is sampled.
pub fn measured(
    prover: &ply_machine::engine::Prover<'_>,
    obligation: &ply_prove::Obligation,
) -> Option<ply_test::obligation::Domain> {
    let sizes: Vec<u64> = obligation
        .generated()
        .iter()
        .map(|binder| ply_prove::domain::cardinality(&binder.ty, prover.world()))
        .collect::<Option<Vec<_>>>()?;
    let points = sizes.iter().try_fold(1u64, |acc, n| acc.checked_mul(*n))?;
    if points == 0 || points > ply_prove::ENUMERATION_BOUND {
        return None;
    }
    let name = if obligation.generated().is_empty() {
        "unit".to_string()
    } else {
        obligation
            .generated()
            .iter()
            .map(|binder| binder.ty.to_string())
            .collect::<Vec<_>>()
            .join(" × ")
    };
    Some(ply_test::obligation::Domain { sizes, name })
}
