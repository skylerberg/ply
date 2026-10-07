//! `std.db`'s TLS, notifications and copies, against a real server.

use ply_eval::{Machine, Span, Symbol, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

const PROGRAM: &str = r#"
import std.net (net)
import std.random (entropy)
import std.seq
import std.string
import std.db
import std.sql
import std.db (db, with_server, transaction, copy_into, copy_from, ReadCommitted, ReadWrite, Notice, Notified, Reconnected, Unheard, Quiet)
import std.sql (stmt, PInt, PText, PNull, PBytes, PArray, PNumeric, CInt, Rows, Count, Failed, Row, DbError)
import std.seq (Seq, seq)

effect set Wire = {net.connect[link], net.start_tls[link], net.peer_certificate[link], net.send_secret[link], net.send[link], net.recv[link], net.close[link], entropy.next}

fn served(answered: Result<Result<String, String>, String>) -> Result<String, String> =
  match answered { Err(why) -> Err(why), Ok(inner) -> inner }

fn failure(e: DbError) -> String = e.code ++ ": " ++ e.detail

fn number(rows: List<Row>) -> String =
  match rows {
    [row, ..] -> match map_get(row, "n") { Some(CInt(n)) -> int_to_string(n), _ -> "not a number" },
    [] -> "no row",
  }

// How many of the server's sessions are encrypted, asked over a connection the URL secures.
pub fn encrypted(url: String) -> Result<String, String> / {Wire, abort.raise, diverges} =
  served(with_server(url, 1, ||
    match db.query[pg_stat_ssl](stmt("select count(*) as n from pg_stat_ssl where ssl"), []) {
      Failed(e) -> Err(failure(e)),
      Count(_) -> Err("a count where rows were due"),
      Rows(rows) -> Ok(number(rows)),
    }))

fn shown(n: Notice) -> String =
  match n {
    Notified(note) -> note.channel ++ ":" ++ note.payload,
    Reconnected -> "reconnected",
    Unheard(e) -> "unheard " ++ e.code,
    Quiet -> "quiet",
  }

fn kept(r: Result<String, db::Rollback>) -> String = match r { Ok(s) -> s, Err(rolled) -> rolled.reason }

// A notification is heard once its transaction commits, once however often it was sent there, never
// when the transaction rolls back, at once outside one, and not once its channel is let go.
pub fn notices(url: String) -> Result<String, String> / {Wire, abort.raise, diverges} =
  with_server(url, 2, || {
    db.listen[jobs]();
    let inside = kept(transaction(ReadCommitted, ReadWrite, || {
      db.notify[jobs]("one");
      db.notify[jobs]("one");
      shown(db.notified(300))
    }));
    let committed = shown(db.notified(10000));
    let once = shown(db.notified(300));
    let rolled = kept(transaction(ReadCommitted, ReadWrite, || {
      db.notify[jobs]("two");
      db.rollback("undone");
      "not rolled back"
    }));
    let none = shown(db.notified(300));
    db.notify[jobs]("three");
    let at_once = shown(db.notified(10000));
    db.listen[other]();
    db.unlisten[jobs]();
    db.notify[jobs]("four");
    let after = shown(db.notified(300));
    string::join([inside, committed, once, rolled, none, at_once, after], " ")
  })

// Until it has heard `want` things other than quiet, each written down as it is heard.
fn heard_until(want: Int, left: Int, acc: List<String>) -> String / {db.notified, db.execute[heard]} =
  if len(acc) >= want || left <= 0 { string::join(acc, ",") } else {
    match db.notified(1000) {
      Quiet -> heard_until(want, left - 1, acc),
      n -> {
        db.execute[heard](stmt("insert into heard (what) values ($1)"), [PText(shown(n))]);
        heard_until(want, left - 1, push(acc, shown(n)))
      },
    }
  }

// A listener whose connection the test ends: what it heard before, that it reconnected, and after.
pub fn reconnects(url: String) -> Result<String, String> / {Wire, abort.raise, diverges} =
  with_server(url, 1, || {
    db.listen[jobs]();
    heard_until(3, 120, [])
  })

fn sample() -> List<List<sql::Param>> =
  [
    [PInt(1), PText("plain"), PBytes(b"\x00\x01\xff"), PArray([PText("a"), PText("b,c")]), PNumeric(1.5m)],
    [PInt(2), PText("tab\there\nnew line\\back"), PNull, PNull, PNull],
    [PInt(3), PText("\\N"), PBytes(b""), PArray([]), PNumeric(0m)],
  ]

fn named(id: Int, name: String) -> List<sql::Param> = [PInt(id), PText(name), PNull, PNull, PNull]

fn all() -> sql::Stmt = stmt("select id, name, data, tags, price from items order by id")

fn counted() -> String / {db.query[items]} =
  match db.query[items](stmt("select count(*) as n from items"), []) {
    Rows(rows) -> number(rows),
    Failed(e) -> failure(e),
    Count(_) -> "a count",
  }

// What a copy of `rows` into `items` answered: how many rows the server took, the SQLSTATE that
// refused it, or the budget its rows ran past.
fn copied_in<| e>(rows: Seq<List<sql::Param> | e>, chunk: Int, budget: Int) -> String / {db.copy_in[items], db.copy_rows[items], db.copy_end[items] | e} =
  match try[seq.spent] { copy_into[items](["id", "name", "data", "tags", "price"], rows, chunk, budget) } {
    Ok(Ok(n)) -> int_to_string(n),
    Ok(Err(e)) -> e.code,
    Err(spent) -> "spent " ++ int_to_string(spent),
  }

// Rows copied in and out: what each copy answered, whether the rows copied out are the rows a query
// answers, and what a refused copy, an abandoned one and a rolled back one each left behind.
pub fn copies(url: String) -> Result<String, String> / {Wire, abort.raise, diverges} =
  with_server(url, 2, || {
    let into = copied_in(seq::of_list(sample()), 2, 100);
    let out = copy_from[items](all(), 2, |rows: Seq<Row | {db.copy_take[items]}>| try[seq.spent] { seq::to_list(rows, 100) });
    let same = match out {
      Ok(Ok(copied)) -> match db.query[items](all(), []) {
        Rows(queried) -> if copied == queried { "same" } else { "different" },
        _ -> "no rows",
      },
      Ok(Err(_)) -> "spent",
      Err(e) -> failure(e),
    };
    let twice = copied_in(seq::of_list([named(4, "new"), named(1, "again")]), 1, 100);
    let after_twice = counted();
    let spent = copied_in(seq::of_list(map(range(10, 15), |i: Int| named(i, "x"))), 2, 3);
    let after_spent = counted();
    let rolled = kept(transaction(ReadCommitted, ReadWrite, || {
      let copied = copied_in(seq::of_list([named(20, "y")]), 10, 10);
      db.rollback("rolled back after " ++ copied);
      "not rolled back"
    }));
    let after_rolled = counted();
    let broken = match copy_from[items](stmt("select 1 / (id - 2) as x from items order by id"), 1, |rows: Seq<Row | {db.copy_take[items]}>| try[seq.spent] { len(seq::to_list(rows, 100)) }) {
      Ok(_) -> "kept",
      Err(e) -> e.code,
    };
    string::join([into, same, twice, after_twice, spent, after_spent, rolled, after_rolled, broken], " ")
  })
"#;

fn compiled() -> (ply_eval::Analysis, &'static ply_codegen::Unit) {
    crate::support::answered::compiled("m", PROGRAM)
}

/// What the entry answered, as `Ok`'s text or `Err`'s, over the real network through `host`.
fn call(host: Arc<ply_host::Host>, entry: &str, url: &str) -> Result<String, String> {
    let (front, unit) = compiled();
    let binding = host
        .registry()
        .bind(&front.check)
        .expect("the declaration and the registration agree");
    let mut machine = Machine::new(&front, ply_eval::Provider::attach(unit))
        .expect("the unit was compiled from this program");
    machine.set_host_binding(Arc::new(binding));
    machine.set_host_runtime({
        let host = Arc::clone(&host);
        Arc::new(move || host.runtime())
    });
    let declared = front
        .check
        .defs
        .get(&Symbol::new(entry))
        .expect("the entry is a definition of the program");
    machine.set_declared_footprint(declared.footprint.clone());
    let answered = machine
        .call(entry, vec![Value::str(url)], Span::DUMMY)
        .into_parts()
        .0
        .unwrap_or_else(|e| panic!("the call answers: {e}"));
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

/// A host that trusts the certificate at `trusted`, as `--trust` would have it.
fn trusting(trusted: &std::path::Path) -> Arc<ply_host::Host> {
    let credentials = ply_host::Credentials::load(&[], &[trusted.to_path_buf()])
        .unwrap_or_else(|why| panic!("the certificate is trusted: {why:?}"));
    Arc::new(ply_host::Host::with_credentials(credentials))
}

/// A cluster that speaks TLS with a certificate made for `names`, and where that certificate is.
fn secured_cluster(
    names: &[&str],
) -> Option<(crate::support::cluster::Cluster, tempfile::TempDir, PathBuf)> {
    if !crate::support::cluster::available() {
        eprintln!("skipping: postgres is not installed here");
        return None;
    }
    let issued = ply_host::certgen::issue(&names.iter().map(|n| n.to_string()).collect::<Vec<_>>())
        .expect("a certificate is made");
    let cluster = crate::support::cluster::Cluster::start_with_tls(
        "ply",
        "pencil",
        &issued.certificate,
        &issued.key,
    );
    let dir = tempfile::tempdir().expect("a directory for the certificate");
    let pem = dir.path().join("server.pem");
    std::fs::write(&pem, &issued.certificate).expect("the certificate is written");
    Some((cluster, dir, pem))
}

fn url(cluster: &crate::support::cluster::Cluster, host: &str, query: &str) -> String {
    format!(
        "postgres://ply:pencil@{host}:{}/ply?{query}",
        cluster.port()
    )
}

#[test]
fn a_secured_connection_is_encrypted_and_verified_for_its_host() {
    let Some((cluster, _dir, pem)) = secured_cluster(&["127.0.0.1"]) else {
        return;
    };
    let host = trusting(&pem);
    for mode in [
        "sslmode=verify-full",
        "sslmode=require",
        "sslmode=verify-ca",
    ] {
        assert_eq!(
            call(
                Arc::clone(&host),
                "m.encrypted",
                &url(&cluster, "127.0.0.1", mode)
            ),
            Ok("1".to_string()),
            "{mode}"
        );
    }
    assert_eq!(
        call(
            Arc::clone(&host),
            "m.encrypted",
            &url(&cluster, "127.0.0.1", "sslmode=disable")
        ),
        Ok("0".to_string()),
        "a plaintext connection to a server that offers TLS stays plaintext"
    );
}

#[test]
fn a_certificate_not_trusted_or_not_for_the_host_is_refused() {
    let Some((cluster, _dir, pem)) = secured_cluster(&["127.0.0.1"]) else {
        return;
    };
    let untrusted = Arc::new(ply_host::Host::new());
    let refused = call(
        untrusted,
        "m.encrypted",
        &url(&cluster, "127.0.0.1", "sslmode=require"),
    )
    .expect_err("an untrusted certificate");
    assert!(
        refused.starts_with("08001: ") && refused.contains("not trusted"),
        "{refused}"
    );
    let elsewhere = call(
        trusting(&pem),
        "m.encrypted",
        &url(&cluster, "localhost", "sslmode=verify-full"),
    )
    .expect_err("a certificate for another name");
    assert!(
        elsewhere.starts_with("08001: ") && elsewhere.contains("not trusted"),
        "{elsewhere}"
    );
}

#[test]
fn a_server_that_will_not_speak_tls_is_not_spoken_to_in_plaintext() {
    if !crate::support::cluster::available() {
        eprintln!("skipping: postgres is not installed here");
        return;
    }
    let cluster = crate::support::cluster::Cluster::start_with_password("ply", "pencil");
    let refused = call(
        Arc::new(ply_host::Host::new()),
        "m.encrypted",
        &url(&cluster, "127.0.0.1", "sslmode=require"),
    )
    .expect_err("a server with TLS off");
    assert!(
        refused.starts_with("08001: ") && refused.contains("does not speak TLS"),
        "{refused}"
    );
}

/// Direct negotiation is Postgres 17's: an older server reads the handshake as a start-up it does
/// not understand and hangs up, which is refused rather than retried in plaintext.
#[test]
fn direct_negotiation_secures_the_connection_without_asking_first() {
    let Some((cluster, _dir, pem)) = secured_cluster(&["127.0.0.1"]) else {
        return;
    };
    let answered = call(
        trusting(&pem),
        "m.encrypted",
        &url(
            &cluster,
            "127.0.0.1",
            "sslmode=verify-full&sslnegotiation=direct",
        ),
    );
    if crate::support::cluster::major() >= 17 {
        assert_eq!(answered, Ok("1".to_string()));
    } else {
        let refused = answered.expect_err("a server before 17");
        assert!(refused.starts_with("08001: "), "{refused}");
    }
}

fn plain_cluster() -> Option<(crate::support::cluster::Cluster, String)> {
    if !crate::support::cluster::available() {
        eprintln!("skipping: postgres is not installed here");
        return None;
    }
    let cluster = crate::support::cluster::Cluster::start_with_password("ply", "pencil");
    let at = url(&cluster, "127.0.0.1", "sslmode=disable");
    Some((cluster, at))
}

#[test]
fn a_notification_is_heard_when_its_transaction_commits_and_never_when_it_rolls_back() {
    let Some((_cluster, at)) = plain_cluster() else {
        return;
    };
    assert_eq!(
        call(Arc::new(ply_host::Host::new()), "m.notices", &at),
        Ok("quiet jobs:one quiet undone quiet jobs:three quiet".to_string())
    );
}

/// Until `ready` answers, polling the cluster for at most `limit`.
fn until(what: &str, limit: Duration, ready: impl Fn() -> bool) {
    let deadline = Instant::now() + limit;
    while !ready() {
        assert!(Instant::now() < deadline, "waited {limit:?} for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_listener_whose_connection_is_ended_listens_again_and_says_so() {
    let Some((cluster, at)) = plain_cluster() else {
        return;
    };
    cluster.psql("ply", "create table heard (what text)");
    let listener =
        std::thread::spawn(move || call(Arc::new(ply_host::Host::new()), "m.reconnects", &at));
    let listening = "select count(*) from pg_stat_activity where query = 'listen \"jobs\"'";
    let heard = |what: &str| {
        cluster.psql(
            "ply",
            &format!("select count(*) from heard where what = '{what}'"),
        ) == "1"
    };
    // The program is compiled first, which under a loaded test run takes a while.
    let compiling = Duration::from_secs(600);
    let a_minute = Duration::from_secs(60);
    until("the listener to listen", compiling, || {
        assert!(
            !listener.is_finished(),
            "the listener ended before it listened"
        );
        cluster.psql("ply", listening) == "1"
    });
    cluster.psql("ply", "select pg_notify('jobs', 'first')");
    until("the first to be heard", a_minute, || heard("jobs:first"));
    cluster.psql(
        "ply",
        "select pg_terminate_backend(pid) from pg_stat_activity where query = 'listen \"jobs\"'",
    );
    until("the listener to reconnect", a_minute, || {
        heard("reconnected")
    });
    cluster.psql("ply", "select pg_notify('jobs', 'second')");
    let answered = listener.join().expect("the listener's thread ends");
    assert_eq!(
        answered,
        Ok("jobs:first,reconnected,jobs:second".to_string())
    );
}

#[test]
fn rows_are_copied_in_and_out_whole_or_not_at_all() {
    let Some((cluster, at)) = plain_cluster() else {
        return;
    };
    cluster.psql(
        "ply",
        "create table items (id int4 primary key, name text, data bytea, tags text[], price numeric(10, 2))",
    );
    assert_eq!(
        call(Arc::new(ply_host::Host::new()), "m.copies", &at),
        Ok("3 same 23505 3 spent 3 3 rolled back after 1 3 22012".to_string())
    );
    assert_eq!(
        cluster.psql("ply", "select string_agg(id::text || '=' || coalesce(name, 'null'), ';' order by id) from items"),
        "1=plain;2=tab\there\nnew line\\back;3=\\N"
    );
}
