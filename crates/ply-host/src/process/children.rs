//! Children that run beside the program: started, fed, read, signalled and reaped by handle.

use super::{MAX_CAPTURE_BYTES, OutputSink, Stream};
use crate::pool::{Ended, Finished, Heard};
use ply_eval::Resource;
use std::collections::{BTreeMap, VecDeque};
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStderr, ChildStdin, ChildStdout, ExitStatus, Stdio};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// What one read from a child's pipe takes at most.
const CHUNK: usize = 64 * 1024;

/// How soon a watcher looks again at a child that changed state without ending.
const RECHECK: Duration = Duration::from_millis(10);

/// `std.process.Output`: where one of a child's output streams goes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Output {
    Keep,
    Discard,
    Inherit,
    Lines,
    File(String),
}

/// `std.process.Io`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Io {
    pub input: bool,
    pub out: Output,
    pub err: Output,
}

/// `std.process.Signal`.
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

/// What one `process.start` asked for, with the executable its label resolved to.
pub(super) struct Launch<'a> {
    pub(super) label: &'a Resource,
    pub(super) program: &'a Path,
    pub(super) args: &'a [String],
    pub(super) dir: &'a str,
    pub(super) env: &'a [(String, String)],
    pub(super) io: &'a Io,
}

/// Why a handle names no child the program may use.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) enum Unusable {
    Unknown,
    /// A `wait` has handed the child back.
    Spent,
    /// Started under another label.
    Elsewhere(Resource),
}

/// Why a job on a child answered no value.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) enum Refusal {
    /// Another `wait` handed the child back first.
    Spent,
    /// A stream outgrew the bound: everything the child wrote to each.
    TooMuch { out: usize, err: usize },
    /// The child's ending could not be read.
    Unreaped(String),
}

/// Every child a host has started and the program has not waited on. Dropping it kills and reaps
/// whatever is still running.
pub struct Children {
    table: Mutex<Table>,
}

struct Table {
    open: BTreeMap<i64, Arc<Child>>,
    /// Handles ascend from 1 and are never reused, so a stale one names nothing.
    next: i64,
    /// Set by [`Children::end_all`]: a child started after it is ended at once.
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

    /// The handle of a child now running, or why it could not be started.
    pub(super) fn start(&self, launch: &Launch<'_>, sink: &Arc<OutputSink>) -> Result<i64, String> {
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
        table.open.retain(|_, held| !held.is_spent());
        let handle = table.next;
        table.next += 1;
        table.open.insert(handle, child);
        Ok(handle)
    }

    /// The child `handle` names, if the program may use it under `at`.
    pub(super) fn get(&self, handle: i64, at: &Resource) -> Result<Arc<Child>, Unusable> {
        let table = lock(&self.table);
        match table.open.get(&handle) {
            Some(child) if child.is_spent() => Err(Unusable::Spent),
            Some(child) if child.label != *at => Err(Unusable::Elsewhere(child.label.clone())),
            Some(child) => Ok(Arc::clone(child)),
            None if handle > 0 && handle < table.next => Err(Unusable::Spent),
            None => Err(Unusable::Unknown),
        }
    }

    /// Kills and reaps every child still running; one started afterwards is killed as it starts.
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
    /// Notified when the child is reaped, when a drain takes a chunk, and when a stream ends.
    changed: Condvar,
}

impl Child {
    pub(super) fn program(&self) -> &Path {
        &self.program
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    fn is_spent(&self) -> bool {
        self.lock().spent
    }

    /// Once the child has ended and every stream the host collects has ended too, its ending and
    /// what those streams hold; `None` at the deadline, and no deadline waits for as long as it
    /// takes.
    pub(super) fn wait(&self, deadline: Option<Instant>) -> Result<Option<Finished>, Refusal> {
        let mut state = self.lock();
        loop {
            if state.spent {
                return Err(Refusal::Spent);
            }
            if let Some(ended) = state.ended.clone()
                && !state.out.open
                && !state.err.open
            {
                state.spent = true;
                let ended = ended.map_err(Refusal::Unreaped)?;
                if state.out.overflowed || state.err.overflowed {
                    return Err(Refusal::TooMuch {
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

    /// The next line any `Lines` stream took, `Quiet` at the deadline, and `Closed` once every
    /// `Lines` stream has ended and each of its lines has been answered.
    pub(super) fn next_line(&self, deadline: Option<Instant>) -> Result<Heard, Refusal> {
        let mut state = self.lock();
        loop {
            // A `wait` has handed back whatever was unread.
            if state.spent {
                return Ok(Heard::Closed);
            }
            if state.lines_overflowed() {
                return Err(Refusal::TooMuch {
                    out: state.out.written,
                    err: state.err.written,
                });
            }
            if let Some(line) = state.next_line() {
                return Ok(Heard::Said(line));
            }
            if !state.lines_open() {
                return Ok(Heard::Closed);
            }
            state = match until(&self.changed, state, deadline) {
                Some(state) => state,
                None => return Ok(Heard::Quiet),
            };
        }
    }

    /// Writes every byte to the child's input: `false` once that input is closed, whether by
    /// `end_input`, by the child, or by never having been opened, and once the child has ended.
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

    /// A write still in flight holds the pipe open until it finishes.
    pub(super) fn end_input(&self) {
        self.lock().stdin = None;
    }

    /// Delivers `signal`, answering `false` when the child had already ended.
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

    /// Kills the child if it is still running, and reaps it.
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
    /// Set once the child is reaped, and only under this lock.
    ended: Option<Result<Ended, String>>,
    stdin: Option<Arc<ChildStdin>>,
    out: CollectedStream,
    err: CollectedStream,
    /// Lines the `Lines` streams took and no `output_line` has answered, in the order they arrived.
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
            Collect::Nothing => {}
            Collect::Keep => {
                held.hold(chunk);
            }
            Collect::Lines => {
                let from = held.bytes.len();
                if held.hold(chunk) {
                    let ended = carve(&mut held.bytes, from);
                    held.queued += ended.iter().map(Vec::len).sum::<usize>();
                    self.lines
                        .extend(ended.into_iter().map(|line| (stream, line)));
                }
            }
            Collect::Forward(to) => {
                let from = held.bytes.len();
                held.bytes.extend_from_slice(chunk);
                for line in carve(&mut held.bytes, from) {
                    forward(sink, to, &line);
                }
            }
        }
    }

    /// The last line of a stream need not end in a newline.
    fn close(&mut self, stream: Stream, sink: &OutputSink) {
        let held = self.held(stream);
        held.open = false;
        if held.bytes.is_empty() {
            return;
        }
        match held.collect {
            Collect::Lines => {
                let tail = std::mem::take(&mut held.bytes);
                held.queued += tail.len();
                self.lines.push_back((stream, tail));
            }
            Collect::Forward(to) => forward(sink, to, &std::mem::take(&mut held.bytes)),
            Collect::Nothing | Collect::Keep => {}
        }
    }

    fn next_line(&mut self) -> Option<String> {
        let (stream, line) = self.lines.pop_front()?;
        self.held(stream).queued -= line.len();
        Some(text_of(&line))
    }

    fn lines_open(&self) -> bool {
        [&self.out, &self.err]
            .iter()
            .any(|held| held.collect == Collect::Lines && held.open)
    }

    fn lines_overflowed(&self) -> bool {
        [&self.out, &self.err]
            .iter()
            .any(|held| held.collect == Collect::Lines && held.overflowed)
    }

    /// What a `wait` hands back of one stream: all a `Keep` stream took, or a `Lines` stream's
    /// unread lines.
    fn collected(&mut self, stream: Stream) -> Vec<u8> {
        match self.held(stream).collect {
            Collect::Keep => std::mem::take(&mut self.held(stream).bytes),
            Collect::Lines => {
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
            Collect::Nothing | Collect::Forward(_) => Vec::new(),
        }
    }
}

/// What the host does with one of a child's output streams.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Collect {
    Nothing,
    Keep,
    Lines,
    /// `Inherit` under a captured sink, which takes lines where a real one takes a descriptor.
    Forward(Stream),
}

/// One output stream, as the host collects it.
struct CollectedStream {
    collect: Collect,
    /// Its drain has not reached the end of the stream.
    open: bool,
    /// `Keep`: everything so far. `Lines` and a forwarded stream: the line not yet ended.
    bytes: Vec<u8>,
    /// `Lines`: the bytes of this stream's lines waiting in `State::lines`.
    queued: usize,
    /// Everything the child wrote to the stream.
    written: usize,
    /// The stream outgrew the bound, and nothing more of it is held.
    overflowed: bool,
}

impl CollectedStream {
    fn new(collect: Collect) -> CollectedStream {
        CollectedStream {
            collect,
            open: collect != Collect::Nothing,
            bytes: Vec::new(),
            queued: 0,
            written: 0,
            overflowed: false,
        }
    }

    /// Whether `chunk` fit under the bound; a stream that outgrows it is still drained, so the
    /// child never blocks on a full pipe.
    fn hold(&mut self, chunk: &[u8]) -> bool {
        if self.overflowed || self.queued + self.bytes.len() + chunk.len() > MAX_CAPTURE_BYTES {
            self.overflowed = true;
            return false;
        }
        self.bytes.extend_from_slice(chunk);
        true
    }
}

/// Splits every line `bytes` ends from `from` on, each with its ending; earlier bytes hold none.
fn carve(bytes: &mut Vec<u8>, from: usize) -> Vec<Vec<u8>> {
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

/// A line without its ending; a byte that is not UTF-8 reads as U+FFFD.
fn text_of(line: &[u8]) -> String {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    String::from_utf8_lossy(line).into_owned()
}

/// A sink that refuses a line has nowhere else to put it, and the child is not the one to tell.
fn forward(sink: &OutputSink, to: Stream, line: &[u8]) {
    let _ = sink.line(to, &text_of(line));
}

struct Destination {
    stdio: Stdio,
    collect: Collect,
}

fn destinations(
    launch: &Launch<'_>,
    sink: &OutputSink,
) -> Result<(Destination, Destination), String> {
    // One file named for both streams is opened once, so the two interleave as `2>&1` does.
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
                collect: Collect::Nothing,
            },
            Destination {
                stdio: shared.into(),
                collect: Collect::Nothing,
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
        Output::Keep => piped(Collect::Keep),
        Output::Lines => piped(Collect::Lines),
        Output::Discard => Destination {
            stdio: Stdio::null(),
            collect: Collect::Nothing,
        },
        Output::File(path) => Destination {
            stdio: create(dir, path)?.into(),
            collect: Collect::Nothing,
        },
        Output::Inherit => match sink {
            OutputSink::Real { out } => Destination {
                stdio: inherited(stream, *out),
                collect: Collect::Nothing,
            },
            OutputSink::Captured(_) => piped(Collect::Forward(stream)),
        },
    })
}

/// The program's own stream: stdout is wherever `process.out` goes, which `--json` moves to stderr.
fn inherited(stream: Stream, out: Stream) -> Stdio {
    match (stream, out) {
        (Stream::Out, Stream::Err) => std::io::stderr().into(),
        _ => Stdio::inherit(),
    }
}

/// Relative to the child's directory, where a shell redirection in it would land; truncated.
fn create(dir: &str, path: &str) -> Result<File, String> {
    let at = Path::new(dir).join(path);
    File::create(&at).map_err(|e| {
        format!(
            "`{}` could not be opened for the child's output: {e}",
            at.display()
        )
    })
}

/// A watcher that reaps the child, and a drain for each stream the host collects.
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

/// Reaps the child once it has ended, under the lock `signal` takes, so a signal never reaches a
/// pid the kernel has handed to another process.
fn watch(child: &Child) {
    loop {
        until_ended(child.pid);
        let mut state = child.lock();
        if state.ended.is_some() {
            return;
        }
        match state.process.try_wait() {
            Ok(Some(status)) => state.ended = Some(Ok(ending(&status))),
            // Stopped rather than ended, which macOS's `waitid` reports too.
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

/// Blocks until the child has ended without reaping it.
#[cfg(unix)]
fn until_ended(pid: u32) {
    loop {
        // SAFETY: `info` is plain data the call only writes, and `WNOWAIT` leaves the child unreaped.
        let answered = unsafe {
            let mut info: libc::siginfo_t = std::mem::zeroed();
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if answered == 0 || std::io::Error::last_os_error().kind() != ErrorKind::Interrupted {
            return;
        }
    }
}

/// Nothing waits without reaping here, so the watcher looks every [`RECHECK`].
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
    // SAFETY: a plain system call; the child is unreaped, so its pid names no other process.
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

/// Waits for a change, or answers `None` once `deadline` has passed; no deadline waits forever.
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

/// The guarded state has no invariant a panicking holder can break, so recovering is correct.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
