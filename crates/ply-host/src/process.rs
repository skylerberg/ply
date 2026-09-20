//! The `process` effect: the arguments, the two output streams and the exit code.

use ply_eval::host::HostRegistry;
use ply_eval::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRequest, HostResource, HostRuntime,
    Linearity, Value,
};
use ply_span::{Diagnostic, Span, Symbol, codes};
use std::io::Write;
use std::ops::RangeInclusive;
use std::sync::{Arc, Mutex, MutexGuard};

/// The Ply declaration the registrations below are checked against.
pub const DECLARATION: &str = ply_std::PROCESS;

pub const MODULE: &str = "std.process";

pub const EFFECT: &str = "std.process.process";

/// What `process.exit` accepts; above it the shell reports a signal or its own failure.
pub const EXIT_RANGE: RangeInclusive<i64> = 0..=125;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stream {
    Out,
    Err,
}

pub enum Sink {
    /// The process's own streams; `out` is where `process.out` goes, since `--json` reserves stdout.
    Real {
        out: Stream,
    },
    Captured(Mutex<Vec<(Stream, String)>>),
}

impl Sink {
    pub fn captured() -> Sink {
        Sink::Captured(Mutex::new(Vec::new()))
    }

    fn write(&self, stream: Stream, text: &str) -> std::io::Result<()> {
        match self {
            Sink::Real { out } => {
                let stream = match stream {
                    Stream::Out => *out,
                    Stream::Err => Stream::Err,
                };
                match stream {
                    Stream::Out => {
                        let mut handle = std::io::stdout().lock();
                        writeln!(handle, "{text}")?;
                        handle.flush()
                    }
                    Stream::Err => {
                        let mut handle = std::io::stderr().lock();
                        writeln!(handle, "{text}")?;
                        handle.flush()
                    }
                }
            }
            Sink::Captured(lines) => {
                lock(lines).push((stream, text.to_string()));
                Ok(())
            }
        }
    }
}

pub struct ProcessHost {
    argv: Vec<String>,
    sink: Sink,
    exit: Mutex<Option<i32>>,
}

impl ProcessHost {
    pub fn new(argv: Vec<String>, sink: Sink) -> ProcessHost {
        ProcessHost {
            argv,
            sink,
            exit: Mutex::new(None),
        }
    }

    pub fn argv(&self) -> &[String] {
        &self.argv
    }

    /// The code the first `process.exit` asked for.
    pub fn requested_exit(&self) -> Option<i32> {
        *lock(&self.exit)
    }

    /// Every line a captured sink took, in order; a real sink keeps nothing.
    pub fn captured(&self) -> Vec<(Stream, String)> {
        match &self.sink {
            Sink::Captured(lines) => lock(lines).clone(),
            Sink::Real { .. } => Vec::new(),
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

/// Withheld without a host, so a run that was given no arguments never binds a process it is not.
pub fn register(registry: &mut HostRegistry, host: Option<&Arc<ProcessHost>>) {
    for (op, handler) in registrations(host) {
        match host {
            Some(_) => registry.register(op, handler),
            None => registry.register_withheld(op, handler),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    Args,
    Out,
    Err,
    Exit,
}

impl Op {
    pub const ALL: [Op; 4] = [Op::Args, Op::Out, Op::Err, Op::Exit];

    pub fn name(self) -> &'static str {
        match self {
            Op::Args => "args",
            Op::Out => "out",
            Op::Err => "err",
            Op::Exit => "exit",
        }
    }

    pub fn what(self) -> &'static str {
        match self {
            Op::Args => "`process.args`",
            Op::Out => "`process.out`",
            Op::Err => "`process.err`",
            Op::Exit => "`process.exit`",
        }
    }

    pub fn path(self) -> &'static str {
        match self {
            Op::Args => "ply_host::process::args",
            Op::Out => "ply_host::process::out",
            Op::Err => "ply_host::process::err",
            Op::Exit => "ply_host::process::exit",
        }
    }

    pub fn arity(self) -> usize {
        match self {
            Op::Args => 0,
            Op::Out | Op::Err | Op::Exit => 1,
        }
    }

    pub fn declaration(self) -> HostOp {
        HostOp {
            effect: Symbol::new(EFFECT),
            op: Symbol::new(self.name()),
            resource: HostResource::Any,
            determinism: Determinism::Nondeterministic,
            // The arguments never change; a line written twice is written twice.
            linearity: match self {
                Op::Args => Linearity::Repeatable,
                Op::Out | Op::Err | Op::Exit => Linearity::AtMostOnce,
            },
            blocking: false,
            secrets: false,
            path: self.path(),
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
            .note("only `ply run --host` binds `process`, and a withheld registration is in no binding index")
            .note("this is a defect in Ply's host dispatch rather than in the program"));
        };
        match self.op {
            Op::Args => {
                let args = host.argv.iter().map(|arg| Value::Str(arg.as_str().into()));
                Ok(HostAnswer::Value(Value::list(args.collect())))
            }
            Op::Out | Op::Err => {
                let text = req.args[0].as_str(span, "the text to write")?;
                let stream = match self.op {
                    Op::Out => Stream::Out,
                    _ => Stream::Err,
                };
                host.sink
                    .write(stream, text)
                    .map_err(|e| err_write(self.op, &e, span))?;
                Ok(HostAnswer::Value(Value::Unit))
            }
            Op::Exit => {
                let code = req.args[0].as_int(span, "an exit code")?;
                if !EXIT_RANGE.contains(&code) {
                    return Err(err_exit_range(code, span));
                }
                let mut requested = lock(&host.exit);
                if requested.is_none() {
                    *requested = Some(code as i32);
                }
                Err(exit_requested(code, span))
            }
        }
    }
}

/// The unwinding the machine does on any diagnostic is what makes `exit` end the program.
#[cold]
fn exit_requested(code: i64, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::PROCESS_EXIT,
        format!("the program asked to exit with code {code}"),
    )
    .primary(span, "nothing after this perform runs")
    .note("`ply run` exits with this code once the host has torn down, and prints no value")
}

#[cold]
fn err_exit_range(code: i64, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!(
            "`process.exit` was given {code}, and an exit code is {} to {}",
            EXIT_RANGE.start(),
            EXIT_RANGE.end()
        ),
    )
    .primary(span, "this code cannot reach the shell as written")
    .note("codes above 125 are how a shell reports a command it could not run or a signal that ended it")
}

#[cold]
fn err_write(op: Op, e: &std::io::Error, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("{} could not write: {e}", op.what()),
    )
    .primary(span, "the stream refused the line")
    .note("the reader of this process's output went away, or the descriptor is closed")
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

/// The guarded state has no invariant a panicking caller can break, so recovering is correct.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
