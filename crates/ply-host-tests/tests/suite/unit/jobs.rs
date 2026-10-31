use ply_core::{HostRuntime, Pending, Span, codes};
use ply_host::jobs::{self, Work};
use ply_host::registry::{facilities_of, job_bytes, run_of, unbound};
use ply_host::signal::{Bounds, Shutdown, ShutdownSignal};
use std::time::{Duration, Instant};

fn returned(runtime: &dyn HostRuntime, ticket: &Pending) -> Vec<u8> {
    job_bytes(runtime, ticket)
        .unwrap_or_else(|d| panic!("the job returned no result: {}", d.message))
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

#[test]
fn a_submitted_job_returns_its_result_on_a_runtime_made_by_a_factory_and_on_a_runtime_made_by_a_host()
 {
    let unbound = unbound()();
    for work in [Work::Computing, Work::Waiting] {
        let ticket = submitted(&*unbound, work, "a test job", || Ok(b"answered".to_vec()));
        assert_eq!(returned(&*unbound, &ticket), b"answered");
    }
    let host = ply_host::Host::new().runtime();
    let ticket = submitted(&*host, Work::Computing, "a test job", || Ok(vec![1, 2, 3]));
    assert_eq!(returned(&*host, &ticket), [1, 2, 3]);
}

#[test]
fn a_finished_job_is_listed_as_returned_by_the_run_that_watches_it_and_the_run_can_then_take_its_result()
 {
    let runtime = ply_host::Host::new().runtime();
    let ticket = submitted(&*runtime, Work::Waiting, "a test job", || {
        Ok(b"late".to_vec())
    });
    let run = run_of(&*runtime).expect("a host made the runtime");
    run.watch(&ticket).expect("the run submitted it");
    while run.returned().is_empty() {
        runtime.park().expect("one ticket is outstanding");
    }
    assert_eq!(run.returned(), [ticket.token]);
    assert_eq!(
        run.taken(&ticket).expect("its own ticket"),
        Some(b"late".to_vec())
    );
}

#[test]
fn a_failed_job_returns_a_diagnostic_naming_the_operation_that_submitted_it() {
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

#[test]
fn a_job_submitted_after_the_shutdown_drain_time_has_run_out_fails_with_a_drain_incomplete_error() {
    let shutdown = Shutdown::new(Bounds {
        lead: Duration::ZERO,
        drain: Duration::from_millis(30),
    });
    let runtime = ply_host::Host::new()
        .stopping_on(shutdown.clone())
        .runtime();
    let quick = || Ok(b"read".to_vec());
    let ticket = submitted(&*runtime, Work::Waiting, "a test job", quick);
    assert_eq!(returned(&*runtime, &ticket), b"read");

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

#[test]
fn a_job_that_rejects_with_a_diagnostic_fails_the_run_with_that_diagnostic_s_code_and_message() {
    let runtime = ply_host::Host::new().runtime();
    let run = run_of(&*runtime).expect("a host made the runtime");
    let ticket = jobs::refusing(
        Work::Waiting,
        run,
        Span::DUMMY,
        "test",
        "a test job",
        || {
            Err(jobs::Failure::Rejected(ply_core::Diagnostic::error(
                codes::PROCESS_OUTPUT_TOO_LARGE,
                "too much",
            )))
        },
    )
    .expect("a thread is had");
    let rejected = job_bytes(&*runtime, &ticket).expect_err("the job failed");
    assert_eq!(rejected.code, codes::PROCESS_OUTPUT_TOO_LARGE);
    assert_eq!(rejected.message, "too much");
}

#[test]
fn runtimes_made_by_one_factory_use_the_same_sockets_and_runtimes_made_by_another_factory_use_different_sockets()
 {
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

#[test]
fn done_here_runs_a_job_on_the_calling_thread_and_its_result_is_available_at_once_even_if_the_job_panics()
 {
    let runtime = ply_host::Host::new().runtime();
    let run = run_of(&*runtime).expect("a host made the runtime");
    let asking = std::thread::current().id();
    let ticket = jobs::done_here(run, Span::DUMMY, "test", "a test job", move || {
        assert_eq!(std::thread::current().id(), asking);
        Ok(b"here".to_vec())
    });
    assert_eq!(
        run.taken(&ticket).expect("its own ticket"),
        Some(b"here".to_vec())
    );
    let ticket = jobs::done_here(run, Span::DUMMY, "test", "`fs.read_file`", || {
        panic!("a host operation panicked")
    });
    let failed = job_bytes(&*runtime, &ticket).expect_err("the job panicked");
    assert!(
        failed.message.contains("`fs.read_file`"),
        "{}",
        failed.message
    );
}
