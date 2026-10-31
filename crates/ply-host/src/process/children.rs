use super::{MAX_CAPTURE_BYTES, OutputSink, Stream};
use crate::native::{int, parts};
use ply_core::Resource;
use std::collections::{BTreeMap, VecDeque};
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStderr, ChildStdin, ChildStdout, ExitStatus, Stdio};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

const CHUNK: usize = 64 * 1024;

const RECHECK: Duration = Duration::from_millis(10);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ended {
    Exited(i64),
    Signalled(i64),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Finished {
    pub ended: Ended,
    pub out: Vec<u8>,
    pub err: Vec<u8>,
}

impl Finished {
    pub fn flat(self) -> Vec<u8> {
        let (kind, n) = match self.ended {
            Ended::Exited(code) => (0, code),
            Ended::Signalled(signal) => (1, signal),
        };
        parts(&[&int(kind)[..], &int(n)[..], &self.out[..], &self.err[..]])
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Heard {
    Said(String),
    Quiet,
    Closed,
}

impl Heard {
    pub fn flat(self) -> Vec<u8> {
        match self {
            Heard::Said(line) => [&[0u8][..], line.as_bytes()].concat(),
            Heard::Quiet => vec![1],
            Heard::Closed => vec![2],
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Output {
    Keep,
    Discard,
    Inherit,
    Lines,
    File(String),
}

impl Output {
    pub fn of(kind: i64, file: &str) -> Option<Output> {
        Some(match kind {
            0 => Output::Keep,
            1 => Output::Discard,
            2 => Output::Inherit,
            3 => Output::Lines,
            4 => Output::File(file.to_string()),
            _ => return None,
        })
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Io {
    pub input: bool,
    pub out: Output,
    pub err: Output,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Signal {
    Hangup,
    Interrupt,
    Terminate,
    Kill,
    User1,
    User2,
    WindowChange,
}

impl Signal {
    pub fn of(number: i64) -> Option<Signal> {
        Some(match number {
            0 => Signal::Hangup,
            1 => Signal::Interrupt,
            2 => Signal::Terminate,
            3 => Signal::Kill,
            4 => Signal::User1,
            5 => Signal::User2,
            6 => Signal::WindowChange,
            _ => return None,
        })
    }
}

pub(super) struct Launch<'a> {
    pub(super) label: &'a Resource,
    pub(super) program: &'a Path,
    pub(super) args: &'a [String],
    pub(super) dir: &'a str,
    pub(super) env: &'a [(String, String)],
    pub(super) io: &'a Io,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) enum Unusable {
    Unknown,
    Spent,
    StartedUnderOtherLabel(Resource),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) enum ProcessError {
    Spent,
    TooMuch { out: usize, err: usize },
    Unreaped(String),
}

pub struct Children {
    table: Mutex<Table>,
}

struct Table {
    open: BTreeMap<i64, Arc<Child>>,
    next: i64,
    ended: bool,
}

impl Default for Children {
    fn default() -> Children {
        Children::new()
    }
}

impl Children {
    pub fn new() -> Children {
        Children {
            table: Mutex::new(Table {
                open: BTreeMap::new(),
                next: 1,
                ended: false,
            }),
        }
    }

    pub(super) fn start(&self, launch: &Launch<'_>, sink: &Arc<OutputSink>) -> Result<i64, String> {
        self.started(launch, sink).map(|(handle, _)| handle)
    }

    pub(super) fn started(
        &self,
        launch: &Launch<'_>,
        sink: &Arc<OutputSink>,
    ) -> Result<(i64, Arc<Child>), String> {
        let (out, err) = destinations(launch, sink)?;
        let mut command = super::command(launch.program, launch.args, launch.dir, launch.env);
        command
            .stdin(if launch.io.input {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(out.stdio)
            .stderr(err.stdio);
        let mut process = command
            .spawn()
            .map_err(|e| format!("`{}` could not be started: {e}", launch.program.display()))?;
        let pid = process.id();
        let stdin = process.stdin.take().map(Arc::new);
        let stdout = process.stdout.take();
        let stderr = process.stderr.take();
        let child = Arc::new(Child {
            label: launch.label.clone(),
            program: launch.program.to_path_buf(),
            pid,
            state: Mutex::new(State {
                process,
                ended: None,
                stdin,
                out: CollectedStream::new(out.collect),
                err: CollectedStream::new(err.collect),
                lines: VecDeque::new(),
                spent: false,
            }),
            changed: Condvar::new(),
        });
        if let Err(e) = attend(&child, stdout, stderr, sink) {
            child.end();
            return Err(format!(
                "`{}` started, and was killed because the host could not start a thread to attend to it: {e}",
                launch.program.display()
            ));
        }
        let mut table = lock(&self.table);
        if table.ended {
            drop(table);
            child.end();
            return Err("the run is ending, so the child was killed as it started".to_string());
        }
        table.open.retain(|_, held| !held.is_waited());
        let handle = table.next;
        table.next += 1;
        table.open.insert(handle, Arc::clone(&child));
        Ok((handle, child))
    }

    pub(super) fn get(&self, handle: i64, at: &Resource) -> Result<Arc<Child>, Unusable> {
        let table = lock(&self.table);
        match table.open.get(&handle) {
            Some(child) if child.is_waited() => Err(Unusable::Spent),
            Some(child) if child.label != *at => {
                Err(Unusable::StartedUnderOtherLabel(child.label.clone()))
            }
            Some(child) => Ok(Arc::clone(child)),
            None if handle > 0 && handle < table.next => Err(Unusable::Spent),
            None => Err(Unusable::Unknown),
        }
    }

    pub fn end_all(&self) {
        let open = {
            let mut table = lock(&self.table);
            table.ended = true;
            std::mem::take(&mut table.open)
        };
        for child in open.values() {
            child.end();
        }
    }
}

impl Drop for Children {
    fn drop(&mut self) {
        self.end_all();
    }
}

pub(super) struct Child {
    label: Resource,
    program: PathBuf,
    pid: u32,
    state: Mutex<State>,
    changed: Condvar,
}

impl Child {
    pub(super) fn program(&self) -> &Path {
        &self.program
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    fn is_waited(&self) -> bool {
        self.lock().spent
    }

    pub(super) fn wait(&self, deadline: Option<Instant>) -> Result<Option<Finished>, ProcessError> {
        let mut state = self.lock();
        loop {
            if state.spent {
                return Err(ProcessError::Spent);
            }
            if let Some(ended) = state.ended.clone()
                && !state.out.open
                && !state.err.open
            {
                state.spent = true;
                let ended = ended.map_err(ProcessError::Unreaped)?;
                if state.out.overflowed || state.err.overflowed {
                    return Err(ProcessError::TooMuch {
                        out: state.out.written,
                        err: state.err.written,
                    });
                }
                let out = state.collected(Stream::Out);
                let err = state.collected(Stream::Err);
                return Ok(Some(Finished { ended, out, err }));
            }
            state = match until(&self.changed, state, deadline) {
                Some(state) => state,
                None => return Ok(None),
            };
        }
    }

    pub(super) fn next_line(&self, deadline: Option<Instant>) -> Result<Heard, ProcessError> {
        let mut state = self.lock();
        loop {
            if state.spent {
                return Ok(Heard::Closed);
            }
            if state.lines_overflowed() {
                return Err(ProcessError::TooMuch {
                    out: state.out.written,
                    err: state.err.written,
                });
            }
            if let Some(line) = state.next_line() {
                return Ok(Heard::Said(line));
            }
            if !state.line_streams_open() {
                return Ok(Heard::Closed);
            }
            state = match until(&self.changed, state, deadline) {
                Some(state) => state,
                None => return Ok(Heard::Quiet),
            };
        }
    }

    pub(super) fn input(&self, bytes: &[u8]) -> bool {
        let pipe = {
            let state = self.lock();
            if state.ended.is_some() {
                return false;
            }
            state.stdin.clone()
        };
        let Some(pipe) = pipe else {
            return false;
        };
        let mut writer = &*pipe;
        writer
            .write_all(bytes)
            .and_then(|()| writer.flush())
            .is_ok()
    }

    pub(super) fn end_input(&self) {
        self.lock().stdin = None;
    }

    pub(super) fn signal(&self, signal: Signal) -> std::io::Result<bool> {
        let mut state = self.lock();
        if state.ended.is_some() {
            return Ok(false);
        }
        if let Some(status) = state.process.try_wait()? {
            state.ended = Some(Ok(ending(&status)));
            drop(state);
            self.changed.notify_all();
            return Ok(false);
        }
        deliver(&mut state.process, self.pid, signal)?;
        Ok(true)
    }

    fn end(&self) {
        let mut state = self.lock();
        state.stdin = None;
        if state.ended.is_none() {
            let reaped = match state.process.kill() {
                Ok(()) => state
                    .process
                    .wait()
                    .map(|status| ending(&status))
                    .map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            state.ended = Some(reaped);
        }
        drop(state);
        self.changed.notify_all();
    }
}

struct State {
    process: std::process::Child,
    ended: Option<Result<Ended, String>>,
    stdin: Option<Arc<ChildStdin>>,
    out: CollectedStream,
    err: CollectedStream,
    lines: VecDeque<(Stream, Vec<u8>)>,
    spent: bool,
}

impl State {
    fn held(&mut self, stream: Stream) -> &mut CollectedStream {
        match stream {
            Stream::Out => &mut self.out,
            Stream::Err => &mut self.err,
        }
    }

    fn take(&mut self, stream: Stream, chunk: &[u8], sink: &OutputSink) {
        let held = self.held(stream);
        held.written += chunk.len();
        match held.collect {
            OutputHandling::Nothing => {}
            OutputHandling::Keep => {
                held.hold(chunk);
            }
            OutputHandling::Lines => {
                let from = held.bytes.len();
                if held.hold(chunk) {
                    let ended = take_complete_lines(&mut held.bytes, from);
                    held.queued += ended.iter().map(Vec::len).sum::<usize>();
                    self.lines
                        .extend(ended.into_iter().map(|line| (stream, line)));
                }
            }
            OutputHandling::Forward(to) => {
                let from = held.bytes.len();
                held.bytes.extend_from_slice(chunk);
                for line in take_complete_lines(&mut held.bytes, from) {
                    forward(sink, to, &line);
                }
            }
        }
    }

    fn close(&mut self, stream: Stream, sink: &OutputSink) {
        let held = self.held(stream);
        held.open = false;
        if held.bytes.is_empty() {
            return;
        }
        match held.collect {
            OutputHandling::Lines => {
                let tail = std::mem::take(&mut held.bytes);
                held.queued += tail.len();
                self.lines.push_back((stream, tail));
            }
            OutputHandling::Forward(to) => forward(sink, to, &std::mem::take(&mut held.bytes)),
            OutputHandling::Nothing | OutputHandling::Keep => {}
        }
    }

    fn next_line(&mut self) -> Option<String> {
        let (stream, line) = self.lines.pop_front()?;
        self.held(stream).queued -= line.len();
        Some(text_of(&line))
    }

    fn line_streams_open(&self) -> bool {
        [&self.out, &self.err]
            .iter()
            .any(|held| held.collect == OutputHandling::Lines && held.open)
    }

    fn lines_overflowed(&self) -> bool {
        [&self.out, &self.err]
            .iter()
            .any(|held| held.collect == OutputHandling::Lines && held.overflowed)
    }

    fn collected(&mut self, stream: Stream) -> Vec<u8> {
        match self.held(stream).collect {
            OutputHandling::Keep => std::mem::take(&mut self.held(stream).bytes),
            OutputHandling::Lines => {
                let mut unread = Vec::new();
                self.lines.retain(|(from, line)| {
                    if *from == stream {
                        unread.extend_from_slice(line);
                        false
                    } else {
                        true
                    }
                });
                self.held(stream).queued = 0;
                unread
            }
            OutputHandling::Nothing | OutputHandling::Forward(_) => Vec::new(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum OutputHandling {
    Nothing,
    Keep,
    Lines,
    Forward(Stream),
}

struct CollectedStream {
    collect: OutputHandling,
    open: bool,
    bytes: Vec<u8>,
    queued: usize,
    written: usize,
    overflowed: bool,
}

impl CollectedStream {
    fn new(collect: OutputHandling) -> CollectedStream {
        CollectedStream {
            collect,
            open: collect != OutputHandling::Nothing,
            bytes: Vec::new(),
            queued: 0,
            written: 0,
            overflowed: false,
        }
    }

    fn hold(&mut self, chunk: &[u8]) -> bool {
        if self.overflowed || self.queued + self.bytes.len() + chunk.len() > MAX_CAPTURE_BYTES {
            self.overflowed = true;
            return false;
        }
        self.bytes.extend_from_slice(chunk);
        true
    }
}

fn take_complete_lines(bytes: &mut Vec<u8>, from: usize) -> Vec<Vec<u8>> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut at = from;
    while let Some(offset) = bytes[at..].iter().position(|b| *b == b'\n') {
        let end = at + offset + 1;
        lines.push(bytes[start..end].to_vec());
        start = end;
        at = end;
    }
    bytes.drain(..start);
    lines
}

fn text_of(line: &[u8]) -> String {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    String::from_utf8_lossy(line).into_owned()
}

fn forward(sink: &OutputSink, to: Stream, line: &[u8]) {
    let _ = sink.line(to, &text_of(line));
}

struct Destination {
    stdio: Stdio,
    collect: OutputHandling,
}

fn destinations(
    launch: &Launch<'_>,
    sink: &OutputSink,
) -> Result<(Destination, Destination), String> {
    if let (Output::File(out), Output::File(err)) = (&launch.io.out, &launch.io.err)
        && out == err
    {
        let file = create(launch.dir, out)?;
        let shared = file
            .try_clone()
            .map_err(|e| format!("`{out}` could not be shared by both streams: {e}"))?;
        return Ok((
            Destination {
                stdio: file.into(),
                collect: OutputHandling::Nothing,
            },
            Destination {
                stdio: shared.into(),
                collect: OutputHandling::Nothing,
            },
        ));
    }
    Ok((
        destination(&launch.io.out, Stream::Out, launch.dir, sink)?,
        destination(&launch.io.err, Stream::Err, launch.dir, sink)?,
    ))
}

fn destination(
    output: &Output,
    stream: Stream,
    dir: &str,
    sink: &OutputSink,
) -> Result<Destination, String> {
    let piped = |collect| Destination {
        stdio: Stdio::piped(),
        collect,
    };
    Ok(match output {
        Output::Keep => piped(OutputHandling::Keep),
        Output::Lines => piped(OutputHandling::Lines),
        Output::Discard => Destination {
            stdio: Stdio::null(),
            collect: OutputHandling::Nothing,
        },
        Output::File(path) => Destination {
            stdio: create(dir, path)?.into(),
            collect: OutputHandling::Nothing,
        },
        Output::Inherit => match sink {
            OutputSink::Real { out } => Destination {
                stdio: inherited(stream, *out),
                collect: OutputHandling::Nothing,
            },
            OutputSink::Captured(_) => piped(OutputHandling::Forward(stream)),
        },
    })
}

fn inherited(stream: Stream, out: Stream) -> Stdio {
    match (stream, out) {
        (Stream::Out, Stream::Err) => std::io::stderr().into(),
        _ => Stdio::inherit(),
    }
}

fn create(dir: &str, path: &str) -> Result<File, String> {
    let at = Path::new(dir).join(path);
    File::create(&at).map_err(|e| {
        format!(
            "`{}` could not be opened for the child's output: {e}",
            at.display()
        )
    })
}

fn attend(
    child: &Arc<Child>,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
    sink: &Arc<OutputSink>,
) -> std::io::Result<()> {
    let watched = Arc::clone(child);
    helper(format!("ply-host-child-{}", child.pid), move || {
        watch(&watched)
    })?;
    if let Some(pipe) = stdout {
        let (child, sink) = (Arc::clone(child), Arc::clone(sink));
        helper(format!("ply-host-child-{}-out", child.pid), move || {
            drain(&child, Stream::Out, pipe, &sink)
        })?;
    }
    if let Some(pipe) = stderr {
        let (child, sink) = (Arc::clone(child), Arc::clone(sink));
        helper(format!("ply-host-child-{}-err", child.pid), move || {
            drain(&child, Stream::Err, pipe, &sink)
        })?;
    }
    Ok(())
}

fn helper(name: String, work: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
    std::thread::Builder::new().name(name).spawn(work).map(drop)
}

fn watch(child: &Child) {
    loop {
        until_ended(child.pid);
        let mut state = child.lock();
        if state.ended.is_some() {
            return;
        }
        match state.process.try_wait() {
            Ok(Some(status)) => state.ended = Some(Ok(ending(&status))),
            Ok(None) => {
                drop(state);
                std::thread::sleep(RECHECK);
                continue;
            }
            Err(e) => state.ended = Some(Err(e.to_string())),
        }
        drop(state);
        child.changed.notify_all();
        return;
    }
}

#[cfg(unix)]
fn until_ended(pid: u32) {
    loop {
        let returned = unsafe {
            let mut info: libc::siginfo_t = std::mem::zeroed();
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if returned == 0 || std::io::Error::last_os_error().kind() != ErrorKind::Interrupted {
            return;
        }
    }
}

#[cfg(not(unix))]
fn until_ended(_: u32) {}

#[cfg(unix)]
fn deliver(_: &mut std::process::Child, pid: u32, signal: Signal) -> std::io::Result<()> {
    let number = match signal {
        Signal::Hangup => libc::SIGHUP,
        Signal::Interrupt => libc::SIGINT,
        Signal::Terminate => libc::SIGTERM,
        Signal::Kill => libc::SIGKILL,
        Signal::User1 => libc::SIGUSR1,
        Signal::User2 => libc::SIGUSR2,
        Signal::WindowChange => libc::SIGWINCH,
    };
    if unsafe { libc::kill(pid as libc::pid_t, number) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
fn deliver(process: &mut std::process::Child, _: u32, signal: Signal) -> std::io::Result<()> {
    match signal {
        Signal::Kill => process.kill(),
        _ => Err(std::io::Error::new(
            ErrorKind::Unsupported,
            "only `Kill` reaches a child on this platform",
        )),
    }
}

fn drain(child: &Child, stream: Stream, mut pipe: impl Read, sink: &OutputSink) {
    let mut chunk = vec![0; CHUNK];
    loop {
        let n = match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        child.lock().take(stream, &chunk[..n], sink);
        child.changed.notify_all();
    }
    child.lock().close(stream, sink);
    child.changed.notify_all();
}

pub(super) fn ending(status: &ExitStatus) -> Ended {
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

fn until<'a>(
    changed: &Condvar,
    state: MutexGuard<'a, State>,
    deadline: Option<Instant>,
) -> Option<MutexGuard<'a, State>> {
    let Some(deadline) = deadline else {
        return Some(changed.wait(state).unwrap_or_else(|e| e.into_inner()));
    };
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return None;
    }
    Some(
        changed
            .wait_timeout(state, left)
            .map(|(state, _)| state)
            .unwrap_or_else(|e| e.into_inner().0),
    )
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
