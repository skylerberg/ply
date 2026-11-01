mod children;

pub(crate) use children::Children;
pub use children::{Ended, Finished, Io, Output, OutputEvent, Signal};

use crate::jobs::{Failure, FallibleJob, Job};
use crate::native::found;
use crate::stdio;
use children::{Child, Launch, ProcessError, Unusable};
use ply_core::{Diagnostic, Resource, Span, Symbol, codes};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

const EFFECT: &str = "std.process.process";

const HANDLER_FUNCTION: &str = "std.process::host_process";

pub const PIPE_EFFECT: &str = "std.process.pipe";
pub const PIPE_BROKEN: &str = "broken";

pub const MAX_CAPTURE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stream {
    Out,
    Err,
}

pub enum OutputSink {
    Real { out: Stream },
    Captured(Mutex<Vec<(Stream, Vec<u8>)>>),
}

impl OutputSink {
    pub fn captured() -> OutputSink {
        OutputSink::Captured(Mutex::new(Vec::new()))
    }

    fn is_uncaptured(&self) -> bool {
        matches!(self, OutputSink::Real { .. })
    }

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

    fn flush(&self) -> std::io::Result<()> {
        match self {
            OutputSink::Real { out: Stream::Out } => stdio::flush_out(),
            OutputSink::Real { out: Stream::Err } | OutputSink::Captured(_) => Ok(()),
        }
    }

    fn make_way(&self) {
        if self.is_uncaptured() {
            stdio::make_way();
        }
    }
}

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

fn malformed(text: &str, reason: &str) -> String {
    format!("`{text}` is not an executable: {reason}; write `--exec NAME=PATH`")
}

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
                "a spawn would fail at the first call; it is rejected before anything runs instead",
            ));
        }
        self.bound.insert(name.to_string(), resolved);
        Ok(())
    }

    pub fn named(&self, label: &str) -> Option<&Path> {
        self.bound.get(label).map(PathBuf::as_path)
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OperationTarget {
    OwnProcess,
    Executable,
}

pub struct ProcessHost {
    argv: Vec<String>,
    sink: Arc<OutputSink>,
    exit: Mutex<Option<i32>>,
    executables: Executables,
    children: Arc<Children>,
    whole: bool,
}

impl ProcessHost {
    pub fn new(argv: Vec<String>, sink: OutputSink) -> ProcessHost {
        ProcessHost {
            argv,
            sink: Arc::new(sink),
            exit: Mutex::new(None),
            executables: Executables::new(),
            children: Arc::new(Children::new()),
            whole: true,
        }
    }

    pub fn spawning(executables: Executables) -> ProcessHost {
        ProcessHost {
            whole: false,
            ..ProcessHost::new(Vec::new(), OutputSink::captured())
        }
        .executing(executables)
    }

    pub fn executing(self, executables: Executables) -> ProcessHost {
        ProcessHost {
            executables,
            ..self
        }
    }

    pub fn requested_exit(&self) -> Option<i32> {
        *lock(&self.exit)
    }

    pub fn captured(&self) -> Vec<(Stream, Vec<u8>)> {
        match &*self.sink {
            OutputSink::Captured(writes) => lock(writes).clone(),
            OutputSink::Real { .. } => Vec::new(),
        }
    }

    fn controls_stdio(&self) -> bool {
        self.whole && self.sink.is_uncaptured()
    }

    pub fn flush_output(&self) {
        if self.controls_stdio() && stdio::flush_output() {
            self.exit_on_broken_pipe();
        }
    }

    pub fn exit_on_broken_pipe(&self) -> ! {
        self.children.end_all();
        stdio::flush_output();
        crate::observe::report_at_exit();
        std::process::exit(stdio::BROKEN_PIPE_STATUS);
    }

    pub fn unhandled(&self, effect: &Symbol, op: &Symbol) {
        if self.controls_stdio() && effect.as_str() == PIPE_EFFECT && op.as_str() == PIPE_BROKEN {
            self.exit_on_broken_pipe();
        }
    }

    pub(crate) fn children(&self) -> &Arc<Children> {
        &self.children
    }

    pub fn end_children(&self) {
        self.children.end_all();
    }

    fn own(&self, op: &'static str, span: Span) -> Result<(), Diagnostic> {
        if self.whole {
            Ok(())
        } else {
            Err(withheld(op, OperationTarget::OwnProcess, span))
        }
    }

    pub fn args(&self, span: Span) -> Result<&[String], Diagnostic> {
        self.own("args", span)?;
        Ok(&self.argv)
    }

    pub fn bound(&self, label: &str) -> bool {
        self.executables.named(label).is_some()
    }

    pub fn line(&self, stream: Stream, text: &str, span: Span) -> Result<(), Diagnostic> {
        let op = match stream {
            Stream::Out => "out",
            Stream::Err => "err",
        };
        self.own(op, span)?;
        match self.sink.line(stream, text) {
            Ok(()) => Ok(()),
            Err(e) if stdio::reader_gone(&e) => self.exit_on_broken_pipe(),
            Err(e) => Err(err_write(op, &e, span)),
        }
    }

    pub fn write(&self, stream: Stream, bytes: &[u8], span: Span) -> Result<bool, Diagnostic> {
        let op = match stream {
            Stream::Out => "out_bytes",
            Stream::Err => "err_bytes",
        };
        self.own(op, span)?;
        taken(op, self.sink.write(stream, bytes), span)
    }

    pub fn flush(&self, span: Span) -> Result<bool, Diagnostic> {
        self.own("flush", span)?;
        taken("flush", self.sink.flush(), span)
    }

    pub fn read_line(&self, span: Span) -> Result<Job, Diagnostic> {
        self.own("line", span)?;
        Ok(Box::new(|| match stdio::read_line() {
            Ok(line) => Ok(found(line)),
            Err(e) => Err(format!("standard input could not be read: {e}")),
        }))
    }

    pub fn read(&self, max: usize, span: Span) -> Result<Job, Diagnostic> {
        self.own("in_bytes", span)?;
        Ok(Box::new(move || {
            stdio::read(max).map_err(|e| format!("standard input could not be read: {e}"))
        }))
    }

    pub fn exit(&self, code: i32, span: Span) -> Diagnostic {
        if let Err(withheld) = self.own("exit", span) {
            return withheld;
        }
        let mut requested = lock(&self.exit);
        if requested.is_none() {
            *requested = Some(code);
        }
        exit_requested(code, span)
    }

    fn program(&self, op: &'static str, label: &str, span: Span) -> Result<&Path, Diagnostic> {
        self.executables
            .named(label)
            .ok_or_else(|| unbound(op, label, span))
    }

    pub fn spawn(
        &self,
        label: &str,
        args: &[String],
        dir: &str,
        env: &[(String, String)],
        span: Span,
    ) -> Result<FallibleJob, Diagnostic> {
        let program = self.program("spawn", label, span)?;
        let io = Io {
            input: false,
            out: Output::Keep,
            err: Output::Keep,
        };
        let at = Resource::Named(Symbol::new(label));
        let launch = Launch {
            label: &at,
            program,
            args,
            dir,
            env,
            io: &io,
        };
        let started = self.children.started(&launch, &self.sink);
        let program = program.to_path_buf();
        Ok(Box::new(move || {
            let (_, child) = started.map_err(Failure::Message)?;
            match child.wait(None) {
                Ok(Some(exit)) => Ok(exit.flat()),
                Err(ProcessError::TooMuch { out, err }) => Err(Failure::Rejected(
                    err_output_over_limit("spawn", &program, out, err, span),
                )),
                Err(ProcessError::Unreaped(reason)) => Err(Failure::Message(format!(
                    "`{}`'s ending could not be read: {reason}",
                    program.display()
                ))),
                Ok(None) | Err(ProcessError::Spent) => Err(Failure::Message(format!(
                    "`{}` was waited on by something other than its spawn",
                    program.display()
                ))),
            }
        }))
    }

    pub fn start(
        &self,
        label: &str,
        args: &[String],
        dir: &str,
        env: &[(String, String)],
        io: &Io,
        span: Span,
    ) -> Result<Result<i64, String>, Diagnostic> {
        let program = self.program("start", label, span)?;
        if io.out == Output::Inherit || io.err == Output::Inherit {
            self.sink.make_way();
        }
        let label = Resource::Named(Symbol::new(label));
        let launch = Launch {
            label: &label,
            program,
            args,
            dir,
            env,
            io,
        };
        Ok(self.children.start(&launch, &self.sink))
    }

    fn child(
        &self,
        op: &'static str,
        label: &str,
        handle: i64,
        span: Span,
    ) -> Result<Arc<Child>, Diagnostic> {
        let at = Resource::Named(Symbol::new(label));
        self.children
            .get(handle, &at)
            .map_err(|reason| unusable(op, handle, &at, reason, span))
    }

    pub fn wait(
        &self,
        label: &str,
        handle: i64,
        timeout_ms: i64,
        span: Span,
    ) -> Result<FallibleJob, Diagnostic> {
        let child = self.child("wait", label, handle, span)?;
        let deadline = deadline(timeout_ms);
        Ok(Box::new(move || match child.wait(deadline) {
            Ok(exit) => Ok(found(exit.map(Finished::flat))),
            Err(rejection) => Err(rejected("wait", handle, &child, rejection, span)),
        }))
    }

    pub fn signal(
        &self,
        label: &str,
        handle: i64,
        signal: Signal,
        span: Span,
    ) -> Result<bool, Diagnostic> {
        self.child("signal", label, handle, span)?
            .signal(signal)
            .map_err(|e| undelivered(handle, signal, &e, span))
    }

    pub fn input(
        &self,
        label: &str,
        handle: i64,
        bytes: Vec<u8>,
        span: Span,
    ) -> Result<Job, Diagnostic> {
        let child = self.child("input", label, handle, span)?;
        Ok(Box::new(move || Ok(vec![u8::from(child.input(&bytes))])))
    }

    pub fn end_input(&self, label: &str, handle: i64, span: Span) -> Result<(), Diagnostic> {
        self.child("end_input", label, handle, span)?.end_input();
        Ok(())
    }

    pub fn output_line(
        &self,
        label: &str,
        handle: i64,
        timeout_ms: i64,
        span: Span,
    ) -> Result<FallibleJob, Diagnostic> {
        let child = self.child("output_line", label, handle, span)?;
        let deadline = deadline(timeout_ms);
        Ok(Box::new(move || match child.next_line(deadline) {
            Ok(heard) => Ok(heard.flat()),
            Err(rejection) => Err(rejected("output_line", handle, &child, rejection, span)),
        }))
    }
}

const RUN_ONLY_HINT: &str = "under `ply run`, and `ply test` withholds it: a test is no process";

const EXEC_LABEL_HINT: &str = "for a label `--exec` binds to a program";

#[cold]
pub fn withheld(op: &'static str, reach: OperationTarget, span: Span) -> Diagnostic {
    let served = match reach {
        OperationTarget::OwnProcess => RUN_ONLY_HINT,
        OperationTarget::Executable => EXEC_LABEL_HINT,
    };
    ply_core::host::err_unbound_operation(
        span,
        &format!("process.{op}"),
        &Symbol::new(EFFECT),
        HANDLER_FUNCTION,
        served,
    )
}

fn deadline(ms: i64) -> Option<Instant> {
    let ms = u64::try_from(ms).ok()?;
    Instant::now().checked_add(Duration::from_millis(ms))
}

fn rejected(
    op: &'static str,
    handle: i64,
    child: &Child,
    rejection: ProcessError,
    span: Span,
) -> Failure {
    match rejection {
        ProcessError::Spent => Failure::Rejected(raced(op, handle, span)),
        ProcessError::TooMuch { out, err } => {
            Failure::Rejected(err_output_over_limit(op, child.program(), out, err, span))
        }
        ProcessError::Unreaped(reason) => Failure::Message(format!(
            "child {handle}'s ending could not be read: {reason}"
        )),
    }
}

fn taken(op: &'static str, wrote: std::io::Result<()>, span: Span) -> Result<bool, Diagnostic> {
    match wrote {
        Ok(()) => Ok(true),
        Err(e) if stdio::reader_gone(&e) => Ok(false),
        Err(e) => Err(err_write(op, &e, span)),
    }
}

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

#[cold]
pub fn unbound(op: &str, label: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::PROCESS_EXEC_UNBOUND,
        format!("`process.{op}` names `{label}`, and no executable is bound to it"),
    )
    .primary(span, format!("`{label}` names no program"))
    .note(format!("bind one beside the run: `--exec {label}=<program>`"))
    .note(format!(
        "a program that can do without it asks `process.bound[{label}]()` before it starts one"
    ))
    .note("the label is the capability: a child process is outside what the effect system can promise, so which program a label may start is named where the run is configured, never in the program")
}

#[cold]
fn err_output_over_limit(
    op: &str,
    program: &Path,
    out: usize,
    err: usize,
    span: Span,
) -> Diagnostic {
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
        "spawn" => "`process.start` can send a stream to a `File`, or hand it over a line at a time as `Lines`",
        _ => "read `Lines` as they arrive with `process.output_line`, or send the stream to a `File`",
    })
}

#[cold]
fn unusable(op: &str, handle: i64, at: &Resource, reason: Unusable, span: Span) -> Diagnostic {
    match reason {
        Unusable::Unknown => Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("`process.{op}` was given {handle}, and no child has that handle"),
        )
        .primary(span, "`process.start` never returned this handle")
        .note("handles ascend and are never reused, so a stale one names nothing rather than whatever started next"),
        Unusable::Spent => Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("child {handle} has already been waited on"),
        )
        .primary(span, "this handle is spent")
        .note("`process.wait` hands a child back once it has ended, and its handle names nothing after that"),
        Unusable::StartedUnderOtherLabel(started) => Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!(
                "child {handle} was started as `process.start{started}` and is used as `process.{op}{at}`"
            ),
        )
        .primary(span, format!("this operation names `{at}`"))
        .note("a child's label is the executable it was started as, and it keeps that label until it is waited on"),
    }
}

#[cold]
fn raced(op: &str, handle: i64, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("child {handle} was waited on while `process.{op}` was waiting"),
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
fn exit_requested(code: i32, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::PROCESS_EXIT,
        format!("the program asked to exit with code {code}"),
    )
    .primary(span, "nothing after this perform runs")
    .note("`ply run` exits with this code once the host has torn down, and prints no value")
}

#[cold]
fn err_write(op: &str, e: &std::io::Error, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`process.{op}` could not write: {e}"),
    )
    .primary(span, "the stream rejected what was written")
    .note("the descriptor is closed, or what it is open on took nothing")
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
