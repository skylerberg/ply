use ply_core::{Diagnostic, Pending, Span, Value, codes};
use ply_host::pool::{Bell, JobOutput, MAX_BLOCKING_OPERATIONS, Pool, Run};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

fn run() -> Arc<Run> {
    Run::new(Arc::new(Bell::default()))
}

fn submitted(pool: &Pool, run: &Arc<Run>, answer: i64) -> Pending {
    pool.submit(
        run,
        Span::DUMMY,
        "test",
        "a test job",
        Box::new(move || JobOutput::Int(answer)),
    )
    .expect("the pool takes the job")
}

fn answered(run: &Run, pending: Pending) -> Value {
    run.block_on(&pending, None)
        .unwrap_or_else(|d| panic!("{d:?}"))
        .expect("an unbounded wait ends answered")
}

/// What `run` collects once a watched operation has been answered.
fn collected(run: &Run) -> Vec<(u64, Result<Value, Diagnostic>)> {
    assert!(
        run.park(Some(Duration::from_secs(10)), &|| false),
        "the run watches something"
    );
    run.resolved()
}

/// Every machine's runtime shares the host's pools, so each collects only what it submitted.
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
    let mine_answered = collected(&mine);
    assert_eq!(mine_answered.len(), 1);
    assert_eq!(mine_answered[0].0, a.token);
    assert_eq!(
        collected(&theirs)
            .into_iter()
            .map(|(token, answer)| (token, answer.expect("answered")))
            .collect::<Vec<_>>(),
        [(b.token, Value::Int(2))]
    );
    assert!(mine.resolved().is_empty(), "an answer is handed back once");
}

/// A job that finishes after its run has ended answers no one, and keeps no thread.
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
                // Each holds until the test's sender is gone.
                let _ = released.lock().map(|r| r.recv());
                JobOutput::Int(0)
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
            Box::new(|| JobOutput::Int(0))
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
            Box::new(|| JobOutput::Int(9)),
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
    assert_eq!(answered(&after, pending), Value::Int(9));
}

/// A pool that gives each operation a thread refuses the one past its bound; a queued one has no
/// such bound, and answers each in its turn.
#[test]
fn a_queued_pool_answers_more_operations_than_a_thread_each_would_be_given() {
    let pool = Pool::queued(2);
    let asking = run();
    let asked: Vec<(i64, Pending)> = (0..4 * MAX_BLOCKING_OPERATIONS as i64)
        .map(|i| (i, submitted(&pool, &asking, i)))
        .collect();
    for (i, token) in asked {
        assert_eq!(answered(&asking, token), Value::Int(i));
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
                    JobOutput::Int(0)
                }),
            )
            .expect("the pool takes the job")
        })
        .collect();
    for token in tokens {
        answered(&asking, token);
    }
    let most = most.load(Ordering::SeqCst);
    assert!(
        (2..=3).contains(&most),
        "{most} ran at once on three threads"
    );
}

/// A job that panics fails its own operation, whichever pool runs it, and a queued pool's thread
/// goes on to the next.
#[test]
fn a_job_that_panics_fails_its_own_operation_and_the_next_is_answered() {
    for pool in [Pool::queued(1), Pool::new()] {
        let asking = run();
        let broken = pool
            .submit(
                &asking,
                Span::DUMMY,
                "test",
                "a test job",
                Box::new(|| -> JobOutput { panic!("a job's own defect") }),
            )
            .expect("the pool takes the job");
        let after = submitted(&pool, &asking, 7);
        let failed = asking
            .block_on(&broken, None)
            .expect_err("a panic is the operation's failure");
        assert_eq!(failed.code, codes::RUNTIME_ERROR);
        assert_eq!(answered(&asking, after), Value::Int(7));
    }
}

/// A token is a run's, so one minted twice would answer the wrong run, and a zeroed `Pending`
/// carries 0.
#[test]
fn no_two_pools_mint_the_same_token_and_none_mints_zero() {
    let pools = [Pool::new(), Pool::new(), Pool::queued(1)];
    let asking = run();
    // Each pool mints on a thread of its own, so they mint at once.
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
        answered(&asking, pending);
    }
}

/// A park waits for the run's own watched answer, through every ring of a bell other runs share.
#[test]
fn a_park_wakes_for_its_own_answer_and_not_for_anothers() {
    let bell = Arc::new(Bell::default());
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
                JobOutput::Int(1)
            }),
        )
        .expect("the pool takes the job");
    mine.watch(&slow).expect("its own");
    let quick = submitted(&pool, &theirs, 2);
    theirs.watch(&quick).expect("its own");
    assert!(mine.park(None, &|| false), "it watches one");
    assert!(
        mine.ready(),
        "a park ends at the run's own answer, not at another run's ring"
    );
    assert!(
        !run().park(None, &|| false),
        "nothing watched, nothing to wait for"
    );
}
