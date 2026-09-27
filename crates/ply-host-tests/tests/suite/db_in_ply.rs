//! The `db` effect served by Ply itself, against a real server.
//!
//! `std.pg` proves the protocol can be spoken in Ply; this proves the *effect* can be served in
//! Ply — the pool, the transaction scope and the text of every value are `std.db`'s, and the host
//! sees only `net`.

use ply_eval::{Machine, Value};
use ply_span::Span;
use std::sync::Arc;

/// A program that handles its own `db`, so nothing about the effect is the host's.
const PROGRAM: &str = r#"
import std.net (net)
import std.db
import std.db (db, serve, server, stmt, transaction, PInt, PText, CInt, CText, Answer, Rows, Count,
               Failed, ReadCommitted, ReadWrite, Row)

pub fn run(host: String, port: Int) -> Result<String, String>
  / {net.connect[link], net.send[link], net.recv[link], net.close[link]} =
  serve(server(host, port, "ply", "ply", Some("pencil")), 4, "a-test-nonce", || {
    match db.execute[items](stmt("create table if not exists items (id int4 primary key, name text)"), []) {
      Failed(e) -> Err(e.detail),
      _ -> match db.returning[items](
        stmt("insert into items (id, name) values ($1, $2) returning id"),
        [PInt(7), PText("seven")],
      ) {
        Failed(e) -> Err(e.detail),
        Count(_) -> Err("a count where rows were due"),
        Rows(rows) -> match rows {
          [] -> Err("no row came back from the insert"),
          [row, ..rest] -> match map_get(row, "id") {
            Some(CInt(n)) -> match db.query[items](
              stmt("select name from items where id = $1"),
              [PInt(7)],
            ) {
              Failed(e) -> Err(e.detail),
              Count(_) -> Err("a count where rows were due"),
              Rows(found) -> Ok(int_to_string(n) ++ " " ++ names(found)),
            },
            _ -> Err("the id is not an int"),
          },
        },
      },
    }
  })

fn names(rows: List<Row>) -> String =
  match rows {
    [] -> "no rows",
    [row, ..rest] -> match map_get(row, "name") {
      Some(CText(t)) -> t,
      _ -> "the name is not text",
    },
  }

// A transaction commits what it did, through `begin` and `commit` on the same connection.
pub fn commit_one(host: String, port: Int) -> Result<String, String>
  / {net.connect[link], net.send[link], net.recv[link], net.close[link]} =
  serve(server(host, port, "ply", "ply", Some("pencil")), 4, "a-test-nonce", || {
    match transaction(ReadCommitted, ReadWrite, || {
      db.execute[items](stmt("insert into items (id, name) values ($1, $2)"), [PInt(8), PText("eight")])
    }) {
      Err(roll) -> Err(roll.reason),
      Ok(_) -> match db.query[items](stmt("select count(*) as n from items"), []) {
        Failed(e) -> Err(e.detail),
        Count(_) -> Err("a count where rows were due"),
        Rows(rows) -> match rows {
          [row, ..rest] -> Ok(shown(row)),
          [] -> Err("no row"),
        },
      },
    }
  })

fn shown(row: Row) -> String =
  match map_get(row, "n") {
    Some(CText(t)) -> t,
    Some(CInt(n)) -> int_to_string(n),
    _ -> "not a count",
  }
"#;

fn tiered(service: &str) -> (ply_ty::Front, &'static ply_codegen::Unit) {
    let answered =
        ply_codegen::c::producer::checked_front_with_std(&[("m".to_string(), service.to_string())])
            .unwrap_or_else(|e| panic!("they check: {e:#}"));
    let front = answered.front;
    let unit = ply_codegen::Unit::over_front(&front, answered.modules.into_iter().collect())
        .expect("this host has a C compiler");
    (front, unit)
}

/// The entry, over the real network: the host's only part in this is the socket.
fn call(entry: &str, port: u16) -> Result<String, String> {
    let host = ply_host::Host::new();
    let (front, unit) = tiered(PROGRAM);
    let binding = host
        .registry()
        .bind(&front.check)
        .expect("the declaration and the registration agree");

    let mut machine = Machine::new(&front);
    machine.set_compiled(ply_eval::Provider::attach(unit));
    machine.set_host_binding(Arc::new(binding));
    machine.set_host_runtime(host.runtime());
    let simple = entry.rsplit('.').next().expect("an entry has a name");
    if let Some(declared) = front
        .check
        .defs
        .values()
        .find(|d| d.simple_name.as_str() == simple)
        .map(|d| d.footprint.clone())
    {
        machine.set_declared_footprint(declared);
    }

    let answered = machine
        .call(
            entry,
            vec![Value::str("127.0.0.1"), Value::Int(i64::from(port))],
            Span::DUMMY,
        )
        .unwrap_or_else(|e| panic!("the call answers: {e}"));
    let Value::Ctor { name, args } = &answered else {
        panic!("the entry answered {answered}, not an `Ok` or an `Err`");
    };
    let Value::Str(text) = &args[0] else {
        panic!("the entry carried {answered}, not text");
    };
    match name.as_str() {
        "Ok" => Ok(text.to_string()),
        "Err" => Err(text.to_string()),
        other => panic!("the entry answered `{other}`"),
    }
}

fn cluster() -> Option<crate::support::cluster::Cluster> {
    if !crate::support::cluster::available() {
        eprintln!("skipping: this machine has no initdb and postgres");
        return None;
    }
    // A password, so the connection is SCRAM rather than trust.
    Some(crate::support::cluster::Cluster::start_with_password(
        "ply", "pencil",
    ))
}

#[test]
fn the_db_effect_is_served_from_a_real_server() {
    let Some(cluster) = cluster() else { return };
    match call("m.run", cluster.port()) {
        Ok(text) => assert_eq!(text, "7 seven"),
        Err(why) => panic!("the driver could not run a statement: {why}"),
    }
}

#[test]
fn a_transaction_commits_what_it_did() {
    let Some(cluster) = cluster() else { return };
    // The table is the first test's, and it is created if it is not there.
    match call("m.run", cluster.port()) {
        Ok(text) => assert_eq!(text, "7 seven"),
        Err(why) => panic!("the driver could not run a statement: {why}"),
    }
    match call("m.commit_one", cluster.port()) {
        Ok(text) => assert_eq!(text, "2"),
        Err(why) => panic!("the transaction did not commit: {why}"),
    }
}
