//! Where a host operation goes when it has to wait.
//! One [`Pool`] per facility, minting in disjoint token ranges.

use ply_eval::{Diagnostic, Pending, Span, Symbol, Value, codes};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

/// How many operations of a pool that gives each a thread may be outstanding at once.
pub const MAX_BLOCKING_OPERATIONS: usize = 64;

pub const NET_FIRST_TOKEN: u64 = 1;

/// Far enough above [`NET_FIRST_TOKEN`] that the two ranges never meet.
pub const FS_FIRST_TOKEN: u64 = 1 << 62;

/// Far enough above [`FS_FIRST_TOKEN`] that the two ranges never meet.
pub const PROCESS_FIRST_TOKEN: u64 = 1 << 63;

/// As far above [`PROCESS_FIRST_TOKEN`] as that is above [`FS_FIRST_TOKEN`].
pub const PASSWORD_FIRST_TOKEN: u64 = 3 << 62;

/// How a spawned process ended: its own code, or the signal that killed it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ended {
    Exited(i64),
    Signalled(i64),
}

/// A `std.process.Finished`: how a child ended and what it left in each stream.
pub struct Finished {
    pub ended: Ended,
    pub out: Vec<u8>,
    pub err: Vec<u8>,
}

/// A `std.process.Heard`: what `process.output_line` answers.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Heard {
    Said(String),
    Quiet,
    Closed,
}

pub enum JobOutput {
    Int(i64),
    /// Whether a write happened; a filesystem's state is not the program's error.
    Bool(bool),
    Bytes(Vec<u8>),
    MaybeBytes(Option<Vec<u8>>),
    MaybeInt(Option<i64>),
    /// `None` when it is not a directory this run can read.
    MaybeStrings(Option<Vec<String>>),
    /// `None` at end of input, which is what reading a line past the last one answers.
    MaybeString(Option<String>),
    /// A walk's entries, each a path and the kind constructor it names; `None` for no directory.
    MaybeEntries(Option<Vec<(String, &'static str)>>),
    /// A path's nine permission bits, as `std.fs.Mode` holds them.
    MaybeMode(Option<u32>),
    /// A constructor with no fields, by the program-wide name the declaring module gives it.
    Ctor(&'static str),
    /// A file's descriptor, or the `std.fs.Refused` that says why it did not open; and whether
    /// it was opened to be written.
    Opened(Result<i64, &'static str>, bool),
    Finished(Finished),
    /// `None` when the child was still running at the deadline.
    MaybeFinished(Option<Finished>),
    Heard(Heard),
    /// The operation failed in a way that is neither the peer's doing nor a deadline.
    Failed(String),
    Refused(Diagnostic),
}

type Job = Box<dyn FnOnce() -> JobOutput + Send + 'static>;

/// Rung by every pool it is handed to whenever one of their operations finishes, and by a stop, so
/// one wait can cover them all: separate condition variables cannot be waited on together.
#[derive(Default)]
pub struct Bell {
    rung: Mutex<u64>,
    heard: Condvar,
}

impl Bell {
    /// How many times it has rung; read before looking for work, so a ring after the look is
    /// never missed.
    pub fn rung(&self) -> u64 {
        *lock(&self.rung)
    }

    /// Blocks until it has rung more than `seen` times.
    pub fn wait_past(&self, seen: u64) {
        let mut rung = lock(&self.rung);
        while *rung == seen {
            rung = wait(&self.heard, rung);
        }
    }

    /// The same, for at most `bound`.
    pub fn wait_past_for(&self, seen: u64, bound: Duration) {
        let rung = lock(&self.rung);
        if *rung == seen {
            drop(wait_timeout(&self.heard, rung, bound));
        }
    }

    pub(crate) fn ring(&self) {
        *lock(&self.rung) += 1;
        self.heard.notify_all();
    }
}

/// One runtime's watched tokens as they resolve, so it collects what resolved, not all it awaits.
#[derive(Default)]
pub struct Inbox {
    tokens: Mutex<Vec<u64>>,
}

impl Inbox {
    fn deliver(&self, token: u64) {
        lock(&self.tokens).push(token);
    }

    fn take(&self) -> Vec<u64> {
        std::mem::take(&mut *lock(&self.tokens))
    }
}

struct Waiting {
    span: Span,
    what: &'static str,
    /// Where the token goes once it resolves, once a runtime watches it.
    inbox: Option<Arc<Inbox>>,
}

#[derive(Default)]
struct State {
    waiting: HashMap<u64, Waiting>,
    done: HashMap<u64, JobOutput>,
}

struct Shared {
    state: Mutex<State>,
    /// Signalled by a job finishing, so neither `park` nor `block_on` spins.
    finished: Condvar,
    next: AtomicU64,
    bell: OnceLock<Arc<Bell>>,
    /// Where jobs wait for a thread; `None` for a pool that starts one for each.
    queue: Option<Queue>,
}

/// The jobs of a pool that runs them on a fixed number of threads, in the order submitted.
struct Queue {
    line: Mutex<Line>,
    posted: Condvar,
    threads: usize,
}

#[derive(Default)]
struct Line {
    jobs: VecDeque<(u64, Job)>,
    started: usize,
    /// The started threads that are waiting for a job.
    idle: usize,
    /// The pool is gone: a thread that finds no job ends.
    closed: bool,
}

/// Not `Clone`: the handler owns it, and jobs hold an [`Arc`] of the shared state.
pub struct Pool {
    shared: Arc<Shared>,
}

impl Drop for Pool {
    fn drop(&mut self) {
        if let Some(queue) = &self.shared.queue {
            lock(&queue.line).closed = true;
            queue.posted.notify_all();
        }
    }
}

impl Pool {
    /// A pool that starts a thread for each operation, [`MAX_BLOCKING_OPERATIONS`] at most.
    /// `first` starts this pool's token range, which must not overlap another pool's.
    pub fn new(first: u64) -> Pool {
        Pool::over(first, None)
    }

    /// A pool whose operations wait their turn for one of `threads` threads, however many are
    /// submitted: for work that fills a core, where a thread each would finish none sooner and a
    /// bound on how many wait would refuse a burst. A thread is started when an operation first
    /// needs one, and ends when the pool is dropped.
    pub fn queued(first: u64, threads: usize) -> Pool {
        Pool::over(
            first,
            Some(Queue {
                line: Mutex::new(Line::default()),
                posted: Condvar::new(),
                threads: threads.max(1),
            }),
        )
    }

    fn over(first: u64, queue: Option<Queue>) -> Pool {
        Pool {
            shared: Arc::new(Shared {
                state: Mutex::new(State::default()),
                finished: Condvar::new(),
                // Token 0 is never minted, so a zeroed `Pending` belongs to no pool.
                next: AtomicU64::new(first),
                bell: OnceLock::new(),
                queue,
            }),
        }
    }

    /// Rings `bell` too whenever an operation finishes; a pool rings one bell at most.
    pub fn ring(&self, bell: &Arc<Bell>) {
        let _ = self.shared.bell.set(Arc::clone(bell));
    }

    /// Whether an operation has finished and is waiting to be polled.
    pub fn ready(&self) -> bool {
        !lock(&self.shared.state).done.is_empty()
    }

    pub fn submit(
        &self,
        span: Span,
        label: &'static str,
        what: &'static str,
        job: Job,
    ) -> Result<Pending, Diagnostic> {
        let token = self.shared.next.fetch_add(1, Ordering::Relaxed);
        if let Some(queue) = &self.shared.queue {
            return self.enqueue(queue, token, span, label, what, job);
        }
        {
            let mut state = lock(&self.shared.state);
            if state.waiting.len() >= MAX_BLOCKING_OPERATIONS {
                return Err(Diagnostic::error(
                    codes::RUNTIME_ERROR,
                    format!(
                        "{what} would be the {}th host operation waiting at once",
                        MAX_BLOCKING_OPERATIONS + 1
                    ),
                )
                .primary(span, "no thread left to wait on this")
                .note(format!(
                    "the host's blocking pool is bounded at {MAX_BLOCKING_OPERATIONS} outstanding operations"
                ))
                .note("W1 has no cancellation, so an operation that never completes holds its thread until the run ends"));
            }
            state.waiting.insert(
                token,
                Waiting {
                    span,
                    what,
                    inbox: None,
                },
            );
        }

        let shared = Arc::clone(&self.shared);
        let spawned = std::thread::Builder::new()
            .name(format!("ply-host-{label}-{token}"))
            .spawn(move || complete(&shared, token, job()));

        if let Err(e) = spawned {
            lock(&self.shared.state).waiting.remove(&token);
            return Err(err_no_thread(what, &e, span));
        }
        Ok(Pending { token, label })
    }

    /// Puts `job` in line, and starts a thread for it if every started one is busy and the pool
    /// may have another.
    fn enqueue(
        &self,
        queue: &Queue,
        token: u64,
        span: Span,
        label: &'static str,
        what: &'static str,
        job: Job,
    ) -> Result<Pending, Diagnostic> {
        lock(&self.shared.state).waiting.insert(
            token,
            Waiting {
                span,
                what,
                inbox: None,
            },
        );
        let mut line = lock(&queue.line);
        line.jobs.push_back((token, job));
        if line.jobs.len() > line.idle && line.started < queue.threads {
            let shared = Arc::clone(&self.shared);
            let spawned = std::thread::Builder::new()
                .name(format!("ply-host-{label}-{}", line.started))
                .spawn(move || work(&shared));
            match spawned {
                Ok(_) => line.started += 1,
                // A thread that is running takes this in its turn.
                Err(_) if line.started > 0 => {}
                Err(e) => {
                    line.jobs.pop_back();
                    drop(line);
                    lock(&self.shared.state).waiting.remove(&token);
                    return Err(err_no_thread(what, &e, span));
                }
            }
        }
        drop(line);
        queue.posted.notify_one();
        Ok(Pending { token, label })
    }

    pub fn owns(&self, pending: &Pending) -> bool {
        let state = lock(&self.shared.state);
        state.waiting.contains_key(&pending.token) || state.done.contains_key(&pending.token)
    }

    /// Delivers `pending` to `inbox` once it resolves, or now if it has.
    pub fn watch(&self, pending: &Pending, inbox: &Arc<Inbox>) -> Result<(), Diagnostic> {
        let mut state = lock(&self.shared.state);
        if state.done.contains_key(&pending.token) {
            inbox.deliver(pending.token);
            return Ok(());
        }
        match state.waiting.get_mut(&pending.token) {
            Some(waiting) => {
                waiting.inbox = Some(Arc::clone(inbox));
                Ok(())
            }
            None => Err(unknown_token(pending)),
        }
    }

    /// The answers of the tokens this pool delivered to `inbox` since it was last collected.
    pub fn collect(&self, inbox: &Inbox) -> Vec<(u64, Result<Value, Diagnostic>)> {
        let tokens = inbox.take();
        if tokens.is_empty() {
            return Vec::new();
        }
        let mut state = lock(&self.shared.state);
        tokens
            .into_iter()
            .filter_map(|token| match take(&mut state, token) {
                Taken::Ready(result) => Some((token, result)),
                Taken::Waiting | Taken::Unknown => None,
            })
            .collect()
    }

    pub fn poll(&self, pending: &Pending) -> Result<Option<Value>, Diagnostic> {
        let mut state = lock(&self.shared.state);
        match take(&mut state, pending.token) {
            Taken::Ready(result) => result.map(Some),
            Taken::Waiting => Ok(None),
            Taken::Unknown => Err(unknown_token(pending)),
        }
    }

    /// Block until at least one outstanding operation has finished.
    pub fn park(&self) -> Result<(), Diagnostic> {
        let mut state = lock(&self.shared.state);
        if state.waiting.is_empty() && state.done.is_empty() {
            return Err(Diagnostic::error(
                codes::INTERNAL_ERROR,
                "the host runtime was asked to wait with no operation outstanding",
            )
            .note("nothing would ever wake it; this is a scheduler bug rather than a fault in the program"));
        }
        while state.done.is_empty() {
            state = wait(&self.shared.finished, state);
        }
        Ok(())
    }

    /// The same, for at most `bound`, and `Ok` whether or not anything resolved.
    pub fn park_until(&self, bound: Duration) -> Result<(), Diagnostic> {
        let state = lock(&self.shared.state);
        if state.done.is_empty() {
            drop(wait_timeout(&self.shared.finished, state, bound));
        }
        Ok(())
    }

    pub fn block_on(&self, pending: Pending) -> Result<Value, Diagnostic> {
        let mut state = lock(&self.shared.state);
        loop {
            match take(&mut state, pending.token) {
                Taken::Ready(result) => return result,
                Taken::Unknown => return Err(unknown_token(&pending)),
                // A wake for another token re-checks and waits again.
                Taken::Waiting => state = wait(&self.shared.finished, state),
            }
        }
    }

    pub fn outstanding(&self) -> usize {
        let state = lock(&self.shared.state);
        state.waiting.len()
    }
}

/// Files what a job answered and wakes whoever waits on it.
fn complete(shared: &Shared, token: u64, outcome: JobOutput) {
    let mut state = lock(&shared.state);
    state.done.insert(token, outcome);
    let inbox = state
        .waiting
        .get(&token)
        .and_then(|waiting| waiting.inbox.clone());
    drop(state);
    // Delivered before the wake, so a runtime that wakes finds its token.
    if let Some(inbox) = inbox {
        inbox.deliver(token);
    }
    shared.finished.notify_all();
    if let Some(bell) = shared.bell.get() {
        bell.ring();
    }
}

/// One thread of a queued pool: the next job in line, until the pool is gone and the line empty.
fn work(shared: &Shared) {
    let Some(queue) = &shared.queue else {
        return;
    };
    loop {
        let (token, job) = {
            let mut line = lock(&queue.line);
            loop {
                if let Some(next) = line.jobs.pop_front() {
                    break next;
                }
                if line.closed {
                    return;
                }
                line.idle += 1;
                line = wait(&queue.posted, line);
                line.idle -= 1;
            }
        };
        // A job that panics is answered as one that failed, and the thread takes the next.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job))
            .unwrap_or_else(|_| JobOutput::Failed("the host panicked computing it".to_string()));
        complete(shared, token, outcome);
    }
}

#[cold]
fn err_no_thread(what: &str, why: &std::io::Error, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("{what} could not start: {why}"),
    )
    .primary(span, "the host could not spawn a thread for this operation")
}

enum Taken {
    Ready(Result<Value, Diagnostic>),
    Waiting,
    Unknown,
}

fn take(state: &mut State, token: u64) -> Taken {
    let Some(done) = state.done.remove(&token) else {
        return if state.waiting.contains_key(&token) {
            Taken::Waiting
        } else {
            Taken::Unknown
        };
    };
    // `Waiting` carries the failure's span, so it is removed here rather than on completion.
    let waiting = state.waiting.remove(&token);
    let (span, what) = match &waiting {
        Some(w) => (w.span, w.what),
        None => (Span::DUMMY, "a host operation"),
    };
    Taken::Ready(match done {
        JobOutput::Int(i) => Ok(Value::Int(i)),
        JobOutput::Bool(b) => Ok(Value::Bool(b)),
        JobOutput::Bytes(b) => Ok(Value::bytes(b)),
        JobOutput::MaybeBytes(b) => Ok(option(b.map(Value::bytes))),
        JobOutput::MaybeInt(n) => Ok(option(n.map(Value::Int))),
        JobOutput::MaybeStrings(names) => {
            Ok(option(names.map(|names| {
                Value::list(names.into_iter().map(Value::str).collect())
            })))
        }
        JobOutput::MaybeString(text) => Ok(option(text.map(Value::str))),
        JobOutput::MaybeEntries(entries) => Ok(option(entries.map(|entries| {
            Value::list(
                entries
                    .into_iter()
                    .map(|(path, kind)| entry(path, kind))
                    .collect(),
            )
        }))),
        JobOutput::MaybeMode(bits) => Ok(option(bits.map(mode))),
        JobOutput::Ctor(name) => Ok(Value::ctor(name, Vec::new())),
        JobOutput::Opened(Ok(descriptor), _) => Ok(Value::ctor("Ok", vec![Value::Int(descriptor)])),
        JobOutput::Opened(Err(why), _) => {
            Ok(Value::ctor("Err", vec![Value::ctor(why, Vec::new())]))
        }
        JobOutput::Finished(exit) => Ok(finished(exit)),
        JobOutput::MaybeFinished(exit) => Ok(option(exit.map(finished))),
        JobOutput::Heard(heard) => Ok(heard_value(heard)),
        JobOutput::Refused(diagnostic) => Err(diagnostic),
        JobOutput::Failed(message) => Err(Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("{what} failed: {message}"),
        )
        .primary(span, "this operation reached the host and the host refused")),
    })
}

/// The record `std.process.Finished` names, built where the `Value` will live.
fn finished(exit: Finished) -> Value {
    let ended = match exit.ended {
        Ended::Exited(code) => Value::ctor("std.process.Exited", vec![Value::Int(code)]),
        Ended::Signalled(signal) => Value::ctor("std.process.Signalled", vec![Value::Int(signal)]),
    };
    let fields: BTreeMap<Symbol, Value> = BTreeMap::from([
        (Symbol::new("ended"), ended),
        (Symbol::new("err"), Value::bytes(exit.err)),
        (Symbol::new("out"), Value::bytes(exit.out)),
    ]);
    Value::Record(Arc::new(fields.into_iter().collect()))
}

/// The record `std.fs.Entry` names.
fn entry(path: String, kind: &'static str) -> Value {
    record([
        ("kind", Value::ctor(kind, Vec::new())),
        ("path", Value::str(path)),
    ])
}

/// The record `std.fs.Mode` names, each `std.fs.Access` one triple of its bits.
fn mode(bits: u32) -> Value {
    let access = |triple: u32| {
        record([
            ("execute", Value::Bool(triple & 1 != 0)),
            ("read", Value::Bool(triple & 4 != 0)),
            ("write", Value::Bool(triple & 2 != 0)),
        ])
    };
    record([
        ("group", access(bits >> 3 & 7)),
        ("other", access(bits & 7)),
        ("owner", access(bits >> 6 & 7)),
    ])
}

fn record<const N: usize>(fields: [(&str, Value); N]) -> Value {
    Value::Record(Arc::new(
        fields
            .into_iter()
            .map(|(name, value)| (Symbol::new(name), value))
            .collect(),
    ))
}

fn heard_value(heard: Heard) -> Value {
    match heard {
        Heard::Said(line) => Value::ctor("std.process.Said", vec![Value::str(line)]),
        Heard::Quiet => Value::ctor("std.process.Quiet", Vec::new()),
        Heard::Closed => Value::ctor("std.process.Closed", Vec::new()),
    }
}

/// Built on the polling thread: a `Value` holds `Rc` and never crosses threads.
fn option(v: Option<Value>) -> Value {
    match v {
        Some(v) => Value::ctor("Some", vec![v]),
        None => Value::ctor("None", Vec::new()),
    }
}

#[cold]
fn unknown_token(pending: &Pending) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("the host runtime was asked about `{pending}`, which it did not mint"),
    )
    .note("a pending token belongs to the facility that answered the operation; asking the wrong one loses the result rather than waiting for it")
}

/// A poisoned lock means a job thread panicked.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

fn wait<'a, T>(condvar: &Condvar, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    condvar.wait(guard).unwrap_or_else(|e| e.into_inner())
}

fn wait_timeout<'a, T>(
    condvar: &Condvar,
    guard: MutexGuard<'a, T>,
    bound: Duration,
) -> MutexGuard<'a, T> {
    condvar
        .wait_timeout(guard, bound)
        .map(|(guard, _)| guard)
        .unwrap_or_else(|e| e.into_inner().0)
}
