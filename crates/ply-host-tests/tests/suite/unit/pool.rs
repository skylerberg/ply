use ply_core::host::MachineId;
use ply_core::{Diagnostic, Pending, Span, codes};
use ply_host::pool::{JobOutput, MAX_BLOCKING_OPERATIONS, Pool, Run, SecretBytes, WakeCounter};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

fn run() -> Arc<Run> {
    Run::new(Arc::new(WakeCounter::default()))
}

fn written(n: i64) -> Vec<u8> {
    n.to_be_bytes().to_vec()
}

fn said(n: i64) -> JobOutput {
    Ok(written(n))
}

fn submitted(pool: &Pool, run: &Arc<Run>, result: i64) -> Pending {
    pool.submit(
        run,
        Span::DUMMY,
        "test",
        "a test job",
        Box::new(move || said(result)),
    )
    .expect("the pool takes the job")
}

fn returned(run: &Run, pending: Pending) -> Vec<u8> {
    run.block_on(&pending, None)
        .unwrap_or_else(|d| panic!("{d:?}"))
        .expect("an unbounded wait ends with a result")
}

fn collected(run: &Run) -> Vec<(u64, Result<Vec<u8>, Diagnostic>)> {
    assert!(
        run.park(Some(Duration::from_secs(10)), &|| false),
        "the run watches something"
    );
    (run.returned().into_iter())
        .map(|token| {
            let pending = Pending {
                token,
                label: "a test job",
            };
            let result = run.taken(&pending).map(|v| v.expect("returned"));
            (token, result)
        })
        .collect()
}

#[test]
fn a_run_collects_its_own_operations_and_none_of_anothers() {
    let pool = Pool::new();
    let (mine, theirs) = (run(), run());
    let a = submitted(&pool, &mine, 1);
    let b = submitted(&pool, &theirs, 2);
    assert_eq!(
        theirs.watch(&a).expect_err("another run's").code,
        codes::INTERNAL_ERROR
    );
    mine.watch(&a).expect("its own");
    theirs.watch(&b).expect("its own");
    let mine_returned = collected(&mine);
    assert_eq!(mine_returned.len(), 1);
    assert_eq!(mine_returned[0].0, a.token);
    assert_eq!(
        collected(&theirs)
            .into_iter()
            .map(|(token, result)| (token, result.expect("returned")))
            .collect::<Vec<_>>(),
        [(b.token, written(2))]
    );
    assert!(mine.returned().is_empty(), "a result is handed back once");
}

#[test]
fn an_ended_runs_operation_is_dropped_and_frees_its_thread() {
    let pool = Pool::new();
    let (release, released) = mpsc::channel::<()>();
    let released = Arc::new(std::sync::Mutex::new(released));
    let ended = run();
    for _ in 0..MAX_BLOCKING_OPERATIONS {
        let released = Arc::clone(&released);
        pool.submit(
            &ended,
            Span::DUMMY,
            "test",
            "a held job",
            Box::new(move || {
                let _ = released.lock().map(|r| r.recv());
                said(0)
            }),
        )
        .expect("the pool takes the job");
    }
    assert!(
        pool.submit(
            &run(),
            Span::DUMMY,
            "test",
            "one too many",
            Box::new(|| said(0))
        )
        .is_err(),
        "every thread is held"
    );
    drop(ended);
    drop(release);
    let after = run();
    let until = std::time::Instant::now() + Duration::from_secs(10);
    let pending = loop {
        match pool.submit(
            &after,
            Span::DUMMY,
            "test",
            "a later job",
            Box::new(|| said(9)),
        ) {
            Ok(pending) => break pending,
            Err(_) => {
                assert!(
                    std::time::Instant::now() < until,
                    "the ended run's jobs never gave their threads back"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    };
    assert_eq!(returned(&after, pending), written(9));
}

#[test]
fn a_queued_pool_handles_more_operations_than_it_has_threads() {
    let pool = Pool::queued(2);
    let asking = run();
    let asked: Vec<(i64, Pending)> = (0..4 * MAX_BLOCKING_OPERATIONS as i64)
        .map(|i| (i, submitted(&pool, &asking, i)))
        .collect();
    for (i, token) in asked {
        assert_eq!(returned(&asking, token), written(i));
    }
}

#[test]
fn a_queued_pool_runs_no_more_at_once_than_it_has_threads() {
    let pool = Pool::queued(3);
    let asking = run();
    let running = Arc::new(AtomicUsize::new(0));
    let most = Arc::new(AtomicUsize::new(0));
    let tokens: Vec<Pending> = (0..24)
        .map(|_| {
            let (running, most) = (Arc::clone(&running), Arc::clone(&most));
            pool.submit(
                &asking,
                Span::DUMMY,
                "test",
                "a test job",
                Box::new(move || {
                    most.fetch_max(running.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(20));
                    running.fetch_sub(1, Ordering::SeqCst);
                    said(0)
                }),
            )
            .expect("the pool takes the job")
        })
        .collect();
    for token in tokens {
        returned(&asking, token);
    }
    let most = most.load(Ordering::SeqCst);
    assert!(
        (2..=3).contains(&most),
        "{most} ran at once on three threads"
    );
}

#[test]
fn a_job_that_panics_fails_its_own_operation_without_affecting_the_next_job() {
    for pool in [Pool::queued(1), Pool::new()] {
        let asking = run();
        let broken = pool
            .submit(
                &asking,
                Span::DUMMY,
                "test",
                "a test job",
                Box::new(|| -> JobOutput { panic!("a job's own internal error") }),
            )
            .expect("the pool takes the job");
        let after = submitted(&pool, &asking, 7);
        let failed = asking
            .block_on(&broken, None)
            .expect_err("a panic is the operation's failure");
        assert_eq!(failed.code, codes::RUNTIME_ERROR);
        assert_eq!(returned(&asking, after), written(7));
    }
}

#[test]
fn no_two_pools_create_the_same_token_and_no_pool_creates_a_zero_token() {
    let pools = [Pool::new(), Pool::new(), Pool::queued(1)];
    let asking = run();
    let minted: Vec<Vec<Pending>> = std::thread::scope(|scope| {
        let minting: Vec<_> = pools
            .iter()
            .map(|pool| {
                let asking = Arc::clone(&asking);
                scope.spawn(move || (0..32).map(|i| submitted(pool, &asking, i)).collect())
            })
            .collect();
        minting
            .into_iter()
            .map(|thread| thread.join().expect("the pool takes each job"))
            .collect()
    });
    let mut tokens = BTreeSet::new();
    for pending in minted.into_iter().flatten() {
        assert_ne!(pending.token, 0);
        assert!(tokens.insert(pending.token), "`{pending}` was minted twice");
        returned(&asking, pending);
    }
}

#[test]
fn a_run_waiting_on_its_job_wakes_when_that_job_finishes_and_not_when_another_run_s_job_does() {
    let bell = Arc::new(WakeCounter::default());
    let (mine, theirs) = (Run::new(Arc::clone(&bell)), Run::new(Arc::clone(&bell)));
    let pool = Pool::new();
    let slow = pool
        .submit(
            &mine,
            Span::DUMMY,
            "test",
            "a slow job",
            Box::new(|| {
                std::thread::sleep(Duration::from_millis(200));
                said(1)
            }),
        )
        .expect("the pool takes the job");
    mine.watch(&slow).expect("its own");
    let quick = submitted(&pool, &theirs, 2);
    theirs.watch(&quick).expect("its own");
    assert!(mine.park(None, &|| false), "it watches one");
    assert!(
        mine.ready(),
        "a park ends at the run's own result, not at another run's ring"
    );
    assert!(
        !run().park(None, &|| false),
        "nothing watched, nothing to wait for"
    );
}

fn secret(text: &str) -> SecretBytes {
    SecretBytes::new(text.as_bytes().to_vec())
}

#[test]
fn a_stored_secret_can_be_removed_once_and_only_by_the_run_that_stored_it() {
    let (mine, theirs) = (run(), run());
    let kept = mine.store_secret(secret("hunter2"));
    assert_ne!(kept, mine.store_secret(secret("other")));
    assert!(
        theirs.remove_secret(kept).is_none(),
        "another run's number names nothing of this one's"
    );
    assert_eq!(
        mine.remove_secret(kept).as_deref().map(Vec::as_slice),
        Some(&b"hunter2"[..])
    );
    assert!(mine.remove_secret(kept).is_none(), "a secret is taken once");
}

#[test]
fn a_stored_secret_that_no_entry_point_removed_is_dropped_when_the_entry_point_ends() {
    let runtime = ply_host::Host::new().runtime();
    let run = Arc::clone(ply_host::registry::run_of(&*runtime).expect("a host made it"));
    let left = run.store_secret(secret("hunter2"));
    let _ = runtime.end_entry_point(MachineId::next());
    assert!(
        run.remove_secret(left).is_none(),
        "the line outlived its entry point"
    );
}
