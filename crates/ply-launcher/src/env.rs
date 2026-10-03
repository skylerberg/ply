//! The environment a launched program runs in, as a lent effect: the variables, whether the
//! streams are terminals, the working directory, the binary's own version and shipped digest, the
//! directory `ply run` files the fronts it reuses in, and the one the emitter's answers are kept in.
//! Bound by the launcher for the program it enters — user programs read configuration, not the
//! environment.
//!
//! Colour is decided here and nowhere else: a program has no terminal to ask.

use ply_eval::host::{HostAnswer, HostHandler, HostOp, HostRequest, HostRuntime, Linearity};
use ply_eval::{Diagnostic, Value, codes};
use std::io::IsTerminal;
use std::sync::Arc;

/// The effect a program declares to read its environment: `env.var[e](..)`, `env.terminal[e](..)`,
/// `env.binary_version[e]()`.
pub const EFFECT: &str = "env";

const OPERATIONS: [(&str, &str); 8] = [
    ("var", "ply_launcher::env::var"),
    ("vars", "ply_launcher::env::vars"),
    ("terminal", "ply_launcher::env::terminal"),
    ("binary_version", "ply_launcher::env::binary_version"),
    ("pwd", "ply_launcher::env::pwd"),
    ("shipped_digest", "ply_launcher::env::shipped_digest"),
    ("fronts", "ply_launcher::env::fronts"),
    ("bodies", "ply_launcher::env::bodies"),
];

/// The ops and the handler, lent with the binary's version.
pub fn registrations(version: &str) -> Vec<(HostOp, Arc<dyn HostHandler>)> {
    let site: Arc<dyn HostHandler> = Arc::new(Site {
        version: version.to_string(),
    });
    OPERATIONS
        .into_iter()
        .map(|(op, path)| {
            let op = ply_machine::hosts::privileged_op(EFFECT, op, Linearity::Repeatable, path);
            (op, Arc::clone(&site))
        })
        .collect()
}

struct Site {
    version: String,
}

impl HostHandler for Site {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let value = match (req.op.op.as_str(), req.args) {
            ("var", [name]) => {
                let name = name.as_str(req.span, "an environment variable's name")?;
                match std::env::var(name) {
                    Ok(value) => Value::ctor("Some", vec![Value::str(value)]),
                    Err(_) => Value::ctor("None", Vec::new()),
                }
            }
            // A name or value that is not UTF-8 is no configuration key's, and is left out.
            ("vars", []) => Value::list(
                std::env::vars_os()
                    .filter_map(|(name, value)| {
                        Some((name.into_string().ok()?, value.into_string().ok()?))
                    })
                    .map(|(name, value)| {
                        ply_machine::payload::record(vec![
                            ("name", Value::str(name)),
                            ("value", Value::str(value)),
                        ])
                    })
                    .collect(),
            ),
            ("terminal", [stream]) => {
                let stream = stream.as_str(req.span, "a stream's name")?;
                let terminal = match stream {
                    "stdout" => std::io::stdout().is_terminal(),
                    "stderr" => std::io::stderr().is_terminal(),
                    _ => false,
                };
                Value::Bool(terminal)
            }
            ("binary_version", []) => Value::str(&self.version),
            // The digest the committed CLI artifact is gated on: the build of the program's own
            // sources writes it beside the artifact.
            ("shipped_digest", []) => Value::str(crate::shipped::identity()),
            // Under the stage root, where `sweep` keeps them to the cache's budget.
            ("fronts", []) => Value::str(
                ply_codegen::c::bundle::stage_dir(ply_codegen::c::sweep::RUNS)
                    .display()
                    .to_string(),
            ),
            // Where the emitter's answers are kept between runs, under the toolchain's sweep.
            ("bodies", []) => Value::str(ply_codegen::c::bodies_dir().display().to_string()),
            // The working directory the `cwd` root is bound to, as the program resolves paths.
            ("pwd", []) => Value::str(
                std::env::current_dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| ".".to_string()),
            ),
            (other, _) => {
                return Err(Diagnostic::error(
                    codes::INTERNAL_ERROR,
                    format!("`env.{other}` is not an operation the environment serves"),
                )
                .primary(req.span, "this is Ply's fault"));
            }
        };
        Ok(HostAnswer::Value(value))
    }
}
