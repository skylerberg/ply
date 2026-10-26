//! The process's pools a native's job runs on, and the runtimes that route its ticket.

use ply_core::{Span, Value};
use ply_host::jobs::{self, Work};

fn bytes_of(answer: Result<Value, ply_core::Diagnostic>) -> Vec<u8> {
    match &answer {
        Ok(Value::Bytes(bytes)) => bytes.to_vec(),
        other => panic!("not bytes: {other:?}"),
    }
}

/// A run with no host waits on its natives' jobs through a runtime over the jobs alone, and a
/// host's runtime routes the same tickets.
#[test]
fn a_jobs_ticket_is_answered_through_a_bare_runtime_and_a_hosts() {
    let bare = jobs::runtime();
    for work in [Work::Computing, Work::Waiting] {
        let ticket = jobs::submit(work, Span::DUMMY, "test", "a test job", || {
            Ok(b"answered".to_vec())
        })
        .expect("a thread is had");
        assert_eq!(bytes_of(bare.block_on(ticket)), b"answered");
    }
    let host = ply_host::Host::new().runtime();
    let ticket = jobs::submit(Work::Computing, Span::DUMMY, "test", "a test job", || {
        Ok(vec![1, 2, 3])
    })
    .expect("a thread is had");
    assert_eq!(bytes_of(host.block_on(ticket)), [1, 2, 3]);
}

/// A production region parks a task on its ticket and collects it once it resolves.
#[test]
fn a_watched_ticket_resolves_into_the_runtime_that_watched_it() {
    let runtime = jobs::runtime();
    let ticket = jobs::submit(Work::Waiting, Span::DUMMY, "test", "a test job", || {
        Ok(b"late".to_vec())
    })
    .expect("a thread is had");
    runtime.watch(&ticket).expect("the jobs minted it");
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
    let ticket = jobs::submit(Work::Computing, Span::DUMMY, "test", "`kdf.argon2`", || {
        Err("no memory to be had".to_string())
    })
    .expect("a thread is had");
    let failed = jobs::runtime()
        .block_on(ticket)
        .expect_err("the job failed");
    assert!(
        failed.message.contains("`kdf.argon2`") && failed.message.contains("no memory to be had"),
        "{}",
        failed.message
    );
}

/// The bare runtime holds no configuration, so a run granted none reads none.
#[test]
fn a_bare_runtime_is_granted_no_configuration() {
    assert_eq!(jobs::runtime().configured("PATH", false), None);
}
