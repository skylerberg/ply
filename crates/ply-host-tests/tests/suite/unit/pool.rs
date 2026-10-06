use ply_eval::{Diagnostic, Pending, Span, Value, codes};
use ply_host::pool::{
    FS_FIRST_TOKEN, Inbox, JobOutput, MAX_BLOCKING_OPERATIONS, NET_FIRST_TOKEN,
    PASSWORD_FIRST_TOKEN, PROCESS_FIRST_TOKEN, Pool,
};
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
    let pool = Pool::new(NET_FIRST_TOKEN);
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
    let pool = Pool::new(NET_FIRST_TOKEN);
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
    let pool = Pool::new(NET_FIRST_TOKEN);
    let stray = Pending {
        token: FS_FIRST_TOKEN,
        label: "stray",
    };
    let refused = pool
        .watch(&stray, &Arc::new(Inbox::default()))
        .expect_err("nothing here minted it");
    assert_eq!(refused.code, codes::INTERNAL_ERROR);
}

/// A pool that gives each operation a thread refuses the one past its bound; a queued one has no
/// such bound, and answers each in its turn.
#[test]
fn a_queued_pool_answers_more_operations_than_a_thread_each_would_be_given() {
    let pool = Pool::queued(PASSWORD_FIRST_TOKEN, 2);
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
    let pool = Pool::queued(PASSWORD_FIRST_TOKEN, 3);
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
    let pool = Pool::queued(PASSWORD_FIRST_TOKEN, 1);
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

/// The first facility to claim a token answers it, so overlapping ranges would hang a poll forever.
#[test]
fn no_two_facilities_mint_the_same_token() {
    let ranges = [
        ("net", NET_FIRST_TOKEN),
        ("fs", FS_FIRST_TOKEN),
        ("process", PROCESS_FIRST_TOKEN),
        ("password", PASSWORD_FIRST_TOKEN),
    ];
    for (i, (whose, first)) in ranges.iter().enumerate() {
        assert!(
            *first > 0,
            "`{whose}` would mint the token a zeroed `Pending` carries"
        );
        for (other, next) in &ranges[i + 1..] {
            assert!(
                first < next,
                "`{whose}` starts at {first} and `{other}` at {next}: the ranges are not ordered"
            );
            // Not maximal: `net` starts at 1 (a zeroed `Pending` carries 0), so no gap is 2^62.
            assert!(
                next - first >= 1 << 61,
                "`{whose}` reaches `{other}` after {} operations",
                next - first
            );
        }
    }
}
