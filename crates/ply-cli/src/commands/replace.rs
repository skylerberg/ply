//! `ply replace` — the runner for the `replace` command of the program in `crates/ply-cli/ply`,
//! and the one thing that command needs a host for: the text to put in the definition's place.

use super::shipped_program::{color, rooted, run};
use crate::artifact::Binds;
use crate::cli::ReplaceArgs;
use crate::hosts::Lent;
use crate::style::Style;
use ply_eval::Value as PlyValue;
use ply_eval::host::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRequest, HostResource, HostRuntime, Linearity,
};
use ply_span::{Diagnostic, Span, Symbol, codes};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The effect `crates/ply-cli/ply/replace.ply` declares. It is lent to that one entry and nowhere
/// else, and the program performs it only once it has a definition to replace, so a run that
/// names nothing waits on no stream.
const EFFECT: &str = "edit";

const ITEM: &str = "ply_cli::replace::item";

pub fn execute(args: &ReplaceArgs, style: Style) -> i32 {
    let (root, inside) = rooted(&args.path);
    let binds = Binds {
        lent: lent(args.with.clone()),
        ..Binds::default()
    };
    run(
        "replace",
        argv(args, style, &root, inside),
        &root,
        binds,
        args.json,
        style,
    )
}

/// What `process.args` answers: the command word, the flags the program reads, then the path.
fn argv(args: &ReplaceArgs, style: Style, root: &Path, inside: String) -> Vec<String> {
    let mut argv = vec![
        "replace".to_string(),
        color(style),
        format!("--root={}", root.display()),
        format!("--query={}", args.query),
    ];
    if args.check {
        argv.push("--check".to_string());
    }
    if args.json {
        argv.push("--json".to_string());
    }
    argv.push(inside);
    argv
}

/// The replacement, read when the program asks for it and not before.
fn lent(with: Option<PathBuf>) -> Vec<Lent> {
    let replacement: Arc<dyn HostHandler> = Arc::new(Replacement { with });
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

struct Replacement {
    with: Option<PathBuf>,
}

impl HostHandler for Replacement {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        if req.op.op.as_str() != "item" {
            return Err(unregistered(req.op.op.as_str(), req.span));
        }
        Ok(HostAnswer::Value(match read(self.with.as_deref()) {
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
