//! Where a native's job runs when it would block the program's thread: two pools for the whole
//! process, whose tokens every runtime routes, so a test run's machines and a `parallel`'s
//! branches share one bound of cores between them.

use crate::pool::{Bell, JobOutput, Pool, Pooled, Routing, err_nothing_outstanding};
use ply_core::host::{HostRuntime, Pending};
use ply_core::{Diagnostic, Span, Value};
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

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

fn pool(work: Work) -> &'static Pool {
    match work {
        Work::Computing => &jobs().computing,
        Work::Waiting => &jobs().waiting,
    }
}

/// `job` run on a thread of `work`'s pool, named `label`, answering the token a wait reads its
/// bytes by. A job that answers why it failed, or panics, fails the run that waits on it, naming
/// `what`.
///
/// # Errors
/// No thread could be had for it: the waiting pool's bound was reached, or one could not start.
pub fn submit(
    work: Work,
    span: Span,
    label: &'static str,
    what: &'static str,
    job: impl FnOnce() -> Result<Vec<u8>, String> + Send + 'static,
) -> Result<Pending, Diagnostic> {
    pool(work).submit(
        span,
        label,
        what,
        Box::new(move || match job() {
            Ok(bytes) => JobOutput::Bytes(bytes),
            Err(why) => JobOutput::Failed(why),
        }),
    )
}

struct Of(Work);

impl Pooled for Of {
    fn pool(&self) -> &Pool {
        pool(self.0)
    }
}

/// Each of the process's job pools, as a facility a runtime routes tokens to.
pub fn facilities() -> Vec<Arc<dyn Pooled>> {
    vec![Arc::new(Of(Work::Computing)), Arc::new(Of(Work::Waiting))]
}

/// A runtime over the jobs alone, for a run that no host facility serves: it routes and waits on
/// a native's tickets, holds no configuration and has no clock.
pub fn runtime() -> Rc<dyn HostRuntime> {
    Rc::new(Alone {
        routing: Routing::over(facilities(), Arc::new(Bell::default())),
    })
}

struct Alone {
    routing: Routing,
}

impl HostRuntime for Alone {
    fn watch(&self, pending: &Pending) -> Result<(), Diagnostic> {
        self.routing.watch(pending)
    }

    fn resolved(&self) -> Vec<(u64, Result<Value, Diagnostic>)> {
        self.routing.resolved()
    }

    fn park(&self) -> Result<(), Diagnostic> {
        if self.routing.park(None) {
            Ok(())
        } else {
            Err(err_nothing_outstanding())
        }
    }

    fn block_on(&self, pending: Pending) -> Result<Value, Diagnostic> {
        self.routing.owner(&pending)?.block_on(pending)
    }
}
