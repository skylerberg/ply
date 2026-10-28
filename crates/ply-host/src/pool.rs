//! Where a host operation goes when it has to wait. A [`Pool`] runs it on a thread, and the [`Run`]
//! that submitted it holds it until it takes the answer: an operation is its run's alone, so one
//! run's answers, kept or abandoned, never wake another's wait. Every pool mints from one count of
//! tokens.

use crate::jobs::Failure;
use crate::observe::Recorder;
use ply_core::{Diagnostic, Pending, Span, codes};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

/// How many operations of a pool that gives each a thread may be running at once.
pub const MAX_BLOCKING_OPERATIONS: usize = 64;

/// The token the next operation of any pool is given: one count for them all, so no two pools
/// mint the same one, and none mints 0, which a zeroed `Pending` carries.
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

/// What a job answered: its bytes, or why it answered none.
pub type JobOutput = Result<Vec<u8>, Failure>;

type Job = Box<dyn FnOnce() -> JobOutput + Send + 'static>;

/// Rung whenever an answer is filed with a run it is handed to, and by a stop, so one wait covers
/// every pool and the stop at once: separate condition variables cannot be waited on together.
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

/// One run's operations, of whichever pool runs each: an operation is the run's from its
/// submission until the run takes its answer. The run holds this, and each operation's job holds it
/// weakly, so the answer of a job that finishes after its run has ended is dropped, and what an
/// ended run left goes with it. What its jobs read for a `Secret` is kept here too, until the run
/// takes it or its entry point ends, and so is the record a test's reads are entered into.
pub struct Run {
    held: Mutex<HashMap<u64, Held>>,
    secrets: Mutex<Secrets>,
    recorder: Mutex<Option<Arc<Recorder>>>,
    bell: Arc<Bell>,
}

/// A secret's bytes as a job read them, wiped when they are dropped.
pub type Kept = Zeroizing<Vec<u8>>;

/// What was read for a `Secret` and not yet taken, each under the number its job answered, wiped
/// as each is dropped.
#[derive(Default)]
struct Secrets {
    last: i64,
    kept: HashMap<i64, Kept>,
}

/// An operation a run holds, until its answer is taken.
struct Held {
    span: Span,
    what: &'static str,
    /// Whether the run collects it with `resolved`, rather than waiting for it alone.
    watched: bool,
    answer: Option<JobOutput>,
}

impl Run {
    /// A run whose answers ring `bell` as they are filed.
    pub fn new(bell: Arc<Bell>) -> Arc<Run> {
        Arc::new(Run {
            held: Mutex::new(HashMap::new()),
            secrets: Mutex::new(Secrets::default()),
            recorder: Mutex::new(None),
            bell,
        })
    }

    pub fn bell(&self) -> &Arc<Bell> {
        &self.bell
    }

    /// What the run read is entered into `recorder` from now on.
    pub fn records_into(&self, recorder: Arc<Recorder>) {
        *lock(&self.recorder) = Some(recorder);
    }

    /// The record what the run reads is entered into, where it is observed.
    pub fn recorder(&self) -> Option<Arc<Recorder>> {
        lock(&self.recorder).clone()
    }

    /// Keeps `secret` for this run under a number nothing else of it is kept under, answered.
    pub fn keep_secret(&self, secret: Kept) -> i64 {
        let mut secrets = lock(&self.secrets);
        secrets.last += 1;
        let number = secrets.last;
        secrets.kept.insert(number, secret);
        number
    }

    /// The secret kept under `number`, which the run then keeps no longer.
    pub fn take_secret(&self, number: i64) -> Option<Kept> {
        lock(&self.secrets).kept.remove(&number)
    }

    /// Drops every secret kept and not taken, each wiped: what an entry point that ended did not
    /// take, nothing after it may.
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
                answer: None,
            },
        );
    }

    fn forget(&self, token: u64) {
        lock(&self.held).remove(&token);
    }

    /// Files a job's answer and rings the bell, after the answer is in place, so a wait that
    /// woke finds it.
    fn file(&self, token: u64, answer: JobOutput) {
        if let Some(held) = lock(&self.held).get_mut(&token) {
            held.answer = Some(answer);
        }
        self.bell.ring();
    }

    /// Has `answered` name `pending` once it is answered; refuses a token this run did not submit.
    pub fn watch(&self, pending: &Pending) -> Result<(), Diagnostic> {
        match lock(&self.held).get_mut(&pending.token) {
            Some(held) => {
                held.watched = true;
                Ok(())
            }
            None => Err(err_unowned(pending)),
        }
    }

    /// Whether a watched operation has been answered and not collected.
    pub fn ready(&self) -> bool {
        lock(&self.held)
            .values()
            .any(|held| held.watched && held.answer.is_some())
    }

    /// The watched operations answered and not yet taken, ascending, their answers left for `taken`.
    pub fn answered(&self) -> Vec<u64> {
        let mut tokens: Vec<u64> = (lock(&self.held).iter())
            .filter(|(_, h)| h.watched && h.answer.is_some())
            .map(|(token, _)| *token)
            .collect();
        tokens.sort_unstable();
        tokens
    }

    /// The bytes a native's job answered `pending` where it has answered, which are then taken.
    pub fn taken(&self, pending: &Pending) -> Result<Option<Vec<u8>>, Diagnostic> {
        let mut held = lock(&self.held);
        match held.get(&pending.token) {
            None => return Err(err_unowned(pending)),
            Some(h) if h.answer.is_none() => return Ok(None),
            Some(_) => {}
        }
        let taken = held.remove(&pending.token);
        drop(held);
        match taken {
            Some(Held {
                span,
                what,
                answer: Some(answer),
                ..
            }) => bytes_of(span, what, answer).map(Some),
            _ => Ok(None),
        }
    }

    /// The bytes a native's job answered `pending` once it has answered, waiting for at most
    /// `bound` where one is given: `None` where the bound passed first.
    pub fn block_on(
        &self,
        pending: &Pending,
        bound: Option<Duration>,
    ) -> Result<Option<Vec<u8>>, Diagnostic> {
        let until = bound.map(|bound| Instant::now() + bound);
        loop {
            let seen = self.bell.rung();
            if let Some(answer) = self.taken(pending)? {
                return Ok(Some(answer));
            }
            match until {
                None => self.bell.wait_past(seen),
                Some(until) => {
                    let left = until.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Ok(None);
                    }
                    self.bell.wait_past_for(seen, left);
                }
            }
        }
    }

    /// Waits until a watched operation is answered, `stop` says to stop waiting, or `bound`
    /// passes; `false`, at once, where no bound is given and the run watches nothing, so nothing
    /// would end the wait.
    pub fn park(&self, bound: Option<Duration>, stop: &dyn Fn() -> bool) -> bool {
        let until = bound.map(|bound| Instant::now() + bound);
        loop {
            let seen = self.bell.rung();
            if self.ready() || stop() {
                return true;
            }
            match until {
                None => {
                    if !lock(&self.held).values().any(|held| held.watched) {
                        return false;
                    }
                    self.bell.wait_past(seen);
                }
                Some(until) => {
                    let left = until.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return true;
                    }
                    self.bell.wait_past_for(seen, left);
                }
            }
        }
    }
}

/// The bytes a native's job answered, which is all one answers.
fn bytes_of(span: Span, what: &str, answer: JobOutput) -> Result<Vec<u8>, Diagnostic> {
    answer.map_err(|failure| match failure {
        Failure::Said(message) => failed(span, what, &message),
        Failure::Refused(diagnostic) => diagnostic,
    })
}

#[cold]
fn failed(span: Span, what: &str, message: &str) -> Diagnostic {
    Diagnostic::error(codes::RUNTIME_ERROR, format!("{what} failed: {message}"))
        .primary(span, "this operation reached the host and the host refused")
}

struct Shared {
    /// How many of the pool's jobs have a thread of their own and have not finished.
    running: Mutex<usize>,
    /// Where jobs wait for a thread; `None` for a pool that starts one for each.
    queue: Option<Queue>,
}

/// A job and the run it answers.
struct Entry {
    token: u64,
    run: Weak<Run>,
    job: Job,
}

/// The jobs of a pool that runs them on a fixed number of threads, in the order submitted.
struct Queue {
    line: Mutex<Line>,
    posted: Condvar,
    threads: usize,
}

#[derive(Default)]
struct Line {
    jobs: VecDeque<Entry>,
    started: usize,
    /// The started threads that are waiting for a job.
    idle: usize,
    /// The pool is gone: a thread that finds no job ends.
    closed: bool,
}

/// Not `Clone`: the facility owns it, and jobs hold an [`Arc`] of the shared state.
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
    /// A pool that starts a thread for each operation, [`MAX_BLOCKING_OPERATIONS`] at once at most.
    pub fn new() -> Pool {
        Pool::over(None)
    }

    /// A pool whose operations wait their turn for one of `threads` threads, however many are
    /// submitted: for work that fills a core, where a thread each would finish none sooner and a
    /// bound on how many wait would refuse a burst. A thread is started when an operation first
    /// needs one, and ends when the pool is dropped.
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

    /// `job` run for `run`, which holds the operation as `what` until it takes the answer.
    ///
    /// # Errors
    /// No thread could be had for it: the pool's bound was reached, or one could not start.
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
                return Err(err_bounded(what, span));
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

    /// Puts `entry` in line, and starts a thread for it if every started one is busy and the pool
    /// may have another.
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
                // A thread that is running takes this in its turn.
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

/// Runs a job, gives back its thread with `freed`, and files its answer with its run where the run
/// has not ended: what every pool's threads do with a job. One that panics is answered as one that
/// failed.
fn finish(entry: Entry, freed: impl FnOnce()) {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(entry.job))
        .unwrap_or_else(|_| Err(Failure::Said("the host panicked doing it".to_string())));
    freed();
    if let Some(run) = entry.run.upgrade() {
        run.file(entry.token, outcome);
    }
}

/// One thread of a queued pool: the next job in line, until the pool is gone and the line empty.
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
fn err_unowned(pending: &Pending) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("the run holds no pending token `{pending}`"),
    )
    .primary(Span::DUMMY, "this token belongs to no operation of this run")
    .note("a token is the run's that submitted its operation, until the run takes the answer; one another run submitted, or one already answered, is none of this one's")
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
    .note("waiting here would never return, so it is refused instead")
    .note("this is Ply's fault: report it with the program that produced it")
}

#[cold]
fn err_bounded(what: &str, span: Span) -> Diagnostic {
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
fn err_no_thread(what: &str, why: &std::io::Error, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("{what} could not start: {why}"),
    )
    .primary(span, "the host could not spawn a thread for this operation")
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
