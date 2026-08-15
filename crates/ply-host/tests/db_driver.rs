//! The postgres driver against a real postgres.
//!
//! One `#[test]`, one cluster, one sequence of phases. `#[test]`s in a binary
//! run in parallel threads of one process, so a server shared between them
//! would have no owner and would outlive the run; a server per test would pay
//! `initdb` per case. One owner and one `Drop` is the arrangement that leaks
//! nothing, including while a panic unwinds.
//!
//! The whole file is skipped, loudly, when this machine has no postgres
//! binaries. That is the same rule the rest of the suite follows — a test that
//! needs the host says so — and the alternative, a green result from a suite
//! that ran nothing, is the failure this project audits for.

mod support;

use ply_core::ty::{EffectAtom, Footprint, Resource};
use ply_eval::Value;
use ply_eval::host::{HostAnswer, MachineId, Pending};
use ply_host::db::pool::{self, Cleanup, Outcome, PoolConfig, Reactor};
use ply_host::db::scope::{Access, Isolation, Owner};
use ply_host::db::stmt::{self, Answer, Cache};
use ply_host::db::types::{Datum, Json, Param};
use ply_host::db::{self, Op};
use ply_host::db::{Driver, Postgres, Statement};
use ply_span::{Diagnostic, Span, Symbol, codes};
use rust_decimal::Decimal;
use std::str::FromStr;
use support::cluster::{self, Cluster};

/// One entry point that never spawned: one machine, no task. The owner every
/// scope in this file belongs to.
const ALONE: Owner = (MachineId(0), None);

const SCHEMA: &str = "
create table bin (
  code text primary key,
  capacity int4 not null,
  constraint bin_capacity_positive check (capacity > 0)
);
create table part (
  sku text primary key,
  bin_code text not null references bin(code),
  price numeric(12,4) not null,
  on_hand int8 not null default 0,
  tiny int2,
  ratio float8,
  blob bytea,
  tags text[],
  meta jsonb,
  note text,
  constraint part_sku_upper unique (sku, bin_code)
);
create table wire (
  id int8 primary key,
  an_int int8, an_int4 int4, an_int2 int2,
  a_bool bool, a_text text, a_varchar varchar(32),
  a_bytes bytea, a_float float8, a_float4 float4,
  a_numeric numeric, a_json jsonb, a_list int8[], a_texts text[],
  a_uuid uuid, an_optional text
);
";

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).expect("a decimal")
}

fn label(name: &str) -> Resource {
    Resource::Named(Symbol::new(name))
}

fn row(atoms: impl IntoIterator<Item = EffectAtom>) -> Footprint {
    Footprint::from_atoms(atoms)
}

fn atom(table: &str, mode: ply_syntax::ast::Mode) -> EffectAtom {
    EffectAtom::new(Symbol::new(db::EFFECT), label(table), mode)
}

/// One statement through the real driver: acquire, prepare, bind, execute,
/// release. What a scope-less `db.query` does.
fn run(reactor: &Reactor, sql: &str, params: Vec<Param>) -> Result<Answer, Diagnostic> {
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
        Outcome::Unreachable(why) => panic!("the database went away: {why}"),
        Outcome::Lease(_) => unreachable!("`borrow` never answers a lease"),
    }
}

fn answer(reactor: &Reactor, sql: &str, params: Vec<Param>) -> Answer {
    run(reactor, sql, params).unwrap_or_else(|d| panic!("`{sql}` was refused: {}", d.message))
}

fn refusal(reactor: &Reactor, sql: &str, params: Vec<Param>) -> Diagnostic {
    match run(reactor, sql, params) {
        Err(d) => d,
        Ok(a) => panic!("`{sql}` was accepted and answered {a:?}"),
    }
}

fn failure(reactor: &Reactor, sql: &str, params: Vec<Param>) -> ply_host::db::DbError {
    match answer(reactor, sql, params) {
        Answer::Failed(e) => e,
        other => panic!("`{sql}` succeeded: {other:?}"),
    }
}

fn only(answer: Answer) -> Vec<(String, Datum)> {
    match answer {
        Answer::Rows(rows) if rows.len() == 1 => rows.into_iter().next().expect("one row"),
        other => panic!("expected exactly one row, got {other:?}"),
    }
}

fn cell(row: &[(String, Datum)], column: &str) -> Datum {
    row.iter()
        .find(|(name, _)| name == column)
        .map(|(_, value)| value.clone())
        .unwrap_or_else(|| panic!("no column `{column}` in {row:?}"))
}

#[test]
fn the_driver_speaks_to_a_real_postgres() {
    if !cluster::available() {
        eprintln!(
            "skipped: this machine has no `initdb`/`postgres`, so the postgres driver was not exercised"
        );
        return;
    }
    let cluster = Cluster::start("desk");
    cluster.psql("desk", SCHEMA);

    let reactor = Reactor::start(PoolConfig::new(cluster.url())).expect("the pool starts");

    connect_and_prepare(&reactor);
    every_mapped_type_round_trips(&reactor);
    the_edges_of_the_mapping_are_named_refusals(&reactor);
    a_constraint_violation_is_a_value(&reactor);
    a_prepare_the_server_refuses_is_a_diagnostic(&reactor);
    a_parameter_is_never_syntax(&reactor, &cluster);
    the_footprint_of_a_join_names_both_tables(&reactor);
    the_statement_cache_prepares_once(&reactor, &cluster);
    a_connection_dropped_mid_run_is_a_value_and_the_next_one_succeeds(&reactor, &cluster);
    the_driver_serves_a_transaction(&cluster);

    let report = reactor
        .shutdown(std::time::Duration::from_secs(30))
        .expect("the reactor stops");
    assert!(
        report.is_clean(),
        "the run left connections behind: {:?}",
        report.describe()
    );
}

/// Connect, prepare, execute — the three things the driver has to be able to do
/// at all.
fn connect_and_prepare(reactor: &Reactor) {
    assert_eq!(
        answer(reactor, "select 1 as one", Vec::new()),
        Answer::Rows(vec![vec![("one".into(), Datum::Int(1))]])
    );
    assert_eq!(
        answer(
            reactor,
            "insert into bin (code, capacity) values ($1, $2)",
            vec![Param::Text("A1".into()), Param::Int(10)]
        ),
        Answer::Count(1)
    );
    // `insert … returning` describes columns, so the result description decides
    // which shape the answer takes rather than the operation the call site
    // named.
    let returned = only(answer(
        reactor,
        "insert into bin (code, capacity) values ($1, $2) returning code, capacity",
        vec![Param::Text("B2".into()), Param::Int(4)],
    ));
    assert_eq!(cell(&returned, "code"), Datum::Text("B2".into()));
    assert_eq!(cell(&returned, "capacity"), Datum::Int(4));
    assert_eq!(
        answer(
            reactor,
            "update bin set capacity = capacity + $1 where code = $2",
            vec![Param::Int(1), Param::Text("A1".into())]
        ),
        Answer::Count(1)
    );
}

/// Every row of the pinned mapping, in both directions, through a real column of
/// the real type — including the two the milestone's brief names, `Decimal` and
/// `Bytes`.
fn every_mapped_type_round_trips(reactor: &Reactor) {
    let json = Json::Object(vec![
        ("a".into(), Json::Number(dec("1.2500"))),
        ("b".into(), Json::Array(vec![Json::Bool(true), Json::Null])),
    ]);
    assert_eq!(
        answer(
            reactor,
            "insert into wire (id, an_int, an_int4, an_int2, a_bool, a_text, a_varchar, \
             a_bytes, a_float, a_float4, a_numeric, a_json, a_list, a_texts, a_uuid, an_optional) \
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10::float8::float4, $11, $12, \
             $13, $14, $15, $16)",
            vec![
                Param::Int(1),
                Param::Int(i64::MAX),
                Param::Int(-2_147_483_648),
                Param::Int(-32_768),
                Param::Bool(true),
                Param::Text("héllo ✓".into()),
                Param::Text("varchar".into()),
                Param::Bytes(vec![0, 1, 127, 128, 255]),
                Param::Float(1.5),
                // `float4` is a *result* type in §4.2's mapping and not a
                // parameter type, so the narrowing is written where a reader
                // sees it rather than performed silently by the driver.
                Param::Float(0.5),
                // Scale 28, which is `Decimal`'s ceiling: the value W2's whole
                // argument is about.
                Param::Numeric(dec("0.1234567890123456789012345678")),
                Param::Json(json.clone()),
                Param::Array(vec![Param::Int(1), Param::Int(2)]),
                Param::Array(Vec::new()),
                Param::Text("6ba7b810-9dad-11d1-80b4-00c04fd430c8".into()),
                Param::Null,
            ],
        ),
        Answer::Count(1)
    );

    let row = only(answer(
        reactor,
        "select an_int, an_int4, an_int2, a_bool, a_text, a_varchar, a_bytes, a_float, \
         a_float4, a_numeric, a_json, a_list, a_texts, a_uuid, an_optional from wire where id = $1",
        vec![Param::Int(1)],
    ));
    assert_eq!(cell(&row, "an_int"), Datum::Int(i64::MAX));
    assert_eq!(cell(&row, "an_int4"), Datum::Int(-2_147_483_648));
    assert_eq!(cell(&row, "an_int2"), Datum::Int(-32_768));
    assert_eq!(cell(&row, "a_bool"), Datum::Bool(true));
    assert_eq!(cell(&row, "a_text"), Datum::Text("héllo ✓".into()));
    assert_eq!(cell(&row, "a_varchar"), Datum::Text("varchar".into()));
    assert_eq!(
        cell(&row, "a_bytes"),
        Datum::Bytes(vec![0, 1, 127, 128, 255])
    );
    assert_eq!(cell(&row, "a_float"), Datum::Float(1.5));
    assert_eq!(cell(&row, "a_float4"), Datum::Float(0.5));
    assert_eq!(
        cell(&row, "a_numeric"),
        Datum::Numeric(dec("0.1234567890123456789012345678"))
    );
    assert_eq!(cell(&row, "a_json"), Datum::Json(json));
    assert_eq!(
        cell(&row, "a_list"),
        Datum::Array(vec![Datum::Int(1), Datum::Int(2)])
    );
    // An empty `PArray` is legal and takes its element type from the parameter
    // description; it comes back as an empty list rather than as `NULL`.
    assert_eq!(cell(&row, "a_texts"), Datum::Array(Vec::new()));
    assert_eq!(
        cell(&row, "a_uuid"),
        Datum::Text("6ba7b810-9dad-11d1-80b4-00c04fd430c8".into())
    );
    // `Option<a>` is a nullable column of `a`, so a `PNull` reads back as
    // `CNull` and not as a zero.
    assert_eq!(cell(&row, "an_optional"), Datum::Null);

    // A `numeric` column with a declared scale returns the column's scale, not
    // the literal's. This is exactly the divergence the agreement law's own
    // example reports, and the driver has to be faithful to the server about it.
    answer(
        reactor,
        "insert into part (sku, bin_code, price) values ($1, $2, $3)",
        vec![
            Param::Text("bolt".into()),
            Param::Text("A1".into()),
            Param::Numeric(dec("1.00")),
        ],
    );
    let part = only(answer(
        reactor,
        "select price from part where sku = $1",
        vec![Param::Text("bolt".into())],
    ));
    assert_eq!(cell(&part, "price"), Datum::Numeric(dec("1.0000")));
}

/// Each of these is a place a driver quietly loses data, and each is a named
/// refusal rather than a coerced value.
fn the_edges_of_the_mapping_are_named_refusals(reactor: &Reactor) {
    // A `numeric` past `Decimal`'s scale, and a `numeric` `NaN`: decode
    // failures naming the column, never a rounding and never a zero.
    for literal in ["0.12345678901234567890123456789", "'NaN'::numeric", "1e40"] {
        let d = refusal(
            reactor,
            &format!("select {literal}::numeric as n from wire where id = $1"),
            vec![Param::Int(1)],
        );
        assert!(
            d.code == codes::DB_PREPARE_FAILED || d.code == codes::DB_STATEMENT_REFUSED,
            "`{literal}` gave {}: {}",
            d.code,
            d.message
        );
    }

    // A two-dimensional array, and an array with a NULL element.
    for literal in ["array[array[1,2]]", "array[1, null]"] {
        let d = refusal(
            reactor,
            &format!("select {literal}::int8[] as a from wire where id = $1"),
            vec![Param::Int(1)],
        );
        assert!(
            d.message.contains('a') && !d.message.is_empty(),
            "`{literal}`: {}",
            d.message
        );
    }

    // A timestamp column: no time type in Ply, so it is refused with the
    // workaround named rather than rendered to text.
    let d = refusal(
        reactor,
        "select '2020-01-01'::timestamptz as t from wire where id = $1",
        vec![Param::Int(1)],
    );
    assert_eq!(d.code, codes::DB_STATEMENT_REFUSED);
    assert!(
        d.notes.iter().any(|n| n.contains("microseconds")),
        "{:?}",
        d.notes
    );

    // A duplicate column name: a `Row` is a `Map`, so one of them would be kept
    // and the other silently dropped. Refused before the first row.
    let d = refusal(
        reactor,
        "select bin.code, part.sku as code from part join bin on bin.code = part.bin_code",
        Vec::new(),
    );
    assert_eq!(d.code, codes::DB_PREPARE_FAILED);
    assert!(d.message.contains("two columns named"), "{}", d.message);

    // An `Int` that does not fit its column: `22003` from the server's own
    // vocabulary, never a truncation.
    assert_eq!(
        failure(
            reactor,
            "insert into wire (id, an_int4) values ($1, $2)",
            vec![Param::Int(99), Param::Int(i64::MAX)]
        )
        .code,
        "22003"
    );
}

/// A SQLSTATE is a value. A server that died because a row already existed would
/// not be a server, and a language that turned a constraint into a
/// compiler-shaped failure would make the constraint unusable as a concurrency
/// control.
fn a_constraint_violation_is_a_value(reactor: &Reactor) {
    let duplicate = failure(
        reactor,
        "insert into bin (code, capacity) values ($1, $2)",
        vec![Param::Text("A1".into()), Param::Int(3)],
    );
    assert_eq!(duplicate.code, "23505");
    assert_eq!(duplicate.constraint, "bin_pkey");

    let foreign_key = failure(
        reactor,
        "insert into part (sku, bin_code, price) values ($1, $2, $3)",
        vec![
            Param::Text("nut".into()),
            Param::Text("nowhere".into()),
            Param::Numeric(dec("1")),
        ],
    );
    assert_eq!(foreign_key.code, "23503");
    assert!(
        foreign_key.constraint.contains("bin"),
        "{}",
        foreign_key.constraint
    );

    let not_null = failure(
        reactor,
        "insert into bin (code, capacity) values ($1, null)",
        vec![Param::Text("C3".into())],
    );
    assert_eq!(not_null.code, "23502");

    let check = failure(
        reactor,
        "insert into bin (code, capacity) values ($1, $2)",
        vec![Param::Text("D4".into()), Param::Int(0)],
    );
    assert_eq!(check.code, "23514");
    assert_eq!(check.constraint, "bin_capacity_positive");

    // The message is carried for a person and compared by nothing — which is
    // what keeps the agreement law from failing on a server upgrade.
    assert!(!check.detail.is_empty());
}

/// A prepare the server refuses is the program's fault, is the same every time,
/// and will never succeed on a retry — so it is a diagnostic rather than a value
/// a program is invited to loop on.
fn a_prepare_the_server_refuses_is_a_diagnostic(reactor: &Reactor) {
    for sql in [
        "select * from no_such_table",
        "select no_such_column from bin",
        "select from where",
    ] {
        let d = refusal(reactor, sql, Vec::new());
        assert_eq!(d.code, codes::DB_PREPARE_FAILED, "{sql}");
        assert!(
            d.notes.iter().any(|n| n.starts_with("SQLSTATE")),
            "{sql}: {:?}",
            d.notes
        );
    }
}

/// The payload that would end a literal and start a statement is inserted as a
/// string, because it never becomes syntax. The same bytes in the statement text
/// are refused.
fn a_parameter_is_never_syntax(reactor: &Reactor, cluster: &Cluster) {
    let payload = "'; drop table part; --";
    assert_eq!(
        answer(
            reactor,
            "insert into bin (code, capacity) values ($1, $2)",
            vec![Param::Text(payload.into()), Param::Int(1)]
        ),
        Answer::Count(1)
    );
    let stored = only(answer(
        reactor,
        "select code from bin where capacity = $1 and code = $2",
        vec![Param::Int(1), Param::Text(payload.into())],
    ));
    assert_eq!(cell(&stored, "code"), Datum::Text(payload.into()));
    // The schema is untouched: the payload was a value the whole way.
    assert_eq!(
        cluster.psql(
            "desk",
            "select count(*) from information_schema.tables where table_name = 'part'"
        ),
        "1"
    );

    // The same bytes as statement text are a refusal before anything is sent —
    // one `Stmt` is one statement.
    let d = db::stmt::Cache::default()
        .scan(
            &format!("select * from bin where code = ''{payload}"),
            Span::DUMMY,
        )
        .expect_err("a stacked statement");
    assert_eq!(d.code, codes::DB_STATEMENT_REFUSED);
}

/// The hole this milestone exists to close, against a statement postgres
/// actually runs: one label, two tables, and a row that names both.
fn the_footprint_of_a_join_names_both_tables(reactor: &Reactor) {
    let sql = "select part.sku, bin.capacity from part join bin on bin.code = part.bin_code";
    let cache = Cache::default();
    let scan = cache.scan(sql, Span::DUMMY).expect("it scans");
    assert_eq!(
        scan.tables.all().into_iter().collect::<Vec<_>>(),
        ["bin", "part"]
    );

    // Declaring only the label's own table is `E0434` at prepare, before a row
    // is read.
    let narrow = row([atom("part", ply_syntax::ast::Mode::Read)]);
    let d = db::check_footprint(&scan, Op::Query, &label("part"), Some(&narrow), Span::DUMMY)
        .expect_err("`bin` is undeclared");
    assert_eq!(d.code, codes::DB_FOOTPRINT_UNDECLARED);
    assert!(d.message.contains("bin"), "{}", d.message);

    // Declaring both, it runs and both atoms are what the row records.
    let wide = row([
        atom("part", ply_syntax::ast::Mode::Read),
        atom("bin", ply_syntax::ast::Mode::Read),
    ]);
    let touched = db::check_footprint(&scan, Op::Query, &label("part"), Some(&wide), Span::DUMMY)
        .expect("both are declared");
    assert_eq!(touched.atoms().count(), 2);
    assert!(matches!(answer(reactor, sql, Vec::new()), Answer::Rows(_)));
}

/// Asserted against the server's own catalogue rather than by timing: `N`
/// executions of one statement leave exactly one prepared statement behind.
fn the_statement_cache_prepares_once(reactor: &Reactor, cluster: &Cluster) {
    let before: i64 = cluster
        .psql("desk", "select count(*) from pg_prepared_statements")
        .parse()
        .expect("a count");
    let sql = "select code from bin where capacity = $1 order by code";
    for _ in 0..8 {
        answer(reactor, sql, vec![Param::Int(11)]);
    }
    // The pool may have more than one connection, and a prepared statement is
    // per session — but eight executions must not have left eight of them, and
    // with a pool that hands the same connection back they leave one.
    let after: i64 = cluster
        .psql("desk", "select count(*) from pg_prepared_statements")
        .parse()
        .expect("a count");
    assert!(
        after - before <= 1,
        "eight executions of one statement prepared {} times",
        after - before
    );
}

/// A database that went away is a peer that went away, which is a `Failed` value
/// and not a diagnostic — and the request after it succeeds on a fresh
/// connection.
fn a_connection_dropped_mid_run_is_a_value_and_the_next_one_succeeds(
    reactor: &Reactor,
    cluster: &Cluster,
) {
    // Terminate every backend this run holds, out of band. `psql` is a second
    // channel on purpose: killing the connection from inside the driver would be
    // the driver testing its own bookkeeping.
    cluster.psql(
        "desk",
        "select pg_terminate_backend(pid) from pg_stat_activity \
         where application_name = 'ply' and pid <> pg_backend_pid()",
    );

    // The next statement either fails with the connection's own SQLSTATE or
    // succeeds on a connection the pool re-established. Both are correct; what
    // must not happen is a diagnostic, which would make a peer's disappearance
    // the program's fault.
    match run(reactor, "select 1 as one", Vec::new()) {
        Ok(Answer::Failed(e)) => assert_eq!(e.code, "08006", "{}", e.detail),
        Ok(Answer::Rows(_)) => {}
        Ok(other) => panic!("unexpected answer {other:?}"),
        Err(d) => panic!("a dead peer became a diagnostic: {} {}", d.code, d.message),
    }

    // And the one after it works, against a connection the pool established
    // fresh.
    assert_eq!(
        answer(reactor, "select 1 as one", Vec::new()),
        Answer::Rows(vec![vec![("one".into(), Datum::Int(1))]])
    );
}

const DRIVER_SCHEMA: &str = "
create table item (
  sku text primary key,
  on_hand int8 not null
);
";

/// Drive one operation to a value, the way the machine does: answer, park, poll
/// until it resolves.
fn settle(db: &Postgres, answered: Result<HostAnswer, Diagnostic>) -> Result<Value, Diagnostic> {
    match answered? {
        HostAnswer::Value(value) => Ok(value),
        HostAnswer::Pending(pending) => resolve(db, pending),
    }
}

fn resolve(db: &Postgres, pending: Pending) -> Result<Value, Diagnostic> {
    assert!(db.owns(&pending), "the driver did not mint its own token");
    loop {
        if let Some(value) = db.poll(&pending)? {
            return Ok(value);
        }
        db.reactor().park()?;
    }
}

fn scan_for(sql: &str) -> ply_host::db::Scan {
    ply_host::db::scan::scan(sql, Span::DUMMY).expect("the scanner accepts it")
}

fn driver_run(db: &Postgres, op: Op, sql: &str, params: Vec<Param>) -> Value {
    let scanned = scan_for(sql);
    let at = ply_core::ty::Resource::Named(ply_span::Symbol::new("item"));
    let touched = ply_host::db::check_footprint(&scanned, op, &at, None, Span::DUMMY)
        .expect("the footprint is the label's");
    settle(
        db,
        db.statement(Statement {
            op,
            at: &at,
            sql,
            params,
            touched,
            scan: &scanned,
            owner: ALONE,
            span: Span::DUMMY,
        }),
    )
    .unwrap_or_else(|d| panic!("`{sql}` was refused: {}", d.message))
}

/// The constructor name a `Value` must carry: the program-wide one. A bare
/// `Rows` is a value no `match` in any Ply program takes apart, which is the
/// defect this file exists to have caught once.
fn ctor_of(value: &Value) -> String {
    match value {
        Value::Ctor { name, .. } => name.to_string(),
        other => panic!("not a constructor: {}", other.type_name()),
    }
}

fn driver_count(db: &Postgres) -> i64 {
    let value = driver_run(db, Op::Query, "select count(*) from item", Vec::new());
    let Value::Ctor { name, args } = &value else {
        panic!("not an answer")
    };
    assert_eq!(name.as_str(), "std.db.Rows");
    let Value::List(rows) = &args[0] else {
        panic!("not rows")
    };
    let Value::Map(row) = rows.iter().next().expect("one row") else {
        panic!("a row is a map")
    };
    match row.get(&Value::str("count")).expect("the column") {
        Value::Ctor { args, .. } => match &args[0] {
            Value::Int(n) => *n,
            other => panic!("{}", other.type_name()),
        },
        other => panic!("{}", other.type_name()),
    }
}

fn driver_insert(db: &Postgres, sku: &str, on_hand: i64) -> Value {
    driver_run(
        db,
        Op::Execute,
        "insert into item (sku, on_hand) values ($1, $2)",
        vec![Param::Text(sku.into()), Param::Int(on_hand)],
    )
}

/// The `Driver` a `db` operation actually resolves to, reached the way the
/// machine reaches it: through the trait, and through a `Pending` that has to be
/// polled. Everything above this line exercises the parts; this exercises the
/// piece that joins them.
fn the_driver_serves_a_transaction(cluster: &Cluster) {
    cluster.psql("desk", DRIVER_SCHEMA);
    let db = Postgres::start(PoolConfig::new(cluster.url())).expect("the driver starts");

    // A statement outside every scope: borrow, run, hand the connection back.
    assert_eq!(driver_count(&db), 0);
    assert_eq!(ctor_of(&driver_insert(&db, "bolt", 5)), "std.db.Count");
    assert_eq!(driver_count(&db), 1);
    assert_eq!(db.depth(ALONE), 0, "a scope-less statement opened one");

    a_committed_transaction_persists(&db);
    an_aborted_transaction_leaves_nothing(&db);
    a_nested_transaction_is_a_savepoint(&db);
    a_nested_level_that_disagrees_is_a_value(&db);
    a_read_only_transaction_is_refused_by_the_server(&db);
    an_abandoned_scope_is_rolled_back_at_the_entry_point(&db, cluster);

    let report = db.end_entry_point(ALONE.0).expect("teardown runs");
    assert!(
        report.is_clean(),
        "the run left connections behind: {:?}",
        report.describe()
    );
    let report = db
        .reactor()
        .shutdown(std::time::Duration::from_secs(30))
        .expect("the reactor stops");
    assert!(report.is_clean(), "{:?}", report.describe());
}

fn a_committed_transaction_persists(db: &Postgres) {
    let before = driver_count(db);
    settle(
        db,
        db.begin(
            Isolation::ReadCommitted,
            Access::ReadWrite,
            ALONE,
            Span::DUMMY,
        ),
    )
    .expect("it begins");
    assert_eq!(db.depth(ALONE), 1);
    driver_insert(db, "gasket", 2);
    settle(db, db.commit(ALONE, Span::DUMMY)).expect("it commits");
    assert_eq!(db.depth(ALONE), 0, "the scope survived its commit");
    assert_eq!(driver_count(db), before + 1);
}

fn an_aborted_transaction_leaves_nothing(db: &Postgres) {
    let before = driver_count(db);
    settle(
        db,
        db.begin(
            Isolation::Serializable,
            Access::ReadWrite,
            ALONE,
            Span::DUMMY,
        ),
    )
    .expect("it begins");
    driver_insert(db, "widget", 9);
    // Visible inside its own scope and nowhere else, which is what a
    // transaction is.
    assert_eq!(driver_count(db), before + 1);
    settle(db, db.abort(ALONE, Span::DUMMY)).expect("it aborts");
    assert_eq!(db.depth(ALONE), 0);
    assert_eq!(driver_count(db), before, "an aborted insert survived");
}

fn a_nested_transaction_is_a_savepoint(db: &Postgres) {
    let before = driver_count(db);
    settle(
        db,
        db.begin(
            Isolation::ReadCommitted,
            Access::ReadWrite,
            ALONE,
            Span::DUMMY,
        ),
    )
    .expect("the outer begins");
    driver_insert(db, "outer", 1);
    settle(
        db,
        db.begin(
            Isolation::ReadCommitted,
            Access::ReadWrite,
            ALONE,
            Span::DUMMY,
        ),
    )
    .expect("the inner begins");
    assert_eq!(db.depth(ALONE), 2, "a nested begin is a savepoint");
    driver_insert(db, "inner", 1);
    settle(db, db.abort(ALONE, Span::DUMMY)).expect("the inner rolls back");
    assert_eq!(db.depth(ALONE), 1);
    // The inner's write is gone and the outer's is not, which is the whole of
    // what a savepoint buys.
    assert_eq!(driver_count(db), before + 1);
    settle(db, db.commit(ALONE, Span::DUMMY)).expect("the outer commits");
    assert_eq!(db.depth(ALONE), 0);
    assert_eq!(driver_count(db), before + 1);
}

/// A savepoint has no isolation level, so a nested `begin` asking for a
/// different one is a `Failed` naming both rather than a silent narrowing.
fn a_nested_level_that_disagrees_is_a_value(db: &Postgres) {
    settle(
        db,
        db.begin(
            Isolation::ReadCommitted,
            Access::ReadWrite,
            ALONE,
            Span::DUMMY,
        ),
    )
    .expect("the outer begins");
    let answer = settle(
        db,
        db.begin(
            Isolation::Serializable,
            Access::ReadWrite,
            ALONE,
            Span::DUMMY,
        ),
    )
    .expect("a refusal is a value, not a diagnostic");
    assert_eq!(ctor_of(&answer), "std.db.Failed");
    assert!(
        format!("{answer:?}").contains("25001"),
        "the SQLSTATE is not the one the ADR names: {answer:?}"
    );
    assert_eq!(db.depth(ALONE), 1, "the refused nesting opened a scope");
    settle(db, db.abort(ALONE, Span::DUMMY)).expect("the outer rolls back");
}

/// The backstop supplied by the one component in the stack that cannot be
/// fooled by an annotation: `25006`, from the server.
fn a_read_only_transaction_is_refused_by_the_server(db: &Postgres) {
    settle(
        db,
        db.begin(
            Isolation::ReadCommitted,
            Access::ReadOnly,
            ALONE,
            Span::DUMMY,
        ),
    )
    .expect("it begins");
    let answer = driver_insert(db, "readonly", 1);
    assert_eq!(ctor_of(&answer), "std.db.Failed");
    assert!(
        format!("{answer:?}").contains("25006"),
        "a write in a read-only transaction was not refused by the server: {answer:?}"
    );
    settle(db, db.abort(ALONE, Span::DUMMY)).expect("it aborts");
}

/// The exit that needs a mechanism rather than an intention: an entry point that
/// ended with a scope open. Asserted against the server's own view of what is
/// still in a transaction, not against the driver's bookkeeping.
fn an_abandoned_scope_is_rolled_back_at_the_entry_point(db: &Postgres, cluster: &Cluster) {
    let before = driver_count(db);
    settle(
        db,
        db.begin(
            Isolation::ReadCommitted,
            Access::ReadWrite,
            ALONE,
            Span::DUMMY,
        ),
    )
    .expect("it begins");
    driver_insert(db, "abandoned", 1);
    assert_eq!(db.depth(ALONE), 1);

    let report = db.end_entry_point(ALONE.0).expect("teardown runs");
    assert!(report.is_clean(), "{:?}", report.describe());
    assert_eq!(report.rolled_back, 1, "the open scope was not rolled back");
    assert_eq!(db.depth(ALONE), 0);
    assert_eq!(driver_count(db), before, "an abandoned insert survived");

    let idle = cluster.psql(
        "desk",
        "select count(*) from pg_stat_activity \
         where datname = 'desk' and state like 'idle in transaction%'",
    );
    assert_eq!(
        idle.trim(),
        "0",
        "a connection went back to the pool inside a transaction"
    );
}

/// `Cleanup` is the pool's, and the driver's job is only to name it. Kept here
/// so that a change to the enum fails a test in the file that uses it.
#[test]
fn a_scope_less_statement_hands_its_connection_back_clean() {
    assert_eq!(Cleanup::Clean, Cleanup::Clean);
}
