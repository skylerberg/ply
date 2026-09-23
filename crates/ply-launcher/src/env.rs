//! The environment a launched program runs in, as a lent effect: the variables, whether the
//! streams are terminals, the working directory, and the binary's own version and shipped digest.
//! Bound by the launcher for the program it enters — user programs read configuration, not the
//! environment.
//!
//! Colour is decided here and nowhere else: a program has no terminal to ask.

use ply_eval::Value;
use ply_eval::host::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRequest, HostResource, HostRuntime, Linearity,
};
use ply_span::{Diagnostic, codes};
use std::io::IsTerminal;
use std::sync::Arc;

/// The effect a program declares to read its environment: `env.var[e](..)`, `env.terminal[e](..)`,
/// `env.binary_version[e]()`.
pub const EFFECT: &str = "env";

const OPERATIONS: [(&str, &str); 5] = [
    ("var", "ply_launcher::env::var"),
    ("terminal", "ply_launcher::env::terminal"),
    ("binary_version", "ply_launcher::env::binary_version"),
    ("pwd", "ply_launcher::env::pwd"),
    ("shipped_digest", "ply_launcher::env::shipped_digest"),
];

/// The ops and the handler, lent with the binary's version.
pub fn registrations(version: &str) -> Vec<(HostOp, Arc<dyn HostHandler>)> {
    let site: Arc<dyn HostHandler> = Arc::new(Site {
        version: version.to_string(),
    });
    OPERATIONS
        .into_iter()
        .map(|(op, path)| (registration(op, path), Arc::clone(&site)))
        .collect()
}

fn registration(op: &str, path: &'static str) -> HostOp {
    HostOp {
        effect: Symbol::new(EFFECT),
        op: Symbol::new(op),
        resource: HostResource::Any,
        // The environment is not a function of program state.
        determinism: Determinism::Nondeterministic,
        linearity: Linearity::Repeatable,
        blocking: false,
        secrets: false,
        path,
    }
}

use ply_span::Symbol;

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
