//! The process's pools a native's job runs on, each job its run's.

use ply_core::{HostRuntime, Pending, Span, codes};
use ply_host::jobs::{self, Work};
use ply_host::registry::{facilities_of, job_bytes, run_of, unbound};
use ply_host::signal::{Bounds, Shutdown, ShutdownSignal};
use std::time::{Duration, Instant};

fn answered(runtime: &dyn HostRuntime, ticket: &Pending) -> Vec<u8> {
    job_bytes(runtime, ticket).unwrap_or_else(|d| panic!("the job was not answered: {}", d.message))
}

fn submitted(
    runtime: &dyn HostRuntime,
    work: Work,
    what: &'static str,
    job: impl FnOnce() -> Result<Vec<u8>, String> + Send + 'static,
) -> ply_core::Pending {
    let run = run_of(runtime).expect("a host made the runtime");
    jobs::submit(work, run, Span::DUMMY, "test", what, job).expect("a thread is had")
}

/// A run no host was bound for waits on its natives' jobs through a host of its own, and a bound
/// host's runtime waits on its own the same way.
#[test]
fn a_jobs_ticket_is_answered_through_an_unbound_runtime_and_a_hosts() {
    let unbound = unbound()();
    for work in [Work::Computing, Work::Waiting] {
        let ticket = submitted(&*unbound, work, "a test job", || Ok(b"answered".to_vec()));
        assert_eq!(answered(&*unbound, &ticket), b"answered");
    }
    let host = ply_host::Host::new().runtime();
    let ticket = submitted(&*host, Work::Computing, "a test job", || Ok(vec![1, 2, 3]));
    assert_eq!(answered(&*host, &ticket), [1, 2, 3]);
}

/// The run's region waits on a ticket it watches and takes its answer once it is answered.
#[test]
fn a_watched_ticket_is_answered_to_the_run_that_watched_it() {
    let runtime = ply_host::Host::new().runtime();
    let ticket = submitted(&*runtime, Work::Waiting, "a test job", || {
        Ok(b"late".to_vec())
    });
    let run = run_of(&*runtime).expect("a host made the runtime");
    run.watch(&ticket).expect("the run submitted it");
    while run.answered().is_empty() {
        runtime.park().expect("one ticket is outstanding");
    }
    assert_eq!(run.answered(), [ticket.token]);
    assert_eq!(
        run.taken(&ticket).expect("its own ticket"),
        Some(b"late".to_vec())
    );
}

/// A job that answers why it failed fails the run that waits on it, naming what asked.
#[test]
fn a_failed_job_is_a_diagnostic_naming_what_asked_for_it() {
    let runtime = ply_host::Host::new().runtime();
    let ticket = submitted(&*runtime, Work::Computing, "`kdf.argon2`", || {
        Err("no memory to be had".to_string())
    });
    let failed = job_bytes(&*runtime, &ticket).expect_err("the job failed");
    assert!(
        failed.message.contains("`kdf.argon2`") && failed.message.contains("no memory to be had"),
        "{}",
        failed.message
    );
}

/// A wait begun past the drain deadline ends the run though its job answers at once, so a serve
/// loop of quick reads still stops there.
#[test]
fn a_wait_past_the_drain_deadline_ends_the_run_however_soon_its_job_answers() {
    let shutdown = Shutdown::new(Bounds {
        lead: Duration::ZERO,
        drain: Duration::from_millis(30),
    });
    let runtime = ply_host::Host::new()
        .stopping_on(shutdown.clone())
        .runtime();
    let quick = || Ok(b"read".to_vec());
    let ticket = submitted(&*runtime, Work::Waiting, "a test job", quick);
    assert_eq!(answered(&*runtime, &ticket), b"read");

    assert!(shutdown.request(ShutdownSignal::Terminate));
    let until = Instant::now() + Duration::from_secs(5);
    while !shutdown.drain_expired() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(shutdown.drain_expired(), "a 30ms drain never expired");

    let ticket = submitted(&*runtime, Work::Waiting, "a test job", quick);
    let ended = job_bytes(&*runtime, &ticket).expect_err("the drain is out of time");
    assert_eq!(ended.code, codes::DRAIN_INCOMPLETE);
}

/// A job's refusal of its own fails the run as it stands, its code kept.
#[test]
fn a_jobs_own_refusal_fails_the_run_with_its_code() {
    let runtime = ply_host::Host::new().runtime();
    let run = run_of(&*runtime).expect("a host made the runtime");
    let ticket = jobs::refusing(
        Work::Waiting,
        run,
        Span::DUMMY,
        "test",
        "a test job",
        || {
            Err(jobs::Failure::Refused(ply_core::Diagnostic::error(
                codes::PROCESS_OUTPUT_TOO_LARGE,
                "too much",
            )))
        },
    )
    .expect("a thread is had");
    let refused = job_bytes(&*runtime, &ticket).expect_err("the job refused");
    assert_eq!(refused.code, codes::PROCESS_OUTPUT_TOO_LARGE);
    assert_eq!(refused.message, "too much");
}

/// Every runtime one unbound factory makes, as each `parallel` branch makes its own, is over the
/// one host: a socket a branch opens is in the table the run reads.
#[test]
fn an_unbound_factorys_runtimes_share_one_host() {
    let factory = unbound();
    let (first, second) = (factory(), factory());
    let sockets = |runtime: &dyn HostRuntime| {
        let facilities = facilities_of(runtime).expect("a host made it");
        std::sync::Arc::as_ptr(facilities.sockets())
    };
    assert_eq!(sockets(&*first), sockets(&*second));
    let other = unbound()();
    assert_ne!(sockets(&*first), sockets(&*other));
}
