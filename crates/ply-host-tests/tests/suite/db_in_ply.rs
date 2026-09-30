//! The `db` effect served by Ply itself, against a real server.
//!
//! `std.pg` proves the protocol can be spoken in Ply; this proves the *effect* can be served in
//! Ply — the pool, the transaction scope and the text of every value are `std.db`'s, and the host
//! sees only `net`. A program serves itself: `with_server` reads the connection string, so nothing
//! about the effect comes from the host.

use ply_eval::{Machine, Value};
use ply_span::Span;
use std::sync::Arc;

/// A program that handles its own `db` from a connection string.
const PROGRAM: &str = r#"
import std.net (net)
import std.random (entropy)
import std.db
import std.db (db, with_server, stmt, transaction, PInt, PText, CInt, CText, Answer, Rows, Count,
               Failed, ReadCommitted, ReadWrite, Row)

pub fn run(url: String) -> Result<String, String>
  / {net.connect[link], net.send[link], net.recv[link], net.close[link], entropy.next} =
  match with_server(url, 4, || {
    match db.returning[items](
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
    }
  }) {
    Err(why) -> Err(why),
    Ok(answered) -> answered,
  }

fn names(rows: List<Row>) -> String =
  match rows {
    [] -> "no rows",
    [row, ..rest] -> match map_get(row, "name") {
      Some(CText(t)) -> t,
      _ -> "the name is not text",
    },
  }

// A statement that writes, performed as `db.query`: the scheduler would treat two of these as
// readers, so the driver refuses it before it reaches the server.
pub fn sneaky(url: String) -> Result<String, String>
  / {net.connect[link], net.send[link], net.recv[link], net.close[link], entropy.next} =
  match with_server(url, 4, || {
    match db.query[items](stmt("insert into items (id, name) values ($1, $2)"), [PInt(1), PText("x")]) {
      _ -> Ok("the write went through as a read"),
    }
  }) {
    Err(why) -> Err(why),
    Ok(answered) -> answered,
  }

// A call site's label is the atom the scheduler records, so a statement that reaches a table the
// label never named would be scheduled against the wrong table.
pub fn mislabelled(url: String) -> Result<String, String>
  / {net.connect[link], net.send[link], net.recv[link], net.close[link], entropy.next} =
  match with_server(url, 4, || {
    match db.query[orders](stmt("select name from items where id = $1"), [PInt(7)]) {
      _ -> Ok("the statement ran under a label it does not touch"),
    }
  }) {
    Err(why) -> Err(why),
    Ok(answered) -> answered,
  }

// A transaction commits what it did, through `begin` and `commit` on the same connection.
pub fn commit_one(url: String) -> Result<String, String>
  / {net.connect[link], net.send[link], net.recv[link], net.close[link], entropy.next} =
  match with_server(url, 4, || {
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
  }) {
    Err(why) -> Err(why),
    Ok(answered) -> answered,
  }

fn shown(row: Row) -> String =
  match map_get(row, "n") {
    Some(CText(t)) -> t,
    Some(CInt(n)) -> int_to_string(n),
    _ -> "not a count",
  }

// The two timeouts as the server holds them for this session, which the connection string set.
pub fn timeouts(url: String) -> Result<String, String>
  / {net.connect[link], net.send[link], net.recv[link], net.close[link], entropy.next} =
  match with_server(url, 1, || {
    match db.query[pg_settings](
      stmt("select name, setting from pg_settings where name = 'statement_timeout' or name = 'idle_in_transaction_session_timeout' order by name"),
      [],
    ) {
      Failed(e) -> Err(e.detail),
      Count(_) -> Err("a count where rows were due"),
      Rows(rows) -> Ok(fold(rows, "", |acc: String, row: Row| acc ++ setting(row) ++ ";")),
    }
  }) {
    Err(why) -> Err(why),
    Ok(answered) -> answered,
  }

fn setting(row: Row) -> String =
  match (map_get(row, "name"), map_get(row, "setting")) {
    (Some(CText(name)), Some(CText(value))) -> name ++ "=" ++ value,
    _ -> "not a setting",
  }

// A statement that runs far past the timeout the connection string set, and what ended it.
pub fn overrun(url: String) -> Result<String, String>
  / {net.connect[link], net.send[link], net.recv[link], net.close[link], entropy.next} =
  match with_server(url, 1, || {
    match db.query[pg_class](
      stmt("select count(*) as n from pg_class a, pg_class b, pg_class c, pg_class d"),
      [],
    ) {
      Failed(e) -> Ok(e.detail),
      _ -> Err("the statement finished inside its timeout"),
    }
  }) {
    Err(why) -> Err(why),
    Ok(answered) -> answered,
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

/// The entry, over the real network: the host's only part in this is the socket and the entropy.
fn call_outcome(entry: &str, url: &str) -> Result<Value, ply_span::Diagnostic> {
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
    machine.call(entry, vec![Value::str(url)], Span::DUMMY)
}

/// What the entry answered, as `Ok`'s text or `Err`'s.
fn call(entry: &str, url: &str) -> Result<String, String> {
    let answered = call_outcome(entry, url).unwrap_or_else(|e| panic!("the call answers: {e}"));
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

/// Why the entry would not run, for the refusals that are the driver's rather than the server's.
fn call_err(entry: &str, url: &str) -> String {
    match call_outcome(entry, url) {
        Ok(other) => panic!("the call answered {other}, not a refusal"),
        Err(why) => format!("{why}"),
    }
}

fn cluster() -> Option<(crate::support::cluster::Cluster, String)> {
    if !crate::support::cluster::available() {
        eprintln!("skipping: postgres is not installed here");
        return None;
    }
    // A password, so the connection is SCRAM rather than trust.
    let cluster = crate::support::cluster::Cluster::start_with_password("ply", "pencil");
    let url = format!(
        "postgres://ply:pencil@127.0.0.1:{}/ply?sslmode=disable",
        cluster.port()
    );
    Some((cluster, url))
}

#[test]
fn the_db_effect_is_served_from_a_real_server() {
    let Some((cluster, url)) = cluster() else {
        return;
    };
    // `db.execute` carries statements, not a schema: a statement that is not one of the four verbs
    // is refused, which is what the Rust driver does too.
    cluster.psql(
        "ply",
        "create table if not exists items (id int4 primary key, name text)",
    );
    match call("m.run", &url) {
        Ok(text) => assert_eq!(text, "7 seven"),
        Err(why) => panic!("the driver could not run a statement: {why}"),
    }
}

#[test]
fn a_write_performed_as_a_read_is_refused() {
    let Some((_cluster, url)) = cluster() else {
        return;
    };
    let why = call_err("m.sneaky", &url);
    assert!(
        why.contains("writes") && why.contains("db.query"),
        "the refusal does not say what is wrong: {why}"
    );
}

#[test]
fn a_label_the_statement_does_not_touch_is_refused() {
    let Some((_cluster, url)) = cluster() else {
        return;
    };
    let why = call_err("m.mislabelled", &url);
    assert!(
        why.contains("names `orders`") && why.contains("items"),
        "the refusal does not say which label does not fit: {why}"
    );
}

#[test]
fn a_transaction_commits_what_it_did() {
    let Some((cluster, url)) = cluster() else {
        return;
    };
    // The schema is the run's, as it is for the Rust driver: `db.execute` carries statements, not
    // DDL, so the table is made before the program starts.
    cluster.psql(
        "ply",
        "create table if not exists items (id int4 primary key, name text)",
    );
    match call("m.run", &url) {
        Ok(text) => assert_eq!(text, "7 seven"),
        Err(why) => panic!("the driver could not run a statement: {why}"),
    }
    match call("m.commit_one", &url) {
        Ok(text) => assert_eq!(text, "2"),
        Err(why) => panic!("the transaction did not commit: {why}"),
    }
}

#[test]
fn a_connection_strings_timeouts_bound_every_statement_on_the_server() {
    let Some((_cluster, url)) = cluster() else {
        return;
    };
    let bounded = format!("{url}&statement_timeout=200&idle_in_transaction_session_timeout=1500");
    match call("m.timeouts", &bounded) {
        Ok(text) => assert_eq!(
            text,
            "idle_in_transaction_session_timeout=1500;statement_timeout=200;"
        ),
        Err(why) => panic!("the settings could not be read back: {why}"),
    }
    match call("m.overrun", &bounded) {
        Ok(why) => assert!(
            why.contains("57014"),
            "the statement ended for another reason: {why}"
        ),
        Err(why) => panic!("the timeout did not bound the statement: {why}"),
    }
}
