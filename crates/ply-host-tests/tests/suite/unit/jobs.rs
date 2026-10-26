//! The process's pools a native's job runs on, each job its run's.

use ply_core::{HostRuntime, Span, Value};
use ply_host::jobs::{self, Work};
use ply_host::registry::{facilities_of, run_of, unbound};

fn bytes_of(answer: Result<Value, ply_core::Diagnostic>) -> Vec<u8> {
    match &answer {
        Ok(Value::Bytes(bytes)) => bytes.to_vec(),
        other => panic!("not bytes: {other:?}"),
    }
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
        assert_eq!(bytes_of(unbound.block_on(ticket)), b"answered");
    }
    let host = ply_host::Host::new().runtime();
    let ticket = submitted(&*host, Work::Computing, "a test job", || Ok(vec![1, 2, 3]));
    assert_eq!(bytes_of(host.block_on(ticket)), [1, 2, 3]);
}

/// A production region parks a task on its ticket and collects it once it resolves.
#[test]
fn a_watched_ticket_resolves_into_the_runtime_that_watched_it() {
    let runtime = ply_host::Host::new().runtime();
    let ticket = submitted(&*runtime, Work::Waiting, "a test job", || {
        Ok(b"late".to_vec())
    });
    runtime.watch(&ticket).expect("the run submitted it");
    let resolved = loop {
        runtime.park().expect("one ticket is outstanding");
        let resolved = runtime.resolved();
        if !resolved.is_empty() {
            break resolved;
        }
    };
    let [(token, answer)] = <[_; 1]>::try_from(resolved).expect("one ticket resolved");
    assert_eq!(token, ticket.token);
    assert_eq!(bytes_of(answer), b"late");
}

/// A job that answers why it failed fails the run that waits on it, naming what asked.
#[test]
fn a_failed_job_is_a_diagnostic_naming_what_asked_for_it() {
    let runtime = ply_host::Host::new().runtime();
    let ticket = submitted(&*runtime, Work::Computing, "`kdf.argon2`", || {
        Err("no memory to be had".to_string())
    });
    let failed = runtime.block_on(ticket).expect_err("the job failed");
    assert!(
        failed.message.contains("`kdf.argon2`") && failed.message.contains("no memory to be had"),
        "{}",
        failed.message
    );
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
