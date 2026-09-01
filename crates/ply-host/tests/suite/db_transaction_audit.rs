//! An adversarial audit of transactions, the pool and parameter safety, against a real postgres.

use crate::support::cluster::{self, Cluster};
use ply_core::ty::{EffectAtom, Footprint, Resource};
use ply_eval::Value;
use ply_eval::host::{HostAnswer, MachineId, Pending};
use ply_eval::sim::TaskId;
use ply_host::db::pool::{self, Outcome, PoolConfig, Reactor};
use ply_host::db::scope::{Access, Isolation, Owner};
use ply_host::db::stmt::{self, Answer};
use ply_host::db::types::{Datum, Param};
use ply_host::db::{self, Driver, Op, Postgres, Statement};
use ply_span::{Diagnostic, Span, Symbol, codes};
use rust_decimal::Decimal;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

/// One entry point that never spawned.
const ALONE: Owner = (MachineId(0), None);

/// A second machine: a different entry point, on a worker of its own.
const OTHER: Owner = (MachineId(1), None);

/// A task of [`ALONE`]'s machine, which owns no scope of its own.
const SIBLING: Owner = (MachineId(0), Some(TaskId(1)));

const SCHEMA: &str = "
create table t (id int8 primary key, n int8, s text);
create table other (id int8 primary key);
create table parent (id int8 primary key);
create table child (
  id int8 primary key,
  p  int8 references parent(id) deferrable initially deferred
);
create table wide (id int8 primary key, small float4, exact numeric, scaled numeric(12,4));
";

// the way the machine does.

fn settle(db: &Postgres, answered: Result<HostAnswer, Diagnostic>) -> Result<Value, Diagnostic> {
    match answered? {
        HostAnswer::Value(value) => Ok(value),
        HostAnswer::Pending(pending) => resolve(db, pending),
    }
}

fn resolve(db: &Postgres, pending: Pending) -> Result<Value, Diagnostic> {
    loop {
        if let Some(value) = db.poll(&pending)? {
            return Ok(value);
        }
        db.reactor().park()?;
    }
}

fn scan_of(sql: &str) -> db::Scan {
    db::scan::scan(sql, Span::DUMMY).unwrap_or_else(|d| panic!("`{sql}`: {}", d.message))
}

/// One data statement through the driver, labelled `table`, with no declared row — so the only
/// footprint check that applies is the label's own.
fn perform(
    db: &Postgres,
    op: Op,
    table: &str,
    sql: &str,
    params: Vec<Param>,
    owner: Owner,
) -> Result<Value, Diagnostic> {
    let scan = scan_of(sql);
    let at = Resource::Named(Symbol::new(table));
    let touched = db::check_footprint(&scan, op, &at, None, Span::DUMMY)?;
    settle(
        db,
        db.statement(Statement {
            op,
            at: &at,
            sql,
            params,
            touched,
            scan: &scan,
            owner,
            span: Span::DUMMY,
        }),
    )
}

fn execute(db: &Postgres, table: &str, sql: &str, params: Vec<Param>) -> Value {
    perform(db, Op::Execute, table, sql, params, ALONE)
        .unwrap_or_else(|d| panic!("`{sql}`: {} {}", d.code, d.message))
}

fn query(db: &Postgres, table: &str, sql: &str, params: Vec<Param>) -> Value {
    perform(db, Op::Query, table, sql, params, ALONE)
        .unwrap_or_else(|d| panic!("`{sql}`: {} {}", d.code, d.message))
}

/// The constructor a `std.db.Answer` carries, which is what a Ply `match` reads.
fn ctor(value: &Value) -> String {
    match value {
        Value::Ctor { name, .. } => name.to_string(),
        other => panic!("not a constructor: {}", other.type_name()),
    }
}

/// The SQLSTATE inside a `Failed`, or `None` for any other answer.
fn sqlstate(value: &Value) -> Option<String> {
    let Value::Ctor { name, args } = value else {
        return None;
    };
    if name.as_str() != "std.db.Failed" {
        return None;
    }
    let Value::Record(fields) = &args[0] else {
        return None;
    };
    match fields.get(&Symbol::new("code")) {
        Some(Value::Str(code)) => Some(code.to_string()),
        _ => None,
    }
}

fn failed_with(value: &Value, code: &str) {
    assert_eq!(
        sqlstate(value).as_deref(),
        Some(code),
        "expected `Failed({code})`, got {value:?}"
    );
}

fn begin(db: &Postgres, level: Isolation, access: Access) -> Value {
    settle(db, db.begin(level, access, ALONE, Span::DUMMY)).expect("a begin is never a diagnostic")
}

fn commit(db: &Postgres) -> Value {
    settle(db, db.commit(ALONE, Span::DUMMY)).expect("a commit is never a diagnostic")
}

fn abort(db: &Postgres) -> Value {
    settle(db, db.abort(ALONE, Span::DUMMY)).expect("an abort is never a diagnostic")
}

/// A raw statement on a pooled connection, bypassing the scope table.
fn raw(reactor: &Reactor, sql: &str, params: Vec<Param>) -> Result<Answer, Diagnostic> {
    let text = sql.to_string();
    let pending = reactor
        .borrow(
            Span::DUMMY,
            "`db.query`",
            pool::job(move |connection| async move {
                let out = stmt::execute(
                    &connection,
                    &text,
                    &params,
                    stmt::DEFAULT_STATEMENT_CACHE,
                    Span::DUMMY,
                )
                .await;
                (connection, out)
            }),
        )
        .expect("the pool takes the job");
    match reactor.block_on(pending).expect("the token resolves") {
        Outcome::Done(payload) => *payload
            .downcast::<Result<Answer, Diagnostic>>()
            .expect("the job's own type"),
        other => panic!("{other:?}"),
    }
}

/// What postgres holds, read through `psql` — never through the driver.
fn rows_in(cluster: &Cluster, table: &str) -> i64 {
    cluster
        .psql("audit", &format!("select count(*) from {table}"))
        .parse()
        .expect("a count")
}

#[test]
fn transactions_the_pool_and_parameters_under_adversarial_conditions() {
    if !cluster::available() {
        eprintln!("skipped: this machine has no `initdb`/`postgres`, so nothing here was audited");
        return;
    }
    let cluster = Cluster::start("audit");
    cluster.psql("audit", SCHEMA);

    a_rollback_is_invisible_to_a_second_connection(&cluster);
    a_nested_rollback_keeps_the_outer_and_an_outer_rollback_takes_the_inner(&cluster);
    a_commit_after_a_failed_statement_reports_the_aborted_scope(&cluster);
    a_commit_after_a_statement_timeout_reports_the_aborted_scope(&cluster);
    a_deferred_constraint_that_fails_at_commit_is_a_value(&cluster);
    a_connection_lost_mid_transaction_is_a_value_and_commits_nothing(&cluster);
    a_task_that_does_not_own_the_scope_is_refused(&cluster);

    a_pool_exhausted_by_open_transactions_is_e0437(&cluster);
    an_abandoned_transaction_is_gone_before_the_connection_is_reused(&cluster);
    session_state_does_not_reach_the_next_borrower(&cluster);
    a_statement_that_could_change_session_state_is_refused(&cluster);

    two_entry_points_are_two_scope_stacks(&cluster);

    every_injection_route_stays_inside_one_statement(&cluster);
    a_float4_parameter_is_refused_rather_than_narrowed(&cluster);
    the_numeric_edges_are_refusals_rather_than_roundings(&cluster);
    a_parameter_whose_type_disagrees_is_refused_before_a_byte_moves(&cluster);
    long_values_and_embedded_nul_bytes_are_data_or_a_named_failure(&cluster);
}

/// The property W4 exists to make impossible, checked from the one vantage point where "committed"
/// means anything: a connection that did not write the rows.
fn a_rollback_is_invisible_to_a_second_connection(cluster: &Cluster) {
    let db = driver(cluster, |_| {});

    begin(&db, Isolation::ReadCommitted, Access::ReadWrite);
    execute(
        &db,
        "t",
        "insert into t (id, n) values ($1, $2)",
        vec![Param::Int(1), Param::Int(1)],
    );
    assert_eq!(
        rows_in(cluster, "t"),
        0,
        "an uncommitted row was visible from another connection"
    );
    abort(&db);
    assert_eq!(rows_in(cluster, "t"), 0, "a rolled-back row survived");
    assert_eq!(db.depth(ALONE), 0);

    begin(&db, Isolation::ReadCommitted, Access::ReadWrite);
    execute(
        &db,
        "t",
        "insert into t (id, n) values ($1, $2)",
        vec![Param::Int(2), Param::Int(2)],
    );
    commit(&db);
    assert_eq!(rows_in(cluster, "t"), 1, "a committed row did not persist");

    cluster.psql("audit", "delete from t");
    finish(cluster, db);
}

/// Nesting is a savepoint, and the question that matters is whether an inner `commit` can smuggle
/// its writes past an outer rollback.
fn a_nested_rollback_keeps_the_outer_and_an_outer_rollback_takes_the_inner(cluster: &Cluster) {
    let db = driver(cluster, |_| {});

    begin(&db, Isolation::ReadCommitted, Access::ReadWrite);
    execute(
        &db,
        "t",
        "insert into t (id, n) values ($1, $2)",
        vec![Param::Int(1), Param::Int(1)],
    );
    begin(&db, Isolation::ReadCommitted, Access::ReadWrite);
    assert_eq!(db.depth(ALONE), 2);
    execute(
        &db,
        "t",
        "insert into t (id, n) values ($1, $2)",
        vec![Param::Int(2), Param::Int(2)],
    );
    abort(&db);
    assert_eq!(db.depth(ALONE), 1, "the outer scope closed with the inner");
    commit(&db);
    assert_eq!(
        rows_in(cluster, "t"),
        1,
        "the inner write survived its abort"
    );

    // The other direction: an inner `commit` inside an outer rollback.
    begin(&db, Isolation::ReadCommitted, Access::ReadWrite);
    begin(&db, Isolation::ReadCommitted, Access::ReadWrite);
    execute(
        &db,
        "t",
        "insert into t (id, n) values ($1, $2)",
        vec![Param::Int(3), Param::Int(3)],
    );
    commit(&db);
    assert_eq!(db.depth(ALONE), 1, "the release closed the outer scope too");
    abort(&db);
    assert_eq!(
        rows_in(cluster, "t"),
        1,
        "a released savepoint's write escaped the outer rollback"
    );

    cluster.psql("audit", "delete from t");
    finish(cluster, db);
}

/// A commit of a transaction a failed statement already aborted is `Failed`.
fn a_commit_after_a_failed_statement_reports_the_aborted_scope(cluster: &Cluster) {
    let db = driver(cluster, |_| {});

    begin(&db, Isolation::ReadCommitted, Access::ReadWrite);
    let written = execute(
        &db,
        "t",
        "insert into t (id, n) values ($1, $2)",
        vec![Param::Int(1), Param::Int(1)],
    );
    assert_eq!(ctor(&written), "std.db.Count");

    let duplicate = execute(
        &db,
        "t",
        "insert into t (id, n) values ($1, $2)",
        vec![Param::Int(1), Param::Int(2)],
    );
    failed_with(&duplicate, "23505");

    let after = execute(
        &db,
        "t",
        "insert into t (id, n) values ($1, $2)",
        vec![Param::Int(9), Param::Int(9)],
    );
    failed_with(&after, "25P02");

    let committed = commit(&db);
    failed_with(&committed, "25P02");
    assert_eq!(
        rows_in(cluster, "t"),
        0,
        "postgres kept a row the commit it reported as successful did not commit"
    );
    assert_eq!(db.depth(ALONE), 0);

    finish(cluster, db);
}

/// The same shape, reached the way a service reaches it.
fn a_commit_after_a_statement_timeout_reports_the_aborted_scope(cluster: &Cluster) {
    let db = driver(cluster, |c| c.statement = Duration::from_millis(200));
    // A table big enough that a nested loop over it outlives the deadline.
    cluster.psql(
        "audit",
        "insert into t (id, n) select g, g from generate_series(1, 20000) g",
    );

    begin(&db, Isolation::ReadCommitted, Access::ReadWrite);
    execute(
        &db,
        "t",
        "insert into t (id, n) values ($1, $2)",
        vec![Param::Int(999_999), Param::Int(5)],
    );
    let slow = query(
        &db,
        "t",
        "select count(*) from t x join t y on x.n > y.n",
        Vec::new(),
    );
    failed_with(&slow, "57014");

    let committed = commit(&db);
    failed_with(&committed, "25P02");
    assert_eq!(
        rows_in(cluster, "t"),
        20000,
        "the write before the timeout was rolled back, and the commit said otherwise"
    );

    cluster.psql("audit", "delete from t");
    finish(cluster, db);
}

/// The exit the ADR's table names first, and the one the driver gets right: a constraint that can
/// only fail at `COMMIT` is a `Failed` the program reads.
fn a_deferred_constraint_that_fails_at_commit_is_a_value(cluster: &Cluster) {
    let db = driver(cluster, |_| {});

    begin(&db, Isolation::ReadCommitted, Access::ReadWrite);
    let inserted = execute(
        &db,
        "child",
        "insert into child (id, p) values ($1, $2)",
        vec![Param::Int(1), Param::Int(999)],
    );
    assert_eq!(
        ctor(&inserted),
        "std.db.Count",
        "a deferred constraint does not fire at the statement"
    );

    let committed = commit(&db);
    failed_with(&committed, "23503");
    assert_eq!(
        db.depth(ALONE),
        0,
        "a failed commit left the scope open, so the connection carries a transaction"
    );
    assert_eq!(rows_in(cluster, "child"), 0);

    finish(cluster, db);
}

/// A backend killed out of band mid-transaction.
fn a_connection_lost_mid_transaction_is_a_value_and_commits_nothing(cluster: &Cluster) {
    let db = driver(cluster, |c| c.size = 2);

    begin(&db, Isolation::ReadCommitted, Access::ReadWrite);
    execute(
        &db,
        "t",
        "insert into t (id, n) values ($1, $2)",
        vec![Param::Int(1), Param::Int(1)],
    );
    cluster.psql(
        "audit",
        "select pg_terminate_backend(pid) from pg_stat_activity \
         where application_name = 'ply' and pid <> pg_backend_pid()",
    );

    let after = perform(
        &db,
        Op::Execute,
        "t",
        "insert into t (id, n) values ($1, $2)",
        vec![Param::Int(2), Param::Int(2)],
        ALONE,
    )
    .expect("a peer that went away is a value and not a diagnostic");
    failed_with(&after, "08006");

    let committed = commit(&db);
    failed_with(&committed, "08006");
    assert_eq!(db.depth(ALONE), 0);
    assert_eq!(rows_in(cluster, "t"), 0, "a lost transaction committed");

    let next = query(&db, "t", "select id from t", Vec::new());
    assert_eq!(
        ctor(&next),
        "std.db.Rows",
        "the pool did not replace the connection it lost"
    );

    finish(cluster, db);
}

/// A spawned task has no scope of its own, and both answers available to the driver are wrong — so
/// it refuses, at the statement and at the close alike.
fn a_task_that_does_not_own_the_scope_is_refused(cluster: &Cluster) {
    let db = driver(cluster, |_| {});

    begin(&db, Isolation::ReadCommitted, Access::ReadWrite);
    let refused = perform(&db, Op::Query, "t", "select id from t", Vec::new(), SIBLING)
        .expect_err("a statement from a task that owns no scope");
    assert_eq!(refused.code, codes::DB_TRANSACTION_SCOPE);

    match db.commit(SIBLING, Span::DUMMY) {
        Err(refused) => assert_eq!(refused.code, codes::DB_TRANSACTION_SCOPE),
        Ok(_) => panic!("a commit from a task that owns no scope was accepted"),
    }

    abort(&db);
    finish(cluster, db);
}

/// Every transaction holds a connection from `db.begin` to its close, so a pool smaller than the
/// transactions in flight cannot make progress.
fn a_pool_exhausted_by_open_transactions_is_e0437(cluster: &Cluster) {
    let db = driver(cluster, |c| {
        c.size = 1;
        c.acquire = Duration::from_millis(300);
    });
    let db = Arc::new(db);

    begin(&db, Isolation::ReadCommitted, Access::ReadWrite);

    // A second entry point, on a thread of its own, asking for the connection the first one is
    // holding.
    let contender = Arc::clone(&db);
    let started = std::time::Instant::now();
    let refused = std::thread::spawn(move || {
        settle(
            &contender,
            contender.begin(
                Isolation::ReadCommitted,
                Access::ReadWrite,
                OTHER,
                Span::DUMMY,
            ),
        )
        .expect_err("the only connection is held")
    })
    .join()
    .expect("the thread finished");
    let waited = started.elapsed();

    assert_eq!(refused.code, codes::DB_POOL_EXHAUSTED);
    assert!(
        refused.message.contains("`db.begin`"),
        "the refusal names the operation that waited: {}",
        refused.message
    );
    assert!(
        refused.notes.iter().any(|n| n.contains("1 connection")),
        "the refusal names the pool's size: {:?}",
        refused.notes
    );
    assert!(
        waited >= Duration::from_millis(250) && waited < Duration::from_secs(5),
        "the wait was neither the deadline nor bounded: {waited:?}"
    );

    abort(&db);
    finish_shared(cluster, db);
}

/// A scope nothing closed, through the driver rather than through the reactor: `end_entry_point`
/// rolls it back, and the *next* borrower sees neither the rows nor the transaction.
fn an_abandoned_transaction_is_gone_before_the_connection_is_reused(cluster: &Cluster) {
    let db = driver(cluster, |c| c.size = 1);

    begin(&db, Isolation::ReadCommitted, Access::ReadWrite);
    execute(
        &db,
        "t",
        "insert into t (id, n) values ($1, $2)",
        vec![Param::Int(1), Param::Int(1)],
    );

    let report = db.end_entry_point(ALONE.0).expect("teardown runs");
    assert_eq!(report.rolled_back, 1);
    assert!(report.is_clean(), "{:?}", report.describe());
    assert_eq!(rows_in(cluster, "t"), 0);

    // The same connection, since the pool holds exactly one.
    let vacuumed = raw(db.reactor(), "select id from t", Vec::new());
    assert!(vacuumed.is_ok(), "{vacuumed:?}");
    let idle = cluster.psql(
        "audit",
        "select count(*) from pg_stat_activity \
         where datname = 'audit' and state like 'idle in transaction%'",
    );
    assert_eq!(
        idle, "0",
        "a connection went back to the pool in a transaction"
    );

    finish(cluster, db);
}

/// A pooled connection carries no session state to its next borrower.
fn session_state_does_not_reach_the_next_borrower(cluster: &Cluster) {
    let db = driver(cluster, |c| c.size = 1);
    let reactor = db.reactor();

    let dirtied = on_connection(
        reactor,
        "set search_path = pg_catalog; \
         create temp table leaked (x int); \
         select pg_advisory_lock(918); \
         prepare leaked_plan as select 1; \
         listen leaked_channel",
    );
    assert!(dirtied.is_ok(), "{dirtied:?}");

    let inherited = inspect_session(reactor);
    assert_eq!(
        inherited,
        Inherited {
            search_path: r#""$user", public"#.to_string(),
            temp_tables: 0,
            advisory_locks: 0,
            // The one that survives, and it is stated rather than silently tolerated.
            prepared: 1,
            listening: 0,
        },
        "a borrower inherited session state from the one before it"
    );

    // The reachability half: no statement the scanner admits can leave one.
    assert_eq!(
        db::scan::scan("prepare p as select 1", Span::DUMMY)
            .expect_err("`prepare` is not an admitted statement shape")
            .code,
        codes::DB_STATEMENT_REFUSED
    );

    finish(cluster, db);
}

/// The reachability half of the one above, and the second half of §2.4.
fn a_statement_that_could_change_session_state_is_refused(cluster: &Cluster) {
    for sql in [
        "select set_config('search_path', 'pg_catalog', false) from t",
        "select pg_advisory_lock(918) from t",
        "select pg_sleep(0) from t",
        "select pg_read_file('/etc/hosts') from t",
        "select id from t where pg_advisory_lock(918) is not null",
        "insert into t (id, n) values (pg_backend_pid(), 1)",
    ] {
        let refused = db::scan::scan(sql, Span::DUMMY)
            .err()
            .unwrap_or_else(|| panic!("`{sql}` was admitted"));
        assert_eq!(refused.code, codes::DB_STATEMENT_REFUSED, "`{sql}`");
    }

    // The functions a statement may still call are unaffected.
    for sql in [
        "select count(*) from t",
        "select sum(n) from t",
        "select coalesce(n, 0) from t",
        "select lower(s) as folded from t",
    ] {
        let scan = scan_of(sql);
        assert_eq!(scan.tables.all().into_iter().collect::<Vec<_>>(), ["t"]);
        db::check_footprint(
            &scan,
            Op::Query,
            &Resource::Named(Symbol::new("t")),
            Some(&row([atom("t", ply_syntax::ast::Mode::Read)])),
            Span::DUMMY,
        )
        .unwrap_or_else(|d| panic!("`{sql}`: {}", d.message));
    }

    let db = driver(cluster, |c| c.size = 1);
    execute(
        &db,
        "t",
        "insert into t (id, n) values ($1, $2)",
        vec![Param::Int(1), Param::Int(1)],
    );
    let sql = "select set_config('search_path', 'pg_catalog', false) from t";
    let refused = db::stmt::Cache::default()
        .scan(sql, Span::DUMMY)
        .expect_err("a session-mutating call never reaches a connection");
    assert_eq!(refused.code, codes::DB_STATEMENT_REFUSED);
    // And the scan is what the handler runs before it acquires anything, so the pool's session is
    // what it was.
    query(&db, "t", "select count(*) from t", Vec::new());
    assert_ne!(
        inspect_session(db.reactor()).search_path,
        "pg_catalog",
        "a refused statement still reached the server"
    );

    cluster.psql("audit", "delete from t");
    finish(cluster, db);
}

// driver.

/// Two entry points, one driver, and the property that makes `ply test --jobs n` mean anything
/// against a real database.
fn two_entry_points_are_two_scope_stacks(cluster: &Cluster) {
    let db = Arc::new(driver(cluster, |c| c.size = 4));

    // Entry point A: a transaction over `t`.
    begin(&db, Isolation::ReadCommitted, Access::ReadWrite);
    execute(
        &db,
        "t",
        "insert into t (id, n) values ($1, $2)",
        vec![Param::Int(1), Param::Int(1)],
    );

    // Entry point B, on a worker of its own, on a machine of its own.
    let b = Arc::clone(&db);
    let seen = std::thread::spawn(move || {
        count_of(
            &perform(
                &b,
                Op::Query,
                "t",
                "select count(*) from t",
                Vec::new(),
                OTHER,
            )
            .expect("B's own connection"),
        )
    })
    .join()
    .expect("the worker finished");
    assert_eq!(
        seen, 0,
        "entry point B read entry point A's uncommitted rows, which means it ran \
         inside A's transaction on A's connection"
    );

    // B ends. It touched nothing of A's and says nothing about A.
    let b = Arc::clone(&db);
    let report = std::thread::spawn(move || b.end_entry_point(OTHER.0).expect("teardown runs"))
        .join()
        .expect("the worker finished");
    assert_eq!(
        report.rolled_back, 0,
        "B's teardown rolled back a scope B did not open"
    );
    assert_eq!(db.depth(ALONE), 1, "A's transaction survived B's teardown");

    let committed = commit(&db);
    assert_eq!(
        ctor(&committed),
        "std.db.Count",
        "A's commit was refused for a transaction B closed"
    );
    assert_eq!(
        rows_in(cluster, "t"),
        1,
        "A's committed write did not survive an unrelated entry point"
    );
    cluster.psql("audit", "delete from t");

    // And an in-flight token survives an unrelated teardown.
    let scan = scan_of("select count(*) from t");
    let at = Resource::Named(Symbol::new("t"));
    let touched = db::check_footprint(&scan, Op::Query, &at, None, Span::DUMMY).expect("a label");
    let pending = match db
        .statement(Statement {
            op: Op::Query,
            at: &at,
            sql: "select count(*) from t",
            params: Vec::new(),
            touched,
            scan: &scan,
            owner: ALONE,
            span: Span::DUMMY,
        })
        .expect("the statement is posted")
    {
        HostAnswer::Pending(pending) => pending,
        HostAnswer::Value(value) => panic!("a `db` operation answered inline: {value:?}"),
    };
    let b = Arc::clone(&db);
    std::thread::spawn(move || b.end_entry_point(OTHER.0).expect("teardown runs"))
        .join()
        .expect("the worker finished");
    let answered = resolve(&db, pending).expect("an in-flight token survives another teardown");
    assert_eq!(ctor(&answered), "std.db.Rows");

    finish_shared(cluster, db);
}

/// Every route a value could take into statement text, tried both ways.
fn every_injection_route_stays_inside_one_statement(cluster: &Cluster) {
    let db = driver(cluster, |_| {});

    let payload = "'; drop table t; --";
    execute(
        &db,
        "t",
        "insert into t (id, n, s) values ($1, $2, $3)",
        vec![Param::Int(1), Param::Int(1), Param::Text(payload.into())],
    );
    for (route, sql, params) in [
        (
            "equality",
            "select s from t where s = $1",
            vec![Param::Text(payload.into())],
        ),
        (
            "like",
            "select s from t where s like $1",
            vec![Param::Text(payload.into())],
        ),
        (
            "in list",
            "select s from t where s = any($1)",
            vec![Param::Array(vec![Param::Text(payload.into())])],
        ),
        (
            "limit",
            "select s from t order by id limit $1",
            vec![Param::Int(5)],
        ),
    ] {
        let value = query(&db, "t", sql, params);
        assert_eq!(ctor(&value), "std.db.Rows", "{route}");
    }
    assert_eq!(
        cluster.psql(
            "audit",
            "select count(*) from information_schema.tables where table_name = 't'"
        ),
        "1",
        "a parameter changed the schema"
    );

    // The same payload spliced into every position a program might splice it.
    for (route, sql) in [
        ("table name", format!("select id from {payload}")),
        ("order by", format!("select id from t order by {payload}")),
        ("limit", format!("select id from t limit {payload}")),
        (
            "in list",
            format!("select id from t where id in ({payload})"),
        ),
        (
            "array literal",
            format!("select id from t where id = any(array[{payload}])"),
        ),
        (
            "like pattern",
            format!("select id from t where s like '{payload}'"),
        ),
    ] {
        let refused = db::scan::scan(&sql, Span::DUMMY)
            .err()
            .unwrap_or_else(|| panic!("{route}: `{sql}` was accepted as one statement"));
        assert_eq!(refused.code, codes::DB_STATEMENT_REFUSED, "{route}");
    }

    // A spliced identifier that names a second relation is caught by the footprint rather than by
    // the scanner, which is the second lock working.
    let scan = scan_of("select id from t, other");
    let refused = db::check_footprint(
        &scan,
        Op::Query,
        &Resource::Named(Symbol::new("t")),
        Some(&row([atom("t", ply_syntax::ast::Mode::Read)])),
        Span::DUMMY,
    )
    .expect_err("`other` is not in the row");
    assert_eq!(refused.code, codes::DB_FOOTPRINT_UNDECLARED);

    // The limit, stated.
    let tautology = "select id from t where s like '%' or 1=1 -- '";
    let scan = scan_of(tautology);
    assert_eq!(scan.tables.all().into_iter().collect::<Vec<_>>(), ["t"]);
    assert_eq!(ctor(&query(&db, "t", tautology, Vec::new())), "std.db.Rows");

    cluster.psql("audit", "delete from t");
    finish(cluster, db);
}

/// A `float4` parameter is a named refusal rather than a silent narrowing.
fn a_float4_parameter_is_refused_rather_than_narrowed(cluster: &Cluster) {
    let db = driver(cluster, |_| {});
    let reactor = db.reactor();

    for value in [1e300, 0.1234567890123, 1.0] {
        let refused = raw(
            reactor,
            "insert into wide (id, small) values ($1, $2)",
            vec![Param::Int(1), Param::Float(value)],
        )
        .expect_err("a `float4` parameter is outside the mapping");
        assert_eq!(refused.code, codes::DB_STATEMENT_REFUSED, "{value}");
        assert!(
            refused.message.contains("float4"),
            "the refusal names the type: {}",
            refused.message
        );
    }
    assert_eq!(
        cluster.psql("audit", "select count(*) from wide"),
        "0",
        "a refused parameter still reached the column"
    );

    // The narrowing the program writes for itself still runs, and it is visible in the statement a
    // reader reads.
    raw(
        reactor,
        "insert into wide (id, small) values ($1, $2::float8::float4)",
        vec![Param::Int(1), Param::Float(0.1234567890123)],
    )
    .expect("an explicit narrowing");
    assert_eq!(
        cluster.psql("audit", "select small::text from wide where id = 1"),
        "0.12345679"
    );

    cluster.psql("audit", "delete from wide");
    finish(cluster, db);
}

/// W2's argument — a total that quietly lost a cent — applied to the wire.
fn the_numeric_edges_are_refusals_rather_than_roundings(cluster: &Cluster) {
    let db = driver(cluster, |_| {});
    let reactor = db.reactor();

    for (id, literal) in [
        (1i64, "0.1234567890123456789012345678"),
        (2, "79228162514264337593543950335"),
        (3, "1e-28"),
        (4, "-1.0000"),
    ] {
        raw(
            reactor,
            &format!("insert into wide (id, exact) values ($1, {literal}::numeric)"),
            vec![Param::Int(id)],
        )
        .expect("the insert");
        let read = raw(
            reactor,
            "select exact from wide where id = $1",
            vec![Param::Int(id)],
        )
        .unwrap_or_else(|d| panic!("`{literal}` did not decode: {}", d.message));
        let Answer::Rows(rows) = read else {
            panic!("`{literal}`: {read:?}")
        };
        let Datum::Numeric(value) = &rows[0][0].1 else {
            panic!("`{literal}`: {rows:?}")
        };
        assert_eq!(
            *value,
            Decimal::from_str(literal)
                .unwrap_or_else(
                    |_| Decimal::from_str("0.0000000000000000000000000001").expect("1e-28")
                ),
            "`{literal}` did not round trip"
        );
    }

    for (id, literal, why) in [
        (10i64, "0.12345678901234567890123456789", "scale 29"),
        (11, "79228162514264337593543950336", "past 96 bits"),
        (12, "1e-29", "scale 29 at the other end"),
        (13, "'NaN'", "no `Decimal` for it"),
        (14, "'Infinity'", "no `Decimal` for it"),
    ] {
        raw(
            reactor,
            &format!("insert into wide (id, exact) values ($1, {literal}::numeric)"),
            vec![Param::Int(id)],
        )
        .expect("postgres holds it happily");
        let refused = raw(
            reactor,
            "select exact from wide where id = $1",
            vec![Param::Int(id)],
        )
        .expect_err(&format!("`{literal}` ({why}) decoded rather than refusing"));
        assert_eq!(refused.code, codes::DB_PREPARE_FAILED, "{literal}");
        assert!(
            refused.message.contains("exact"),
            "the refusal names the column: {}",
            refused.message
        );
    }

    // A column's own scale is the server's answer and the driver is faithful to it rather than to
    // the literal's.
    raw(
        reactor,
        "insert into wide (id, scaled) values ($1, $2)",
        vec![
            Param::Int(20),
            Param::Numeric(Decimal::from_str("1.00").expect("a decimal")),
        ],
    )
    .expect("the insert");
    let read = raw(
        reactor,
        "select scaled from wide where id = $1",
        vec![Param::Int(20)],
    )
    .expect("it decodes");
    let Answer::Rows(rows) = read else {
        panic!("{read:?}")
    };
    assert_eq!(
        rows[0][0].1,
        Datum::Numeric(Decimal::from_str("1.0000").expect("a decimal"))
    );

    cluster.psql("audit", "delete from wide");
    finish(cluster, db);
}

/// A Ply value and a postgres column type that disagree is a refusal naming the position and the
/// type, before anything is sent — never a coercion.
fn a_parameter_whose_type_disagrees_is_refused_before_a_byte_moves(cluster: &Cluster) {
    let db = driver(cluster, |_| {});
    let reactor = db.reactor();

    for (what, params) in [
        (
            "text into int8",
            vec![Param::Int(1), Param::Text("5".into())],
        ),
        ("bool into int8", vec![Param::Int(1), Param::Bool(true)]),
        ("float into int8", vec![Param::Int(1), Param::Float(5.0)]),
        (
            "numeric into int8",
            vec![
                Param::Int(1),
                Param::Numeric(Decimal::from_str("5").expect("a decimal")),
            ],
        ),
        (
            "bytes into int8",
            vec![Param::Int(1), Param::Bytes(vec![5])],
        ),
        (
            "array into int8",
            vec![Param::Int(1), Param::Array(vec![Param::Int(5)])],
        ),
    ] {
        let refused = raw(reactor, "insert into t (id, n) values ($1, $2)", params)
            .expect_err(&format!("{what} was accepted"));
        assert_eq!(refused.code, codes::DB_STATEMENT_REFUSED, "{what}");
        assert!(
            refused.message.contains("$2"),
            "{what}: {}",
            refused.message
        );
    }

    // An array with a `PNull`, and a nested array: `List<a>` has no shape for either, so both
    // refuse rather than flattening.
    for params in [
        vec![Param::Array(vec![Param::Int(1), Param::Null])],
        vec![Param::Array(vec![Param::Array(vec![Param::Int(1)])])],
    ] {
        let refused = raw(reactor, "select $1::int8[] as a", params)
            .expect_err("an array Ply has no shape for");
        assert_eq!(refused.code, codes::DB_STATEMENT_REFUSED);
    }

    // Too few and too many parameters, which is a claim about the statement.
    let refused = raw(
        reactor,
        "insert into t (id, n) values ($1, $2)",
        vec![Param::Int(1)],
    )
    .expect_err("one parameter for two placeholders");
    assert_eq!(refused.code, codes::DB_STATEMENT_REFUSED);
    assert!(refused.message.contains("takes 2"), "{}", refused.message);

    assert_eq!(
        rows_in(cluster, "t"),
        0,
        "a refused parameter reached a row"
    );
    finish(cluster, db);
}

/// Bytes with embedded nulls are data; text with an embedded null is the server's own `22021`
/// rather than a truncated string; four megabytes of either survives the round trip.
fn long_values_and_embedded_nul_bytes_are_data_or_a_named_failure(cluster: &Cluster) {
    let db = driver(cluster, |_| {});
    let reactor = db.reactor();

    let embedded: Vec<u8> = vec![0, 1, 0, 255, 0];
    raw(
        reactor,
        "insert into t (id, s) values ($1, encode($2, 'hex'))",
        vec![Param::Int(1), Param::Bytes(embedded.clone())],
    )
    .expect("bytes with embedded nulls are data");

    let refused = raw(
        reactor,
        "insert into t (id, s) values ($1, $2)",
        vec![Param::Int(2), Param::Text("a\u{0}b".into())],
    )
    .expect("a text NUL is not a driver-side refusal");
    match refused {
        Answer::Failed(e) => assert_eq!(e.code, "22021"),
        other => panic!("a NUL byte reached a `text` column: {other:?}"),
    }
    assert_eq!(
        rows_in(cluster, "t"),
        1,
        "the refused row was written anyway"
    );

    let long = "x".repeat(4 * 1024 * 1024);
    raw(
        reactor,
        "insert into t (id, s) values ($1, $2)",
        vec![Param::Int(3), Param::Text(long.clone())],
    )
    .expect("four megabytes of text");
    let read = raw(
        reactor,
        "select s from t where id = $1",
        vec![Param::Int(3)],
    )
    .expect("it comes back");
    let Answer::Rows(rows) = read else {
        panic!("{read:?}")
    };
    assert_eq!(rows[0][0].1, Datum::Text(long));

    cluster.psql("audit", "delete from t");
    finish(cluster, db);
}

fn driver(cluster: &Cluster, edit: impl FnOnce(&mut PoolConfig)) -> Postgres {
    let mut config = PoolConfig::new(cluster.url());
    edit(&mut config);
    Postgres::start(config).expect("the driver starts")
}

/// Close a phase: nothing is left open, and nothing is left for the next phase to inherit through
/// the server.
fn finish(cluster: &Cluster, db: Postgres) {
    let _ = db.end_entry_point(ALONE.0);
    let _ = db.end_entry_point(OTHER.0);
    let _ = db.reactor().shutdown(std::time::Duration::from_secs(30));
    assert_eq!(
        cluster.psql(
            "audit",
            "select count(*) from pg_stat_activity \
             where datname = 'audit' and state like 'idle in transaction%'"
        ),
        "0",
        "a phase left a connection inside a transaction"
    );
}

fn finish_shared(cluster: &Cluster, db: Arc<Postgres>) {
    let _ = db.end_entry_point(ALONE.0);
    let _ = db.end_entry_point(OTHER.0);
    let _ = db.reactor().shutdown(std::time::Duration::from_secs(30));
    assert_eq!(
        cluster.psql(
            "audit",
            "select count(*) from pg_stat_activity \
             where datname = 'audit' and state like 'idle in transaction%'"
        ),
        "0",
        "a phase left a connection inside a transaction"
    );
}

fn atom(table: &str, mode: ply_syntax::ast::Mode) -> EffectAtom {
    EffectAtom::new(
        Symbol::new(db::EFFECT),
        Resource::Named(Symbol::new(table)),
        mode,
    )
}

fn row(atoms: impl IntoIterator<Item = EffectAtom>) -> Footprint {
    Footprint::from_atoms(atoms)
}

/// The one integer inside `Rows([{count: CInt(n)}])`.
fn count_of(value: &Value) -> i64 {
    let Value::Ctor { name, args } = value else {
        panic!("not an answer: {value:?}")
    };
    assert_eq!(name.as_str(), "std.db.Rows", "{value:?}");
    let Value::List(rows) = &args[0] else {
        panic!("{value:?}")
    };
    let Value::Map(row) = rows.iter().next().expect("one row") else {
        panic!("{value:?}")
    };
    match row.get(&Value::str("count")).expect("a `count` column") {
        Value::Ctor { args, .. } => match &args[0] {
            Value::Int(n) => *n,
            other => panic!("{}", other.type_name()),
        },
        other => panic!("{}", other.type_name()),
    }
}

/// A statement run directly on a pooled connection, for the session-state phases.
fn on_connection(reactor: &Reactor, sql: &'static str) -> Result<usize, String> {
    let pending = reactor
        .borrow(
            Span::DUMMY,
            "`db.execute`",
            pool::job(move |connection| async move {
                let out = connection
                    .simple_query(sql)
                    .await
                    .map(|messages| messages.len())
                    .map_err(|e| e.to_string());
                (connection, out)
            }),
        )
        .expect("the pool takes the job");
    match reactor.block_on(pending).expect("the token resolves") {
        Outcome::Done(payload) => *payload
            .downcast::<Result<usize, String>>()
            .expect("the job's own type"),
        other => panic!("{other:?}"),
    }
}

#[derive(PartialEq, Eq, Debug)]
struct Inherited {
    search_path: String,
    temp_tables: i64,
    advisory_locks: i64,
    prepared: i64,
    listening: i64,
}

/// What a borrower finds on the connection the pool hands it.
fn inspect_session(reactor: &Reactor) -> Inherited {
    let pending = reactor
        .borrow(
            Span::DUMMY,
            "`db.query`",
            pool::job(|connection| async move {
                let one = |sql: &'static str| async {
                    connection
                        .query_one(sql, &[])
                        .await
                        .map(|r| r.get::<_, i64>(0))
                        .unwrap_or(-1)
                };
                let inherited = Inherited {
                    search_path: connection
                        .query_one("select current_setting('search_path')", &[])
                        .await
                        .map(|r| r.get::<_, String>(0))
                        .unwrap_or_else(|e| format!("unreadable: {e}")),
                    temp_tables: one("select count(*) from pg_class where relname = 'leaked'")
                        .await,
                    advisory_locks: one("select count(*) from pg_locks \
                         where locktype = 'advisory' and pid = pg_backend_pid()")
                    .await,
                    prepared: one(
                        "select count(*) from pg_prepared_statements where name = 'leaked_plan'",
                    )
                    .await,
                    listening: one("select count(*) from pg_listening_channels()").await,
                };
                (connection, inherited)
            }),
        )
        .expect("the pool takes the job");
    match reactor.block_on(pending).expect("the token resolves") {
        Outcome::Done(payload) => *payload.downcast::<Inherited>().expect("the job's own type"),
        other => panic!("{other:?}"),
    }
}
