//! The replacement `ply replace` writes: the one thing that command needs a host for. The text
//! is read when the program asks for it and not before — from `--with`'s file, else from stdin.

use ply_eval::host::{HostAnswer, HostHandler, HostOp, HostRequest, HostRuntime, Linearity};
use ply_eval::{Diagnostic, Value as PlyValue};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The effect `crates/ply-cli/ply/replace.ply` declares.
const EFFECT: &str = "edit";

const ITEM: &str = "ply_machine::edit::item";

/// The op and the one handler serving it. `with` is a program-passed path now: the program names
/// the file, or `None` for stdin.
pub fn lent() -> Vec<(HostOp, Arc<dyn HostHandler>)> {
    let replacement: Arc<dyn HostHandler> = Arc::new(Replacement);
    // Reading stdin consumes it, and a second read would answer with nothing.
    let op = crate::hosts::privileged_op(EFFECT, "item", Linearity::AtMostOnce, ITEM);
    vec![(op, replacement)]
}

struct Replacement;

impl HostHandler for Replacement {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        let with = match (req.op.op.as_str(), req.args) {
            ("item", [value]) => crate::payload::option_of(value, "the replacement's file", span)?
                .map(|path| {
                    path.as_str(span, "the replacement's file")
                        .map(PathBuf::from)
                })
                .transpose()?,
            (other, _) => return Err(crate::hosts::unserved(EFFECT, other, span)),
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
