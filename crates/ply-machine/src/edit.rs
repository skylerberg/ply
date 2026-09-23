//! The replacement `ply replace` writes: the one thing that command needs a host for. The text
//! is read when the program asks for it and not before — from `--with`'s file, else from stdin.

use ply_eval::Value as PlyValue;
use ply_eval::host::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRequest, HostResource, HostRuntime, Linearity,
};
use ply_span::{Diagnostic, Span, Symbol, codes};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The effect `crates/ply-cli/ply/replace.ply` declares.
const EFFECT: &str = "edit";

const ITEM: &str = "ply_machine::edit::item";

/// The op and the one handler serving it. `with` is a program-passed path now: the program names
/// the file, or `None` for stdin.
pub fn lent() -> Vec<(HostOp, Arc<dyn HostHandler>)> {
    let replacement: Arc<dyn HostHandler> = Arc::new(Replacement);
    vec![(registration(), replacement)]
}

fn registration() -> HostOp {
    HostOp {
        effect: Symbol::new(EFFECT),
        op: Symbol::new("item"),
        resource: HostResource::Any,
        // A file on disk and a stream are not functions of program state.
        determinism: Determinism::Nondeterministic,
        // Reading stdin consumes it, and a second read would answer with nothing.
        linearity: Linearity::AtMostOnce,
        // The handler reads it here rather than dispatching: one entry, no other task to stall.
        blocking: false,
        secrets: false,
        path: ITEM,
    }
}

struct Replacement;

impl HostHandler for Replacement {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        if req.op.op.as_str() != "item" {
            return Err(unregistered(req.op.op.as_str(), span));
        }
        let with = match req.args {
            [] => None,
            [value] => match value {
                PlyValue::Ctor { name, args } if name.as_str() == "Some" => Some(PathBuf::from(
                    args.first()
                        .ok_or_else(|| unregistered("item", span))?
                        .as_str(span, "the replacement's file")?,
                )),
                PlyValue::Ctor { name, .. } if name.as_str() == "None" => None,
                _ => None,
            },
            _ => return Err(unregistered("item", span)),
        };
        Ok(HostAnswer::Value(match read(with.as_deref()) {
            Ok(item) => PlyValue::ctor("Ok", vec![PlyValue::str(item)]),
            Err(why) => PlyValue::ctor("Err", vec![PlyValue::str(why)]),
        }))
    }
}

/// The new item from `--with`, else from this process's stdin.
fn read(with: Option<&Path>) -> Result<String, String> {
    match with {
        Some(path) => std::fs::read_to_string(path)
            .map_err(|e| format!("could not read `{}`: {e}", path.display())),
        None => std::io::read_to_string(std::io::stdin())
            .map_err(|e| format!("could not read stdin: {e}")),
    }
}

#[cold]
fn unregistered(op: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("`{EFFECT}.{op}` is not an operation this command serves"),
    )
    .primary(span, "performed here")
    .note("this is a defect in Ply's host dispatch rather than in the program")
}
