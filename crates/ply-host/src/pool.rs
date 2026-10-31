use crate::jobs::Failure;
use crate::observe::Recorder;
use ply_core::{Diagnostic, Pending, Span, codes};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

pub const MAX_BLOCKING_OPERATIONS: usize = 64;

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

pub type JobOutput = Result<Vec<u8>, Failure>;

type Job = Box<dyn FnOnce() -> JobOutput + Send + 'static>;

#[derive(Default)]
pub struct WakeCounter {
    count: Mutex<u64>,
    heard: Condvar,
}

impl WakeCounter {
    pub fn count(&self) -> u64 {
        *lock(&self.count)
    }

    pub fn wait_for_change(&self, seen: u64) {
        let mut count = lock(&self.count);
        while *count == seen {
            count = wait(&self.heard, count);
        }
    }

    pub fn wait_for_change_timeout(&self, seen: u64, bound: Duration) {
        let count = lock(&self.count);
        if *count == seen {
            drop(wait_timeout(&self.heard, count, bound));
        }
    }

    pub(crate) fn ring(&self) {
        *lock(&self.count) += 1;
        self.heard.notify_all();
    }
}

pub struct Run {
    held: Mutex<HashMap<u64, Held>>,
    secrets: Mutex<SecretTable>,
    recorder: Mutex<Option<Arc<Recorder>>>,
    bell: Arc<WakeCounter>,
}

pub type Kept = Zeroizing<Vec<u8>>;

#[derive(Default)]
struct SecretTable {
    last: i64,
    kept: HashMap<i64, Kept>,
}

struct Held {
    span: Span,
    what: &'static str,
    watched: bool,
    result: Option<JobOutput>,
}

impl Run {
    pub fn new(bell: Arc<WakeCounter>) -> Arc<Run> {
        Arc::new(Run {
            held: Mutex::new(HashMap::new()),
            secrets: Mutex::new(SecretTable::default()),
            recorder: Mutex::new(None),
            bell,
        })
    }

    pub fn bell(&self) -> &Arc<WakeCounter> {
        &self.bell
    }

    pub fn records_into(&self, recorder: Arc<Recorder>) {
        *lock(&self.recorder) = Some(recorder);
    }

    pub fn recorder(&self) -> Option<Arc<Recorder>> {
        lock(&self.recorder).clone()
    }

    pub fn store_secret(&self, secret: Kept) -> i64 {
        let mut secrets = lock(&self.secrets);
        secrets.last += 1;
        let number = secrets.last;
        secrets.kept.insert(number, secret);
        number
    }

    pub fn remove_secret(&self, number: i64) -> Option<Kept> {
        lock(&self.secrets).kept.remove(&number)
    }

    pub fn drop_secrets(&self) {
        lock(&self.secrets).kept.clear();
    }

    fn enter(&self, token: u64, span: Span, what: &'static str) {
        lock(&self.held).insert(
            token,
            Held {
                span,
                what,
                watched: false,
                result: None,
            },
        );
    }

    fn forget(&self, token: u64) {
        lock(&self.held).remove(&token);
    }

    pub fn done_here(
        &self,
        span: Span,
        label: &'static str,
        what: &'static str,
        job: impl FnOnce() -> JobOutput + Send + 'static,
    ) -> Pending {
        let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
        self.enter(token, span, what);
        self.file(token, outcome_of(Box::new(job)));
        Pending { token, label }
    }

    fn file(&self, token: u64, result: JobOutput) {
        if let Some(held) = lock(&self.held).get_mut(&token) {
            held.result = Some(result);
        }
        self.bell.ring();
    }

    pub fn watch(&self, pending: &Pending) -> Result<(), Diagnostic> {
        match lock(&self.held).get_mut(&pending.token) {
            Some(held) => {
                held.watched = true;
                Ok(())
            }
            None => Err(err_token_not_pending_here(pending)),
        }
    }

    pub fn ready(&self) -> bool {
        lock(&self.held)
            .values()
            .any(|held| held.watched && held.result.is_some())
    }

    pub fn returned(&self) -> Vec<u64> {
        let mut tokens: Vec<u64> = (lock(&self.held).iter())
            .filter(|(_, h)| h.watched && h.result.is_some())
            .map(|(token, _)| *token)
            .collect();
        tokens.sort_unstable();
        tokens
    }

    pub fn taken(&self, pending: &Pending) -> Result<Option<Vec<u8>>, Diagnostic> {
        let mut held = lock(&self.held);
        match held.get(&pending.token) {
            None => return Err(err_token_not_pending_here(pending)),
            Some(h) if h.result.is_none() => return Ok(None),
            Some(_) => {}
        }
        let taken = held.remove(&pending.token);
        drop(held);
        match taken {
            Some(Held {
                span,
                what,
                result: Some(result),
                ..
            }) => bytes_of(span, what, result).map(Some),
            _ => Ok(None),
        }
    }

    pub fn block_on(
        &self,
        pending: &Pending,
        bound: Option<Duration>,
    ) -> Result<Option<Vec<u8>>, Diagnostic> {
        let until = bound.map(|bound| Instant::now() + bound);
        loop {
            let seen = self.bell.count();
            if let Some(result) = self.taken(pending)? {
                return Ok(Some(result));
            }
            match until {
                None => self.bell.wait_for_change(seen),
                Some(until) => {
                    let left = until.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Ok(None);
                    }
                    self.bell.wait_for_change_timeout(seen, left);
                }
            }
        }
    }

    pub fn park(&self, bound: Option<Duration>, stop: &dyn Fn() -> bool) -> bool {
        let until = bound.map(|bound| Instant::now() + bound);
        loop {
            let seen = self.bell.count();
            if self.ready() || stop() {
                return true;
            }
            match until {
                None => {
                    if !lock(&self.held).values().any(|held| held.watched) {
                        return false;
                    }
                    self.bell.wait_for_change(seen);
                }
                Some(until) => {
                    let left = until.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return true;
                    }
                    self.bell.wait_for_change_timeout(seen, left);
                }
            }
        }
    }
}

fn bytes_of(span: Span, what: &str, result: JobOutput) -> Result<Vec<u8>, Diagnostic> {
    result.map_err(|failure| match failure {
        Failure::Said(message) => failed(span, what, &message),
        Failure::Rejected(diagnostic) => diagnostic,
    })
}

#[cold]
fn failed(span: Span, what: &str, message: &str) -> Diagnostic {
    Diagnostic::error(codes::RUNTIME_ERROR, format!("{what} failed: {message}")).primary(
        span,
        "this operation reached the host and the host rejected it",
    )
}

struct Shared {
    running: Mutex<usize>,
    queue: Option<Queue>,
}

struct Entry {
    token: u64,
    run: Weak<Run>,
    job: Job,
}

struct Queue {
    line: Mutex<Line>,
    posted: Condvar,
    threads: usize,
}

#[derive(Default)]
struct Line {
    jobs: VecDeque<Entry>,
    started: usize,
    idle: usize,
    closed: bool,
}

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

impl Default for Pool {
    fn default() -> Pool {
        Pool::new()
    }
}

impl Pool {
    pub fn new() -> Pool {
        Pool::over(None)
    }

    pub fn queued(threads: usize) -> Pool {
        Pool::over(Some(Queue {
            line: Mutex::new(Line::default()),
            posted: Condvar::new(),
            threads: threads.max(1),
        }))
    }

    fn over(queue: Option<Queue>) -> Pool {
        Pool {
            shared: Arc::new(Shared {
                running: Mutex::new(0),
                queue,
            }),
        }
    }

    pub fn submit(
        &self,
        run: &Arc<Run>,
        span: Span,
        label: &'static str,
        what: &'static str,
        job: Job,
    ) -> Result<Pending, Diagnostic> {
        let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
        let entry = Entry {
            token,
            run: Arc::downgrade(run),
            job,
        };
        if let Some(queue) = &self.shared.queue {
            run.enter(token, span, what);
            return self.enqueue(queue, run, entry, span, label, what);
        }
        {
            let mut running = lock(&self.shared.running);
            if *running >= MAX_BLOCKING_OPERATIONS {
                return Err(err_too_many_blocking_operations(what, span));
            }
            *running += 1;
        }
        run.enter(token, span, what);
        let shared = Arc::clone(&self.shared);
        let spawned = std::thread::Builder::new()
            .name(format!("ply-host-{label}-{token}"))
            .spawn(move || finish(entry, || *lock(&shared.running) -= 1));
        if let Err(e) = spawned {
            *lock(&self.shared.running) -= 1;
            run.forget(token);
            return Err(err_no_thread(what, &e, span));
        }
        Ok(Pending { token, label })
    }

    fn enqueue(
        &self,
        queue: &Queue,
        run: &Run,
        entry: Entry,
        span: Span,
        label: &'static str,
        what: &'static str,
    ) -> Result<Pending, Diagnostic> {
        let token = entry.token;
        let mut line = lock(&queue.line);
        line.jobs.push_back(entry);
        if line.jobs.len() > line.idle && line.started < queue.threads {
            let shared = Arc::clone(&self.shared);
            let spawned = std::thread::Builder::new()
                .name(format!("ply-host-{label}-{}", line.started))
                .spawn(move || work(&shared));
            match spawned {
                Ok(_) => line.started += 1,
                Err(_) if line.started > 0 => {}
                Err(e) => {
                    line.jobs.pop_back();
                    drop(line);
                    run.forget(token);
                    return Err(err_no_thread(what, &e, span));
                }
            }
        }
        drop(line);
        queue.posted.notify_one();
        Ok(Pending { token, label })
    }
}

fn outcome_of(job: Job) -> JobOutput {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(job))
        .unwrap_or_else(|_| Err(Failure::Said("the host panicked doing it".to_string())))
}

fn finish(entry: Entry, freed: impl FnOnce()) {
    let outcome = outcome_of(entry.job);
    freed();
    if let Some(run) = entry.run.upgrade() {
        run.file(entry.token, outcome);
    }
}

fn work(shared: &Shared) {
    let Some(queue) = &shared.queue else {
        return;
    };
    loop {
        let entry = {
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
        finish(entry, || {});
    }
}

#[cold]
#[inline(never)]
fn err_token_not_pending_here(pending: &Pending) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("the run holds no pending token `{pending}`"),
    )
    .primary(Span::DUMMY, "this token belongs to no operation of this run")
    .note("a token is the run's that submitted its operation, until the run takes the result; one another run submitted, or one whose result was taken, is none of this one's")
    .note("this is Ply's fault: report it with the program that produced it")
}

#[cold]
#[inline(never)]
pub(crate) fn err_nothing_outstanding() -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        "the run asked the host runtime to wait with nothing outstanding to wait for",
    )
    .primary(
        Span::DUMMY,
        "no task is enabled and no host operation is pending",
    )
    .note("waiting here would never return, so it is rejected instead")
    .note("this is Ply's fault: report it with the program that produced it")
}

#[cold]
fn err_too_many_blocking_operations(what: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
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
    .note("W1 has no cancellation, so an operation that never completes holds its thread until the run ends")
}

#[cold]
fn err_no_thread(what: &str, reason: &std::io::Error, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("{what} could not start: {reason}"),
    )
    .primary(span, "the host could not spawn a thread for this operation")
}

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
