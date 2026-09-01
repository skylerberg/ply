//! The drain's remaining questions, against a real postgres.

use crate::support::cluster::{self, Cluster};
use ply_core::ty::Resource;
use ply_eval::Value;
use ply_eval::host::{HostAnswer, MachineId, Pending};
use ply_host::db::scope::{Access, Isolation, Owner};
use ply_host::db::types::Param;
use ply_host::db::{self, Driver, Op, Postgres, Statement};
use ply_host::signal::{Bounds, Shutdown, Signal, Transactions};
use ply_host::tls::Credentials;
use ply_host::{Host, db::PoolConfig};
use ply_span::{Diagnostic, Span, Symbol, codes};
use std::sync::Arc;
use std::time::{Duration, Instant};

const SCHEMA: &str = "create table ledger (id int8 primary key, note text);";

/// Two entry points of one run, which is what a task-per-connection server has.
const ONE: Owner = (MachineId(101), None);
const TWO: Owner = (MachineId(102), None);

fn label(name: &str) -> Resource {
    Resource::Named(Symbol::new(name))
}

fn resolve(db: &Postgres, pending: Pending) -> Result<Value, Diagnostic> {
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(value) = db.poll(&pending)? {
            return Ok(value);
        }
        assert!(Instant::now() < until, "`{pending}` never resolved");
        db.reactor().park_timeout(Duration::from_millis(20))?;
    }
}

fn settle(db: &Postgres, answered: Result<HostAnswer, Diagnostic>) -> Result<Value, Diagnostic> {
    match answered? {
        HostAnswer::Value(value) => Ok(value),
        HostAnswer::Pending(pending) => resolve(db, pending),
    }
}

/// Issue a statement and hand back the token rather than the answer, which is what a machine holds
/// while a task is blocked on a host handler.
fn issue(db: &Postgres, op: Op, table: &str, sql: &str, owner: Owner) -> Pending {
    let scan = db::scan::scan(sql, Span::DUMMY).expect("the statement scans");
    let at = label(table);
    let touched =
        db::check_footprint(&scan, op, &at, None, Span::DUMMY).expect("the label covers it");
    match db.statement(Statement {
        op,
        at: &at,
        sql,
        params: Vec::new(),
        touched,
        scan: &scan,
        owner,
        span: Span::DUMMY,
    }) {
        Ok(HostAnswer::Pending(pending)) => pending,
        Ok(HostAnswer::Value(_)) => panic!("a real statement waits on the reactor"),
        Err(d) => panic!("`{sql}`: {} {}", d.code, d.message),
    }
}

fn execute(db: &Postgres, table: &str, sql: &str, params: Vec<Param>, owner: Owner) -> Value {
    let scan = db::scan::scan(sql, Span::DUMMY).expect("the statement scans");
    let at = label(table);
    let touched = db::check_footprint(&scan, Op::Execute, &at, None, Span::DUMMY)
        .expect("the label covers it");
    settle(
        db,
        db.statement(Statement {
            op: Op::Execute,
            at: &at,
            sql,
            params,
            touched,
            scan: &scan,
            owner,
            span: Span::DUMMY,
        }),
    )
    .unwrap_or_else(|d| panic!("`{sql}`: {} {}", d.code, d.message))
}

fn begin(db: &Postgres, owner: Owner) {
    settle(
        db,
        db.begin(
            Isolation::ReadCommitted,
            Access::ReadWrite,
            owner,
            Span::DUMMY,
        ),
    )
    .expect("the transaction opens");
}

fn host(cluster: &Cluster, shutdown: &Arc<Shutdown>, size: usize, statement: Duration) -> Host {
    Host::with_database(
        Credentials::empty(),
        PoolConfig {
            url: cluster.url(),
            size,
            acquire: Duration::from_secs(5),
            statement,
            idle_txn: Duration::from_secs(30),
            connect: Duration::from_secs(5),
            statements: 64,
        },
    )
    .expect("the pool opens")
    .stopping_on(Arc::clone(shutdown))
}

fn idle_in_transaction(cluster: &Cluster) -> i64 {
    cluster
        .psql(
            &cluster.database,
            "select count(*) from pg_stat_activity where state like 'idle in transaction%'",
        )
        .parse()
        .expect("a count")
}

/// Every session this run's pool has, by the `application_name` the URL sets.
fn sessions(cluster: &Cluster) -> i64 {
    cluster
        .psql(
            &cluster.database,
            "select count(*) from pg_stat_activity where application_name = 'ply'",
        )
        .parse()
        .expect("a count")
}

fn rows(cluster: &Cluster) -> String {
    cluster.psql(&cluster.database, "select id, note from ledger order by id")
}

fn until_drain_expired(shutdown: &Arc<Shutdown>) {
    let until = Instant::now() + Duration::from_secs(10);
    while !shutdown.drain_expired() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(shutdown.drain_expired(), "the drain never expired");
}

#[test]
fn the_drain_answers_its_remaining_questions() {
    if !cluster::available() {
        eprintln!(
            "skipping: this machine has no `initdb`/`postgres`/`psql`, so there is nothing to \
             assert a drain against. A green result from a suite that ran nothing is the failure \
             this project audits for, so this says so rather than passing quietly."
        );
        return;
    }
    let cluster = Cluster::start("w5_drain_audit");
    cluster.psql(&cluster.database, SCHEMA);

    a_task_blocked_on_a_host_handler_does_not_outlast_the_drain(&cluster);
    a_drain_with_the_pool_exhausted_still_rolls_back(&cluster);
    the_pool_closes_after_the_rollbacks_and_leaves_no_session(&cluster);
    a_second_signal_during_the_drain_names_what_it_abandons(&cluster);
}

/// **Does the drain hang?**
fn a_task_blocked_on_a_host_handler_does_not_outlast_the_drain(cluster: &Cluster) {
    cluster.psql(
        &cluster.database,
        "insert into ledger (id, note) values (1, 'contended')",
    );
    let shutdown = Shutdown::new(Bounds {
        lead: Duration::ZERO,
        drain: Duration::from_millis(200),
    });
    // A statement timeout far above the drain, so "it came back" can only be the drain and never
    // the statement giving up.
    let host = host(cluster, &shutdown, 4, Duration::from_secs(20));
    let db = host.database().expect("a database").clone();
    let runtime = host.runtime();

    // One entry point takes the row lock and keeps it.
    begin(&db, ONE);
    execute(
        &db,
        "ledger",
        "update ledger set note = 'holder' where id = 1",
        Vec::new(),
        ONE,
    );
    // A second entry point blocks on that lock inside a host operation: issued, and not awaited.
    begin(&db, TWO);
    let pending = issue(
        &db,
        Op::Execute,
        "ledger",
        "update ledger set note = 'waiter' where id = 1",
        TWO,
    );
    // Let it actually reach the lock rather than asserting against a statement that has not been
    // sent yet.
    std::thread::sleep(Duration::from_millis(200));

    shutdown.request(Signal::Terminate);
    let started = Instant::now();
    let refused = runtime
        .block_on(pending)
        .expect_err("the drain expired under a blocked task, so `block_on` has to give it back");
    let elapsed = started.elapsed();

    assert_eq!(
        refused.code,
        codes::DRAIN_INCOMPLETE,
        "the block came back for a reason other than the drain: {} {}",
        refused.code,
        refused.message
    );
    assert!(
        elapsed < Duration::from_secs(8),
        "`block_on` sat inside the statement rather than observing the drain: {elapsed:?}"
    );
    assert!(
        refused.notes.iter().any(|n| n.contains("cancellation")),
        "`W0608` has to say the task was not cancelled, because that is what the client saw: {:?}",
        refused.notes
    );

    // The teardown still runs, and still has to come back **inside the budget it was given**: the
    // second entry point's statement is *still executing* on a pool thread, blocked on the first's
    // row lock.
    let started = Instant::now();
    let report = runtime.shutdown(200);
    // The budget, plus the connect deadline the last step may hand its answer back within, plus a
    // second of slack for a loaded machine — and nothing about the twenty-second statement timeout,
    // which is what a teardown that waited on the server rather than on the operator's deadline
    // would take.
    let bound = Duration::from_millis(200) + Duration::from_secs(5) + Duration::from_secs(1);
    assert!(
        started.elapsed() < bound,
        "the teardown waited out the blocked statement rather than closing under it: {:?} against \
         a 200ms budget and a 20s statement timeout",
        started.elapsed(),
    );
    assert_eq!(
        report.transactions_rolled_back, 2,
        "both scopes were open at the deadline and both have to be rolled back: {report:?}"
    );
    assert_eq!(
        rows(cluster),
        "1|contended",
        "neither the holder's write nor the waiter's survived, and the committed row did"
    );
    assert_eq!(idle_in_transaction(cluster), 0);
    let until = Instant::now() + Duration::from_secs(30);
    while sessions(cluster) > 0 && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(
        sessions(cluster),
        0,
        "the pool left a session behind after the teardown said it had closed"
    );
    cluster.psql(&cluster.database, "delete from ledger");
}

/// **The pool exhausted at the signal.**
fn a_drain_with_the_pool_exhausted_still_rolls_back(cluster: &Cluster) {
    let shutdown = Shutdown::new(Bounds {
        lead: Duration::ZERO,
        drain: Duration::from_millis(50),
    });
    // One connection, and the request holds it.
    let host = host(cluster, &shutdown, 1, Duration::from_secs(10));
    let db = host.database().expect("a database").clone();

    begin(&db, ONE);
    execute(
        &db,
        "ledger",
        "insert into ledger (id, note) values (10, 'held the only connection')",
        Vec::new(),
        ONE,
    );
    assert_eq!(db.open_scopes(), 1);
    assert_eq!(
        rows(cluster),
        "",
        "an uncommitted insert is invisible to a second session"
    );

    shutdown.request(Signal::Interrupt);
    until_drain_expired(&shutdown);
    assert_eq!(
        shutdown.at_stop().2,
        1,
        "the coordinator's own account of what was open at the stop is what the banner prints"
    );

    let report = host.runtime().shutdown(50);
    assert_eq!(
        report.transactions_rolled_back, 1,
        "the drain could not roll back the transaction holding the only connection"
    );
    assert_eq!(
        rows(cluster),
        "",
        "the drain committed a half-finished body, which is the outcome no retry can fix"
    );
    assert_eq!(idle_in_transaction(cluster), 0);
    assert!(
        report.connections_closed.is_empty(),
        "the connection was discarded rather than rolled back, so postgres aborted the \
         transaction on the disconnect whenever it noticed: {:?}",
        report.connections_closed
    );
}

/// **Before or after.**
fn the_pool_closes_after_the_rollbacks_and_leaves_no_session(cluster: &Cluster) {
    cluster.psql(
        &cluster.database,
        "insert into ledger (id, note) values (20, 'committed')",
    );
    let shutdown = Shutdown::new(Bounds {
        lead: Duration::ZERO,
        drain: Duration::from_millis(50),
    });
    let host = host(cluster, &shutdown, 4, Duration::from_secs(10));
    let db = host.database().expect("a database").clone();

    begin(&db, ONE);
    execute(
        &db,
        "ledger",
        "update ledger set note = 'half a body' where id = 20",
        Vec::new(),
        ONE,
    );
    // A second entry point with its own scope, so the teardown has more than one to unwind and the
    // order is a question rather than a single step.
    begin(&db, TWO);
    execute(
        &db,
        "ledger",
        "insert into ledger (id, note) values (21, 'the other request')",
        Vec::new(),
        TWO,
    );
    assert_eq!(db.open_scopes(), 2);

    shutdown.request(Signal::Terminate);
    until_drain_expired(&shutdown);
    let report = host.runtime().shutdown(50);

    assert_eq!(
        report.transactions_rolled_back, 2,
        "both entry points' transactions have to be rolled back, not just the first"
    );
    // The row lock is gone the moment the teardown returned.
    let locked = cluster.psql(
        &cluster.database,
        "select note from ledger where id = 20 for update nowait",
    );
    assert_eq!(
        locked, "committed",
        "the row still held the abandoned transaction's write, so the rollback had not landed \
         when the teardown returned"
    );
    assert_eq!(
        rows(cluster),
        "20|committed",
        "the drain kept a write it was supposed to roll back"
    );
    assert_eq!(idle_in_transaction(cluster), 0);

    let until = Instant::now() + Duration::from_secs(30);
    while sessions(cluster) > 0 && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(
        sessions(cluster),
        0,
        "the pool left sessions behind after the teardown said it had closed"
    );
    cluster.psql(&cluster.database, "delete from ledger");
}

/// **The second signal.**
fn a_second_signal_during_the_drain_names_what_it_abandons(cluster: &Cluster) {
    let shutdown = Shutdown::new(Bounds {
        lead: Duration::ZERO,
        drain: Duration::from_secs(30),
    });
    let host = host(cluster, &shutdown, 4, Duration::from_secs(10));
    let db = host.database().expect("a database").clone();

    begin(&db, ONE);
    execute(
        &db,
        "ledger",
        "insert into ledger (id, note) values (30, 'abandoned')",
        Vec::new(),
        ONE,
    );

    assert!(
        shutdown.request(Signal::Terminate),
        "the first signal starts the drain"
    );
    let until = Instant::now() + Duration::from_secs(5);
    while !shutdown.stopped_accepting() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(2));
    }
    let deadline_after_first = shutdown.deadline_ms();
    assert!(deadline_after_first > 0, "the drain is running");

    assert!(
        !shutdown.request(Signal::Interrupt),
        "a second signal is refused rather than starting the phases again"
    );
    assert!(shutdown.second_requested());
    assert_eq!(
        shutdown.signal(),
        Some(Signal::Terminate),
        "the signal that started the drain is the one the banner names, not the one that ended it"
    );
    assert!(
        shutdown.deadline_ms() <= deadline_after_first,
        "the second signal extended the drain, which is the opposite of what it means"
    );
    assert_eq!(Signal::Interrupt.exit_code(), 130);
    assert_eq!(Signal::Terminate.exit_code(), 143);

    // The line `exit_now` prints is these two numbers, read from the facilities the coordinator was
    // attached to.
    assert_eq!(
        db.open_scopes(),
        1,
        "the abandoned line's transaction count is the driver's own"
    );
    assert_eq!(
        <Postgres as Transactions>::open_scopes(&db),
        1,
        "the coordinator reads the same number through the trait it holds"
    );

    // And the outcome an operator has to be able to rely on after a `kill -9` shaped exit: postgres
    // aborted the transaction on the disconnect, so the half-finished body is not durable.
    drop(host);
    drop(db);
    let until = Instant::now() + Duration::from_secs(30);
    while idle_in_transaction(cluster) > 0 && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(
        rows(cluster),
        "",
        "a second signal made a half-finished body durable, which no retry can undo"
    );
}
