//! Where a host operation goes when it has to wait.

use ply_eval::{Pending, Value};
use ply_span::{Diagnostic, Span, codes};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

/// How many host operations may be waiting at once, across every socket.
pub const MAX_BLOCKING_OPERATIONS: usize = 64;

/// What a job hands back.
pub enum Done {
    Int(i64),
    /// `net.recv`'s answer.
    MaybeBytes(Option<Vec<u8>>),
    /// `net.send`'s answer, under the same rule.
    MaybeInt(Option<i64>),
    /// The operation failed in a way that is neither the peer's doing nor a deadline.
    Failed(String),
}

type Job = Box<dyn FnOnce() -> Done + Send + 'static>;

struct Waiting {
    span: Span,
    /// The operation, as a diagnostic names it: `` `net.recv` ``.
    what: &'static str,
}

#[derive(Default)]
struct State {
    waiting: HashMap<u64, Waiting>,
    done: HashMap<u64, Done>,
}

struct Shared {
    state: Mutex<State>,
    /// Signalled by a job finishing, so neither `park` nor `block_on` spins.
    finished: Condvar,
    next: AtomicU64,
}

/// Clone-free by design: the pool is owned by the handler that submits to it, and the jobs hold an
/// [`Arc`] of the shared state instead.
pub struct Pool {
    shared: Arc<Shared>,
}

impl Default for Pool {
    fn default() -> Pool {
        Pool::new()
    }
}

impl Pool {
    pub fn new() -> Pool {
        Pool {
            shared: Arc::new(Shared {
                state: Mutex::new(State::default()),
                finished: Condvar::new(),
                // Token 0 is never minted, so a zeroed `Pending` is a token this pool does not own
                // rather than its first job.
                next: AtomicU64::new(1),
            }),
        }
    }

    /// Start `job` and answer the token that will carry its result.
    pub fn submit(
        &self,
        span: Span,
        label: &'static str,
        what: &'static str,
        job: Job,
    ) -> Result<Pending, Diagnostic> {
        let token = self.shared.next.fetch_add(1, Ordering::Relaxed);
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
            state.waiting.insert(token, Waiting { span, what });
        }

        let shared = Arc::clone(&self.shared);
        let spawned = std::thread::Builder::new()
            .name(format!("ply-host-{label}-{token}"))
            .spawn(move || {
                let outcome = job();
                let mut state = lock(&shared.state);
                state.done.insert(token, outcome);
                drop(state);
                shared.finished.notify_all();
            });

        if let Err(e) = spawned {
            lock(&self.shared.state).waiting.remove(&token);
            return Err(Diagnostic::error(
                codes::RUNTIME_ERROR,
                format!("{what} could not start: {e}"),
            )
            .primary(span, "the host could not spawn a thread for this operation"));
        }
        Ok(Pending { token, label })
    }

    /// Whether this pool minted the token.
    pub fn owns(&self, pending: &Pending) -> bool {
        let state = lock(&self.shared.state);
        state.waiting.contains_key(&pending.token) || state.done.contains_key(&pending.token)
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

    /// Drive until this token resolves.
    pub fn block_on(&self, pending: Pending) -> Result<Value, Diagnostic> {
        let mut state = lock(&self.shared.state);
        loop {
            match take(&mut state, pending.token) {
                Taken::Ready(result) => return result,
                Taken::Unknown => return Err(unknown_token(&pending)),
                // Waiting on the condvar rather than on this token specifically: a wake for another
                // token re-checks and waits again, which is correct and — because the result stays
                // in `done` until its own poll consumes it — cannot spin.
                Taken::Waiting => state = wait(&self.shared.finished, state),
            }
        }
    }

    pub fn outstanding(&self) -> usize {
        let state = lock(&self.shared.state);
        state.waiting.len()
    }
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
    // A finished job's `Waiting` entry is what carries the span its failure is reported at, so it
    // is removed here and not when the job completes.
    let waiting = state.waiting.remove(&token);
    let (span, what) = match &waiting {
        Some(w) => (w.span, w.what),
        None => (Span::DUMMY, "a host operation"),
    };
    Taken::Ready(match done {
        Done::Int(i) => Ok(Value::Int(i)),
        Done::MaybeBytes(b) => Ok(option(b.map(Value::bytes))),
        Done::MaybeInt(n) => Ok(option(n.map(Value::Int))),
        Done::Failed(message) => Err(Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("{what} failed: {message}"),
        )
        .primary(span, "this operation reached the host and the host refused")),
    })
}

/// The prelude's `Option`, built on the polling thread because a `Value` holds `Rc` and never
/// crosses one.
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
        format!("the host runtime was polled for `{pending}`, which it did not mint"),
    )
    .note("a pending token belongs to the facility that answered the operation; polling the wrong one loses the result rather than waiting for it")
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
