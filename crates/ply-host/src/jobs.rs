//! Where a native's job runs when it would block the program's thread: pools for the whole
//! process, so a test run's machines and a `parallel`'s branches share one bound of threads between
//! them, while each job stays the run's that submitted it; or that thread itself, where nothing
//! else of the run could go on while it waits.

use crate::pool::{MAX_BLOCKING_OPERATIONS, Pool, Run};
use ply_core::host::Pending;
use ply_core::{Diagnostic, Span};
use std::sync::{Arc, OnceLock};

/// What a native's operation does on a thread of its own: its answer flat, or why it failed.
pub type Job = Box<dyn FnOnce() -> Result<Vec<u8>, String> + Send>;

/// Why a native's job answered nothing.
#[derive(Debug)]
pub enum Failure {
    /// What the system said, which the run's failure names the job by.
    Said(String),
    /// A refusal of the job's own, which fails the run as it stands.
    Refused(Diagnostic),
}

/// A job whose failure may be a refusal of its own.
pub type Refusing = Box<dyn FnOnce() -> Result<Vec<u8>, Failure> + Send>;

/// Which of the process's pools a job waits for a thread in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Work {
    /// It fills a core: in line for as many threads as the machine has cores, so a burst is a
    /// line rather than a refusal and more at once would finish none sooner.
    Computing,
    /// It waits on something outside the process: a thread of its own, a bounded number at once.
    Waiting,
    /// It reads or writes the file system: in line for one of a bounded number of threads, so a
    /// burst of small operations is a line rather than a refusal.
    Filing,
}

struct Jobs {
    computing: Pool,
    waiting: Pool,
    filing: Pool,
}

fn jobs() -> &'static Jobs {
    static JOBS: OnceLock<Jobs> = OnceLock::new();
    JOBS.get_or_init(|| Jobs {
        computing: Pool::queued(std::thread::available_parallelism().map_or(1, |n| n.get())),
        waiting: Pool::new(),
        filing: Pool::queued(MAX_BLOCKING_OPERATIONS),
    })
}

/// `job` run for `run` on a thread of `work`'s pool, named `label`, answering the token a wait
/// reads its bytes by. A job that answers why it failed, or panics, fails the run that waits on
/// it, naming `what`.
///
/// # Errors
/// No thread could be had for it: the waiting pool's bound was reached, or one could not start.
pub fn submit(
    work: Work,
    run: &Arc<Run>,
    span: Span,
    label: &'static str,
    what: &'static str,
    job: impl FnOnce() -> Result<Vec<u8>, String> + Send + 'static,
) -> Result<Pending, Diagnostic> {
    refusing(work, run, span, label, what, move || {
        job().map_err(Failure::Said)
    })
}

/// `job` run for `run` on this thread, its ticket answered as it is handed: for a job no other task
/// of the run could go on beside, which a thread of a pool would only make wait longer. As
/// [`refusing`] runs it otherwise.
pub fn done_here(
    run: &Run,
    span: Span,
    label: &'static str,
    what: &'static str,
    job: impl FnOnce() -> Result<Vec<u8>, Failure> + Send + 'static,
) -> Pending {
    run.done_here(span, label, what, job)
}

/// `job` as [`submit`] runs it, where the job may refuse with a diagnostic of its own.
///
/// # Errors
/// As [`submit`].
pub fn refusing(
    work: Work,
    run: &Arc<Run>,
    span: Span,
    label: &'static str,
    what: &'static str,
    job: impl FnOnce() -> Result<Vec<u8>, Failure> + Send + 'static,
) -> Result<Pending, Diagnostic> {
    let pool = match work {
        Work::Computing => &jobs().computing,
        Work::Waiting => &jobs().waiting,
        Work::Filing => &jobs().filing,
    };
    pool.submit(run, span, label, what, Box::new(job))
}
