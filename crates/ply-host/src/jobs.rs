//! Where a native's job runs when it would block the program's thread: two pools for the whole
//! process, so a test run's machines and a `parallel`'s branches share one bound of cores between
//! them, while each job stays the run's that submitted it.

use crate::pool::{JobOutput, Pool, Run};
use ply_core::host::Pending;
use ply_core::{Diagnostic, Span};
use std::sync::{Arc, OnceLock};

/// What a native's operation does on a thread of its own: its answer flat, or why it failed.
pub type Job = Box<dyn FnOnce() -> Result<Vec<u8>, String> + Send>;

/// Which of the process's pools a job waits for a thread in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Work {
    /// It fills a core: in line for as many threads as the machine has cores, so a burst is a
    /// line rather than a refusal and more at once would finish none sooner.
    Computing,
    /// It waits on something outside the process: a thread of its own, a bounded number at once.
    Waiting,
}

struct Jobs {
    computing: Pool,
    waiting: Pool,
}

fn jobs() -> &'static Jobs {
    static JOBS: OnceLock<Jobs> = OnceLock::new();
    JOBS.get_or_init(|| Jobs {
        computing: Pool::queued(std::thread::available_parallelism().map_or(1, |n| n.get())),
        waiting: Pool::new(),
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
    let pool = match work {
        Work::Computing => &jobs().computing,
        Work::Waiting => &jobs().waiting,
    };
    pool.submit(
        run,
        span,
        label,
        what,
        Box::new(move || match job() {
            Ok(bytes) => JobOutput::Bytes(bytes),
            Err(why) => JobOutput::Failed(why),
        }),
    )
}
