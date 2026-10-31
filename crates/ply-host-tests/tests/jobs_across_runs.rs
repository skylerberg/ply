use ply_core::Span;
use ply_core::host::HostRuntime;
use ply_host::jobs::{self, Work};
use ply_host::registry::{job_bytes, run_of, unbound};
use std::sync::mpsc;
use std::time::Duration;

fn submitted(
    runtime: &dyn HostRuntime,
    work: Work,
    what: &'static str,
    job: impl FnOnce() -> Result<Vec<u8>, String> + Send + 'static,
) -> ply_core::Pending {
    let run = run_of(runtime).expect("a host made the runtime");
    jobs::submit(work, run, Span::DUMMY, "test", what, job).expect("a thread is had")
}

fn abandon_a_finished_job() {
    let gone = unbound()();
    let (ran, heard) = mpsc::channel();
    let ticket = submitted(&*gone, Work::Computing, "an abandoned hash", move || {
        let _ = ran.send(());
        Ok(vec![0])
    });
    (run_of(&*gone).expect("a host made the runtime"))
        .watch(&ticket)
        .expect("the run submitted it");
    heard.recv().expect("the job ran");
}

#[test]
fn a_run_still_waits_for_its_own_job_when_an_earlier_run_ended_without_reading_its_finished_job_result()
 {
    abandon_a_finished_job();
    let rt = unbound()();
    let ticket = submitted(&*rt, Work::Computing, "a hash", || {
        std::thread::sleep(Duration::from_millis(200));
        Ok(b"hashed".to_vec())
    });
    let run = run_of(&*rt).expect("a host made the runtime");
    run.watch(&ticket).expect("the run submitted it");
    while run.returned().is_empty() {
        rt.park().expect("the run watches its own job");
    }
    assert_eq!(run.returned(), vec![ticket.token]);
    assert_eq!(
        run.taken(&ticket).expect("its own ticket"),
        Some(b"hashed".to_vec())
    );
}

#[test]
fn a_waiting_job_that_panics_fails_the_run_that_waits_on_it() {
    let (returned, heard) = mpsc::channel();
    std::thread::spawn(move || {
        let runtime = unbound()();
        let ticket = submitted(
            &*runtime,
            Work::Waiting,
            "`a waiting job`",
            || -> Result<Vec<u8>, String> { panic!("the job broke") },
        );
        let outcome = job_bytes(&*runtime, &ticket);
        let _ = returned.send(outcome.map(|_| ()).map_err(|d| d.message));
    });
    match heard.recv_timeout(Duration::from_secs(10)) {
        Ok(Err(message)) => assert!(message.contains("`a waiting job`"), "{message}"),
        Ok(Ok(())) => panic!("a job that panicked returned a result"),
        Err(_) => panic!("a waiting job panicked and the run waiting on it was never woken"),
    }
}

#[test]
fn a_run_cannot_watch_or_read_the_result_of_a_job_that_a_different_run_submitted() {
    let factory = unbound();
    let (mine, theirs) = (factory(), factory());
    let ticket = submitted(&*mine, Work::Waiting, "a lookup", || Ok(b"mine".to_vec()));
    assert!(
        (run_of(&*theirs).expect("a host made the runtime"))
            .watch(&ticket)
            .is_err(),
        "another run may not watch it"
    );
    assert!(job_bytes(&*theirs, &ticket).is_err(), "nor wait on it");
    match job_bytes(&*mine, &ticket) {
        Ok(bytes) => assert_eq!(bytes, b"mine"),
        Err(d) => panic!("the run that submitted it got no result: {}", d.message),
    }
}
