//! The `term` effect: whether the process's streams are a terminal, the terminal's size and input
//! mode, and what is typed at it. The terminal is the run's own process's, so the operations are
//! served where `process`'s own are, and wait on that host's pool.

use crate::pool::JobOutput;
use crate::process::ProcessHost;
use crate::stdio::{self, Mode, Standard};
use ply_eval::host::HostRegistry;
use ply_eval::{
    Determinism, Diagnostic, HostAnswer, HostHandler, HostOp, HostRequest, HostResource,
    HostRuntime, Linearity, Plain, Span, Symbol, Value, codes, slot,
};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const MODULE: &str = "std.term";

pub const EFFECT: &str = "std.term.term";

operations! {
    what "term";
    path "term";
    IsTerminal = "is_terminal" / 1,
    Size = "size" / 0,
    Mode = "mode" / 1,
    Input = "input" / 2,
    SecretLine = "secret_line" / 0,
}

impl Op {
    /// Waits for a person.
    pub fn waits(self) -> bool {
        match self {
            Op::Input | Op::SecretLine => true,
            Op::IsTerminal | Op::Size | Op::Mode => false,
        }
    }

    pub fn declaration(self) -> HostOp {
        HostOp {
            effect: Symbol::new(EFFECT),
            op: Symbol::new(self.name()),
            resource: HostResource::Any,
            determinism: Determinism::Nondeterministic,
            linearity: match self {
                // Asking what a stream is, or how large the terminal is, changes neither.
                Op::IsTerminal | Op::Size => Linearity::Repeatable,
                Op::Mode | Op::Input | Op::SecretLine => Linearity::AtMostOnce,
            },
            blocking: self.waits(),
            // `secret_line` answers a `Secret`; no operation is handed one.
            secrets: false,
            path: self.path(),
        }
    }
}

pub fn registrations(host: Option<&Arc<ProcessHost>>) -> Vec<(HostOp, Arc<dyn HostHandler>)> {
    Op::ALL
        .iter()
        .map(|op| {
            let handler: Arc<dyn HostHandler> = Arc::new(Operation {
                op: *op,
                host: host.cloned(),
            });
            (op.declaration(), handler)
        })
        .collect()
}

/// Bound for a host that is the run's own process, and withheld for one that only starts
/// programs: a test has no terminal.
pub fn register(registry: &mut HostRegistry, host: Option<&Arc<ProcessHost>>) {
    let served = host.is_some_and(|host| host.is_whole());
    for (op, handler) in registrations(host) {
        if served {
            registry.register(op, handler);
        } else {
            registry.register_withheld(op, handler, crate::process::ONLY_A_RUN);
        }
    }
}

struct Operation {
    op: Op,
    host: Option<Arc<ProcessHost>>,
}

impl HostHandler for Operation {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        if req.args.len() != self.op.arity() {
            return Err(arity(self.op, req.args.len(), span));
        }
        // A withheld registration is never resolved, so this is a dispatch bug.
        let Some(host) = &self.host else {
            return Err(Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!(
                    "{} was dispatched to a handler this run withheld",
                    self.op.what()
                ),
            )
            .primary(span, "performed here")
            .note("only `ply run --host` binds `term`, and a withheld registration is in no binding index")
            .note("this is a defect in Ply's host dispatch rather than in the program"));
        };
        match self.op {
            Op::IsTerminal => {
                let stream = stream_of(&req.args[0], span)?;
                Ok(HostAnswer::Value(Value::Bool(stdio::is_terminal(stream))))
            }
            Op::Size => Ok(HostAnswer::Value(option(stdio::terminal_size().map(
                |(columns, rows)| {
                    record([
                        ("columns", Value::Int(i64::from(columns))),
                        ("rows", Value::Int(i64::from(rows))),
                    ])
                },
            )))),
            Op::Mode => {
                let mode = mode_of(&req.args[0], span)?;
                let was = stdio::set_mode(mode).map_err(|e| unset(mode, &e, span))?;
                Ok(HostAnswer::Value(option(was.map(mode_value))))
            }
            Op::Input => {
                let max = req.args[0].as_int(span, "a length")?;
                let Ok(max) = usize::try_from(max) else {
                    return Err(negative_read(max, span));
                };
                let deadline = deadline(req.args[1].as_int(span, "a timeout")?);
                let pending = host.waiting(
                    span,
                    "term-input",
                    Op::Input.what(),
                    Box::new(move || match stdio::read_within(max, deadline) {
                        Ok(bytes) => JobOutput::MaybeBytes(bytes),
                        Err(e) => JobOutput::Failed(format!("the terminal could not be read: {e}")),
                    }),
                )?;
                Ok(HostAnswer::Pending(pending))
            }
            Op::SecretLine => {
                let pending = host.waiting(
                    span,
                    "term-secret-line",
                    Op::SecretLine.what(),
                    Box::new(|| match stdio::read_line() {
                        Ok(line) => JobOutput::MaybeSecret(line),
                        Err(e) => {
                            JobOutput::Failed(format!("standard input could not be read: {e}"))
                        }
                    }),
                )?;
                Ok(HostAnswer::Pending(pending))
            }
        }
    }
}

/// A negative timeout waits for as long as it takes, and so does one no clock can reach.
fn deadline(ms: i64) -> Option<Instant> {
    let ms = u64::try_from(ms).ok()?;
    Instant::now().checked_add(Duration::from_millis(ms))
}

/// A constructor of `module` by its simple name, when it carries nothing.
fn nullary<'a>(value: &'a Value, module: &str) -> Option<&'a str> {
    match value {
        Value::Ctor { name, args } if args.is_empty() => name
            .as_str()
            .strip_prefix(module)
            .and_then(|rest| rest.strip_prefix('.')),
        _ => None,
    }
}

fn stream_of(value: &Value, span: Span) -> Result<Standard, Diagnostic> {
    Ok(match nullary(value, crate::process::MODULE) {
        Some("Stdin") => Standard::In,
        Some("Stdout") => Standard::Out,
        Some("Stderr") => Standard::Err,
        _ => return Err(malformed(Op::IsTerminal, "Stream", value, span)),
    })
}

fn mode_of(value: &Value, span: Span) -> Result<Mode, Diagnostic> {
    Ok(match nullary(value, MODULE) {
        Some("Cooked") => Mode::Cooked,
        Some("Silent") => Mode::Silent,
        Some("Raw") => Mode::Raw,
        _ => return Err(malformed(Op::Mode, "Mode", value, span)),
    })
}

fn mode_value(mode: Mode) -> Value {
    let simple = match mode {
        Mode::Cooked => "Cooked",
        Mode::Silent => "Silent",
        Mode::Raw => "Raw",
    };
    Value::ctor(format!("{MODULE}.{simple}"), Vec::new())
}

fn option(value: Option<Value>) -> Value {
    match value {
        Some(value) => Value::ctor("Some", vec![value]),
        None => Value::ctor("None", Vec::new()),
    }
}

fn record<const N: usize>(fields: [(&str, Value); N]) -> Value {
    Value::Record(Arc::new(
        fields
            .into_iter()
            .map(|(name, value)| (Symbol::new(name), value))
            .collect(),
    ))
}

#[cold]
fn unset(mode: Mode, e: &std::io::Error, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`term.mode` could not put the terminal in `{mode:?}`: {e}"),
    )
    .primary(span, "the terminal refused its settings")
}

#[cold]
fn negative_read(max: i64, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`term.input` was asked for {max} bytes"),
    )
    .primary(span, "a read's length cannot be negative")
}

#[cold]
fn malformed(op: Op, wanted: &str, value: &Value, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("{} was given a `{wanted}` that is {}", op.what(), slot(0)),
    )
    .primary(span, "this perform reached the host handler")
    .note("inference checks a perform's argument types, so reaching this means the evaluator was handed a module that was never checked")
    .showing(vec![Plain::shown(value)])
}

#[cold]
fn arity(op: Op, got: usize, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!(
            "{} was performed with {got} argument(s) and takes {}",
            op.what(),
            op.arity()
        ),
    )
    .primary(span, "this perform reached the host handler")
    .note("inference checks a perform's arity, so reaching this means the evaluator was handed a module that was never checked")
}
