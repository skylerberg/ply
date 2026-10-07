//! The `db` effect served by Ply itself, against a real server.
//!
//! `std.pg` proves the protocol can be spoken in Ply; this proves the *effect* can be served in
//! Ply — the pool, the transaction scope and the text of every value are `std.db`'s, and the host
//! sees only `net`. A program serves itself: `with_server` reads the connection string, so nothing
//! about the effect comes from the host.

use ply_eval::{Machine, Span, Symbol, Value};
use std::sync::Arc;

/// A program that handles its own `db` from a connection string.
const PROGRAM: &str = r#"
import std.net (net)
import std.random (entropy)
import std.db
import std.db (db, with_server, transaction, is_retryable, ReadCommitted, ReadWrite, Serializable, Rollback)
import std.sql (stmt, PInt, PText, CInt, CText, Answer, Rows, Count, Failed, Row)

pub fn run(url: String) -> Result<String, String>
  / {net.connect[link], net.start_tls[link], net.peer_certificate[link], net.send_secret[link], net.send[link], net.recv[link], net.close[link], entropy.next, abort.raise, diverges} =
  run_with(url, None)

// `run`, its password given beside the connection string, as `config.secret` answers one.
pub fn run_given_password(url: String) -> Result<String, String>
  / {net.connect[link], net.start_tls[link], net.peer_certificate[link], net.send_secret[link], net.send[link], net.recv[link], net.close[link], entropy.next, abort.raise, diverges} =
  run_with(url, Some(secret_of_bytes(b"p@ss:w")))

fn run_with(url: String, password: Option<Secret<Bytes>>) -> Result<String, String>
  / {net.connect[link], net.start_tls[link], net.peer_certificate[link], net.send_secret[link], net.send[link], net.recv[link], net.close[link], entropy.next, abort.raise, diverges} =
  match with_server(url, 4, password: password, body: || {
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
  / {net.connect[link], net.start_tls[link], net.peer_certificate[link], net.send_secret[link], net.send[link], net.recv[link], net.close[link], entropy.next, abort.raise, diverges} =
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
  / {net.connect[link], net.start_tls[link], net.peer_certificate[link], net.send_secret[link], net.send[link], net.recv[link], net.close[link], entropy.next, abort.raise, diverges} =
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
  / {net.connect[link], net.start_tls[link], net.peer_certificate[link], net.send_secret[link], net.send[link], net.recv[link], net.close[link], entropy.next, abort.raise, diverges} =
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
  / {net.connect[link], net.start_tls[link], net.peer_certificate[link], net.send_secret[link], net.send[link], net.recv[link], net.close[link], entropy.next, abort.raise, diverges} =
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
  / {net.connect[link], net.start_tls[link], net.peer_certificate[link], net.send_secret[link], net.send[link], net.recv[link], net.close[link], entropy.next, abort.raise, diverges} =
  match with_server(url, 1, || {
    match db.query[pg_class](
      stmt("select count(*) as n from pg_class a, pg_class b, pg_class c, pg_class d"),
      [],
    ) {
      Failed(e) -> Ok(e.code ++ ": " ++ e.detail),
      _ -> Err("the statement finished inside its timeout"),
    }
  }) {
    Err(why) -> Err(why),
    Ok(answered) -> answered,
  }

fn bump(id: Int, by: Int) -> Answer / {db.execute[ledger]} =
  db.execute[ledger](stmt("update ledger set n = n + $2 where id = $1"), [PInt(id), PInt(by)])

fn ended(out: Result<Unit, Rollback>) -> String =
  match out { Ok(_) -> "committed", Err(rolled) -> rolled.reason }

// Two tasks in transactions at once on one pool, over one row: each holds a connection of its own,
// so the one that rolls back undoes only its own write, whichever takes the row first.
pub fn two_at_once(url: String) -> Result<String, String>
  / {net.connect[link], net.start_tls[link], net.peer_certificate[link], net.send_secret[link], net.send[link], net.recv[link], net.close[link], entropy.next, task.spawn, task.join, abort.raise, diverges} =
  with_server(url, 2, || {
    let kept = task.spawn(|| ended(transaction(ReadCommitted, ReadWrite, || {
      bump(1, 1);
      ()
    })));
    let undone = task.spawn(|| ended(transaction(ReadCommitted, ReadWrite, || {
      bump(1, 10);
      db.rollback("undone")
    })));
    task.join(kept) ++ " " ++ task.join(undone)
  })

// Two serializable transactions writing one row: the one that writes second is refused with 40001,
// and run again from its `begin` it goes through. The other runs on a pool of its own, inside the
// first attempt.
pub fn serialized(url: String) -> Result<String, String>
  / {net.connect[link], net.start_tls[link], net.peer_certificate[link], net.send_secret[link], net.send[link], net.recv[link], net.close[link], entropy.next, abort.raise, diverges} =
  with_server(url, 1, || contended(url, 3, ""))

fn contended(url: String, left: Int, seen: String) -> String / {db.query[ledger], db.execute[ledger], db.abort, db.begin, db.commit, db.rollback, net.close[link], net.connect[link], net.start_tls[link], net.peer_certificate[link], net.send_secret[link], net.recv[link], net.send[link], entropy.next, abort.raise, diverges} =
  match transaction(Serializable, ReadWrite, || {
      db.query[ledger](stmt("select n from ledger where id = $1"), [PInt(1)]);
      if seen == "" {
        with_server(url, 1, || transaction(Serializable, ReadWrite, || bump(1, 10)));
        ()
      };
      match bump(1, 1) {
        Failed(e) -> db.rollback(if is_retryable(e) { "retry " ++ e.code } else { e.code }),
        _ -> (),
      }
    }) {
    Ok(_) -> seen ++ "committed",
    Err(rolled) -> if string_starts_with(rolled.reason, "retry ") && left > 1 {
      contended(url, left - 1, seen ++ string_slice(rolled.reason, 6, string_len(rolled.reason)) ++ " then ")
    } else { seen ++ rolled.reason },
  }

// Two tasks, each on a pool of its own, taking two rows in opposite orders: the server breaks the
// deadlock by refusing one with 40P01, and the other commits once that one has rolled back.
pub fn deadlocked(url: String) -> Result<String, String>
  / {net.connect[link], net.start_tls[link], net.peer_certificate[link], net.send_secret[link], net.send[link], net.recv[link], net.close[link], entropy.next, task.spawn, task.join, abort.raise, diverges} = {
  let a = task.spawn(|| crossing(url, 1, 2));
  let b = task.spawn(|| crossing(url, 2, 1));
  Ok(task.join(a) ++ " " ++ task.join(b))
}

fn crossing(url: String, first: Int, second: Int) -> String / {net.close[link], net.connect[link], net.start_tls[link], net.peer_certificate[link], net.send_secret[link], net.recv[link], net.send[link], entropy.next, abort.raise, diverges} =
  match with_server(url, 1, || ended(transaction(ReadCommitted, ReadWrite, || {
      bump(first, 1);
      both_written(400);
      match bump(second, 1) { Failed(e) -> db.rollback(e.code), _ -> () }
    }))) {
    Err(why) -> why,
    Ok(text) -> text,
  }

// Until both transactions have written, when each holds a transaction id of its own.
fn both_written(left: Int) -> Unit / {db.query[pg_locks]} =
  if left <= 0 { () } else {
    match db.query[pg_locks](
        stmt("select count(*) as n from pg_locks where locktype = 'transactionid' and granted"),
        [],
      ) {
      Rows([row, ..]) -> match map_get(row, "n") {
        Some(CInt(n)) -> if n >= 2 { () } else { both_written(left - 1) },
        _ -> both_written(left - 1),
      },
      _ -> both_written(left - 1),
    }
  }
"#;

fn compiled(service: &str) -> (ply_eval::Analysis, &'static ply_codegen::Unit) {
    crate::support::answered::compiled("m", service)
}
/// The entry, over the real network: the host's only part in this is the socket and the entropy.
fn call_outcome(entry: &str, url: &str) -> Result<Value, ply_eval::Diagnostic> {
    let host = std::sync::Arc::new(ply_host::Host::new());
    let (front, unit) = compiled(PROGRAM);
    let binding = host
        .registry()
        .bind(&front.check)
        .expect("the declaration and the registration agree");

    let mut machine = Machine::new(&front, ply_eval::Provider::attach(unit))
        .expect("the unit was compiled from this program");
    machine.set_host_binding(Arc::new(binding));
    machine.set_host_runtime({
        let host = std::sync::Arc::clone(&host);
        std::sync::Arc::new(move || host.runtime())
    });
    let declared = front
        .check
        .defs
        .get(&Symbol::new(entry))
        .expect("the entry is a definition of the program");
    machine.set_declared_footprint(declared.footprint.clone());
    machine
        .call(entry, vec![Value::str(url)], Span::DUMMY)
        .into_parts()
        .0
}

/// What the entry answered, as `Ok`'s text or `Err`'s.
fn call(entry: &str, url: &str) -> Result<String, String> {
    let answered = call_outcome(entry, url).unwrap_or_else(|e| panic!("the call answers: {e}"));
    let Value::Ctor { name, args } = &answered else {
        panic!("the entry answered {answered:?}, not an `Ok` or an `Err`");
    };
    let Value::Str(text) = &args[0] else {
        panic!("the entry carried {answered:?}, not text");
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
        Ok(other) => panic!("the call answered {other:?}, not a refusal"),
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

/// The password, sealed where it is read (the connection string, or settings beside it), sent in
/// the clear-text message a `password` cluster asks for.
#[test]
fn the_db_effect_sends_a_password_in_the_clear_when_asked_from_the_url_or_settings() {
    if !crate::support::cluster::available() {
        eprintln!("skipping: postgres is not installed here");
        return;
    }
    let cluster = crate::support::cluster::Cluster::start_with_clear_text_password("ply", "p@ss:w");
    cluster.psql(
        "ply",
        "create table if not exists items (id int4 primary key, name text)",
    );
    let port = cluster.port();
    let in_url = format!("postgres://ply:p%40ss%3Aw@127.0.0.1:{port}/ply?sslmode=disable");
    match call("m.run", &in_url) {
        Ok(text) => assert_eq!(text, "7 seven"),
        Err(why) => panic!("the driver could not authenticate in the clear: {why}"),
    }
    cluster.psql("ply", "delete from items");
    let bare = format!("postgres://ply@127.0.0.1:{port}/ply?sslmode=disable");
    match call("m.run_given_password", &bare) {
        Ok(text) => assert_eq!(text, "7 seven"),
        Err(why) => panic!("the driver could not authenticate with the given password: {why}"),
    }
    let refused = call("m.run_given_password", &in_url)
        .expect_err("a password both in the URL and given is refused");
    assert!(refused.contains("give it once"), "{refused}");
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
            why.starts_with("57014: ") && why.contains("statement timeout"),
            "the statement ended for another reason, or its SQLSTATE was not its code: {why}"
        ),
        Err(why) => panic!("the timeout did not bound the statement: {why}"),
    }
}

/// A table of counters, one row per id, each starting at zero.
fn ledger(cluster: &crate::support::cluster::Cluster, ids: &[i32]) {
    cluster.psql(
        "ply",
        "create table ledger (id int4 primary key, n int4 not null)",
    );
    for id in ids {
        cluster.psql("ply", &format!("insert into ledger values ({id}, 0)"));
    }
}

#[test]
fn two_tasks_in_transactions_at_once_each_hold_a_connection_of_their_own() {
    let Some((cluster, url)) = cluster() else {
        return;
    };
    ledger(&cluster, &[1]);
    match call("m.two_at_once", &url) {
        Ok(text) => assert_eq!(text, "committed undone"),
        Err(why) => panic!("the pool could not hold two transactions: {why}"),
    }
    assert_eq!(
        cluster.psql("ply", "select n from ledger where id = 1"),
        "1",
        "the rolled-back write landed, or the committed one did not"
    );
}

#[test]
fn a_serialization_failure_is_40001_and_the_retried_transaction_commits() {
    let Some((cluster, url)) = cluster() else {
        return;
    };
    ledger(&cluster, &[1]);
    match call("m.serialized", &url) {
        Ok(text) => assert_eq!(text, "40001 then committed"),
        Err(why) => panic!("the serialization failure was not retried: {why}"),
    }
    assert_eq!(
        cluster.psql("ply", "select n from ledger where id = 1"),
        "11",
        "the other transaction's write and the retried one's are not both there"
    );
}

#[test]
fn a_deadlock_is_40p01_and_the_transaction_left_standing_commits() {
    let Some((cluster, url)) = cluster() else {
        return;
    };
    ledger(&cluster, &[1, 2]);
    let text = call("m.deadlocked", &url).unwrap_or_else(|why| panic!("{why}"));
    let mut ends: Vec<&str> = text.split(' ').collect();
    ends.sort_unstable();
    assert_eq!(ends, ["40P01", "committed"], "{text}");
    assert_eq!(
        cluster.psql("ply", "select sum(n) from ledger"),
        "2",
        "the survivor's two writes are not all that landed"
    );
}
