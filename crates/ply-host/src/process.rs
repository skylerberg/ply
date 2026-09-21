//! The `process` effect: the arguments, the two output streams, the exit code, and spawning.

use crate::pool::{Done, Ended, PROCESS_FIRST_TOKEN, Pool};
use ply_eval::host::HostRegistry;
use ply_eval::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRequest, HostResource, HostRuntime,
    Linearity, Pending, Value,
};
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_ty::Resource;
use std::collections::BTreeMap;
use std::io::Write;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// The Ply declaration the registrations below are checked against.
pub const DECLARATION: &str = ply_std::PROCESS;

pub const MODULE: &str = "std.process";

pub const EFFECT: &str = "std.process.process";

/// What `process.exit` accepts; above it the shell reports a signal or its own failure.
pub const EXIT_RANGE: RangeInclusive<i64> = 0..=125;

/// What one captured stream holds; a spawn answers with the whole of each, as `fs.read_file` does.
pub const MAX_CAPTURE_BYTES: usize = 64 * 1024 * 1024;

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

/// One `--exec NAME=PATH`, as the argument was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecSpec {
    pub name: String,
    pub path: PathBuf,
}

impl ExecSpec {
    pub fn parse(text: &str) -> Result<ExecSpec, String> {
        let (name, path) = text
            .split_once('=')
            .ok_or_else(|| malformed(text, "there is no `=`"))?;
        if name.is_empty() {
            return Err(malformed(text, "the executable has no name"));
        }
        if path.is_empty() {
            return Err(malformed(text, "the path is empty"));
        }
        if !name.chars().all(|c| c.is_alphanumeric() || c == '_')
            || name.chars().next().is_some_and(|c| c.is_numeric())
        {
            return Err(malformed(
                text,
                "an executable's name is a resource label, so it is an identifier: letters, digits and `_`, not starting with a digit",
            ));
        }
        Ok(ExecSpec {
            name: name.to_string(),
            path: PathBuf::from(path),
        })
    }
}

fn malformed(text: &str, why: &str) -> String {
    format!("`{text}` is not an executable: {why}; write `--exec NAME=PATH`")
}

/// What `--exec NAME=PATH` bound, resolved once: the only programs this run can start.
#[derive(Clone, Debug, Default)]
pub struct Executables {
    bound: BTreeMap<String, PathBuf>,
}

impl Executables {
    pub fn new() -> Executables {
        Executables::default()
    }

    pub fn load(specs: &[ExecSpec], span: Span) -> Result<Executables, Diagnostic> {
        let mut executables = Executables::new();
        for spec in specs {
            executables.bind(&spec.name, &spec.path, span)?;
        }
        Ok(executables)
    }

    pub fn bind(&mut self, name: &str, path: &Path, span: Span) -> Result<(), Diagnostic> {
        let resolved = path.canonicalize().map_err(|e| {
            Diagnostic::error(
                codes::PROCESS_EXEC_INVALID,
                format!("`--exec {name}={}` does not resolve: {e}", path.display()),
            )
            .primary(span, "this program does not exist")
            .note("every executable is resolved once, before anything runs, and the resolved path is what a spawn starts")
        })?;
        if !resolved.is_file() {
            return Err(Diagnostic::error(
                codes::PROCESS_EXEC_INVALID,
                format!("`--exec {name}={}` is not a file", path.display()),
            )
            .primary(span, "an executable is a file")
            .note("a label names one program, so a directory has nothing to start"));
        }
        if !is_executable(&resolved) {
            return Err(Diagnostic::error(
                codes::PROCESS_EXEC_INVALID,
                format!("`--exec {name}={}` is not executable", path.display()),
            )
            .primary(span, "this file has no execute permission")
            .note(
                "a spawn would fail at the first call; it is refused before anything runs instead",
            ));
        }
        self.bound.insert(name.to_string(), resolved);
        Ok(())
    }

    pub fn get(&self, at: &Resource) -> Option<&Path> {
        match at {
            Resource::Named(name) => self.bound.get(name.as_str()).map(PathBuf::as_path),
            // Unreachable when well-typed; `None` keeps a malformed registration a diagnostic.
            Resource::Var(_) | Resource::Singleton => None,
        }
    }

    pub fn listing(&self) -> impl Iterator<Item = (&str, &Path)> {
        self.bound
            .iter()
            .map(|(name, path)| (name.as_str(), path.as_path()))
    }

    pub fn is_empty(&self) -> bool {
        self.bound.is_empty()
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(_: &Path) -> bool {
    true
}

pub struct ProcessHost {
    argv: Vec<String>,
    sink: Sink,
    exit: Mutex<Option<i32>>,
    /// The programs `--exec NAME=PATH` bound; a label outside this is `E0456`.
    executables: Executables,
    /// Where a spawn waits, so a driver that starts a compiler does not stop the machine.
    pool: Pool,
}

impl ProcessHost {
    pub fn new(argv: Vec<String>, sink: Sink) -> ProcessHost {
        ProcessHost {
            argv,
            sink,
            exit: Mutex::new(None),
            executables: Executables::new(),
            pool: Pool::new(PROCESS_FIRST_TOKEN),
        }
    }

    pub fn executing(self, executables: Executables) -> ProcessHost {
        ProcessHost {
            executables,
            ..self
        }
    }

    pub fn executables(&self) -> &Executables {
        &self.executables
    }

    pub fn owns(&self, pending: &Pending) -> bool {
        self.pool.owns(pending)
    }

    pub fn poll(&self, pending: &Pending) -> Result<Option<Value>, Diagnostic> {
        self.pool.poll(pending)
    }

    pub fn park(&self) -> Result<(), Diagnostic> {
        self.pool.park()
    }

    pub fn park_until(&self, bound: Duration) -> Result<(), Diagnostic> {
        self.pool.park_until(bound)
    }

    pub fn outstanding(&self) -> usize {
        self.pool.outstanding()
    }

    pub fn block_on(&self, pending: Pending) -> Result<Value, Diagnostic> {
        self.pool.block_on(pending)
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
    Spawn,
}

impl Op {
    pub const ALL: [Op; 5] = [Op::Args, Op::Out, Op::Err, Op::Exit, Op::Spawn];

    pub fn name(self) -> &'static str {
        match self {
            Op::Args => "args",
            Op::Out => "out",
            Op::Err => "err",
            Op::Exit => "exit",
            Op::Spawn => "spawn",
        }
    }

    pub fn what(self) -> &'static str {
        match self {
            Op::Args => "`process.args`",
            Op::Out => "`process.out`",
            Op::Err => "`process.err`",
            Op::Exit => "`process.exit`",
            Op::Spawn => "`process.spawn`",
        }
    }

    pub fn path(self) -> &'static str {
        match self {
            Op::Args => "ply_host::process::args",
            Op::Out => "ply_host::process::out",
            Op::Err => "ply_host::process::err",
            Op::Exit => "ply_host::process::exit",
            Op::Spawn => "ply_host::process::spawn",
        }
    }

    pub fn arity(self) -> usize {
        match self {
            Op::Args => 0,
            Op::Out | Op::Err | Op::Exit => 1,
            Op::Spawn => 3,
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
                Op::Out | Op::Err | Op::Exit | Op::Spawn => Linearity::AtMostOnce,
            },
            // A spawn waits for another process, so it waits in the pool rather than on the machine.
            blocking: self == Op::Spawn,
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
            // The resolved atom's resource, never one the handler re-derives.
            Op::Spawn => {
                let Some(program) = host.executables.get(&req.atom.resource) else {
                    return Err(unbound(&req.atom.resource, span));
                };
                let program = program.to_path_buf();
                let args = argument_vector(&req.args[0], span)?;
                let dir = req.args[1].as_str(span, "a working directory")?.to_string();
                let env = environment(&req.args[2], span)?;
                let pending = host.pool.submit(
                    span,
                    "process-spawn",
                    Op::Spawn.what(),
                    Box::new(move || start(&program, &args, &dir, &env, span)),
                )?;
                Ok(HostAnswer::Pending(pending))
            }
        }
    }
}

fn argument_vector(value: &Value, span: Span) -> Result<Vec<String>, Diagnostic> {
    value
        .as_list(span, "an argument vector")?
        .iter()
        .map(|arg| arg.as_str(span, "an argument").map(str::to_string))
        .collect()
}

/// The whole environment a spawn runs under; a later entry for one name wins, as `map_insert` does.
fn environment(value: &Value, span: Span) -> Result<Vec<(String, String)>, Diagnostic> {
    let mut out = Vec::new();
    for entry in value.as_list(span, "an environment")?.iter() {
        let Value::Record(fields) = entry else {
            return Err(malformed_env(entry.type_name(), span));
        };
        let (Some(name), Some(setting)) = (
            fields.get(&Symbol::new("name")),
            fields.get(&Symbol::new("value")),
        ) else {
            return Err(malformed_env("a record without `name` and `value`", span));
        };
        out.push((
            name.as_str(span, "a variable's name")?.to_string(),
            setting.as_str(span, "a variable's value")?.to_string(),
        ));
    }
    Ok(out)
}

/// Buffered, never streamed: Ply has no file handles, so each stream arrives whole or not at all.
fn start(program: &Path, args: &[String], dir: &str, env: &[(String, String)], span: Span) -> Done {
    let mut command = Command::new(program);
    command.args(args).env_clear().stdin(Stdio::null());
    for (name, value) in env {
        command.env(name, value);
    }
    if !dir.is_empty() {
        command.current_dir(dir);
    }
    match command.output() {
        Err(e) => Done::Failed(format!("`{}` could not be started: {e}", program.display())),
        Ok(done) => {
            if done.stdout.len() > MAX_CAPTURE_BYTES || done.stderr.len() > MAX_CAPTURE_BYTES {
                return Done::Refused(too_much(
                    program,
                    done.stdout.len(),
                    done.stderr.len(),
                    span,
                ));
            }
            Done::Spawned {
                ended: ended(&done.status),
                out: done.stdout,
                err: done.stderr,
            }
        }
    }
}

fn ended(status: &ExitStatus) -> Ended {
    match status.code() {
        Some(code) => Ended::Exited(i64::from(code)),
        None => Ended::Signalled(signal_of(status)),
    }
}

#[cfg(unix)]
fn signal_of(status: &ExitStatus) -> i64 {
    use std::os::unix::process::ExitStatusExt;
    status.signal().map_or(0, i64::from)
}

#[cfg(not(unix))]
fn signal_of(_: &ExitStatus) -> i64 {
    0
}

#[cold]
pub fn unbound(at: &Resource, span: Span) -> Diagnostic {
    let label = match at {
        Resource::Named(name) => name.as_str().to_string(),
        Resource::Var(v) => ply_ty::label_var_name(*v),
        Resource::Singleton => "the singleton resource".to_string(),
    };
    Diagnostic::error(
        codes::PROCESS_EXEC_UNBOUND,
        format!("`process.spawn` names `{label}`, and no executable is bound to it"),
    )
    .primary(span, format!("`{label}` names no program"))
    .note(format!("bind one beside the run: `--exec {label}=<program>`"))
    .note("the label is the capability: a spawned process is outside what the effect system can promise, so which program a label may start is named where the run is configured, never in the program")
}

#[cold]
fn too_much(program: &Path, out: usize, err: usize, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::PROCESS_OUTPUT_TOO_LARGE,
        format!(
            "`{}` wrote {out} byte(s) to stdout and {err} to stderr",
            program.display()
        ),
    )
    .primary(span, "this is more than a captured stream holds")
    .note(format!(
        "`process.spawn` answers with each stream whole, and the bound is {MAX_CAPTURE_BYTES} bytes"
    ))
    .note("there are no file handles and no streaming in v1, so there is no way to read part of it")
}

#[cold]
fn malformed_env(found: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("`process.spawn` was given an environment entry that is {found}"),
    )
    .primary(span, "this perform reached the host handler")
    .note("inference checks a perform's argument types, so reaching this means the evaluator was handed a module that was never checked")
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
