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

/// Installs what this binary ships for the runtime's natives to serve, once.
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

    fn result(&self, path: &str, offset: usize, len: usize) -> Option<Vec<u8>> {
        ply_pack::installed().result(path, offset, len)
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
