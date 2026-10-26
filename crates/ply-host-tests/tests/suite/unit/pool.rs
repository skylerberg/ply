use ply_core::host::MachineId;
use ply_core::{Diagnostic, Pending, Span, Value, codes};
use ply_host::pool::{Bell, JobOutput, Kept, MAX_BLOCKING_OPERATIONS, Pool, Run};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

fn run() -> Arc<Run> {
    Run::new(Arc::new(Bell::default()))
}

/// What a job that answers `n` answers.
fn said(n: i64) -> JobOutput {
    Ok(n.to_be_bytes().to_vec())
}

/// What a run reads of a job that answered `n`.
fn heard(n: i64) -> Value {
    Value::bytes(n.to_be_bytes())
}

fn submitted(pool: &Pool, run: &Arc<Run>, answer: i64) -> Pending {
    pool.submit(
        run,
        Span::DUMMY,
        "test",
        "a test job",
        Box::new(move || said(answer)),
    )
    .expect("the pool takes the job")
}

fn answered(run: &Run, pending: Pending) -> Value {
    run.block_on(&pending, None)
        .unwrap_or_else(|d| panic!("{d:?}"))
        .expect("an unbounded wait ends answered")
}

/// What `run` takes once a watched operation has been answered: each answered token beside its
/// answer.
fn collected(run: &Run) -> Vec<(u64, Result<Value, Diagnostic>)> {
    assert!(
        run.park(Some(Duration::from_secs(10)), &|| false),
        "the run watches something"
    );
    (run.answered().into_iter())
        .map(|token| {
            let pending = Pending {
                token,
                label: "a test job",
            };
            let answer = run.poll(&pending).map(|v| v.expect("answered"));
            (token, answer)
        })
        .collect()
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
        [(b.token, heard(2))]
    );
    assert!(mine.answered().is_empty(), "an answer is handed back once");
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
    assert_eq!(answered(&after, pending), heard(9));
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
        assert_eq!(answered(&asking, token), heard(i));
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
        assert_eq!(answered(&asking, after), heard(7));
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
        "a park ends at the run's own answer, not at another run's ring"
    );
    assert!(
        !run().park(None, &|| false),
        "nothing watched, nothing to wait for"
    );
}

fn secret(text: &str) -> Kept {
    Kept::new(text.as_bytes().to_vec())
}

/// What is read for a `Secret` is its run's, taken once, by that run alone.
#[test]
fn a_secret_is_taken_once_and_by_the_run_that_kept_it() {
    let (mine, theirs) = (run(), run());
    let kept = mine.keep_secret(secret("hunter2"));
    assert_ne!(kept, mine.keep_secret(secret("other")));
    assert!(
        theirs.take_secret(kept).is_none(),
        "another run's number names nothing of this one's"
    );
    assert_eq!(
        mine.take_secret(kept).as_deref().map(Vec::as_slice),
        Some(&b"hunter2"[..])
    );
    assert!(mine.take_secret(kept).is_none(), "a secret is taken once");
}

/// A run whose entry point ends between reading a secret and taking it leaves nothing held.
#[test]
fn a_secret_an_entry_point_did_not_take_is_dropped_when_it_ends() {
    let runtime = ply_host::Host::new().runtime();
    let run = Arc::clone(ply_host::registry::run_of(&*runtime).expect("a host made it"));
    let left = run.keep_secret(secret("hunter2"));
    let _ = runtime.end_entry_point(MachineId::next());
    assert!(
        run.take_secret(left).is_none(),
        "the line outlived its entry point"
    );
}
