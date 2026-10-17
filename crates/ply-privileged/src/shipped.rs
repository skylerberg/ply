//! What this binary ships: a function of the binary alone, so reading it keeps a test det.

use ply_core::host::{HostAnswer, HostHandler, HostOp, HostRequest, HostRuntime, Linearity};
use ply_core::{Diagnostic, Value as PlyValue, codes};
use std::sync::{Arc, OnceLock};

const EFFECT: &str = "shipped";

const OPERATIONS: [(&str, &str); 12] = [
    ("names", "ply_privileged::shipped::names"),
    ("module", "ply_privileged::shipped::module"),
    ("answer", "ply_privileged::shipped::answer"),
    ("digests", "ply_privileged::shipped::digests"),
    ("reached", "ply_privileged::shipped::reached"),
    ("version", "ply_privileged::shipped::version"),
    ("stamps", "ply_privileged::shipped::stamps"),
    ("runtime", "ply_privileged::shipped::runtime"),
    ("runnable", "ply_privileged::shipped::runnable"),
    ("front", "ply_privileged::shipped::front"),
    ("values", "ply_privileged::shipped::values"),
    ("definitions", "ply_privileged::shipped::definitions"),
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

/// The family as the launcher's program is lent it: `definitions` answers for that program.
pub fn lent() -> Vec<(HostOp, Arc<dyn HostHandler>)> {
    let shipped: Arc<dyn HostHandler> = Arc::new(Shipped);
    OPERATIONS
        .into_iter()
        .map(|(op, path)| {
            let op = crate::hosts::hermetic_op(EFFECT, op, Linearity::Repeatable, path);
            (op, Arc::clone(&shipped))
        })
        .collect()
}

struct Shipped;

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
            ("definitions", []) => ply_host::observe::program(req.machine),
            ("reached", [view, pulled]) => {
                let view = view.as_bytes(span, "a load's view of the shipped modules")?;
                let pulled = pulled.as_bool(span, "whether the load pulled its modules")?;
                ply_host::observe::reached(req.machine, &String::from_utf8_lossy(view), pulled);
                return Ok(HostAnswer::Value(PlyValue::Unit));
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
                let module =
                    crate::shipped_modules::source(&ply_core::ModuleName::from_dotted(name))
                        .map(str::as_bytes);
                crate::payload::option(
                    module
                        .or_else(|| crate::shipped_modules::data(name))
                        .map(PlyValue::bytes),
                )
            }
            ("answer", [path, offset, len]) => {
                let path = path.as_str(span, "a path into what the shipped modules answer")?;
                let offset = offset.as_int(span, "where a frame of the answers starts")?;
                let len = len.as_int(span, "how long a frame of the answers is")?;
                let frame = match (usize::try_from(offset), usize::try_from(len)) {
                    (Ok(offset), Ok(len)) => ply_pack::installed().answer(path, offset, len),
                    _ => None,
                };
                crate::payload::option(frame.map(PlyValue::bytes))
            }
            ("digests", []) => PlyValue::bytes(crate::shipped_modules::digests().as_bytes()),
            ("version", []) => PlyValue::str(env!("CARGO_PKG_VERSION")),
            ("stamps", []) => PlyValue::str(STAMPS.get().map_or("", String::as_str)),
            ("definitions", []) => {
                PlyValue::bytes(ENTERED.get().map_or("", String::as_str).as_bytes())
            }
            ("runtime", []) => runtime(),
            ("runnable", [entry, files, dump]) => {
                let entry = entry.as_str(span, "an entry point's name")?;
                let bytes = crate::runnable::encode(entry, files, dump).map_err(|why| {
                    Diagnostic::error(
                        codes::INTERNAL_ERROR,
                        format!("the program does not encode as a runnable: {why}"),
                    )
                    .primary(span, "this is Ply's fault")
                })?;
                PlyValue::bytes(bytes)
            }
            ("front", [linked]) => {
                PlyValue::bytes(ply_core::codec::encode(linked).map_err(|why| {
                    Diagnostic::error(
                        codes::INTERNAL_ERROR,
                        format!("the program's front end does not encode: {why}"),
                    )
                    .primary(span, "this is Ply's fault")
                })?)
            }
            ("values", [files, dump, unit, roots, steps]) => {
                values(files, dump, unit, roots, steps, span)?
            }
            (other, _) => return Err(crate::hosts::unserved(EFFECT, other, req.span)),
        };
        Ok(HostAnswer::Value(value))
    }
}

/// What each stored root answers on the unit emitted over `files` and `dump` and compiled into the
/// library `unit`, as [`crate::enter::stored_values`] enters them.
fn values(
    files: &PlyValue,
    dump: &PlyValue,
    unit: &PlyValue,
    roots: &PlyValue,
    steps: &PlyValue,
    span: ply_core::Span,
) -> Result<PlyValue, Diagnostic> {
    let front = crate::driver::loaded_analysis_of(
        &crate::payload::record(vec![
            ("dump", dump.clone()),
            ("files", files.clone()),
            ("read_ms", PlyValue::Int(0)),
            ("front_ms", PlyValue::Int(0)),
            ("file_ms", PlyValue::Int(0)),
            ("cached", PlyValue::Bool(false)),
        ]),
        span,
    )?;
    let unit = std::path::PathBuf::from(unit.as_str(span, "the unit's library")?);
    let roots = roots
        .as_list(span, "the stored roots")?
        .iter()
        .map(|root| root.as_str(span, "a stored root").map(str::to_string))
        .collect::<Result<Vec<_>, _>>()?;
    let steps = steps.as_int(span, "the calls each may make")?;
    let answered = crate::enter::stored_values(front, unit, roots, steps)?;
    Ok(PlyValue::list(
        answered
            .into_iter()
            .map(|value| crate::payload::option(value.map(PlyValue::bytes)))
            .collect(),
    ))
}

/// Each builtin by its name in the prelude, with what the function compiled code calls it through
/// takes and answers, as that function's type says.
fn natives() -> impl Iterator<Item = (&'static str, ply_compiled::native::Signature)> {
    (ply_core::Builtin::all().iter()).map(|b| (b.name(), ply_compiled::rt::signature(*b)))
}

/// The runtime a unit is compiled against, as [`runtime`] answers it, written as JSON: what a build
/// of a program for this runtime by another `ply` reads it from.
pub fn runtime_json() -> String {
    use ply_compiled::c;
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
                "words": s.words,
                "answers": s.answers,
                "raises": s.raises,
            }))
            .collect::<Vec<_>>(),
        // The natives' names alone, which a `ply` that reads no `natives` emits a builder against.
        "builtins": natives().map(|(name, _)| name).collect::<Vec<_>>(),
        "layouts": layouts_json(),
        "identity": c::runtime_identity(),
    })
    .to_string()
}

fn layouts_json() -> serde_json::Value {
    use ply_compiled::laid::Fact;
    serde_json::Value::Object(
        ply_compiled::laid::layouts()
            .into_iter()
            .map(|(object, facts)| {
                let facts = facts.into_iter().map(|(name, fact)| {
                    let value = match fact {
                        Fact::Number(n) => serde_json::json!(n),
                        Fact::Field(field) => serde_json::json!(field),
                    };
                    (name.to_string(), value)
                });
                (
                    object.to_string(),
                    serde_json::Value::Object(facts.collect()),
                )
            })
            .collect(),
    )
}

fn layouts_value() -> PlyValue {
    use crate::payload::record;
    use ply_compiled::laid::Fact;
    record(
        ply_compiled::laid::layouts()
            .into_iter()
            .map(|(object, facts)| {
                let facts = facts.into_iter().map(|(name, fact)| {
                    let value = match fact {
                        Fact::Number(n) => PlyValue::Int(n as i64),
                        Fact::Field(field) => PlyValue::bytes(field),
                    };
                    (name, value)
                });
                (object, record(facts.collect()))
            })
            .collect(),
    )
}

/// The runtime a unit is compiled against, as `compiler.unit.Runtime` reads it: the C a unit opens
/// and closes with, the helpers it binds by position, the natives an `extern fn` names, how it lays
/// out what the emitter writes in place, and what all of that is a function of.
fn runtime() -> PlyValue {
    use crate::payload::record;
    use ply_compiled::c;
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
            "natives",
            PlyValue::list(
                natives()
                    .map(|(name, s)| {
                        record(vec![
                            ("name", PlyValue::bytes(name)),
                            ("words", PlyValue::Int(s.words as i64)),
                            ("answers", PlyValue::Bool(s.answers)),
                            ("raises", PlyValue::Bool(s.raises)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("layouts", layouts_value()),
        ("identity", PlyValue::bytes(c::runtime_identity())),
    ])
}
