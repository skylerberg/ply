use ply_eval::{Diagnostic, Pending, Span, Value, codes};
use ply_host::pool::{Inbox, JobOutput, MAX_BLOCKING_OPERATIONS, Pool};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

fn submitted(pool: &Pool, answer: i64) -> Pending {
    pool.submit(
        Span::DUMMY,
        "test",
        "a test job",
        Box::new(move || JobOutput::Int(answer)),
    )
    .expect("the pool takes the job")
}

/// What `inbox` collects once something has been delivered to it.
fn collected(pool: &Pool, inbox: &Inbox) -> Vec<(u64, Result<Value, Diagnostic>)> {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let answers = pool.collect(inbox);
        if !answers.is_empty() {
            return answers;
        }
        assert!(Instant::now() < until, "nothing was delivered");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn answered(answers: Vec<(u64, Result<Value, Diagnostic>)>) -> Vec<(u64, Value)> {
    answers
        .into_iter()
        .map(|(token, answer)| (token, answer.unwrap_or_else(|d| panic!("{d:?}"))))
        .collect()
}

/// Every machine's runtime shares the host's pools, so each collects the tokens it watched alone.
#[test]
fn an_inbox_collects_the_tokens_it_watched_and_none_it_did_not() {
    let pool = Pool::new();
    let (mine, theirs) = (Arc::new(Inbox::default()), Arc::new(Inbox::default()));
    let a = submitted(&pool, 1);
    let b = submitted(&pool, 2);
    pool.watch(&a, &mine).expect("this pool minted it");
    pool.watch(&b, &theirs).expect("this pool minted it");

    assert_eq!(
        answered(collected(&pool, &mine)),
        [(a.token, Value::Int(1))]
    );
    assert_eq!(
        answered(collected(&pool, &theirs)),
        [(b.token, Value::Int(2))]
    );
    assert!(
        pool.collect(&mine).is_empty(),
        "a token is handed back once"
    );
    assert_eq!(pool.outstanding(), 0);
}

/// Watching races the job, so a token that resolved first is delivered on the spot.
#[test]
fn a_token_watched_after_it_resolved_is_delivered_at_once() {
    let pool = Pool::new();
    let inbox = Arc::new(Inbox::default());
    let token = submitted(&pool, 3);
    let until = Instant::now() + Duration::from_secs(10);
    while !pool.ready() {
        assert!(Instant::now() < until, "the job never finished");
        std::thread::sleep(Duration::from_millis(1));
    }
    pool.watch(&token, &inbox).expect("this pool minted it");
    assert_eq!(
        answered(pool.collect(&inbox)),
        [(token.token, Value::Int(3))]
    );
}

#[test]
fn a_token_this_pool_did_not_mint_cannot_be_watched() {
    let pool = Pool::new();
    let stray = submitted(&Pool::new(), 0);
    assert!(!pool.owns(&stray));
    let refused = pool
        .watch(&stray, &Arc::new(Inbox::default()))
        .expect_err("nothing here minted it");
    assert_eq!(refused.code, codes::INTERNAL_ERROR);
}

/// A pool that gives each operation a thread refuses the one past its bound; a queued one has no
/// such bound, and answers each in its turn.
#[test]
fn a_queued_pool_answers_more_operations_than_a_thread_each_would_be_given() {
    let pool = Pool::queued(2);
    let asked: Vec<(i64, Pending)> = (0..4 * MAX_BLOCKING_OPERATIONS as i64)
        .map(|i| (i, submitted(&pool, i)))
        .collect();
    for (i, token) in asked {
        assert_eq!(pool.block_on(token).expect("answered"), Value::Int(i));
    }
    assert_eq!(pool.outstanding(), 0);
}

#[test]
fn a_queued_pool_runs_no_more_at_once_than_it_has_threads() {
    let pool = Pool::queued(3);
    let running = Arc::new(AtomicUsize::new(0));
    let most = Arc::new(AtomicUsize::new(0));
    let tokens: Vec<Pending> = (0..24)
        .map(|_| {
            let (running, most) = (Arc::clone(&running), Arc::clone(&most));
            pool.submit(
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
        pool.block_on(token).expect("answered");
    }
    let most = most.load(Ordering::SeqCst);
    assert!(
        (2..=3).contains(&most),
        "{most} ran at once on three threads"
    );
}

/// A queued pool's thread outlives the job it ran, so a job's panic must not take the thread, or
/// the operations behind it would wait forever.
#[test]
fn a_job_that_panics_fails_its_own_operation_and_the_next_is_answered() {
    let pool = Pool::queued(1);
    let broken = pool
        .submit(
            Span::DUMMY,
            "test",
            "a test job",
            Box::new(|| -> JobOutput { panic!("a job's own defect") }),
        )
        .expect("the pool takes the job");
    let after = submitted(&pool, 7);
    let failed = pool
        .block_on(broken)
        .expect_err("a panic is the operation's failure");
    assert_eq!(failed.code, codes::RUNTIME_ERROR);
    assert_eq!(pool.block_on(after).expect("answered"), Value::Int(7));
}

/// The first facility to claim a token answers it, so one that two pools minted would hang a poll
/// forever, and a zeroed `Pending` carries 0.
#[test]
fn no_two_pools_mint_the_same_token_and_none_mints_zero() {
    let pools = [Pool::new(), Pool::new(), Pool::queued(1)];
    // Each pool mints on a thread of its own, so they mint at once.
    let minted: Vec<Vec<Pending>> = std::thread::scope(|scope| {
        let minting: Vec<_> = pools
            .iter()
            .map(|pool| scope.spawn(move || (0..32).map(|i| submitted(pool, i)).collect()))
            .collect();
        minting
            .into_iter()
            .map(|thread| thread.join().expect("the pool takes each job"))
            .collect()
    });
    let mut tokens = BTreeSet::new();
    for (pool, minted) in pools.iter().zip(minted) {
        for pending in minted {
            assert_ne!(pending.token, 0);
            assert!(tokens.insert(pending.token), "`{pending}` was minted twice");
            pool.block_on(pending).expect("answered");
        }
    }
}
