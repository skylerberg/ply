//! Generating corpora in tests, through the corpus package's own generator.

use ply_corpus::spec::CorpusSpec;
use std::path::{Path, PathBuf};

pub fn ply() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/ply")
}

/// Call a `gen.*` entry of the corpus package and answer its text payload.
fn call(entry: &str, args: Vec<ply_eval::Value>, cwd: &Path) -> String {
    let value = ply_corpus::cmd::run_ply_subcommand(entry, args, cwd, &ply())
        .unwrap_or_else(|e| panic!("`{entry}` would not run: {e:#}"));
    let ply_eval::Value::Ctor { name, args } = &value else {
        panic!("`{entry}` answered {value}, not an `Ok` or an `Err`");
    };
    let ply_eval::Value::Str(text) = &args[0] else {
        panic!("`{entry}`'s answer is not text: {value}");
    };
    match name.as_str() {
        "Ok" => text.to_string(),
        "Err" => panic!("`{entry}` refused: {text}"),
        other => panic!("`{entry}` answered `{other}`"),
    }
}

/// A corpus generated and verified on disk, as `ply-corpus gen` writes it.
pub fn generate(root: &Path, spec: &CorpusSpec) {
    let parent = root.parent().expect("a corpus root has a parent");
    let name = root.file_name().expect("a corpus root has a name");
    call(
        "gen.run",
        vec![
            ply_eval::Value::str(name.to_string_lossy()),
            ply_eval::Value::str(root.to_string_lossy()),
            ply_eval::Value::str(serde_json::to_string(spec).unwrap()),
            ply_eval::Value::Bool(false),
        ],
        parent,
    );
}

/// The files the generator emits for a spec, `(path, text)` each.
pub fn dump_files(spec: &CorpusSpec) -> Vec<(String, String)> {
    let dir = tempfile::tempdir().unwrap();
    let text = call(
        "gen.dump",
        vec![ply_eval::Value::str(serde_json::to_string(spec).unwrap())],
        dir.path(),
    );
    let dump: serde_json::Value = serde_json::from_str(&text).expect("the dump is JSON");
    dump["files"]
        .as_array()
        .expect("files is an array")
        .iter()
        .map(|f| {
            (
                f["path"].as_str().unwrap().to_string(),
                f["text"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// The manifest the generator writes for a spec, as decoded JSON.
pub fn dump_manifest(spec: &CorpusSpec) -> serde_json::Value {
    let dir = tempfile::tempdir().unwrap();
    let text = call(
        "gen.dump",
        vec![ply_eval::Value::str(serde_json::to_string(spec).unwrap())],
        dir.path(),
    );
    let dump: serde_json::Value = serde_json::from_str(&text).expect("the dump is JSON");
    dump["manifest"].clone()
}
