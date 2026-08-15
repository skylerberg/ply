//! A park that holds two facilities has to wake for either of them.
//!
//! `Facilities` composes a socket pool and a database reactor, and each of them
//! blocks on a condition variable of its own. Nothing signals both. So a park
//! that waits on one alone sits through the other's completion — and that is not
//! a corner case: a task-per-connection server always has an `accept`
//! outstanding, so *every* query a spawned task issues would wait for the next
//! connection to arrive before the scheduler noticed it had an answer.
//!
//! The failure shape is the one this project audits for: not a crash and not a
//! wrong answer, but a request that never comes back, invisible to a sequential
//! accept loop and therefore to every benchmark and every test that used one.
//! `db::pool::Reactor::park_timeout` was written for this caller and was not
//! being called by it.
//!
//! The test is a stopwatch, which is usually the wrong instrument — but the
//! defect *is* a wait, so what has to be asserted is that one ended.

mod support;

use ply_core::ty::Resource;
use ply_eval::host::{HostAnswer, MachineId, Pending};
use ply_host::db::scope::Owner;
use ply_host::db::{self, Driver, Op, Postgres, Statement};
use ply_host::tcp::Net;
use ply_host::tls::Credentials;
use ply_host::{Host, db::PoolConfig};
use ply_span::{Span, Symbol};
use std::time::{Duration, Instant};
use support::cluster::{self, Cluster};

const ALONE: Owner = (MachineId(0), None);

/// How long the query is given once the socket is also outstanding. Three orders
/// of magnitude above the alternation bound and two above the query, so a
/// failure here is the hang rather than a slow machine.
const PATIENCE: Duration = Duration::from_secs(10);

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("a port");
    let port = listener.local_addr().expect("an address").port();
    drop(listener);
    port
}

fn pending(answered: Result<HostAnswer, ply_span::Diagnostic>, what: &str) -> Pending {
    match answered.unwrap_or_else(|d| panic!("{what}: {} {}", d.code, d.message)) {
        HostAnswer::Pending(pending) => pending,
        HostAnswer::Value(value) => panic!("{what} answered {value} rather than waiting"),
    }
}

fn value(answered: Result<HostAnswer, ply_span::Diagnostic>, what: &str) -> ply_eval::Value {
    match answered.unwrap_or_else(|d| panic!("{what}: {} {}", d.code, d.message)) {
        HostAnswer::Value(value) => value,
        HostAnswer::Pending(_) => panic!("{what} waited rather than answering"),
    }
}

fn open(cluster: &Cluster) -> Host {
    Host::with_database(
        Credentials::empty(),
        PoolConfig {
            url: cluster.url(),
            size: 2,
            acquire: Duration::from_secs(5),
            statement: Duration::from_secs(30),
            idle_txn: Duration::from_secs(30),
            connect: Duration::from_secs(5),
            statements: 16,
        },
    )
    .expect("the pool opens")
}

/// A `select` through the driver, left outstanding.
fn query(db: &Postgres) -> Pending {
    const SQL: &str = "select id from ledger";
    let scan = db::scan::scan(SQL, Span::DUMMY).expect("the statement scans");
    let at = Resource::Named(Symbol::new("ledger"));
    let touched =
        db::check_footprint(&scan, Op::Query, &at, None, Span::DUMMY).expect("the label covers it");
    pending(
        db.statement(Statement {
            op: Op::Query,
            at: &at,
            sql: SQL,
            params: Vec::new(),
            touched,
            scan: &scan,
            owner: ALONE,
            span: Span::DUMMY,
        }),
        "the query",
    )
}

#[test]
fn a_query_resolves_while_an_accept_nobody_will_answer_is_outstanding() {
    if !cluster::available() {
        eprintln!(
            "skipping: this machine has no `initdb`/`postgres`/`psql`, so there is no database \
             for a park to hold a token from"
        );
        return;
    }
    let cluster = Cluster::start("park");
    cluster.psql(
        &cluster.database,
        "create table ledger (id int8 primary key)",
    );
    let host = open(&cluster);
    let runtime = host.runtime();

    // The socket half: a listener nobody will ever connect to, with an `accept`
    // parked on it. This is a task-per-connection server between requests, and
    // it is outstanding for the whole life of such a server.
    let at = Resource::Named(Symbol::new("listener"));
    let listener = value(
        host.net().listen(&at, free_port(), Span::DUMMY),
        "the listen",
    );
    let ply_eval::Value::Int(listener) = listener else {
        panic!("`listen` answered {listener}, which is not a socket");
    };
    let accept = pending(host.net().accept(&at, listener, Span::DUMMY), "the accept");

    // The database half, issued after the accept is already parked, which is the
    // order a spawned task would produce.
    let db = host.database().expect("a database").clone();
    let answer = query(&db);

    let until = Instant::now() + PATIENCE;
    let resolved = loop {
        if let Some(value) = runtime.poll(&answer).expect("the token is ours") {
            break value;
        }
        assert!(
            Instant::now() < until,
            "the query never resolved: `park` waited on the socket pool while the database held \
             the token, which is the hang a task-per-connection server sees on every request"
        );
        runtime.park().expect("something is outstanding");
    };
    assert!(
        matches!(resolved, ply_eval::Value::Ctor { .. }),
        "the query answered {resolved}"
    );

    // And the socket is still waiting, which is what makes the assertion above
    // about the park rather than about an accept that happened to return.
    assert!(
        runtime.poll(&accept).expect("the token is ours").is_none(),
        "the accept resolved, so the park had a socket event to wake on and this proved nothing"
    );
}

/// And the reason it resolves: the two facilities mint from disjoint ranges, so
/// "did you mint this token" has one answer.
///
/// Asserted on the tokens themselves rather than on the constant, because what
/// matters is the pair and either half moving is what would break it.
#[test]
fn the_two_facilities_mint_tokens_that_cannot_collide() {
    if !cluster::available() {
        eprintln!("skipping: this machine has no postgres binaries");
        return;
    }
    let cluster = Cluster::start("tokens");
    cluster.psql(
        &cluster.database,
        "create table ledger (id int8 primary key)",
    );
    let host = open(&cluster);

    let at = Resource::Named(Symbol::new("listener"));
    let listener = value(
        host.net().listen(&at, free_port(), Span::DUMMY),
        "the listen",
    );
    let ply_eval::Value::Int(listener) = listener else {
        panic!("`listen` answered {listener}, which is not a socket");
    };
    let socket = pending(host.net().accept(&at, listener, Span::DUMMY), "the accept");
    let db = host.database().expect("a database").clone();
    let statement = query(&db);

    assert_ne!(socket.token, statement.token);
    assert!(
        !host.net().owns(&statement),
        "the socket pool claims the database's token, so a poll would land on the wrong table"
    );
    assert!(
        host.database().expect("a database").owns(&statement),
        "the database disowns its own token"
    );
}
