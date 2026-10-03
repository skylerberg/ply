//! What this binary ships: a function of the binary alone, so reading it keeps a test det.

use ply_eval::host::{HostAnswer, HostHandler, HostOp, HostRequest, HostRuntime, Linearity};
use ply_eval::{Diagnostic, Value as PlyValue};
use std::sync::{Arc, OnceLock};

const EFFECT: &str = "shipped";

const OPERATIONS: [(&str, &str); 5] = [
    ("names", "ply_machine::shipped::names"),
    ("module", "ply_machine::shipped::module"),
    ("version", "ply_machine::shipped::version"),
    ("stamps", "ply_machine::shipped::stamps"),
    ("runtime", "ply_machine::shipped::runtime"),
];

/// Empty in a process the launcher did not start.
static STAMPS: OnceLock<String> = OnceLock::new();

pub fn stamp(stamps: String) {
    let _ = STAMPS.set(stamps);
}

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
        let value = match (req.op.op.as_str(), req.args) {
            ("names", []) => PlyValue::list(
                crate::shelf::sources()
                    .iter()
                    .map(|(name, _)| PlyValue::str(name))
                    .collect(),
            ),
            ("module", [name]) => {
                let name = name.as_str(span, "a module's name")?;
                crate::payload::option(
                    crate::shelf::sources()
                        .iter()
                        .find(|(n, _)| n == name)
                        .map(|(_, text)| PlyValue::bytes(text.as_bytes())),
                )
            }
            ("version", []) => PlyValue::str(env!("CARGO_PKG_VERSION")),
            ("stamps", []) => PlyValue::str(STAMPS.get().map_or("", String::as_str)),
            ("runtime", []) => runtime(),
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
