//! What this binary ships: a function of the binary alone, so reading it keeps a test det.

use ply_eval::host::{HostAnswer, HostHandler, HostOp, HostRequest, HostRuntime, Linearity};
use ply_eval::{Diagnostic, Value as PlyValue, codes};
use std::sync::{Arc, OnceLock};

const EFFECT: &str = "shipped";

const OPERATIONS: [(&str, &str); 8] = [
    ("names", "ply_machine::shipped::names"),
    ("module", "ply_machine::shipped::module"),
    ("version", "ply_machine::shipped::version"),
    ("stamps", "ply_machine::shipped::stamps"),
    ("runtime", "ply_machine::shipped::runtime"),
    ("runnable", "ply_machine::shipped::runnable"),
    ("builtins", "ply_machine::shipped::builtins"),
    ("definitions", "ply_machine::shipped::definitions"),
];

/// Empty in a process the launcher did not start.
static STAMPS: OnceLock<String> = OnceLock::new();

/// The program the launcher entered, as [`definitions`] spells one; empty where it entered none.
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

/// The digest a trace holds of the shipped module `name`, or of the list of them for the empty
/// name.
pub fn module_digest(name: &str) -> Option<String> {
    if name.is_empty() {
        let mut hasher = blake3::Hasher::new();
        for module in crate::shipped_modules::names() {
            hasher.update(module.as_bytes());
            hasher.update(&[0]);
        }
        return Some(hasher.finalize().to_hex().to_string());
    }
    crate::shipped_modules::source(&ply_eval::ModuleName::from_dotted(name))
        .map(|text| blake3::hash(text.as_bytes()).to_hex().to_string())
}

/// Every `fn` of a program, or each its `entry` reaches, and the hash its front end gave it, a
/// `name hash` line each in name order. A hash covers all the definition reaches, so it is what an
/// answer that definition computes is a function of on the program's side, and an entry's covers
/// everything it reaches.
pub fn definitions(program: &ply_eval::Analysis, entry: Option<&str>) -> String {
    let reached = entry.map(|entry| program.hashes.reach([&ply_eval::Symbol::new(entry)]));
    let mut rows: Vec<(&str, String)> = program
        .hashes
        .defs
        .iter()
        .filter(|(name, _)| reached.as_ref().is_none_or(|r| r.contains(*name)))
        .map(|(name, hash)| (name.as_str(), hash.to_hex()))
        .collect();
    rows.sort();
    rows.into_iter()
        .map(|(name, hash)| format!("{name} {hash}\n"))
        .collect()
}

/// The family as the launcher's program is lent it: `definitions` answers for that program.
pub fn lent() -> Vec<(HostOp, Arc<dyn HostHandler>)> {
    lent_by(Arc::new(Shipped { definitions: None }))
}

/// The family as a program entered beside the launcher's is lent it: `definitions` answers for
/// `program`, the one entered, every `fn` of it, since a builder files what it keeps under names
/// it need not reach.
pub fn lent_over(program: &ply_eval::Analysis) -> Vec<(HostOp, Arc<dyn HostHandler>)> {
    lent_by(Arc::new(Shipped {
        definitions: Some(definitions(program, None)),
    }))
}

fn lent_by(shipped: Arc<dyn HostHandler>) -> Vec<(HostOp, Arc<dyn HostHandler>)> {
    OPERATIONS
        .into_iter()
        .map(|(op, path)| {
            let op = crate::hosts::hermetic_op(EFFECT, op, Linearity::Repeatable, path);
            (op, Arc::clone(&shipped))
        })
        .collect()
}

struct Shipped {
    definitions: Option<String>,
}

impl HostHandler for Shipped {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        // The rest answer for the runtime, which a pass's store is stamped with.
        match (req.op.op.as_str(), req.args) {
            ("names", []) => ply_host::observe::shipped(req.machine, ""),
            ("module", [name]) => {
                if let Ok(name) = name.as_str(span, "a module's name") {
                    ply_host::observe::shipped(req.machine, name);
                }
            }
            ("definitions", []) if self.definitions.is_none() => {
                ply_host::observe::program(req.machine)
            }
            _ => {}
        }
        let value = match (req.op.op.as_str(), req.args) {
            ("names", []) => PlyValue::list(
                crate::shipped_modules::names()
                    .iter()
                    .map(PlyValue::str)
                    .collect(),
            ),
            ("module", [name]) => {
                let name = name.as_str(span, "a module's name")?;
                crate::payload::option(
                    crate::shipped_modules::source(&ply_eval::ModuleName::from_dotted(name))
                        .map(|text| PlyValue::bytes(text.as_bytes())),
                )
            }
            ("version", []) => PlyValue::str(env!("CARGO_PKG_VERSION")),
            ("stamps", []) => PlyValue::str(STAMPS.get().map_or("", String::as_str)),
            ("definitions", []) => PlyValue::bytes(
                self.definitions
                    .as_deref()
                    .or(ENTERED.get().map(String::as_str))
                    .unwrap_or("")
                    .as_bytes(),
            ),
            ("runtime", []) => runtime(),
            ("builtins", []) => PlyValue::list(
                ply_eval::Builtin::all()
                    .iter()
                    .map(|b| {
                        crate::payload::record(vec![
                            ("name", PlyValue::bytes(b.name())),
                            ("arity", PlyValue::Int(b.arity() as i64)),
                            ("raises", PlyValue::Bool(b.raises())),
                        ])
                    })
                    .collect(),
            ),
            ("runnable", [entry, files, dump, unit]) => {
                let entry = entry.as_str(span, "an entry point's name")?;
                let unit = unit.as_bytes(span, "the program's unit")?;
                let bytes = crate::runnable::encode(entry, files, dump, unit).map_err(|why| {
                    Diagnostic::error(
                        codes::INTERNAL_ERROR,
                        format!("the program does not encode as a runnable: {why}"),
                    )
                    .primary(span, "this is Ply's fault")
                })?;
                PlyValue::bytes(bytes)
            }
            (other, _) => return Err(crate::hosts::unserved(EFFECT, other, req.span)),
        };
        Ok(HostAnswer::Value(value))
    }
}

/// The runtime a unit is compiled against, as `compiler.unit.Runtime` reads it: the C a unit opens
/// and closes with, the helpers it binds by position, the builtins a body may call, and what all of
/// that is a function of.
fn runtime() -> PlyValue {
    use crate::payload::record;
    use ply_codegen::c;
    record(vec![
        ("head", PlyValue::bytes(c::unit_head())),
        ("tail", PlyValue::bytes(c::runtime_object())),
        (
            "helpers",
            PlyValue::list(
                c::exports::runtime_helpers()
                    .into_iter()
                    .map(|h| {
                        record(vec![
                            ("name", PlyValue::bytes(h.name)),
                            ("args", PlyValue::Int(h.args as i64)),
                            ("answers", PlyValue::Bool(h.answers)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "builtins",
            PlyValue::list(
                ply_eval::Builtin::all()
                    .iter()
                    .map(|b| PlyValue::bytes(b.name()))
                    .collect(),
            ),
        ),
        ("identity", PlyValue::bytes(c::runtime_identity())),
    ])
}
