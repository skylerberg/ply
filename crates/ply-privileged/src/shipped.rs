//! What this binary ships: a function of the binary alone, so reading it keeps a test det.

use ply_core::host::{HostAnswer, HostHandler, HostOp, HostRequest, HostRuntime, Linearity};
use ply_core::{Diagnostic, Value as PlyValue, codes};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

const EFFECT: &str = "shipped";

const OPERATIONS: [(&str, &str); 11] = [
    ("names", "ply_privileged::shipped::names"),
    ("module", "ply_privileged::shipped::module"),
    ("answer", "ply_privileged::shipped::answer"),
    ("digests", "ply_privileged::shipped::digests"),
    ("reached", "ply_privileged::shipped::reached"),
    ("version", "ply_privileged::shipped::version"),
    ("stamps", "ply_privileged::shipped::stamps"),
    ("runtime", "ply_privileged::shipped::runtime"),
    ("values", "ply_privileged::shipped::values"),
    ("entered", "ply_privileged::shipped::entered"),
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
    fn call(&self, rt: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        let recorder = ply_host::observe::of(rt);
        let noted = |note: &dyn Fn(&ply_host::observe::Recorder)| {
            if let Some(recorder) = &recorder {
                note(recorder);
            }
        };
        // The rest answer for the runtime, which a pass's store is stamped with.
        match (req.op.op.as_str(), req.args) {
            ("names", []) => noted(&|r| r.shipped("")),
            ("module", [name]) => {
                if let Ok(name) = name.as_str(span, "a module's name") {
                    noted(&|r| r.shipped(name));
                }
            }
            ("definitions", []) => noted(&|r| r.ran_program()),
            ("reached", [view, pulled]) => {
                let view = view.as_bytes(span, "a load's view of the shipped modules")?;
                let pulled = pulled.as_bool(span, "whether the load pulled its modules")?;
                noted(&|r| r.reached(&String::from_utf8_lossy(view), pulled));
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
                let module = crate::shipped_modules::source(name).map(str::as_bytes);
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
            ("runtime", []) => PlyValue::bytes(ply_compiled::c::described()),
            ("values", [library, roots, steps]) => values(library, roots, steps, span)?,
            ("entered", [library, root, argument, steps]) => {
                let library = PathBuf::from(library.as_str(span, "the unit's library")?);
                let root = root.as_str(span, "the root to enter")?;
                let argument = argument.as_bytes(span, "an argument")?;
                let steps = steps.as_int(span, "the calls the entry may make")?;
                crate::payload::option(
                    entry_answered(&library, root, argument, steps, span)?.map(PlyValue::bytes),
                )
            }
            (other, _) => return Err(crate::hosts::unserved(EFFECT, other, req.span)),
        };
        Ok(HostAnswer::Value(value))
    }
}

/// What each of `roots`, a `const` definition's stored root, answers on the unit compiled into the
/// library `library`: each entered by name with nothing, binding no host, under `steps` calls. A
/// stored root answers the bytes a build keeps its definition's value as; one that raised, ran past
/// its calls, answered anything else or is not in the unit answers none.
fn values(
    library: &PlyValue,
    roots: &PlyValue,
    steps: &PlyValue,
    span: ply_core::Span,
) -> Result<PlyValue, Diagnostic> {
    use ply_compiled::entry::{self, Ended, Run};
    let library = PathBuf::from(library.as_str(span, "the unit's library")?);
    let roots = roots
        .as_list(span, "the stored roots")?
        .iter()
        .map(|root| root.as_str(span, "a stored root").map(str::to_string))
        .collect::<Result<Vec<_>, _>>()?;
    let run = Run {
        steps: steps.as_int(span, "the calls each may make")?,
        ..Run::unbounded(ply_core::DEFAULT_MAX_CALLS as i64)
    };
    let opened = opened_unit(&library).map_err(|why| {
        Diagnostic::error(
            codes::BACKEND_UNAVAILABLE,
            format!("the unit could not be loaded: {why}"),
        )
        .primary(span, "its constants were to be entered here")
    })?;
    let answered = entry::on_program_thread(|| {
        roots
            .iter()
            .map(|root| {
                match entry::enter(&opened, root, entry::no_arguments(), &run, None, None).ended {
                    Ended::Answered(bytes) => Some(bytes),
                    _ => None,
                }
            })
            .collect::<Vec<_>>()
    })
    .unwrap_or_else(|| roots.iter().map(|_| None).collect());
    Ok(PlyValue::list(
        answered
            .into_iter()
            .map(|bytes| {
                let kept = bytes.and_then(|b| match ply_core::codec::decode(&b) {
                    Ok(kept @ PlyValue::Bytes(_)) => Some(kept),
                    _ => None,
                });
                crate::payload::option(kept)
            })
            .collect(),
    ))
}

/// What `root` of the unit compiled into `library`, a function of one parameter, answers for the
/// value the PLV1 bytes `argument` hold under `steps` calls, as the root writes it: none where it
/// raised, ran past its calls or answered nothing a run can hold.
fn entry_answered(
    library: &std::path::Path,
    root: &str,
    argument: &[u8],
    steps: i64,
    span: ply_core::Span,
) -> Result<Option<Vec<u8>>, Diagnostic> {
    use ply_compiled::entry::{self, Ended, Run};
    let refused = |why: String| {
        Diagnostic::error(codes::INTERNAL_ERROR, why).primary(span, "this is Ply's fault")
    };
    let opened = opened_unit(library)
        .map_err(|why| refused(format!("an entered unit does not load: {why}")))?;
    let value = ply_core::codec::decode(argument)
        .map_err(|why| refused(format!("an entered unit's argument does not decode: {why}")))?;
    let arguments = ply_core::codec::encode(&PlyValue::list(vec![value]))
        .map_err(|why| refused(format!("an entered unit's argument does not encode: {why}")))?;
    let run = Run {
        steps,
        ..Run::unbounded(ply_core::DEFAULT_MAX_CALLS as i64)
    };
    Ok(entry::on_program_thread(|| {
        match entry::enter(&opened, root, arguments, &run, None, None).ended {
            Ended::Answered(bytes) => Some(bytes),
            _ => None,
        }
    })
    .flatten())
}

/// The unit compiled into `library`, opened the first time this process asks and kept after.
fn opened_unit(library: &std::path::Path) -> Result<Arc<ply_compiled::entry::Opened>, String> {
    use std::collections::HashMap;
    use std::sync::Mutex;
    type Units = Mutex<HashMap<std::path::PathBuf, Arc<ply_compiled::entry::Opened>>>;
    static UNITS: OnceLock<Units> = OnceLock::new();
    let units = UNITS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(opened) = units.lock().unwrap_or_else(|e| e.into_inner()).get(library) {
        return Ok(Arc::clone(opened));
    }
    let opened = ply_compiled::entry::Opened::open(library)?;
    let mut held = units.lock().unwrap_or_else(|e| e.into_inner());
    Ok(Arc::clone(
        held.entry(library.to_path_buf()).or_insert(opened),
    ))
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
