//! What `std.process`'s host handler asks the host through its natives: the arguments, the two
//! output streams, standard input, the exit code, and the programs a run may start, to completion
//! or beside it. Each result that is structured is flat: a child's ending as [`Finished::flat`]
//! writes it, a line heard as [`Heard::flat`] does.

mod children;

pub(crate) use children::Children;
pub use children::{Ended, Finished, Heard, Io, Output, Signal};

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

/// The effect, by the program-wide name a rejection names its module by.
const EFFECT: &str = "std.process.process";

/// What serves the effect, for a rejection to name.
const SERVED_BY: &str = "std.process::served";

/// The effect whose raise says a standard stream's reader went away, and that raise.
pub const PIPE_EFFECT: &str = "std.process.pipe";
pub const PIPE_BROKEN: &str = "broken";

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

fn malformed(text: &str, reason: &str) -> String {
    format!("`{text}` is not an executable: {reason}; write `--exec NAME=PATH`")
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
                "a spawn would fail at the first call; it is rejected before anything runs instead",
            ));
        }
        self.bound.insert(name.to_string(), resolved);
        Ok(())
    }

    /// The program bound to `label`.
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

/// What an operation of `process` reaches: the run's own process, or a program `--exec` binds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reach {
    Own,
    Executable,
}

pub struct ProcessHost {
    argv: Vec<String>,
    /// Shared with the drains that forward an inheriting child's lines to a captured sink.
    sink: Arc<OutputSink>,
    exit: Mutex<Option<i32>>,
    /// The programs `--exec NAME=PATH` bound; a label outside them is `E0456`.
    executables: Executables,
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

    pub fn executing(self, executables: Executables) -> ProcessHost {
        ProcessHost {
            executables,
            ..self
        }
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

    /// What this host makes of a raise no clause handled: an unhandled `pipe.broken` ends the
    /// process as the closed pipe would have.
    pub fn unhandled(&self, effect: &Symbol, op: &Symbol) {
        if self.owns_the_streams() && effect.as_str() == PIPE_EFFECT && op.as_str() == PIPE_BROKEN {
            self.end_of_broken_pipe();
        }
    }

    pub(crate) fn children(&self) -> &Arc<Children> {
        &self.children
    }

    /// Kills and reaps every child `start` or `spawn` launched that is still running.
    pub fn end_children(&self) {
        self.children.end_all();
    }

    /// The run's own process, which a host that is not one withholds.
    fn own(&self, op: &'static str, span: Span) -> Result<(), Diagnostic> {
        if self.whole {
            Ok(())
        } else {
            Err(withheld(op, Reach::Own, span))
        }
    }

    /// The arguments the run was given.
    ///
    /// # Errors
    /// The host is not the run's own process.
    pub fn args(&self, span: Span) -> Result<&[String], Diagnostic> {
        self.own("args", span)?;
        Ok(&self.argv)
    }

    /// Whether the run bound a program to `label`.
    pub fn bound(&self, label: &str) -> bool {
        self.executables.named(label).is_some()
    }

    /// `text` and a line ending written to `stream`. A stream whose reader has gone ends the
    /// process, since a line returns nothing its writer could be told by.
    ///
    /// # Errors
    /// The host is not the run's own process, or the stream rejected the write.
    pub fn line(&self, stream: Stream, text: &str, span: Span) -> Result<(), Diagnostic> {
        let op = match stream {
            Stream::Out => "out",
            Stream::Err => "err",
        };
        self.own(op, span)?;
        match self.sink.line(stream, text) {
            Ok(()) => Ok(()),
            Err(e) if stdio::reader_gone(&e) => self.end_of_broken_pipe(),
            Err(e) => Err(err_write(op, &e, span)),
        }
    }

    /// Whether `bytes` reached `stream`: `false` once its reader has gone away.
    ///
    /// # Errors
    /// The host is not the run's own process, or the stream rejected the write.
    pub fn write(&self, stream: Stream, bytes: &[u8], span: Span) -> Result<bool, Diagnostic> {
        let op = match stream {
            Stream::Out => "out_bytes",
            Stream::Err => "err_bytes",
        };
        self.own(op, span)?;
        taken(op, self.sink.write(stream, bytes), span)
    }

    /// Whether what standard output's buffer held reached it: `false` once its reader has gone.
    ///
    /// # Errors
    /// The host is not the run's own process, or the stream rejected the write.
    pub fn flush(&self, span: Span) -> Result<bool, Diagnostic> {
        self.own("flush", span)?;
        taken("flush", self.sink.flush(), span)
    }

    /// The job of one line of standard input without its ending, found: none at the end of input.
    ///
    /// # Errors
    /// The host is not the run's own process.
    pub fn read_line(&self, span: Span) -> Result<Job, Diagnostic> {
        self.own("line", span)?;
        Ok(Box::new(|| match stdio::read_line() {
            Ok(line) => Ok(found(line)),
            Err(e) => Err(format!("standard input could not be read: {e}")),
        }))
    }

    /// The job of what comes next on standard input, at most `max` bytes.
    ///
    /// # Errors
    /// The host is not the run's own process.
    pub fn read(&self, max: usize, span: Span) -> Result<Job, Diagnostic> {
        self.own("in_bytes", span)?;
        Ok(Box::new(move || {
            stdio::read(max).map_err(|e| format!("standard input could not be read: {e}"))
        }))
    }

    /// What ends the run with `code`, the first code asked for being the one kept.
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

    /// The job of the program `label` names run to its end with empty input, in `dir` (the run's
    /// own where empty) under the whole environment `env`: how it ended and both its streams, as
    /// [`Finished::flat`] writes them. It is started now and held with the started children, so a
    /// stop or a teardown ends it as it ends them.
    ///
    /// # Errors
    /// No program is bound to `label`.
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
            let (_, child) = started.map_err(Failure::Said)?;
            match child.wait(None) {
                Ok(Some(exit)) => Ok(exit.flat()),
                Err(ProcessError::TooMuch { out, err }) => Err(Failure::Rejected(too_much(
                    "spawn", &program, out, err, span,
                ))),
                Err(ProcessError::Unreaped(reason)) => Err(Failure::Said(format!(
                    "`{}`'s ending could not be read: {reason}",
                    program.display()
                ))),
                Ok(None) | Err(ProcessError::Spent) => Err(Failure::Said(format!(
                    "`{}` was waited on by something other than its spawn",
                    program.display()
                ))),
            }
        }))
    }

    /// A child of the program `label` names, started beside the program and wired as `io` says:
    /// its handle, or why it could not start.
    ///
    /// # Errors
    /// No program is bound to `label`.
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
        // What the program wrote before the child comes before what the child writes.
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

    /// The child `handle` names, where `op` under `label` may reach it.
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

    /// The job of how the child ended and what its streams hold, found once it has: none while it
    /// runs past `timeout_ms`, which waits for as long as it takes where negative.
    ///
    /// # Errors
    /// The handle names no child `label` may reach.
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

    /// Whether `signal` reached the child: `false` where it had already ended.
    ///
    /// # Errors
    /// The handle names no child `label` may reach, or the signal could not be delivered.
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

    /// The job of `bytes` written to the child's input: byte 1 where every byte was, 0 once its
    /// input is closed or it has ended.
    ///
    /// # Errors
    /// The handle names no child `label` may reach.
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

    /// The child's input closed.
    ///
    /// # Errors
    /// The handle names no child `label` may reach.
    pub fn end_input(&self, label: &str, handle: i64, span: Span) -> Result<(), Diagnostic> {
        self.child("end_input", label, handle, span)?.end_input();
        Ok(())
    }

    /// The job of the next line any of the child's `Lines` streams wrote, as [`Heard::flat`]
    /// writes it, waiting at most `timeout_ms` as [`ProcessHost::wait`] does.
    ///
    /// # Errors
    /// The handle names no child `label` may reach.
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

/// Where an operation about the run's own process is served, for a rejection to say.
const ONLY_A_RUN: &str = "under `ply run`, and `ply test` withholds it: a test is no process";

/// Where an operation on a program `--exec` binds is served.
const AN_EXECUTABLE: &str = "for a label `--exec` binds to a program";

/// `process.<op>` rejected in a run whose host serves none of what `reach` names.
#[cold]
pub fn withheld(op: &'static str, reach: Reach, span: Span) -> Diagnostic {
    let served = match reach {
        Reach::Own => ONLY_A_RUN,
        Reach::Executable => AN_EXECUTABLE,
    };
    ply_core::host::err_withheld(
        span,
        &format!("process.{op}"),
        &Symbol::new(EFFECT),
        SERVED_BY,
        served,
    )
}

/// A negative timeout waits for as long as it takes, and so does one no clock can reach.
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
            Failure::Rejected(too_much(op, child.program(), out, err, span))
        }
        ProcessError::Unreaped(reason) => Failure::Said(format!(
            "child {handle}'s ending could not be read: {reason}"
        )),
    }
}

/// Whether a write of bytes reached its stream: `false` once the stream's reader has gone away,
/// which the program is told rather than ended by.
fn taken(op: &'static str, wrote: std::io::Result<()>, span: Span) -> Result<bool, Diagnostic> {
    match wrote {
        Ok(()) => Ok(true),
        Err(e) if stdio::reader_gone(&e) => Ok(false),
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
fn too_much(op: &str, program: &Path, out: usize, err: usize, span: Span) -> Diagnostic {
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
        Unusable::Elsewhere(started) => Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!(
                "child {handle} was started as `process.start{started}` and is used as `process.{op}{at}`"
            ),
        )
        .primary(span, format!("this operation names `{at}`"))
        .note("a child's label is the executable it was started as, and it keeps that label until it is waited on"),
    }
}

/// Another `process.wait` on the same child returned while this one was waiting.
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

/// The unwinding the machine does on any diagnostic is what makes `exit` end the program.
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

/// The guarded state has no invariant a panicking caller can break, so recovering is correct.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
