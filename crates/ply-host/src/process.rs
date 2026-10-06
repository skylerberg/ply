//! The `process` effect: the arguments, the two output streams, standard input, the exit code, and
//! the programs a run may start, to completion or beside it.

mod children;

pub(crate) use children::{Children, Io, Output, Signal};
pub use children::{Ended, Finished, Heard};

use crate::pool::{JobOutput, Pool, Pooled, option};
use crate::stdio;
use children::{Child, Launch, Refusal, Unusable};
use ply_eval::host::{HostRegistry, MachineId};
use ply_eval::{
    Determinism, Diagnostic, HostAnswer, HostHandler, HostOp, HostRequest, HostResource,
    HostRuntime, Linearity, Plain, Resource, Span, Symbol, Value, codes, slot,
};
use std::collections::BTreeMap;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub const MODULE: &str = "std.process";

pub const EFFECT: &str = "std.process.process";

/// The effect whose raise says a standard stream's reader went away, and that raise.
pub const PIPE_EFFECT: &str = "std.process.pipe";
pub const PIPE_BROKEN: &str = "broken";

/// What `process.exit` accepts; above it the shell reports a signal or its own failure.
pub const EXIT_RANGE: RangeInclusive<i64> = 0..=125;

/// What one captured stream holds: a spawn's whole stream, or what a child's `Keep` stream took
/// and its `Lines` stream holds unread.
pub const MAX_CAPTURE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stream {
    Out,
    Err,
}

pub enum OutputSink {
    /// The process's own streams; `out` is where `process.out` goes, since `--json` reserves stdout.
    Real {
        out: Stream,
    },
    Captured(Mutex<Vec<(Stream, Vec<u8>)>>),
}

impl OutputSink {
    pub fn captured() -> OutputSink {
        OutputSink::Captured(Mutex::new(Vec::new()))
    }

    /// Whether what is written leaves the process, where a reader can go away.
    fn is_real(&self) -> bool {
        matches!(self, OutputSink::Real { .. })
    }

    /// Standard output is buffered only where it is the process's own: redirected onto standard
    /// error it is written as that stream is, at once.
    fn write(&self, stream: Stream, bytes: &[u8]) -> std::io::Result<()> {
        match self {
            OutputSink::Real { out } => match (stream, out) {
                (Stream::Out, Stream::Out) => stdio::write_out(bytes),
                _ => stdio::write_err(bytes),
            },
            OutputSink::Captured(writes) => {
                lock(writes).push((stream, bytes.to_vec()));
                Ok(())
            }
        }
    }

    /// One line, and with it everything its stream's buffer held.
    pub(crate) fn line(&self, stream: Stream, text: &str) -> std::io::Result<()> {
        let mut line = Vec::with_capacity(text.len() + 1);
        line.extend_from_slice(text.as_bytes());
        line.push(b'\n');
        self.write(stream, &line)?;
        match stream {
            Stream::Out => self.flush(),
            Stream::Err => Ok(()),
        }
    }

    /// Writes what the program's standard output holds back, which is nothing where it is
    /// redirected onto standard error or captured.
    fn flush(&self) -> std::io::Result<()> {
        match self {
            OutputSink::Real { out: Stream::Out } => stdio::flush_out(),
            OutputSink::Real { out: Stream::Err } | OutputSink::Captured(_) => Ok(()),
        }
    }

    /// Writes what standard output holds back ahead of something else that writes the stream.
    fn make_way(&self) {
        if self.is_real() {
            stdio::make_way();
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
            Resource::Var(_) | Resource::Singleton | Resource::Every => None,
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
    /// Shared with the drains that forward an inheriting child's lines to a captured sink.
    sink: Arc<OutputSink>,
    exit: Mutex<Option<i32>>,
    /// The programs `--exec NAME=PATH` bound, which `bound` reads; a label outside them is `E0456`.
    executables: Executables,
    /// Where a spawn waits, so a driver that starts a compiler does not stop the machine.
    pool: Pool,
    /// Held nowhere else, so the children go when the host does.
    children: Arc<Children>,
    /// Whether the run is a process of its own; one that is not only starts other programs.
    whole: bool,
}

impl ProcessHost {
    pub fn new(argv: Vec<String>, sink: OutputSink) -> ProcessHost {
        ProcessHost {
            argv,
            sink: Arc::new(sink),
            exit: Mutex::new(None),
            executables: Executables::new(),
            pool: Pool::new(),
            children: Arc::new(Children::new()),
            whole: true,
        }
    }

    /// A host for a run that is not itself a process, such as a test: it starts and drives the
    /// programs `executables` binds, and its own arguments, streams, input and exit code stay
    /// withheld.
    pub fn spawning(executables: Executables) -> ProcessHost {
        ProcessHost {
            whole: false,
            ..ProcessHost::new(Vec::new(), OutputSink::captured())
        }
        .executing(executables)
    }

    /// Whether the run is a process of its own, with arguments, streams and a terminal.
    pub fn is_whole(&self) -> bool {
        self.whole
    }

    /// Whether this host serves `op`, rather than leaving it withheld.
    pub fn serves(&self, op: Op) -> bool {
        self.whole || op.names_an_executable()
    }

    pub fn executing(self, executables: Executables) -> ProcessHost {
        ProcessHost {
            executables,
            ..self
        }
    }

    pub fn argv(&self) -> &[String] {
        &self.argv
    }

    /// The code the first `process.exit` asked for.
    pub fn requested_exit(&self) -> Option<i32> {
        *lock(&self.exit)
    }

    /// Every write a captured sink took, in order, a line with its newline; a real sink keeps
    /// nothing.
    pub fn captured(&self) -> Vec<(Stream, Vec<u8>)> {
        match &*self.sink {
            OutputSink::Captured(writes) => lock(writes).clone(),
            OutputSink::Real { .. } => Vec::new(),
        }
    }

    /// Whether this host is the run's own process, and its streams the process's own.
    fn owns_the_streams(&self) -> bool {
        self.whole && self.sink.is_real()
    }

    /// Leaves the process's streams as a program that ended leaves them: standard output's
    /// buffer written and the terminal as it was found. A buffer whose reader has gone ends the
    /// process as [`ProcessHost::end_of_broken_pipe`] does.
    pub fn settle(&self) {
        if self.owns_the_streams() && stdio::settle() {
            self.end_of_broken_pipe();
        }
    }

    /// Ends the process where it stands, as a closed pipe ends one that did not ask to be told:
    /// nothing more is written, and the status is the one a shell reports for `SIGPIPE`.
    pub fn end_of_broken_pipe(&self) -> ! {
        self.children.end_all();
        stdio::settle();
        crate::observe::exiting();
        std::process::exit(stdio::BROKEN_PIPE_STATUS);
    }

    /// What this host makes of a raise no clause answered: an unanswered `pipe.broken` ends the
    /// process as the closed pipe would have.
    pub fn unanswered(&self, effect: &Symbol, op: &Symbol) {
        if self.owns_the_streams() && effect.as_str() == PIPE_EFFECT && op.as_str() == PIPE_BROKEN {
            self.end_of_broken_pipe();
        }
    }

    pub(crate) fn children(&self) -> &Arc<Children> {
        &self.children
    }

    /// Kills and reaps every child `process.start` launched that is still running.
    pub fn end_children(&self) {
        self.children.end_all();
    }
}

impl Pooled for ProcessHost {
    fn pool(&self) -> &Pool {
        &self.pool
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

/// Where an operation about the run's own process is served, for a refusal to say.
pub const ONLY_A_RUN: &str =
    "under `ply run --host`, and `ply test` withholds it whether or not `--host` was passed";

/// Where an operation on a program `--exec` binds is served.
const AN_EXECUTABLE: &str = "under `--host` for a label `--exec` binds to a program";

/// Withheld without a host, so a run that was given no arguments never binds a process it is not;
/// a host that only starts programs withholds what is about the run's own process.
pub fn register(registry: &mut HostRegistry, host: Option<&Arc<ProcessHost>>) {
    for (op, (declared, handler)) in Op::ALL.into_iter().zip(registrations(host)) {
        match host {
            Some(host) if host.serves(op) => registry.register(declared, handler),
            _ => {
                let served = if op.names_an_executable() {
                    AN_EXECUTABLE
                } else {
                    ONLY_A_RUN
                };
                registry.register_withheld(declared, handler, served)
            }
        }
    }
}

operations! {
    what "process";
    path "process";
    Args = "args" / 0,
    Bound = "bound" / 0,
    Out = "out" / 1,
    Err = "err" / 1,
    OutBytes = "out_bytes" / 1,
    ErrBytes = "err_bytes" / 1,
    Flush = "flush" / 0,
    Line = "line" / 0,
    InBytes = "in_bytes" / 1,
    Exit = "exit" / 1,
    Spawn = "spawn" / 3,
    Start = "start" / 4,
    Wait = "wait" / 2,
    Signal = "signal" / 2,
    Input = "input" / 2,
    EndInput = "end_input" / 1,
    OutputLine = "output_line" / 2,
}

impl Op {
    /// The stream an operation that writes one writes to.
    fn stream(self) -> Stream {
        match self {
            Op::Err | Op::ErrBytes => Stream::Err,
            _ => Stream::Out,
        }
    }

    /// Labelled by the program `--exec` bound rather than by the run's own process.
    pub fn names_an_executable(self) -> bool {
        match self {
            Op::Bound
            | Op::Spawn
            | Op::Start
            | Op::Wait
            | Op::Signal
            | Op::Input
            | Op::EndInput
            | Op::OutputLine => true,
            Op::Args
            | Op::Out
            | Op::Err
            | Op::OutBytes
            | Op::ErrBytes
            | Op::Flush
            | Op::Line
            | Op::InBytes
            | Op::Exit => false,
        }
    }

    /// Waits for another process, for a person, or on a pipe a child drains at its own pace.
    pub fn waits(self) -> bool {
        match self {
            Op::Spawn | Op::Line | Op::InBytes | Op::Wait | Op::Input | Op::OutputLine => true,
            Op::Args
            | Op::Bound
            | Op::Out
            | Op::Err
            | Op::OutBytes
            | Op::ErrBytes
            | Op::Flush
            | Op::Exit
            | Op::Start
            | Op::Signal
            | Op::EndInput => false,
        }
    }

    pub fn declaration(self) -> HostOp {
        HostOp {
            effect: Symbol::new(EFFECT),
            op: Symbol::new(self.name()),
            resource: HostResource::Any,
            determinism: Determinism::Nondeterministic,
            // The arguments and the executables never change; everything else writes, starts,
            // signals, reaps or consumes.
            linearity: match self {
                Op::Args | Op::Bound => Linearity::Repeatable,
                Op::Out
                | Op::Err
                | Op::OutBytes
                | Op::ErrBytes
                | Op::Flush
                | Op::Line
                | Op::InBytes
                | Op::Exit
                | Op::Spawn
                | Op::Start
                | Op::Wait
                | Op::Signal
                | Op::Input
                | Op::EndInput
                | Op::OutputLine => Linearity::AtMostOnce,
            },
            blocking: self.waits(),
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
            Op::Bound => Ok(HostAnswer::Value(Value::Bool(
                host.executables.get(&req.atom.resource).is_some(),
            ))),
            Op::Out | Op::Err => {
                let text = req.args[0].as_str(span, "the text to write")?;
                match host.sink.line(self.op.stream(), text) {
                    Ok(()) => Ok(HostAnswer::Value(Value::Unit)),
                    // A line answers nothing, so its writer cannot be told: the pipe ends it.
                    Err(e) if stdio::reader_gone(&e) => host.end_of_broken_pipe(),
                    Err(e) => Err(err_write(self.op, &e, span)),
                }
            }
            Op::OutBytes | Op::ErrBytes => {
                let bytes = req.args[0].as_bytes(span, "the bytes to write")?;
                taken(self.op, host.sink.write(self.op.stream(), bytes), span)
            }
            Op::Flush => taken(self.op, host.sink.flush(), span),
            Op::Line => {
                let pending =
                    host.pool
                        .submit(span, "process-line", Op::Line.what(), Box::new(read_line))?;
                Ok(HostAnswer::Pending(pending))
            }
            Op::InBytes => {
                let max = req.args[0].as_int(span, "a length")?;
                let Ok(max) = usize::try_from(max) else {
                    return Err(negative_read(max, span));
                };
                let pending = host.pool.submit(
                    span,
                    "process-in-bytes",
                    Op::InBytes.what(),
                    Box::new(move || match stdio::read(max) {
                        Ok(bytes) => JobOutput::Bytes(bytes),
                        Err(e) => {
                            JobOutput::Failed(format!("standard input could not be read: {e}"))
                        }
                    }),
                )?;
                Ok(HostAnswer::Pending(pending))
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
                    return Err(unbound(self.op, &req.atom.resource, span));
                };
                let program = program.to_path_buf();
                let args = argument_vector(&req.args[0], span)?;
                let dir = req.args[1].as_str(span, "a working directory")?.to_string();
                let env = traced(req.machine, environment(self.op, &req.args[2], span)?);
                let pending = host.pool.submit(
                    span,
                    "process-spawn",
                    Op::Spawn.what(),
                    Box::new(move || run_to_end(&program, &args, &dir, &env, span)),
                )?;
                Ok(HostAnswer::Pending(pending))
            }
            Op::Start => {
                let Some(program) = host.executables.get(&req.atom.resource) else {
                    return Err(unbound(self.op, &req.atom.resource, span));
                };
                let args = argument_vector(&req.args[0], span)?;
                let env = traced(req.machine, environment(self.op, &req.args[2], span)?);
                let io = io_of(&req.args[3], span)?;
                let dir = req.args[1].as_str(span, "a working directory")?;
                // What the program wrote before the child comes before what the child writes.
                if io.out == Output::Inherit || io.err == Output::Inherit {
                    host.sink.make_way();
                }
                let launch = Launch {
                    label: &req.atom.resource,
                    program,
                    args: &args,
                    dir,
                    env: &env,
                    io: &io,
                };
                let started = host.children.start(&launch, &host.sink);
                for output in [&io.out, &io.err] {
                    if let Output::File(path) = output
                        && let Ok(at) = Path::new(dir).join(path).canonicalize()
                    {
                        crate::observe::wrote(req.machine, &at);
                    }
                }
                Ok(HostAnswer::Value(match started {
                    Ok(handle) => Value::ctor("Ok", vec![Value::Int(handle)]),
                    Err(why) => Value::ctor("Err", vec![Value::str(why)]),
                }))
            }
            Op::Wait => {
                let (handle, child) = child_of(host, self.op, req)?;
                let deadline = deadline(req.args[1].as_int(span, "a timeout")?);
                let pending = host.pool.submit(
                    span,
                    "process-wait",
                    Op::Wait.what(),
                    Box::new(move || match child.wait(deadline) {
                        Ok(exit) => JobOutput::built(move || option(exit.map(Finished::value))),
                        Err(refusal) => refused(Op::Wait, handle, &child, refusal, span),
                    }),
                )?;
                Ok(HostAnswer::Pending(pending))
            }
            Op::Signal => {
                let (handle, child) = child_of(host, self.op, req)?;
                let signal = signal_of(&req.args[1], span)?;
                let delivered = child
                    .signal(signal)
                    .map_err(|e| undelivered(handle, signal, &e, span))?;
                Ok(HostAnswer::Value(Value::Bool(delivered)))
            }
            Op::Input => {
                let (_, child) = child_of(host, self.op, req)?;
                let bytes = Arc::clone(req.args[1].as_bytes(span, "the bytes to write")?);
                let pending = host.pool.submit(
                    span,
                    "process-input",
                    Op::Input.what(),
                    Box::new(move || JobOutput::Bool(child.input(&bytes))),
                )?;
                Ok(HostAnswer::Pending(pending))
            }
            Op::EndInput => {
                let (_, child) = child_of(host, self.op, req)?;
                child.end_input();
                Ok(HostAnswer::Value(Value::Unit))
            }
            Op::OutputLine => {
                let (handle, child) = child_of(host, self.op, req)?;
                let deadline = deadline(req.args[1].as_int(span, "a timeout")?);
                let pending = host.pool.submit(
                    span,
                    "process-output-line",
                    Op::OutputLine.what(),
                    Box::new(move || match child.next_line(deadline) {
                        Ok(heard) => JobOutput::built(move || heard.value()),
                        Err(refusal) => refused(Op::OutputLine, handle, &child, refusal, span),
                    }),
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

/// The whole environment a child runs under; a later entry for one name wins, as `map_insert`
/// does.
/// `env` and, when a test is observing `machine`, where a `ply` the child is reports what it read.
fn traced(machine: MachineId, mut env: Vec<(String, String)>) -> Vec<(String, String)> {
    if let Some(file) = crate::observe::child_trace(machine) {
        env.push((
            crate::observe::TRACE_VAR.to_string(),
            file.to_string_lossy().into_owned(),
        ));
    }
    env
}

fn environment(op: Op, value: &Value, span: Span) -> Result<Vec<(String, String)>, Diagnostic> {
    let mut out = Vec::new();
    for entry in value.as_list(span, "an environment")?.iter() {
        let Value::Record(fields) = entry else {
            return Err(malformed_argument(
                op,
                &format!("an environment entry that is {}", entry.type_name()),
                span,
            ));
        };
        let (Some(name), Some(setting)) = (
            fields.get(&Symbol::new("name")),
            fields.get(&Symbol::new("value")),
        ) else {
            return Err(malformed_argument(
                op,
                "an environment entry without `name` and `value`",
                span,
            ));
        };
        out.push((
            name.as_str(span, "a variable's name")?.to_string(),
            setting.as_str(span, "a variable's value")?.to_string(),
        ));
    }
    Ok(out)
}

/// A constructor of `std.process` by its simple name, with what it carries.
fn constructor(value: &Value) -> Option<(&str, &[Value])> {
    match value {
        Value::Ctor { name, args } => name
            .as_str()
            .strip_prefix(MODULE)
            .and_then(|rest| rest.strip_prefix('.'))
            .map(|simple| (simple, args.as_slice())),
        _ => None,
    }
}

fn io_of(value: &Value, span: Span) -> Result<Io, Diagnostic> {
    let Value::Record(fields) = value else {
        return Err(malformed_argument(
            Op::Start,
            &format!("an `Io` that is {}", value.type_name()),
            span,
        ));
    };
    let field = |name: &str| {
        fields.get(&Symbol::new(name)).ok_or_else(|| {
            malformed_argument(Op::Start, &format!("an `Io` without `{name}`"), span)
        })
    };
    Ok(Io {
        input: field("input")?.as_bool(span, "whether the child reads input")?,
        out: output_of(field("out")?, span)?,
        err: output_of(field("err")?, span)?,
    })
}

fn output_of(value: &Value, span: Span) -> Result<Output, Diagnostic> {
    Ok(match constructor(value) {
        Some(("Keep", [])) => Output::Keep,
        Some(("Discard", [])) => Output::Discard,
        Some(("Inherit", [])) => Output::Inherit,
        Some(("Lines", [])) => Output::Lines,
        Some(("File", [path])) => Output::File(path.as_str(span, "a file's path")?.to_string()),
        _ => {
            return Err(malformed_argument(
                Op::Start,
                &format!("an `Output` that is {}", slot(0)),
                span,
            )
            .showing(vec![Plain::shown(value)]));
        }
    })
}

fn signal_of(value: &Value, span: Span) -> Result<Signal, Diagnostic> {
    Ok(match constructor(value) {
        Some(("Hangup", [])) => Signal::Hangup,
        Some(("Interrupt", [])) => Signal::Interrupt,
        Some(("Terminate", [])) => Signal::Terminate,
        Some(("Kill", [])) => Signal::Kill,
        Some(("User1", [])) => Signal::User1,
        Some(("User2", [])) => Signal::User2,
        Some(("WindowChange", [])) => Signal::WindowChange,
        _ => {
            return Err(malformed_argument(
                Op::Signal,
                &format!("a `Signal` that is {}", slot(0)),
                span,
            )
            .showing(vec![Plain::shown(value)]));
        }
    })
}

/// The child the first argument names, if this operation's label may reach it.
fn child_of(
    host: &ProcessHost,
    op: Op,
    req: &HostRequest<'_>,
) -> Result<(i64, Arc<Child>), Diagnostic> {
    let handle = req.args[0].as_int(req.span, "a child's handle")?;
    host.children
        .get(handle, &req.atom.resource)
        .map(|child| (handle, child))
        .map_err(|why| unusable(op, handle, &req.atom.resource, why, req.span))
}

/// A negative timeout waits for as long as it takes, and so does one no clock can reach.
fn deadline(ms: i64) -> Option<Instant> {
    let ms = u64::try_from(ms).ok()?;
    Instant::now().checked_add(Duration::from_millis(ms))
}

fn refused(op: Op, handle: i64, child: &Child, refusal: Refusal, span: Span) -> JobOutput {
    match refusal {
        Refusal::Spent => JobOutput::Refused(raced(op, handle, span)),
        Refusal::TooMuch { out, err } => {
            JobOutput::Refused(too_much(op, child.program(), out, err, span))
        }
        Refusal::Unreaped(why) => {
            JobOutput::Failed(format!("child {handle}'s ending could not be read: {why}"))
        }
    }
}

/// One line of this process's standard input, without its ending; `None` at end of input. Read in
/// the pool, because a console waits for a person.
fn read_line() -> JobOutput {
    match stdio::read_line() {
        Ok(line) => JobOutput::MaybeString(line),
        Err(e) => JobOutput::Failed(format!("standard input could not be read: {e}")),
    }
}

/// Whether a write of bytes reached its stream: `false` once the stream's reader has gone away,
/// which the program is told rather than ended by.
fn taken(op: Op, wrote: std::io::Result<()>, span: Span) -> Result<HostAnswer, Diagnostic> {
    match wrote {
        Ok(()) => Ok(HostAnswer::Value(Value::Bool(true))),
        Err(e) if stdio::reader_gone(&e) => Ok(HostAnswer::Value(Value::Bool(false))),
        Err(e) => Err(err_write(op, &e, span)),
    }
}

/// The whole environment is `env`, and `""` is the run's own directory.
fn command(program: &Path, args: &[String], dir: &str, env: &[(String, String)]) -> Command {
    let mut command = Command::new(program);
    command.args(args).env_clear();
    for (name, value) in env {
        command.env(name, value);
    }
    if !dir.is_empty() {
        command.current_dir(dir);
    }
    command
}

/// Each stream arrives whole or not at all; `process.start` is what streams.
fn run_to_end(
    program: &Path,
    args: &[String],
    dir: &str,
    env: &[(String, String)],
    span: Span,
) -> JobOutput {
    match command(program, args, dir, env)
        .stdin(Stdio::null())
        .output()
    {
        Err(e) => JobOutput::Failed(format!("`{}` could not be started: {e}", program.display())),
        Ok(done) => {
            if done.stdout.len() > MAX_CAPTURE_BYTES || done.stderr.len() > MAX_CAPTURE_BYTES {
                return JobOutput::Refused(too_much(
                    Op::Spawn,
                    program,
                    done.stdout.len(),
                    done.stderr.len(),
                    span,
                ));
            }
            let exit = Finished {
                ended: children::ending(&done.status),
                out: done.stdout,
                err: done.stderr,
            };
            JobOutput::built(move || exit.value())
        }
    }
}

#[cold]
pub fn unbound(op: Op, at: &Resource, span: Span) -> Diagnostic {
    let label = match at {
        Resource::Named(name) => name.as_str().to_string(),
        Resource::Var(v) => ply_eval::label_var_name(*v),
        Resource::Singleton => "the singleton resource".to_string(),
        Resource::Every => "every label".to_string(),
    };
    Diagnostic::error(
        codes::PROCESS_EXEC_UNBOUND,
        format!(
            "{} names `{label}`, and no executable is bound to it",
            op.what()
        ),
    )
    .primary(span, format!("`{label}` names no program"))
    .note(format!("bind one beside the run: `--exec {label}=<program>`"))
    .note(format!(
        "a program that can do without it asks `process.bound[{label}]()` before it starts one"
    ))
    .note("the label is the capability: a child process is outside what the effect system can promise, so which program a label may start is named where the run is configured, never in the program")
}

#[cold]
fn too_much(op: Op, program: &Path, out: usize, err: usize, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::PROCESS_OUTPUT_TOO_LARGE,
        format!(
            "`{}` wrote {out} byte(s) to stdout and {err} to stderr",
            program.display()
        ),
    )
    .primary(span, "this is more than a captured stream holds")
    .note(format!(
        "what the host holds of one stream, whole or as lines not yet read, is at most {MAX_CAPTURE_BYTES} bytes"
    ))
    .note(match op {
        Op::Spawn => "`process.start` can send a stream to a `File`, or hand it over a line at a time as `Lines`",
        _ => "read `Lines` as they arrive with `process.output_line`, or send the stream to a `File`",
    })
}

#[cold]
fn unusable(op: Op, handle: i64, at: &Resource, why: Unusable, span: Span) -> Diagnostic {
    match why {
        Unusable::Unknown => Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("{} was given {handle}, and no child has that handle", op.what()),
        )
        .primary(span, "`process.start` never answered this handle")
        .note("handles ascend and are never reused, so a stale one names nothing rather than whatever started next"),
        Unusable::Spent => Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("child {handle} has already been waited on"),
        )
        .primary(span, "this handle is spent")
        .note("`process.wait` hands a child back once it has ended, and its handle names nothing after that"),
        Unusable::Elsewhere(started) => Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!(
                "child {handle} was started as `process.start{started}` and is used as `process.{}{at}`",
                op.name()
            ),
        )
        .primary(span, format!("this operation names `{at}`"))
        .note("a child's label is the executable it was started as, and it keeps that label until it is waited on"),
    }
}

/// Another `process.wait` on the same child answered while this one was waiting.
#[cold]
fn raced(op: Op, handle: i64, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!(
            "child {handle} was waited on while {} was waiting",
            op.what()
        ),
    )
    .primary(span, "another `process.wait` handed this child back first")
    .note("one child is handed back once; a program that waits on it from two tasks races them")
}

#[cold]
fn undelivered(handle: i64, signal: Signal, e: &std::io::Error, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`{signal:?}` could not be delivered to child {handle}: {e}"),
    )
    .primary(span, "the host could not signal this child")
}

#[cold]
fn malformed_argument(op: Op, found: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("{} was given {found}", op.what()),
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
    .primary(span, "the stream refused what was written")
    .note("the descriptor is closed, or what it is open on took nothing")
}

#[cold]
fn negative_read(max: i64, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`process.in_bytes` was asked for {max} bytes"),
    )
    .primary(span, "a read's length cannot be negative")
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
