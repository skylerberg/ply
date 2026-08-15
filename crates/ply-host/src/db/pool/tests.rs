//! What the pool has to get right, tested against a real server.
//!
//! Every test here needs postgres, so every test here is skipped without
//! `PLY_TEST_DB` — which is the same rule `ply test` applies to `--host`, for
//! the same reason: a suite that silently acquires a dependency on a live
//! database is the failure this language exists to prevent, and the only
//! reliable defence is that the hermetic path is the one you get by not
//! thinking about it.
//!
//! ```sh
//! PLY_TEST_DB='postgresql://ply@127.0.0.1:5432/ply_test?sslmode=disable' \
//!   cargo test -p ply-host db::pool
//! ```
//!
//! The tests that do **not** need a server — configuration, refusals, the
//! session SQL — run always, because those are the ones a defect would
//! otherwise hide behind an unset variable.

use super::*;
use ply_span::Span;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

/// The database this run may touch, or `None`.
fn url() -> Option<String> {
    std::env::var("PLY_TEST_DB").ok().filter(|u| !u.is_empty())
}

/// A reactor over the test database, or `None` when there is no test database.
///
/// Deliberately not a `panic!`: a developer without postgres running must be
/// able to `cargo test -p ply-host` and get a green, honest result.
///
/// `tag` becomes the connections' `application_name`, and it is not decoration:
/// these tests share one database and one of them terminates backends, so a
/// test needs a way to name its own connections and only its own.
fn reactor(tag: &str, edit: impl FnOnce(&mut PoolConfig)) -> Option<Reactor> {
    let url = url()?;
    let separator = if url.contains('?') { '&' } else { '?' };
    let mut config = PoolConfig::new(format!("{url}{separator}application_name=ply_{tag}"));
    edit(&mut config);
    Some(Reactor::start(config).expect("the test database is reachable"))
}

fn span() -> Span {
    Span::DUMMY
}

/// Run a statement and hand back what the server said, as a string.
fn simple(sql: &'static str) -> Job {
    job(move |connection| async move {
        let out = connection
            .simple_query(sql)
            .await
            .map(|messages| messages.len())
            .map_err(|e| e.to_string());
        (connection, out)
    })
}

fn done<T: 'static>(outcome: Outcome) -> T {
    match outcome {
        Outcome::Done(payload) => *payload
            .downcast::<T>()
            .expect("the payload is what the job produced"),
        Outcome::Lease(id) => panic!("expected a job's answer, got {id}"),
        Outcome::Unreachable(why) => panic!("expected a job's answer, got unreachable: {why}"),
    }
}

fn lease_of(outcome: Outcome) -> LeaseId {
    match outcome {
        Outcome::Lease(id) => id,
        Outcome::Done(_) => panic!("expected a lease, got a job's answer"),
        Outcome::Unreachable(why) => panic!("expected a lease, got unreachable: {why}"),
    }
}

// ---------------------------------------------------------------------------
// Configuration, which needs no server.
// ---------------------------------------------------------------------------

#[test]
fn a_connection_string_that_does_not_parse_is_e0431() {
    let error = PoolConfig::new("this is not a connection string")
        .pg_config()
        .expect_err("a URL that does not parse is refused");
    assert_eq!(error.code, codes::DB_NOT_CONFIGURED);
}

/// ADR 0014 §10: `require` and above is `E0431` naming the paragraph, because
/// wiring rustls into the postgres client is a real decision about the trusted
/// computing base rather than a line to add untested.
#[test]
fn sslmode_require_is_refused_rather_than_quietly_downgraded() {
    let error = PoolConfig::new("postgresql://ply@127.0.0.1:5432/ply?sslmode=require")
        .pg_config()
        .expect_err("W4 does not configure TLS to postgres");
    assert_eq!(error.code, codes::DB_NOT_CONFIGURED);
    assert!(
        error.message.contains("sslmode=require"),
        "the refusal names what was asked for: {}",
        error.message
    );
    for mode in ["disable", "prefer"] {
        assert!(
            PoolConfig::new(format!(
                "postgresql://ply@127.0.0.1:5432/ply?sslmode={mode}"
            ))
            .pg_config()
            .is_ok(),
            "`sslmode={mode}` is what W4 accepts"
        );
    }
}

/// A bound nobody chose is a bound set to infinity, and postgres reads zero as
/// exactly that. So the two server-side timeouts may not be turned off.
#[test]
fn a_server_side_timeout_of_zero_is_refused() {
    for edit in [
        (|c: &mut PoolConfig| c.statement = Duration::ZERO) as fn(&mut PoolConfig),
        |c: &mut PoolConfig| c.idle_txn = Duration::ZERO,
        |c: &mut PoolConfig| c.size = 0,
    ] {
        let mut config = PoolConfig::new("postgresql://ply@127.0.0.1:5432/ply?sslmode=disable");
        edit(&mut config);
        assert_eq!(
            config
                .pg_config()
                .expect_err("a bound of zero is a bound removed")
                .code,
            codes::DB_NOT_CONFIGURED
        );
    }
}

#[test]
fn the_checkout_statement_sets_both_timeouts_and_resets_the_session() {
    let mut config = PoolConfig::new("postgresql://ply@127.0.0.1:5432/ply");
    config.statement = Duration::from_millis(1234);
    config.idle_txn = Duration::from_millis(4321);

    let recycled = session_sql(&config, true);
    assert!(recycled.starts_with("ROLLBACK; "), "{recycled}");
    assert!(
        recycled.contains("SET statement_timeout = 1234"),
        "{recycled}"
    );
    assert!(
        recycled.contains("SET idle_in_transaction_session_timeout = 4321"),
        "{recycled}"
    );
    for reset in [
        "RESET ALL",
        "UNLISTEN *",
        "pg_advisory_unlock_all()",
        "CLOSE ALL",
        "DISCARD TEMP",
    ] {
        assert!(recycled.contains(reset), "{reset} is missing: {recycled}");
    }
    // §4.3: the prepared-statement cache is the thing the pool exists to
    // amortise, and `DISCARD ALL` would drop it.
    assert!(!recycled.contains("DISCARD ALL"), "{recycled}");
    assert!(
        recycled.find("RESET ALL") < recycled.find("SET statement_timeout"),
        "the reset must run before the settings it would otherwise undo: {recycled}"
    );

    let fresh = session_sql(&config, false);
    assert!(!fresh.contains("ROLLBACK"));
    assert!(!fresh.contains("RESET ALL"));
}

#[test]
fn an_unreachable_server_at_start_is_e0431_rather_than_a_pool_that_never_connects() {
    // Port 1 on loopback: nothing listens there, and a refusal arrives long
    // before the connect deadline.
    let mut config = PoolConfig::new("postgresql://ply@127.0.0.1:1/ply?sslmode=disable");
    config.connect = Duration::from_millis(500);
    let error = Reactor::start(config).expect_err("a pool that cannot connect refuses to start");
    assert_eq!(error.code, codes::DB_NOT_CONFIGURED);
}

// ---------------------------------------------------------------------------
// Acquisition, exhaustion and the acquire deadline.
// ---------------------------------------------------------------------------

#[test]
fn a_statement_outside_a_scope_acquires_runs_and_gives_the_connection_back() {
    let Some(reactor) = reactor("borrow", |c| c.size = 1) else {
        return;
    };
    for _ in 0..4 {
        let pending = reactor
            .borrow(span(), "`db.query`", simple("select 1"))
            .unwrap();
        let rows: Result<usize, String> = done(reactor.block_on(pending).unwrap());
        assert!(rows.is_ok(), "{rows:?}");
        assert_eq!(
            reactor.status().checked_out,
            0,
            "a scope-less statement releases its connection before it answers"
        );
    }
}

/// ADR 0014 §13, test 26. Not a hang and not a deadlock report: a sentence
/// naming the size and the operation.
#[test]
fn an_exhausted_pool_is_e0437_after_the_acquire_deadline() {
    let Some(reactor) = reactor("exhaust", |c| {
        c.size = 1;
        c.acquire = Duration::from_millis(300);
    }) else {
        return;
    };

    let held = lease_of(
        reactor
            .block_on(reactor.lease(span(), "`db.begin`").unwrap())
            .unwrap(),
    );

    let started = std::time::Instant::now();
    let waiting = reactor
        .borrow(span(), "`db.query[items]`", simple("select 1"))
        .unwrap();
    let error = reactor
        .block_on(waiting)
        .expect_err("the only connection is leased, so this one cannot be served");
    let waited = started.elapsed();

    assert_eq!(error.code, codes::DB_POOL_EXHAUSTED);
    assert!(
        error.message.contains("`db.query[items]`"),
        "the refusal names the operation that waited: {}",
        error.message
    );
    assert!(
        error.notes.iter().any(|n| n.contains("1 connection")),
        "the refusal names the pool's size: {:?}",
        error.notes
    );
    assert!(
        error.notes.iter().any(|n| n.contains("1 checked out")),
        "the refusal names how many are out: {:?}",
        error.notes
    );
    assert!(
        waited >= Duration::from_millis(250),
        "it waited for the deadline rather than failing fast: {waited:?}"
    );

    reactor.release(held, Cleanup::Rollback).unwrap();
    let report = reactor
        .drain(&[], std::time::Duration::from_secs(30))
        .unwrap();
    assert!(report.is_clean(), "{report:?}");
}

/// The one property that makes an acquire safe to perform from a handler: it
/// never blocks the caller. A `db` operation costs a pending token and no
/// blocking-pool thread, so the two capacities are independent and a service
/// can have 64 socket operations and a pool's worth of queries at once.
#[test]
fn acquiring_never_blocks_the_thread_that_asked() {
    let Some(reactor) = reactor("nonblocking", |c| {
        c.size = 1;
        c.acquire = Duration::from_millis(2_000);
    }) else {
        return;
    };
    let held = lease_of(
        reactor
            .block_on(reactor.lease(span(), "`db.begin`").unwrap())
            .unwrap(),
    );

    // More outstanding operations than W1's blocking pool has threads. Each is
    // a token on one reactor thread rather than an OS thread of its own, so
    // submitting them all costs nothing and returns immediately.
    let started = std::time::Instant::now();
    let pending: Vec<_> = (0..crate::tcp::MAX_BLOCKING_OPERATIONS * 2)
        .map(|_| {
            reactor
                .borrow(span(), "`db.query`", simple("select 1"))
                .unwrap()
        })
        .collect();
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "submitting {} operations blocked the caller for {:?}",
        pending.len(),
        started.elapsed()
    );
    assert_eq!(reactor.outstanding(), pending.len());

    reactor.release(held, Cleanup::Clean).unwrap();
    for token in pending {
        let answer: Result<usize, String> = done(reactor.block_on(token).unwrap());
        assert!(answer.is_ok(), "{answer:?}");
    }
}

// ---------------------------------------------------------------------------
// Abandoned transactions.
// ---------------------------------------------------------------------------

/// ADR 0014 §1.3 and §13 test 9. A connection recycled with an open transaction
/// makes the *next* request read uncommitted rows of a request that already
/// failed, and it is invisible from either request. So the release rolls back,
/// and the next borrower sees a session with no transaction and no leftover
/// rows.
#[test]
fn a_connection_whose_transaction_was_abandoned_is_safe_to_reuse() {
    let Some(reactor) = reactor("abandoned", |c| c.size = 1) else {
        return;
    };
    setup_table(&reactor, "pool_abandoned");

    let held = lease_of(
        reactor
            .block_on(reactor.lease(span(), "`db.begin`").unwrap())
            .unwrap(),
    );
    for sql in ["begin", "insert into pool_abandoned values (1)", "select 1"] {
        let answer: Result<usize, String> = done(
            reactor
                .block_on(
                    reactor
                        .on(held, span(), "`db.execute`", simple(sql))
                        .unwrap(),
                )
                .unwrap(),
        );
        assert!(answer.is_ok(), "{sql}: {answer:?}");
    }

    // The scope is abandoned: nobody committed and nobody aborted.
    let report = reactor
        .drain(&[held], std::time::Duration::from_secs(30))
        .unwrap();
    assert_eq!(report.rolled_back, 1, "{report:?}");
    assert!(report.is_clean(), "the rollback succeeded: {report:?}");
    assert_eq!(reactor.status().checked_out, 0);

    // The same connection, since the pool holds exactly one.
    assert_eq!(
        rows(&reactor, "pool_abandoned"),
        0,
        "the abandoned insert was rolled back before the connection was reused"
    );
    assert_no_open_transaction(&reactor, "pool_abandoned");
}

/// Rows in a table, read on a freshly checked-out connection.
fn rows(reactor: &Reactor, table: &'static str) -> i64 {
    let counted: Result<i64, String> = done(
        reactor
            .block_on(
                reactor
                    .borrow(
                        span(),
                        "`db.query`",
                        job(move |connection| async move {
                            let sql = format!("select count(*) from {table}");
                            let out = connection
                                .query_one(&sql, &[])
                                .await
                                .map(|row| row.get::<_, i64>(0))
                                .map_err(|e| e.to_string());
                            (connection, out)
                        }),
                    )
                    .unwrap(),
            )
            .unwrap(),
    );
    counted.expect("the connection answers")
}

/// That the connection handed out next is not inside a transaction block.
///
/// `VACUUM` is the check because postgres itself refuses it inside one, with
/// `25001`. Reading `pg_stat_activity.state` would not do: a backend running
/// the query that reads it is `active` whether or not a transaction is open,
/// so the obvious assertion is the one that cannot fail.
fn assert_no_open_transaction(reactor: &Reactor, table: &'static str) {
    let vacuumed: Result<usize, String> = done(
        reactor
            .block_on(
                reactor
                    .borrow(
                        span(),
                        "`db.execute`",
                        job(move |connection| async move {
                            let sql = format!("vacuum {table}");
                            let out = connection
                                .simple_query(&sql)
                                .await
                                .map(|m| m.len())
                                .map_err(|e| e.to_string());
                            (connection, out)
                        }),
                    )
                    .unwrap(),
            )
            .unwrap(),
    );
    assert!(
        vacuumed.is_ok(),
        "the connection handed out was still inside a transaction block: {vacuumed:?}"
    );
}

/// The second lock, and the one that matters most because it catches the
/// driver being wrong rather than the driver being right.
///
/// A release that claims `Clean` when a transaction is open is a mistake no
/// type can prevent. The checkout that follows runs `session_sql` with a
/// leading `ROLLBACK`, so the *next* borrower gets a clean session and never
/// reads the uncommitted rows of a request that already failed.
#[test]
fn a_lease_released_as_clean_with_a_transaction_open_is_still_cleaned_before_reuse() {
    let Some(reactor) = reactor("secondlock", |c| c.size = 1) else {
        return;
    };
    setup_table(&reactor, "pool_second_lock");

    let held = lease_of(
        reactor
            .block_on(reactor.lease(span(), "`db.begin`").unwrap())
            .unwrap(),
    );
    for sql in ["begin", "insert into pool_second_lock values (1)"] {
        let answer: Result<usize, String> = done(
            reactor
                .block_on(
                    reactor
                        .on(held, span(), "`db.execute`", simple(sql))
                        .unwrap(),
                )
                .unwrap(),
        );
        assert!(answer.is_ok(), "{sql}: {answer:?}");
    }
    // The lie: a transaction is open and the driver says there is not.
    reactor.release(held, Cleanup::Clean).unwrap();

    assert_eq!(rows(&reactor, "pool_second_lock"), 0);
    assert_no_open_transaction(&reactor, "pool_second_lock");
}

fn setup_table(reactor: &Reactor, table: &'static str) {
    for sql in ["drop table if exists ", "create table "] {
        let statement = match sql {
            "drop table if exists " => format!("drop table if exists {table}"),
            _ => format!("create table {table} (id int primary key)"),
        };
        let answer: Result<usize, String> = done(
            reactor
                .block_on(
                    reactor
                        .borrow(
                            span(),
                            "setup",
                            job(move |connection| async move {
                                let out = connection
                                    .simple_query(&statement)
                                    .await
                                    .map(|m| m.len())
                                    .map_err(|e| e.to_string());
                                (connection, out)
                            }),
                        )
                        .unwrap(),
                )
                .unwrap(),
        );
        answer.expect("the fixture table is created");
    }
}

/// The other half of §1.3: a connection whose `ROLLBACK` fails is closed and
/// discarded rather than returned. The server terminating the backend is how
/// that happens for real, so that is how it is provoked.
#[test]
fn a_connection_whose_rollback_fails_is_discarded_rather_than_returned() {
    let Some(reactor) = reactor("rollbackfails", |c| {
        c.size = 1;
        c.acquire = Duration::from_millis(2_000);
    }) else {
        return;
    };
    let held = lease_of(
        reactor
            .block_on(reactor.lease(span(), "`db.begin`").unwrap())
            .unwrap(),
    );
    let opened: Result<usize, String> = done(
        reactor
            .block_on(
                reactor
                    .on(held, span(), "`db.begin`", simple("begin"))
                    .unwrap(),
            )
            .unwrap(),
    );
    assert!(opened.is_ok(), "{opened:?}");

    // Kill this connection's own backend from inside it. Everything after this
    // is a session the server has already thrown away.
    let killed: Result<usize, String> = done(
        reactor
            .block_on(
                reactor
                    .on(
                        held,
                        span(),
                        "`db.execute`",
                        simple("select pg_terminate_backend(pg_backend_pid())"),
                    )
                    .unwrap(),
            )
            .unwrap(),
    );
    assert!(killed.is_err(), "terminating the backend ends the session");

    let report = reactor
        .drain(&[held], std::time::Duration::from_secs(30))
        .unwrap();
    assert_eq!(
        report.discarded.len(),
        1,
        "the rollback could not run, so the connection was closed: {report:?}"
    );
    assert_eq!(report.discarded[0].lease, Some(held));

    // The pool refills: the next borrower gets a fresh connection rather than
    // the dead one.
    let answer: Result<usize, String> = done(
        reactor
            .block_on(
                reactor
                    .borrow(span(), "`db.query`", simple("select 1"))
                    .unwrap(),
            )
            .unwrap(),
    );
    assert!(
        answer.is_ok(),
        "the pool replaced the discarded connection: {answer:?}"
    );
}

/// The checkout round trip is what makes this detectable at all: a connection
/// the server has hung up on fails `session_sql` and `deadpool` discards it and
/// creates another, so nothing hands a dead socket to a statement.
#[test]
fn a_connection_the_server_closed_is_detected_rather_than_handed_out() {
    let Some(reactor) = reactor("serverclosed", |c| {
        c.size = 1;
        c.acquire = Duration::from_millis(2_000);
    }) else {
        return;
    };
    // Establish one connection and return it to the pool.
    reactor
        .block_on(reactor.borrow(span(), "warm", simple("select 1")).unwrap())
        .unwrap();
    assert_eq!(reactor.status().open, 1);

    // Kill every backend of this application other than the one doing the
    // killing, which is the pooled connection now sitting idle.
    reactor
        .block_on(
            reactor
                .borrow(
                    span(),
                    "kill",
                    job(|connection| async move {
                        let out = connection
                            .simple_query(
                                "select pg_terminate_backend(pid) from pg_stat_activity \
                                 where application_name = 'ply_serverclosed' and pid <> pg_backend_pid()",
                            )
                            .await
                            .map(|m| m.len())
                            .map_err(|e| e.to_string());
                        (connection, out)
                    }),
                )
                .unwrap(),
        )
        .unwrap();

    // Whatever is in the pool now, the next statement must succeed: a closed
    // connection is replaced at checkout rather than handed out to fail.
    for _ in 0..3 {
        let answer: Result<usize, String> = done(
            reactor
                .block_on(
                    reactor
                        .borrow(span(), "`db.query`", simple("select 1"))
                        .unwrap(),
                )
                .unwrap(),
        );
        assert!(
            answer.is_ok(),
            "a connection the server closed reached a statement: {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Session state.
// ---------------------------------------------------------------------------

/// ADR 0014 §13, test 27, asserted by reading `current_setting` through the
/// same connection rather than by trusting the string the pool sent.
#[test]
fn both_server_side_timeouts_are_set_on_every_connection_at_checkout() {
    let Some(reactor) = reactor("settings", |c| {
        c.size = 1;
        c.statement = Duration::from_millis(7_000);
        c.idle_txn = Duration::from_millis(11_000);
    }) else {
        return;
    };
    // Twice: the first checkout creates the connection and the second recycles
    // it, and the two take different paths through `deadpool`.
    for round in 0..2 {
        let settings: Result<(String, String), String> = done(
            reactor
                .block_on(
                    reactor
                        .borrow(
                            span(),
                            "`db.query`",
                            job(|connection| async move {
                                let out = connection
                                    .query_one(
                                        "select current_setting('statement_timeout'), \
                                         current_setting('idle_in_transaction_session_timeout')",
                                        &[],
                                    )
                                    .await
                                    .map(|row| (row.get::<_, String>(0), row.get::<_, String>(1)))
                                    .map_err(|e| e.to_string());
                                (connection, out)
                            }),
                        )
                        .unwrap(),
                )
                .unwrap(),
        );
        let (statement, idle) = settings.expect("the connection answers");
        assert_eq!(statement, "7s", "round {round}");
        assert_eq!(idle, "11s", "round {round}");
    }
}

// ---------------------------------------------------------------------------
// Draining.
// ---------------------------------------------------------------------------

/// Draining waits for work already in flight instead of dropping it. The job
/// counts its own completions, so "it finished" is asserted from the job rather
/// than from the pool's bookkeeping.
#[test]
fn a_drain_waits_for_work_in_flight_rather_than_dropping_it() {
    let Some(reactor) = reactor("drain", |c| c.size = 2) else {
        return;
    };
    static FINISHED: AtomicUsize = AtomicUsize::new(0);
    FINISHED.store(0, AtomicOrdering::SeqCst);

    let held = lease_of(
        reactor
            .block_on(reactor.lease(span(), "`db.begin`").unwrap())
            .unwrap(),
    );
    // Posted and deliberately not polled: the drain is what has to wait for it.
    let _pending = reactor
        .on(
            held,
            span(),
            "`db.execute`",
            job(|connection| async move {
                let out = connection
                    .simple_query("select pg_sleep(0.4)")
                    .await
                    .is_ok();
                let _ = FINISHED.fetch_add(1, AtomicOrdering::SeqCst);
                (connection, out)
            }),
        )
        .unwrap();

    let report = reactor
        .drain(&[held], std::time::Duration::from_secs(30))
        .unwrap();
    assert_eq!(
        FINISHED.load(AtomicOrdering::SeqCst),
        1,
        "the drain returned before the statement it was holding finished"
    );
    assert_eq!(report.awaited, 1, "{report:?}");
    assert_eq!(report.rolled_back, 1, "{report:?}");
    assert!(report.is_clean(), "{report:?}");
    assert_eq!(reactor.status().leases, 0);
}

#[test]
fn shutdown_rolls_back_every_lease_and_stops_the_thread() {
    let Some(reactor) = reactor("shutdown", |c| c.size = 4) else {
        return;
    };
    let leases: Vec<LeaseId> = (0..3)
        .map(|_| {
            lease_of(
                reactor
                    .block_on(reactor.lease(span(), "`db.begin`").unwrap())
                    .unwrap(),
            )
        })
        .collect();
    for lease in &leases {
        let answer: Result<usize, String> = done(
            reactor
                .block_on(
                    reactor
                        .on(*lease, span(), "`db.begin`", simple("begin"))
                        .unwrap(),
                )
                .unwrap(),
        );
        assert!(answer.is_ok(), "{answer:?}");
    }
    assert_eq!(reactor.status().leases, 3);

    let report = reactor
        .shutdown(std::time::Duration::from_secs(30))
        .unwrap();
    assert_eq!(report.rolled_back, 3, "{report:?}");
    assert!(report.is_clean(), "{report:?}");

    // Idempotent, and a stopped reactor refuses new work with a sentence rather
    // than a hang.
    assert!(reactor.shutdown(std::time::Duration::from_secs(30)).is_ok());
    let refused = reactor
        .borrow(span(), "`db.query`", simple("select 1"))
        .expect_err("a stopped reactor serves nothing");
    assert_eq!(refused.code, codes::DB_NOT_CONFIGURED);
}

/// Two statements on one lease are serialised by the lease's own task, which is
/// not a policy: a postgres connection carries one conversation, and two
/// statements in flight on it at once is a protocol violation.
#[test]
fn statements_on_one_lease_run_one_at_a_time_and_in_order() {
    let Some(reactor) = reactor("serial", |c| c.size = 2) else {
        return;
    };
    let held = lease_of(
        reactor
            .block_on(reactor.lease(span(), "`db.begin`").unwrap())
            .unwrap(),
    );
    let first = reactor
        .on(held, span(), "one", simple("select pg_sleep(0.2), 1"))
        .unwrap();
    let second = reactor.on(held, span(), "two", simple("select 2")).unwrap();

    let two: Result<usize, String> = done(reactor.block_on(second).unwrap());
    let one: Result<usize, String> = done(reactor.block_on(first).unwrap());
    assert!(one.is_ok() && two.is_ok(), "{one:?} {two:?}");

    reactor.release(held, Cleanup::Rollback).unwrap();
    assert!(
        reactor
            .drain(&[], std::time::Duration::from_secs(30))
            .unwrap()
            .is_clean()
    );
}

/// A lease is released once. A statement performed after its scope closed has
/// no connection to run on, and saying so is better than acquiring a second
/// connection and putting the statement outside the transaction its author
/// believed it was in.
#[test]
fn a_lease_released_twice_and_a_statement_after_the_release_are_both_refused() {
    let Some(reactor) = reactor("doublerelease", |c| c.size = 2) else {
        return;
    };
    let held = lease_of(
        reactor
            .block_on(reactor.lease(span(), "`db.begin`").unwrap())
            .unwrap(),
    );
    reactor.release(held, Cleanup::Clean).unwrap();
    assert_eq!(
        reactor
            .release(held, Cleanup::Clean)
            .expect_err("a lease is released once")
            .code,
        codes::INTERNAL_ERROR
    );
    assert_eq!(
        reactor
            .on(held, span(), "`db.query`", simple("select 1"))
            .expect_err("a released lease runs nothing")
            .code,
        codes::INTERNAL_ERROR
    );
}

#[test]
fn a_lease_survives_the_statements_of_one_scope_without_waiting_for_the_pool() {
    let Some(reactor) = reactor("scope", |c| {
        c.size = 1;
        // Short enough that any acquisition inside the scope would fail rather
        // than quietly succeed: the claim is that there is no acquisition.
        c.acquire = Duration::from_millis(50);
    }) else {
        return;
    };
    let held = lease_of(
        reactor
            .block_on(reactor.lease(span(), "`db.begin`").unwrap())
            .unwrap(),
    );
    for _ in 0..8 {
        let answer: Result<usize, String> = done(
            reactor
                .block_on(
                    reactor
                        .on(held, span(), "`db.query`", simple("select 1"))
                        .unwrap(),
                )
                .unwrap(),
        );
        assert!(answer.is_ok(), "{answer:?}");
    }
    reactor.release(held, Cleanup::Rollback).unwrap();
    assert!(
        reactor
            .drain(&[], std::time::Duration::from_secs(30))
            .unwrap()
            .is_clean()
    );
}

/// `park` is what the scheduler calls with nothing enabled, and it must not
/// spin, must not return before something is ready, and must refuse to wait for
/// nothing.
#[test]
fn parking_waits_for_an_outstanding_token_and_refuses_to_wait_for_nothing() {
    let Some(reactor) = reactor("park", |c| c.size = 2) else {
        return;
    };
    assert_eq!(
        reactor
            .park()
            .expect_err("waiting with nothing outstanding would never return")
            .code,
        codes::INTERNAL_ERROR
    );

    let pending = reactor
        .borrow(span(), "`db.query`", simple("select pg_sleep(0.2)"))
        .unwrap();
    assert!(reactor.poll(&pending).unwrap().is_none());
    assert!(!reactor.park_timeout(Duration::from_millis(20)).unwrap());
    reactor.park().unwrap();
    assert!(reactor.poll(&pending).unwrap().is_some());
    assert_eq!(reactor.outstanding(), 0);
}

// ---------------------------------------------------------------------------
// Timing out mid-borrow, and a server that went away.
// ---------------------------------------------------------------------------

/// `statement_timeout` is what stops one slow query from being a service
/// outage, so it has to fire, and the connection has to survive it: postgres
/// cancels the statement and keeps the session, and a pool that discarded the
/// connection would turn every slow query into a reconnect.
#[test]
fn a_statement_that_outruns_the_statement_timeout_is_cancelled_and_the_connection_survives() {
    let Some(reactor) = reactor("stmttimeout", |c| {
        c.size = 1;
        c.statement = Duration::from_millis(250);
    }) else {
        return;
    };
    let slow: Result<usize, String> = done(
        reactor
            .block_on(
                reactor
                    .borrow(
                        span(),
                        "`db.query`",
                        job(|connection| async move {
                            // The SQLSTATE and not the message: the message is
                            // postgres's prose and moves between versions and
                            // locales, and this assertion is about which
                            // component cancelled the statement.
                            let out = connection
                                .simple_query("select pg_sleep(5)")
                                .await
                                .map(|m| m.len())
                                .map_err(|e| {
                                    e.code().map(|c| c.code().to_string()).unwrap_or_default()
                                });
                            (connection, out)
                        }),
                    )
                    .unwrap(),
            )
            .unwrap(),
    );
    assert_eq!(
        slow.expect_err("the statement outran `--db-statement-ms`"),
        "57014",
        "the server cancelled it rather than the driver giving up on it"
    );

    let after: Result<usize, String> = done(
        reactor
            .block_on(
                reactor
                    .borrow(span(), "`db.query`", simple("select 1"))
                    .unwrap(),
            )
            .unwrap(),
    );
    assert!(
        after.is_ok(),
        "the connection survived its statement being cancelled: {after:?}"
    );
}

/// `idle_in_transaction_session_timeout` is the one that holds locks the rest
/// of the service is waiting on, so the server terminates the session outright.
/// The pool's job is then to notice: the rollback on release cannot run, so the
/// connection is closed and discarded rather than returned, and the pool
/// refills.
#[test]
fn a_scope_left_idle_past_the_idle_transaction_timeout_is_discarded_not_returned() {
    let Some(reactor) = reactor("idletxn", |c| {
        c.size = 1;
        c.acquire = Duration::from_millis(2_000);
        c.idle_txn = Duration::from_millis(250);
    }) else {
        return;
    };
    let held = lease_of(
        reactor
            .block_on(reactor.lease(span(), "`db.begin`").unwrap())
            .unwrap(),
    );
    let opened: Result<usize, String> = done(
        reactor
            .block_on(
                reactor
                    .on(held, span(), "`db.begin`", simple("begin"))
                    .unwrap(),
            )
            .unwrap(),
    );
    assert!(opened.is_ok(), "{opened:?}");
    std::thread::sleep(Duration::from_millis(900));

    let report = reactor
        .drain(&[held], std::time::Duration::from_secs(30))
        .unwrap();
    assert_eq!(
        report.discarded.len(),
        1,
        "the server had already ended the session, so the rollback failed: {report:?}"
    );
    let after: Result<usize, String> = done(
        reactor
            .block_on(
                reactor
                    .borrow(span(), "`db.query`", simple("select 1"))
                    .unwrap(),
            )
            .unwrap(),
    );
    assert!(after.is_ok(), "the pool refilled: {after:?}");
}

/// A TCP relay in front of the server, so a test can make the database go away
/// and come back without touching the server every other test is using.
struct Relay {
    port: u16,
    stop: Arc<std::sync::atomic::AtomicBool>,
    live: Arc<Mutex<Vec<std::net::TcpStream>>>,
}

impl Relay {
    fn start(upstream: &str, port: u16) -> Relay {
        let listener =
            std::net::TcpListener::bind(("127.0.0.1", port)).expect("the relay's port is free");
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let live: Arc<Mutex<Vec<std::net::TcpStream>>> = Arc::new(Mutex::new(Vec::new()));
        let upstream = upstream.to_string();
        let accepting = Arc::clone(&stop);
        let tracked = Arc::clone(&live);
        std::thread::spawn(move || {
            while !accepting.load(AtomicOrdering::SeqCst) {
                match listener.accept() {
                    Ok((downstream, _)) => {
                        let Ok(up) = std::net::TcpStream::connect(&upstream) else {
                            continue;
                        };
                        downstream.set_nonblocking(false).unwrap();
                        {
                            let mut held = tracked.lock().unwrap();
                            held.push(downstream.try_clone().unwrap());
                            held.push(up.try_clone().unwrap());
                        }
                        for (mut from, mut to) in [
                            (downstream.try_clone().unwrap(), up.try_clone().unwrap()),
                            (up, downstream),
                        ] {
                            std::thread::spawn(move || {
                                let _ = std::io::copy(&mut from, &mut to);
                                let _ = to.shutdown(std::net::Shutdown::Both);
                            });
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        Relay { port, stop, live }
    }

    /// The database goes away: nothing new connects, and everything already
    /// connected is cut.
    fn cut(&self) {
        self.stop.store(true, AtomicOrdering::SeqCst);
        for stream in self.live.lock().unwrap().drain(..) {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
        // Let the accept loop notice and drop its listener, so the port is free
        // for the relay that replaces it.
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// ADR 0014 §3.2: a connect failure *during* a run is a value the program
/// matches on and not a diagnostic, because a database that restarted is a peer
/// that went away — and ADR 0013 §7.1 already decided what those are. A
/// diagnostic here would stop a service for something a retry fixes.
#[test]
fn a_database_that_went_away_mid_run_is_a_value_and_the_next_request_reconnects() {
    let Some(upstream) = url() else {
        return;
    };
    let upstream = upstream
        .rsplit_once('@')
        .and_then(|(_, rest)| rest.split('/').next())
        .expect("the test URL names a host and port")
        .to_string();

    let relay = Relay::start(&upstream, 0);
    let mut config = PoolConfig::new(format!(
        "postgresql://ply@127.0.0.1:{}/ply_pool?sslmode=disable&application_name=ply_relay",
        relay.port
    ));
    config.size = 1;
    config.connect = Duration::from_millis(500);
    config.acquire = Duration::from_millis(2_000);
    let reactor = Reactor::start(config).expect("the relay is up, so the pool starts");

    let before: Result<usize, String> = done(
        reactor
            .block_on(
                reactor
                    .borrow(span(), "`db.query`", simple("select 1"))
                    .unwrap(),
            )
            .unwrap(),
    );
    assert!(before.is_ok(), "{before:?}");

    let port = relay.port;
    relay.cut();
    match reactor
        .block_on(
            reactor
                .borrow(span(), "`db.query`", simple("select 1"))
                .unwrap(),
        )
        .unwrap()
    {
        Outcome::Unreachable(_) => {}
        other => panic!("a server that went away is a value, not {other:?}"),
    }

    let _restored = Relay::start(&upstream, port);
    let after: Result<usize, String> = done(
        reactor
            .block_on(
                reactor
                    .borrow(span(), "`db.query`", simple("select 1"))
                    .unwrap(),
            )
            .unwrap(),
    );
    assert!(
        after.is_ok(),
        "the next request succeeded against a fresh connection: {after:?}"
    );
}

// ---------------------------------------------------------------------------
// Contention, which is where `ply test`'s workers meet one pool.
// ---------------------------------------------------------------------------

/// One reactor, many machine threads. `ply test` runs its workers in parallel
/// and they share a binding, so every entry point in this file is reached from
/// more than one thread at once in a real run — and a pool that were only
/// correct under one caller would be correct in no run that matters.
#[test]
fn many_threads_share_one_pool_without_losing_a_connection() {
    let Some(reactor) = reactor("contention", |c| {
        c.size = 3;
        c.acquire = Duration::from_millis(5_000);
    }) else {
        return;
    };
    let reactor = Arc::new(reactor);

    std::thread::scope(|scope| {
        for _ in 0..8 {
            let reactor = Arc::clone(&reactor);
            let _ = scope.spawn(move || {
                for _ in 0..6 {
                    let held = lease_of(
                        reactor
                            .block_on(reactor.lease(span(), "`db.begin`").unwrap())
                            .unwrap(),
                    );
                    for sql in ["begin", "select 1", "commit"] {
                        let answer: Result<usize, String> = done(
                            reactor
                                .block_on(
                                    reactor.on(held, span(), "`db.query`", simple(sql)).unwrap(),
                                )
                                .unwrap(),
                        );
                        assert!(answer.is_ok(), "{sql}: {answer:?}");
                    }
                    assert!(
                        reactor
                            .drain(&[held], std::time::Duration::from_secs(30))
                            .unwrap()
                            .is_clean()
                    );

                    let scopeless: Result<usize, String> = done(
                        reactor
                            .block_on(
                                reactor
                                    .borrow(span(), "`db.query`", simple("select 1"))
                                    .unwrap(),
                            )
                            .unwrap(),
                    );
                    assert!(scopeless.is_ok(), "{scopeless:?}");
                }
            });
        }
    });

    let status = reactor.status();
    assert_eq!(status.leases, 0, "{status:?}");
    assert_eq!(status.checked_out, 0, "{status:?}");
    assert!(
        status.open <= 3,
        "the pool never exceeded its size: {status:?}"
    );
    assert!(reactor.take_discards().is_clean());
}

/// More callers than connections, with a deadline short enough that some of
/// them cannot be served. Each is either served or told `E0437`; none hangs,
/// none is served twice, and the pool is intact afterwards. That last clause is
/// the one that matters: an exhaustion that leaked a connection would make the
/// *next* request fail for a reason its caller could not see.
#[test]
fn exhaustion_under_contention_refuses_some_callers_and_leaks_no_connection() {
    let Some(reactor) = reactor("exhaustrace", |c| {
        c.size = 2;
        c.acquire = Duration::from_millis(100);
    }) else {
        return;
    };
    let reactor = Arc::new(reactor);
    static SERVED: AtomicUsize = AtomicUsize::new(0);
    static REFUSED: AtomicUsize = AtomicUsize::new(0);
    SERVED.store(0, AtomicOrdering::SeqCst);
    REFUSED.store(0, AtomicOrdering::SeqCst);

    std::thread::scope(|scope| {
        for _ in 0..12 {
            let reactor = Arc::clone(&reactor);
            let _ = scope.spawn(move || {
                let pending = reactor
                    .borrow(span(), "`db.query[items]`", simple("select pg_sleep(0.15)"))
                    .unwrap();
                match reactor.block_on(pending) {
                    Ok(outcome) => {
                        let answer: Result<usize, String> = done(outcome);
                        assert!(answer.is_ok(), "{answer:?}");
                        let _ = SERVED.fetch_add(1, AtomicOrdering::SeqCst);
                    }
                    Err(diagnostic) => {
                        assert_eq!(diagnostic.code, codes::DB_POOL_EXHAUSTED);
                        let _ = REFUSED.fetch_add(1, AtomicOrdering::SeqCst);
                    }
                }
            });
        }
    });

    let served = SERVED.load(AtomicOrdering::SeqCst);
    let refused = REFUSED.load(AtomicOrdering::SeqCst);
    assert_eq!(served + refused, 12, "every caller got an answer");
    assert!(served > 0, "the pool served what it could");
    assert!(
        refused > 0,
        "a 100ms deadline over 12 sleeping callers refuses some"
    );

    let status = reactor.status();
    assert_eq!(status.checked_out, 0, "{status:?}");
    let after: Result<usize, String> = done(
        reactor
            .block_on(
                reactor
                    .borrow(span(), "`db.query`", simple("select 1"))
                    .unwrap(),
            )
            .unwrap(),
    );
    assert!(
        after.is_ok(),
        "the pool is intact after an exhaustion: {after:?}"
    );
}

/// `Cleanup::Discard` is for a session the driver knows is unusable. The
/// connection is closed rather than returned and the pool refills, so a driver
/// that knows more than the pool does can act on it without the pool losing
/// capacity.
///
/// Asserted on the backend's pid, because that is the only thing that
/// distinguishes "a fresh connection" from "the same one handed back": a pool
/// of one means the borrower after the discard can only be served by a
/// connection that did not exist before it.
#[test]
fn a_discarded_connection_is_closed_and_the_pool_refills_with_a_new_one() {
    let Some(reactor) = reactor("discard", |c| {
        c.size = 1;
        c.acquire = Duration::from_millis(2_000);
    }) else {
        return;
    };
    let pid = |on: Option<LeaseId>| -> i32 {
        let read = job(|connection| async move {
            let out = connection
                .query_one("select pg_backend_pid()", &[])
                .await
                .map(|row| row.get::<_, i32>(0))
                .map_err(|e| e.to_string());
            (connection, out)
        });
        let pending = match on {
            Some(lease) => reactor.on(lease, span(), "`db.query`", read).unwrap(),
            None => reactor.borrow(span(), "`db.query`", read).unwrap(),
        };
        let out: Result<i32, String> = done(reactor.block_on(pending).unwrap());
        out.expect("the connection answers")
    };

    let held = lease_of(
        reactor
            .block_on(reactor.lease(span(), "`db.begin`").unwrap())
            .unwrap(),
    );
    let before = pid(Some(held));
    reactor.release(held, Cleanup::Discard).unwrap();

    assert_ne!(
        pid(None),
        before,
        "the discarded connection was handed straight back"
    );
    assert_eq!(reactor.status().open, 1, "the pool refilled");
}

/// What a run puts in front of a person when a drain could not hand everything
/// back. `is_clean` decides whether anything is said at all, so a clean drain
/// has to say nothing: a warning that fires on every run is a warning nobody
/// reads.
#[test]
fn a_drain_report_says_nothing_when_it_is_clean_and_names_the_reason_when_it_is_not() {
    assert_eq!(DrainReport::default().describe(), None);
    let report = DrainReport {
        rolled_back: 1,
        discarded: vec![Discarded {
            lease: None,
            reason: "the rollback on release failed: connection closed".to_string(),
        }],
        awaited: 0,
        abandoned: 0,
    };
    let described = report.describe().expect("a discard is worth saying");
    assert!(described.contains("1 connection"), "{described}");
    assert!(
        described.contains("the rollback on release failed"),
        "{described}"
    );
}
