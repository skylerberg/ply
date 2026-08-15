//! The drain, against a real postgres.
//!
//! ADR 0015 §4.4 pins the teardown order and calls a wrong one a *data-loss bug
//! rather than a mess*. This file is the check on that claim, and it is written
//! the way `db_transaction_audit.rs` is written for the same reason: **every
//! assertion about what postgres holds is made through `psql`**, a channel the
//! driver has no part in. The driver's own bookkeeping is not evidence about the
//! driver — W4's audit found a rollback that worked in memory and leaked in
//! postgres, and a drain that abandons an open transaction is the same class.
//!
//! What is asked of each phase is the only question a shutdown has to answer:
//! **is the write there, and is the connection clean?** A transaction the drain
//! abandoned shows up as a row that should not exist, or as a session left
//! `idle in transaction` holding the locks the rest of a rolling restart is
//! waiting on, and both are visible from `pg_stat_activity` and from the table.
//!
//! One `#[test]`, one cluster, one sequence of phases, for the reason
//! `db_driver.rs` gives: `#[test]`s in a binary run in parallel threads of one
//! process, and a server shared between them would have no owner.

mod support;

use ply_core::ty::Resource;
use ply_eval::Value;
use ply_eval::host::{HostAnswer, MachineId, Pending, ShutdownReport};
use ply_host::db::scope::{Access, Isolation, Owner};
use ply_host::db::types::Param;
use ply_host::db::{self, Driver, Op, Postgres, Statement};
use ply_host::signal::{Accepting, Bounds, Shutdown, Signal, Transactions};
use ply_host::tls::Credentials;
use ply_host::{Host, db::PoolConfig};
use ply_span::{Diagnostic, Span, Symbol};
use std::sync::Arc;
use std::time::{Duration, Instant};
use support::cluster::{self, Cluster};

/// One entry point that never spawned.
const ALONE: Owner = (MachineId(0), None);

const SCHEMA: &str = "create table ledger (id int8 primary key, note text);";

fn label(name: &str) -> Resource {
    Resource::Named(Symbol::new(name))
}

fn settle(db: &Postgres, answered: Result<HostAnswer, Diagnostic>) -> Result<Value, Diagnostic> {
    match answered? {
        HostAnswer::Value(value) => Ok(value),
        HostAnswer::Pending(pending) => resolve(db, pending),
    }
}

fn resolve(db: &Postgres, pending: Pending) -> Result<Value, Diagnostic> {
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(value) = db.poll(&pending)? {
            return Ok(value);
        }
        assert!(Instant::now() < until, "`{pending}` never resolved");
        db.reactor().park()?;
    }
}

fn execute(db: &Postgres, table: &str, sql: &str, params: Vec<Param>) -> Value {
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
            owner: ALONE,
            span: Span::DUMMY,
        }),
    )
    .unwrap_or_else(|d| panic!("`{sql}`: {} {}", d.code, d.message))
}

fn ctor(value: &Value) -> String {
    match value {
        Value::Ctor { name, .. } => name.to_string(),
        other => panic!("not a constructor: {}", other.type_name()),
    }
}

fn host(cluster: &Cluster, shutdown: &Arc<Shutdown>) -> Host {
    Host::with_database(
        Credentials::empty(),
        PoolConfig {
            url: cluster.url(),
            size: 4,
            acquire: Duration::from_secs(5),
            statement: Duration::from_secs(30),
            idle_txn: Duration::from_secs(30),
            connect: Duration::from_secs(5),
            statements: 64,
        },
    )
    .expect("the pool opens")
    .stopping_on(Arc::clone(shutdown))
}

/// The teardown, as `ply run` reaches it: through the `HostRuntime` the machine
/// was given, on the machine's own thread, after the entry point ended.
fn tear_down(host: &Host, drain_ms: u64) -> ShutdownReport {
    host.runtime().shutdown(drain_ms)
}

/// Sessions this cluster has sitting inside a transaction. The number that says
/// whether a drain left a `BEGIN` for postgres to clean up whenever it noticed.
fn idle_in_transaction(cluster: &Cluster) -> i64 {
    cluster
        .psql(
            &cluster.database,
            "select count(*) from pg_stat_activity where state like 'idle in transaction%'",
        )
        .parse()
        .expect("a count")
}

fn rows(cluster: &Cluster) -> String {
    cluster.psql(&cluster.database, "select id, note from ledger order by id")
}

#[test]
fn the_drain_never_commits_and_never_leaks() {
    if !cluster::available() {
        eprintln!(
            "skipping: this machine has no `initdb`/`postgres`/`psql`, so there is nothing to \
             assert a drain against. A green result from a suite that ran nothing is the failure \
             this project audits for, so this says so rather than passing quietly."
        );
        return;
    }
    let cluster = Cluster::start("w5_shutdown");
    cluster.psql(&cluster.database, SCHEMA);

    an_open_transaction_at_shutdown_is_rolled_back(&cluster);
    a_committed_transaction_survives_the_drain(&cluster);
    the_sink_is_flushed_before_the_pool_closes(&cluster);
    the_drain_reports_what_it_rolled_back(&cluster);
}

/// **The one that matters.** W4 made a transaction a scoped handler over a
/// pooled connection, so a drain that closed the pool under a request holding
/// one would lose whatever that request had written — and a drain that
/// *committed* it would be worse, because a half-finished body would be durable
/// and no retry could undo it.
///
/// The assertion is against the table and against `pg_stat_activity`, never
/// against the driver: a driver that believes it rolled back and left a `BEGIN`
/// on the wire is exactly the defect this is looking for.
fn an_open_transaction_at_shutdown_is_rolled_back(cluster: &Cluster) {
    let shutdown = Shutdown::new(Bounds {
        lead: Duration::ZERO,
        drain: Duration::from_millis(50),
    });
    let host = host(cluster, &shutdown);
    let db = host.database().expect("a database").clone();

    settle(
        &db,
        db.begin(
            Isolation::ReadCommitted,
            Access::ReadWrite,
            ALONE,
            Span::DUMMY,
        ),
    )
    .expect("the transaction opens");
    execute(
        &db,
        "ledger",
        "insert into ledger (id, note) values ($1, $2)",
        vec![Param::Int(1), Param::Text("half a body".to_string())],
    );
    assert_eq!(
        db.open_scopes(),
        1,
        "the scope has to be open for this phase to be about anything"
    );
    assert_eq!(
        rows(cluster),
        "",
        "an uncommitted insert is invisible to a second session, which is what makes the next \
         assertion mean something"
    );

    // The signal arrives while the request is inside the transaction, and the
    // drain runs out before the body could finish.
    shutdown.request(Signal::Terminate);
    let until = Instant::now() + Duration::from_secs(5);
    while !shutdown.drain_expired() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(shutdown.drain_expired(), "the 50ms drain never expired");

    let report = tear_down(&host, 50);
    assert_eq!(
        report.transactions_rolled_back, 1,
        "the drain has to have found the scope in order to have rolled it back"
    );
    assert_eq!(
        rows(cluster),
        "",
        "the drain committed a half-finished body, which is the outcome no retry can fix"
    );
    assert_eq!(
        idle_in_transaction(cluster),
        0,
        "a session was left inside a transaction, holding whatever locks it took until postgres \
         happened to notice the disconnect"
    );
    assert_eq!(
        db.open_scopes(),
        0,
        "the scope table still believes a transaction is open"
    );
    assert!(
        report.connections_closed.is_empty(),
        "the connection was discarded rather than rolled back, so postgres aborted the transaction          on the disconnect whenever it noticed rather than the drain doing it: {:?}",
        report.connections_closed
    );
}

/// The other half of the claim, and the one that would make the first vacuous: a
/// drain rolls back what was *open*, and takes nothing back that a body already
/// committed.
fn a_committed_transaction_survives_the_drain(cluster: &Cluster) {
    let shutdown = Shutdown::new(Bounds::default());
    let host = host(cluster, &shutdown);
    let db = host.database().expect("a database").clone();

    settle(
        &db,
        db.begin(
            Isolation::ReadCommitted,
            Access::ReadWrite,
            ALONE,
            Span::DUMMY,
        ),
    )
    .expect("the transaction opens");
    execute(
        &db,
        "ledger",
        "insert into ledger (id, note) values ($1, $2)",
        vec![Param::Int(2), Param::Text("a whole body".to_string())],
    );
    let committed = settle(&db, db.commit(ALONE, Span::DUMMY)).expect("the commit is answered");
    assert_ne!(ctor(&committed), "std.db.Failed", "the commit failed");

    shutdown.request(Signal::Interrupt);
    let report = tear_down(&host, 30_000);
    assert_eq!(
        report.transactions_rolled_back, 0,
        "there was nothing open, so a drain that rolled something back rolled back the wrong thing"
    );
    assert_eq!(
        rows(cluster),
        "2|a whole body",
        "the drain took back a write the program had already committed"
    );
    assert_eq!(idle_in_transaction(cluster), 0);
    cluster.psql(&cluster.database, "delete from ledger");
}

/// The pinned order, checked at the one place it is observable: the sink is
/// flushed **while the pool is still open**, so a record naming a rolled-back
/// transaction is written by a run that still holds the connection that rolled
/// it back. A flush after the pool closed would be a log that says nothing about
/// the shutdown that produced it.
fn the_sink_is_flushed_before_the_pool_closes(cluster: &Cluster) {
    use ply_host::trace::{Level, Record, Sink, Trace};
    use std::sync::Mutex;

    /// Records whether the database was still usable at the moment of the flush.
    struct Watching {
        db: Mutex<Option<Arc<Postgres>>>,
        pool_open_at_flush: Mutex<Option<bool>>,
    }

    impl Sink for Watching {
        fn path(&self) -> &'static str {
            "ply_host::trace::discard"
        }
        fn destination(&self) -> &'static str {
            "nothing"
        }
        fn wants(&self, _: Level) -> bool {
            false
        }
        fn write(&self, _: &Record<'_>) {}
        fn flush(&self) {
            let open = self
                .db
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|db| db.reactor().status().open > 0);
            *self.pool_open_at_flush.lock().unwrap() = Some(open);
        }
    }

    let shutdown = Shutdown::new(Bounds::default());
    let sink = Arc::new(Watching {
        db: Mutex::new(None),
        pool_open_at_flush: Mutex::new(None),
    });
    let host = Host::with_database(
        Credentials::empty(),
        PoolConfig {
            url: cluster.url(),
            size: 2,
            acquire: Duration::from_secs(5),
            statement: Duration::from_secs(30),
            idle_txn: Duration::from_secs(30),
            connect: Duration::from_secs(5),
            statements: 64,
        },
    )
    .expect("the pool opens")
    .traced(Arc::new(Trace::new(Arc::clone(&sink) as Arc<dyn Sink>)))
    .stopping_on(Arc::clone(&shutdown));
    let db = host.database().expect("a database").clone();
    *sink.db.lock().unwrap() = Some(Arc::clone(&db));

    // One statement, so a connection has actually been established: a pool that
    // never opened one would make the assertion below true for the wrong reason.
    execute(
        &db,
        "ledger",
        "insert into ledger (id, note) values ($1, $2)",
        vec![Param::Int(3), Param::Text("recorded".to_string())],
    );
    assert!(
        db.reactor().status().open > 0,
        "no connection was established, so this phase would pass whatever the order was"
    );

    shutdown.request(Signal::Terminate);
    let report = tear_down(&host, 30_000);
    assert!(
        report.records_flushed.is_some(),
        "the sink was never flushed"
    );
    assert_eq!(
        sink.pool_open_at_flush.lock().unwrap().as_ref(),
        Some(&true),
        "the sink was flushed after the pool closed, so a record naming a rolled-back transaction \
         would be written by a run that no longer held the connection that rolled it back"
    );
    assert_eq!(
        db.reactor().status().open,
        0,
        "the pool was left open after the teardown"
    );
    cluster.psql(&cluster.database, "delete from ledger");
}

/// A run that shut down uncleanly still shut down, so what the teardown could
/// not hand back is `W0606` and data rather than a verdict. The counts have to
/// be the run's own, because the shutdown banner prints them and a number
/// computed for a banner is a number that can be wrong without anything failing.
fn the_drain_reports_what_it_rolled_back(cluster: &Cluster) {
    let shutdown = Shutdown::new(Bounds::default());
    let host = host(cluster, &shutdown);
    let db = host.database().expect("a database").clone();

    for id in 10..13 {
        let owner: Owner = (MachineId(id as u64), None);
        settle(
            &db,
            db.begin(
                Isolation::ReadCommitted,
                Access::ReadWrite,
                owner,
                Span::DUMMY,
            ),
        )
        .expect("the transaction opens");
        let scan = db::scan::scan("insert into ledger (id, note) values ($1, $2)", Span::DUMMY)
            .expect("the statement scans");
        let at = label("ledger");
        let touched = db::check_footprint(&scan, Op::Execute, &at, None, Span::DUMMY)
            .expect("the label covers it");
        settle(
            &db,
            db.statement(Statement {
                op: Op::Execute,
                at: &at,
                sql: "insert into ledger (id, note) values ($1, $2)",
                params: vec![Param::Int(id), Param::Text("never".to_string())],
                touched,
                scan: &scan,
                owner,
                span: Span::DUMMY,
            }),
        )
        .expect("the insert runs");
    }
    assert_eq!(db.open_scopes(), 3);
    assert_eq!(
        <Postgres as Transactions>::open_scopes(&db),
        3,
        "the coordinator asks the same question the banner prints"
    );

    shutdown.request(Signal::Terminate);
    let report = tear_down(&host, 30_000);
    assert_eq!(
        report.transactions_rolled_back, 3,
        "a drain that reported fewer than it rolled back would understate what a deployment lost"
    );
    assert!(
        report.is_clean(),
        "a clean teardown reports nothing, and this one reported {:?}",
        report.problems
    );
    assert_eq!(
        rows(cluster),
        "",
        "three half-finished bodies were committed by the teardown"
    );
    assert_eq!(idle_in_transaction(cluster), 0);
}

/// The coordinator holds the socket side by trait, so a `Host` built with a
/// database still answers phase 2's questions. Cheap, and it is the wiring a
/// second signal's abandoned-line reads.
#[test]
fn a_host_with_a_database_still_answers_what_phase_two_asks() {
    let shutdown = Shutdown::new(Bounds::default());
    let host = Host::new().stopping_on(Arc::clone(&shutdown));
    assert_eq!(host.net().connections_in_flight(), 0);
    assert_eq!(host.net().accepts_in_flight(), 0);
    assert_eq!(host.net().stop_accepting(), 0);
    assert!(host.shutdown().is_some());
}
