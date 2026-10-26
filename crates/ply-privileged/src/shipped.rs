//! What this binary ships: a function of the binary alone, so reading it keeps a test det.
//! `compiler.unit`'s `shipped` asks it through the runtime's natives.

use std::sync::OnceLock;

/// Empty in a process the launcher did not start.
static STAMPS: OnceLock<String> = OnceLock::new();

/// The program the launcher entered, a `name hash` line each; empty where it entered none.
static ENTERED: OnceLock<String> = OnceLock::new();

/// The line of [`ENTERED`] that names the entry: its hash covers everything the program runs.
static ENTRY: OnceLock<String> = OnceLock::new();

pub fn stamp(stamps: String) {
    let _ = STAMPS.set(stamps);
}

pub fn entered(entry: &str, definitions: String) {
    let named = format!("{entry} ");
    let line = definitions
        .lines()
        .find(|line| line.starts_with(&named))
        .unwrap_or("");
    let _ = ENTRY.set(line.to_string());
    let _ = ENTERED.set(definitions);
}

/// What the running `ply` program is a function of: the runtime's stamps and its entry's hash. A
/// trace whose test asked what program ran holds this.
pub fn program_digest() -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(STAMPS.get().map_or("", String::as_str).as_bytes());
    hasher.update(&[0]);
    hasher.update(ENTRY.get().map_or("", String::as_str).as_bytes());
    hasher.finalize().to_hex().to_string()
}

/// The digest a trace holds of the shipped module or data file `name`, or of the list of modules
/// for the empty name.
pub fn module_digest(name: &str) -> Option<String> {
    if name.is_empty() {
        let mut hasher = blake3::Hasher::new();
        for module in crate::shipped_modules::names() {
            hasher.update(module.as_bytes());
            hasher.update(&[0]);
        }
        return Some(hasher.finalize().to_hex().to_string());
    }
    crate::shipped_modules::digest_of(name)
}

/// Installs what this binary ships for the runtime's natives to answer, once.
pub fn install() {
    ply_compiled::hosted::shipped::install(Box::new(Shipping));
}

struct Shipping;

impl ply_compiled::hosted::shipped::Ships for Shipping {
    fn names(&self) -> Vec<String> {
        crate::shipped_modules::names()
    }

    fn module(&self, name: &str) -> Option<&'static [u8]> {
        crate::shipped_modules::source(name)
            .map(str::as_bytes)
            .or_else(|| crate::shipped_modules::data(name))
    }

    fn answer(&self, path: &str, offset: usize, len: usize) -> Option<Vec<u8>> {
        ply_pack::installed().answer(path, offset, len)
    }

    fn digests(&self) -> String {
        crate::shipped_modules::digests()
    }

    fn stamps(&self) -> String {
        STAMPS.get().cloned().unwrap_or_default()
    }

    fn definitions(&self) -> String {
        ENTERED.get().cloned().unwrap_or_default()
    }
}

/// The runtime a unit is compiled against as a released `ply`'s bootstrap reads it, written as
/// JSON: the C its units open and close with, the helpers they bind, each native with its machine
/// kinds, the context's fields compiled code reads and the structs emitted code lays out (each field
/// in order with its offset and bytes), how deeply drop roots nest before a release is put off, and
/// where a hold keeps what it holds.
pub fn runtime_json() -> String {
    use ply_compiled::c;
    let natives = c::described::natives;
    serde_json::json!({
        "head": c::unit_head(),
        "tail": c::runtime_object(),
        "helpers": c::exports::runtime_helpers()
            .into_iter()
            .map(|h| serde_json::json!({ "name": h.name, "args": h.args, "answers": h.answers }))
            .collect::<Vec<_>>(),
        "natives": natives()
            .map(|(name, s)| serde_json::json!({
                "name": name,
                "words": s.words(),
                "answers": s.answers(),
                "raises": s.raises,
                "params": s.params.iter().map(|m| m.name()).collect::<Vec<_>>(),
                "answer": s.answer.map(|m| m.name()),
            }))
            .collect::<Vec<_>>(),
        // The natives' names alone, which a `ply` that reads no `natives` emits a builder against.
        "builtins": natives().map(|(name, _)| name).collect::<Vec<_>>(),
        "ctx": fields_json(ply_compiled::laid::ctx_fields()),
        "descriptors": ply_compiled::laid::descriptors()
            .into_iter()
            .map(|d| serde_json::json!({
                "name": d.name,
                "bytes": d.bytes,
                "fields": fields_json(d.fields),
            }))
            .collect::<Vec<_>>(),
        "release_depth": ply_compiled::rt::RELEASE_DEPTH,
        "hold": facts_json(ply_compiled::laid::hold()),
        "identity": c::runtime_identity(),
    })
    .to_string()
}

fn fields_json(fields: Vec<ply_compiled::laid::Field>) -> serde_json::Value {
    fields
        .into_iter()
        .map(|f| serde_json::json!({ "name": f.name, "at": f.at, "bytes": f.bytes }))
        .collect()
}

fn facts_json(facts: Vec<(&'static str, usize)>) -> serde_json::Value {
    serde_json::Value::Object(
        facts
            .into_iter()
            .map(|(name, n)| (name.to_string(), serde_json::json!(n)))
            .collect(),
    )
}
