use crate::pool::{MAX_BLOCKING_OPERATIONS, Pool, Run};
use ply_core::host::Pending;
use ply_core::{Diagnostic, Span};
use std::sync::{Arc, OnceLock};

pub type Job = Box<dyn FnOnce() -> Result<Vec<u8>, String> + Send>;

#[derive(Debug)]
pub enum Failure {
    Message(String),
    Rejected(Diagnostic),
}

pub type FallibleJob = Box<dyn FnOnce() -> Result<Vec<u8>, Failure> + Send>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Work {
    Computing,
    Waiting,
    WritingFiles,
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

pub fn submit(
    work: Work,
    run: &Arc<Run>,
    span: Span,
    label: &'static str,
    what: &'static str,
    job: impl FnOnce() -> Result<Vec<u8>, String> + Send + 'static,
) -> Result<Pending, Diagnostic> {
    queue_fallible(work, run, span, label, what, move || {
        job().map_err(Failure::Message)
    })
}

pub fn done_here(
    run: &Run,
    span: Span,
    label: &'static str,
    what: &'static str,
    job: impl FnOnce() -> Result<Vec<u8>, Failure> + Send + 'static,
) -> Pending {
    run.done_here(span, label, what, job)
}

pub fn queue_fallible(
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
        Work::WritingFiles => &jobs().filing,
    };
    pool.submit(run, span, label, what, Box::new(job))
}
