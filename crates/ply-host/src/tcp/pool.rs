//! Where a host operation goes when it has to wait.
//!
//! ADR 0008 §8: a blocking host handler stalls every task the scheduler owns, so
//! an operation that waits runs on a dedicated pool and answers
//! [`HostAnswer::Pending`](ply_eval::HostAnswer::Pending) immediately. This is
//! that pool.
//!
//! It is `std::thread` rather than `tokio::task::spawn_blocking` so its bound is
//! a number someone chose and a reviewer can read, and it is one thread per
//! outstanding operation rather than a fixed set of workers behind a queue —
//! because a queue in front of N workers deadlocks silently the moment N
//! operations are parked in `accept`, and W1 has no cancellation to break it
//! with. Exceeding [`MAX_BLOCKING_OPERATIONS`] is a diagnostic naming the
//! operation, which is the loud failure the quiet one would have replaced.
//!
//! A `Value` holds `Rc` and belongs to one thread, so a job produces plain data
//! and the polling thread — the machine's — builds the `Value`.

use ply_eval::{Pending, Value};
use ply_span::{Diagnostic, Span, codes};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

/// How many host operations may be waiting at once, across every socket.
///
/// One real thread each. The number decides how many connections a runaway
/// program can hold open, which is the reason it is stated here rather than
/// inherited from a runtime's default.
pub const MAX_BLOCKING_OPERATIONS: usize = 64;

/// What a job hands back. Not a [`Value`]: a `Value` holds `Rc` and never
/// crosses a thread.
pub enum Done {
    Int(i64),
    Bytes(Vec<u8>),
    /// The operation failed. Rendered against the `perform`'s span by whoever
    /// polls, so a socket error points at Ply source rather than at Rust.
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

/// Clone-free by design: the pool is owned by the handler that submits to it,
/// and the jobs hold an [`Arc`] of the shared state instead.
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
                // Token 0 is never minted, so a zeroed `Pending` is a token this
                // pool does not own rather than its first job.
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

    /// Whether this pool minted the token. What a composed runtime routes on.
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
    ///
    /// Called by a scheduler with nothing enabled. Waiting with nothing
    /// outstanding would be a deadlock, so it is a diagnostic instead.
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

    /// Drive until this token resolves.
    ///
    /// The only place a Ply computation blocks a real thread, reached when there
    /// is no scheduler region to park the performing task in.
    pub fn block_on(&self, pending: Pending) -> Result<Value, Diagnostic> {
        let mut state = lock(&self.shared.state);
        loop {
            match take(&mut state, pending.token) {
                Taken::Ready(result) => return result,
                Taken::Unknown => return Err(unknown_token(&pending)),
                // Waiting on the condvar rather than on this token specifically:
                // a wake for another token re-checks and waits again, which is
                // correct and — because the result stays in `done` until its own
                // poll consumes it — cannot spin.
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
    // A finished job's `Waiting` entry is what carries the span its failure is
    // reported at, so it is removed here and not when the job completes.
    let waiting = state.waiting.remove(&token);
    let (span, what) = match &waiting {
        Some(w) => (w.span, w.what),
        None => (Span::DUMMY, "a host operation"),
    };
    Taken::Ready(match done {
        Done::Int(i) => Ok(Value::Int(i)),
        Done::Bytes(b) => Ok(Value::bytes(b)),
        Done::Failed(message) => Err(Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("{what} failed: {message}"),
        )
        .primary(span, "this operation reached the host and the host refused")),
    })
}

#[cold]
fn unknown_token(pending: &Pending) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("the host runtime was polled for `{pending}`, which it did not mint"),
    )
    .note("a pending token belongs to the facility that answered the operation; polling the wrong one loses the result rather than waiting for it")
}

/// A poisoned lock means a job thread panicked. The state behind it is two maps
/// and neither has an invariant a panic can break, so recovering is correct and
/// propagating the panic would take out the machine's thread as well.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

fn wait<'a, T>(condvar: &Condvar, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    condvar.wait(guard).unwrap_or_else(|e| e.into_inner())
}
