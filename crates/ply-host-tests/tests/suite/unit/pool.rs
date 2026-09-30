use ply_eval::{Diagnostic, Pending, Span, Value, codes};
use ply_host::pool::{Done, FS_FIRST_TOKEN, Inbox, NET_FIRST_TOKEN, Pool};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn submitted(pool: &Pool, answer: i64) -> Pending {
    pool.submit(
        Span::DUMMY,
        "test",
        "a test job",
        Box::new(move || Done::Int(answer)),
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

/// The first facility to claim a token answers it, so overlapping ranges would hang a poll forever.
#[test]
fn no_two_facilities_mint_the_same_token() {
    let ranges = [("net", NET_FIRST_TOKEN), ("fs", FS_FIRST_TOKEN)];
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
