//! What `std.pg`'s client does with a connection, over a scripted server.
//!
//! `SimNet` hands the client the bytes a server would have sent and records what it sent back,
//! so the framing, the handshake and both query cycles are decided without a socket.

use ply_eval::{Machine, Span, Symbol, Value};
use ply_host::tcp::{Net, SimNet};
use std::sync::Arc;

/// The client, entered once. Trust authentication, because the script decides the handshake.
const CLIENT: &str = r#"
import std.net (net)
import std.pg (connect, simple_query, extended_query, finish, default_client, Answer, ClientError, client_error_text, server_text, Rejected, NoTls)
import std.db (db, serve, server_of, transaction, is_retryable, Serializable, ReadWrite)
import std.sql (stmt, Rows, Count, Failed)

// The driver over the same script: what a connection string asks of every connection it opens.
pub fn told(url: String) -> Result<String, String>
  / {net.connect[link], net.start_tls[link], net.peer_certificate[link], net.send_secret[link], net.send[link], net.recv[link], net.close[link], abort.raise, diverges} =
  match server_of(url) {
    Err(why) -> Err(why),
    Ok(cfg) ->
      Ok(serve(cfg, 1, "test-nonce", ||
        match db.query[items](stmt("select count(*) as n from items"), []) {
          Rows(_) -> "rows",
          Count(_) -> "a count",
          Failed(e) -> e.detail,
        })),
  }

pub fn ask(host: String, port: Int) -> Result<String, String>
  / {net.connect[link], net.start_tls[link], net.peer_certificate[link], net.send_secret[link], net.send[link], net.recv[link], net.close[link], abort.raise, diverges} =
  match connect[link](host, port, NoTls, "ply", "ply", [], None, "test-nonce", default_client()) {
    Err(e) -> Err(client_error_text(e)),
    Ok(session) -> match simple_query[link](session, "select 1", default_client()) {
      Err(e) -> Err(client_error_text(e)),
      Ok(reply) -> {
        finish[link](reply.session, default_client());
        Ok(first_text(reply.answer))
      },
    },
  }

pub fn ask_with(host: String, port: Int, value: String) -> Result<String, String>
  / {net.connect[link], net.start_tls[link], net.peer_certificate[link], net.send_secret[link], net.send[link], net.recv[link], net.close[link], abort.raise, diverges} =
  match connect[link](host, port, NoTls, "ply", "ply", [], None, "test-nonce", default_client()) {
    Err(e) -> Err(client_error_text(e)),
    Ok(session) -> match extended_query[link](session, "select $1", [Some(value)], default_client()) {
      Err(e) -> Err(client_error_text(e)),
      Ok(reply) -> {
        finish[link](reply.session, default_client());
        Ok(first_text(reply.answer))
      },
    },
  }

// A transaction the server refuses to commit, run again while the refusal says to.
pub fn retried(url: String) -> Result<String, String>
  / {net.connect[link], net.start_tls[link], net.peer_certificate[link], net.send_secret[link], net.send[link], net.recv[link], net.close[link], abort.raise, diverges} =
  match server_of(url) {
    Err(why) -> Err(why),
    Ok(cfg) -> Ok(serve(cfg, 1, "test-nonce", || attempts(3, ""))),
  }

fn attempts(left: Int, seen: String) -> String / {db.execute[items], db.abort, db.begin, db.commit} =
  match transaction(Serializable, ReadWrite, ||
      db.execute[items](stmt("insert into items (id) values (1)"), [])) {
    Ok(_) -> seen ++ "committed",
    Err(rolled) -> match rolled.error {
      Some(e) -> if is_retryable(e) && left > 1 { attempts(left - 1, seen ++ e.code ++ " then ") }
      else { seen ++ e.code ++ ": " ++ e.detail },
      None -> seen ++ rolled.reason,
    },
  }

// A refusal is not the end of the connection: the second query runs on the session the first
// one came back with.
pub fn refuse_then_ask(host: String, port: Int) -> Result<String, String>
  / {net.connect[link], net.start_tls[link], net.peer_certificate[link], net.send_secret[link], net.send[link], net.recv[link], net.close[link], abort.raise, diverges} =
  match connect[link](host, port, NoTls, "ply", "ply", [], None, "test-nonce", default_client()) {
    Err(e) -> Err(client_error_text(e)),
    Ok(session) -> match simple_query[link](session, "select nope", default_client()) {
      Ok(_) -> Err("the server accepted what it should have refused"),
      Err(Rejected(after, refusal)) ->
        match simple_query[link](after, "select 1", default_client()) {
          Err(e) -> Err(client_error_text(e)),
          Ok(reply) -> {
            finish[link](reply.session, default_client());
            Ok(server_text(refusal) ++ " then " ++ first_text(reply.answer))
          },
        },
      Err(e) -> Err(client_error_text(e)),
    },
  }

// Authenticated as the server asks, by SCRAM or a clear-text password; the nonce is the one RFC
// 7677 works out, so the scripted server can be that example. The password is sealed at once.
pub fn ask_with_password(
  host: String,
  port: Int,
  user: String,
  password: String,
  nonce: String,
) -> Result<String, String> / {net.connect[link], net.start_tls[link], net.peer_certificate[link], net.send_secret[link], net.send[link], net.recv[link], net.close[link], abort.raise, diverges} =
  match connect[link](host, port, NoTls, user, "ply", [], Some(secret_bytes(secret_of_string(password))), nonce, default_client()) {
    Err(e) -> Err(client_error_text(e)),
    Ok(session) -> match simple_query[link](session, "select 1", default_client()) {
      Err(e) -> Err(client_error_text(e)),
      Ok(reply) -> {
        finish[link](reply.session, default_client());
        Ok(first_text(reply.answer))
      },
    },
  }

fn first_text(answer: Answer) -> String / {abort.raise} =
  match answer.rows {
    [] -> "no rows",
    [row, ..tail] -> match row {
      [] -> "no columns",
      [value, ..more] -> match value {
        None -> "null",
        Some(text) -> string_of_bytes(text),
      },
    },
  }
"#;

fn compiled(service: &str) -> (ply_eval::Analysis, std::sync::Arc<ply_codegen::Unit>) {
    crate::support::answered::compiled("m", service)
}
struct Ran {
    text: String,
    sent: Vec<u8>,
}

fn ran(answered: Value, net: Option<Arc<SimNet>>) -> Result<Ran, String> {
    let Value::Ctor { name, args } = &answered else {
        panic!("the entry answered {answered:?}, not an `Ok` or an `Err`");
    };
    let Value::Str(text) = &args[0] else {
        panic!("the entry carried {answered:?}, not text");
    };
    match name.as_str() {
        "Ok" => Ok(Ran {
            text: text.to_string(),
            sent: net.map(|net| net.sent(1)).unwrap_or_default(),
        }),
        "Err" => Err(text.to_string()),
        other => panic!("the entry answered `{other}`"),
    }
}

/// Enter `entry` with the server's scripted half of the conversation, and answer what the
/// client said and what the entry returned.
fn run(entry: &str, args: Vec<Value>, script: Vec<Vec<u8>>) -> Result<Ran, String> {
    let net = Arc::new(SimNet::new(vec![script]));
    let (front, unit) = compiled(CLIENT);
    let binding = ply_host::tcp::registry(Arc::clone(&net) as Arc<dyn Net>)
        .bind(&front.check)
        .expect("the declaration and the registration agree");

    let mut machine = Machine::new(&front, ply_eval::Provider::attach(unit))
        .expect("the unit was compiled from this program");
    machine.set_host_binding(Arc::new(binding));
    let declared = front
        .check
        .defs
        .get(&Symbol::new(entry))
        .expect("the entry is a definition of the program");
    machine.set_declared_footprint(declared.footprint.clone());
    let answered = machine
        .call(entry, args, Span::DUMMY)
        .into_parts()
        .0
        .unwrap_or_else(|e| panic!("the call answers: {e}"));
    ran(answered, Some(net))
}

/// The same entries, but over the real network: a cluster is the only peer that can say whether
/// the client's SCRAM is a SCRAM a server accepts.
fn run_over_tcp(entry: &str, args: Vec<Value>) -> Result<Ran, String> {
    let host = std::sync::Arc::new(ply_host::Host::new());
    let (front, unit) = compiled(CLIENT);
    let binding = host
        .registry()
        .bind(&front.check)
        .expect("the declaration and the registration agree");

    let mut machine = Machine::new(&front, ply_eval::Provider::attach(unit))
        .expect("the unit was compiled from this program");
    machine.set_host_binding(Arc::new(binding));
    // The real socket answers `Pending`, so the machine needs something to wait on.
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
    let answered = machine
        .call(entry, args, Span::DUMMY)
        .into_parts()
        .0
        .unwrap_or_else(|e| panic!("the call answers: {e}"));
    ran(answered, None)
}

/// One back-end frame: a kind byte, a length that counts itself, and a body.
fn message(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![kind];
    out.extend_from_slice(&(body.len() as i32 + 4).to_be_bytes());
    out.extend_from_slice(body);
    out
}

/// A zero-terminated string, which is how the protocol spells a name.
fn named(text: &str) -> Vec<u8> {
    let mut out = text.as_bytes().to_vec();
    out.push(0);
    out
}

/// AuthenticationOk, the server's version, its key, and readiness.
fn greeting() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(message(b'R', &0i32.to_be_bytes()));
    out.extend(message(
        b'S',
        &[named("server_version"), named("16.0")].concat(),
    ));
    out.extend(message(
        b'K',
        &[7i32.to_be_bytes(), 9i32.to_be_bytes()].concat(),
    ));
    out.extend(message(b'Z', b"I"));
    out
}

/// One text column named `?column?`, of type `int4`, whatever its value.
fn one_column() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&1i16.to_be_bytes());
    out.extend(named("?column?"));
    out.extend_from_slice(&0i32.to_be_bytes());
    out.extend_from_slice(&0i16.to_be_bytes());
    out.extend_from_slice(&23i32.to_be_bytes());
    out.extend_from_slice(&4i16.to_be_bytes());
    out.extend_from_slice(&(-1i32).to_be_bytes());
    out.extend_from_slice(&0i16.to_be_bytes());
    out
}

/// A row of one text value.
fn one_value(value: &str) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&1i16.to_be_bytes());
    out.extend_from_slice(&(value.len() as i32).to_be_bytes());
    out.extend(value.as_bytes());
    out
}

/// `select 1` answered the simple way: a description, one row, the tag, and readiness.
fn select_one() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(message(b'T', &one_column()));
    out.extend(message(b'D', &one_value("1")));
    out.extend(message(b'C', &named("SELECT 1")));
    out.extend(message(b'Z', b"I"));
    out
}

/// The same answer through the extended cycle, whose only extra is the two completions.
fn extended_one(value: &str) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(message(b'1', b""));
    out.extend(message(b'2', b""));
    out.extend(message(b'T', &one_column()));
    out.extend(message(b'D', &one_value(value)));
    out.extend(message(b'C', &named("SELECT 1")));
    out.extend(message(b'Z', b"I"));
    out
}

/// A refusal, and the readiness that says the connection is still good.
fn refused(code: &str, text: &str) -> Vec<u8> {
    let mut body = Vec::new();
    body.push(b'S');
    body.extend(named("ERROR"));
    body.push(b'C');
    body.extend(named(code));
    body.push(b'M');
    body.extend(named(text));
    body.push(0);
    let mut out = message(b'E', &body);
    out.extend(message(b'Z', b"I"));
    out
}

/// A command's tag, and the readiness that says where the transaction stands.
fn completed(tag: &str, status: u8) -> Vec<u8> {
    let mut out = message(b'C', &named(tag));
    out.extend(message(b'Z', &[status]));
    out
}

/// An insert through the extended cycle, inside a transaction.
fn inserted() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(message(b'1', b""));
    out.extend(message(b'2', b""));
    out.extend(message(b'n', b""));
    out.extend(completed("INSERT 0 1", b'T'));
    out
}

fn sent_text(sent: &[u8]) -> String {
    String::from_utf8_lossy(sent).to_string()
}

/// RFC 7677's exchange from the server's side: the mechanisms on offer, its first message, its
/// final one, then the authentication that completes and the readiness that follows it.
fn scram_greeting() -> Vec<u8> {
    let mut out = Vec::new();
    let mut offered = 10i32.to_be_bytes().to_vec();
    offered.extend(named("SCRAM-SHA-256"));
    offered.push(0);
    out.extend(message(b'R', &offered));

    let mut first = 11i32.to_be_bytes().to_vec();
    first.extend(
        "r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096"
            .as_bytes(),
    );
    out.extend(message(b'R', &first));

    let mut last = 12i32.to_be_bytes().to_vec();
    last.extend(b"v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=");
    out.extend(message(b'R', &last));

    out.extend(message(b'R', &0i32.to_be_bytes()));
    out.extend(message(b'Z', b"I"));
    out
}

#[test]
fn the_client_shakes_hands_and_runs_a_query_over_a_scripted_server() {
    let outcome = run(
        "m.ask",
        vec![Value::str("127.0.0.1"), Value::Int(5432)],
        vec![greeting(), select_one()],
    )
    .expect("the server refused nothing");
    assert_eq!(outcome.text, "1");

    // What the client sent: a start-up frame whose length counts itself and whose next word is
    // protocol 3.0, then the query, then the terminate that ends the connection.
    let sent = &outcome.sent;
    assert!(sent.len() > 8, "the client sent {} bytes", sent.len());
    let declared = u32::from_be_bytes([sent[0], sent[1], sent[2], sent[3]]) as usize;
    assert_eq!(
        u32::from_be_bytes([sent[4], sent[5], sent[6], sent[7]]),
        196608,
        "the start-up frame names the protocol version"
    );
    assert!(
        declared < sent.len() && sent[declared] == b'Q',
        "the start-up frame's length does not reach the query that follows it"
    );
    let text = sent_text(sent);
    assert!(
        text.contains("user\x00ply\x00database\x00ply\x00"),
        "{text}"
    );
    assert!(text.contains("select 1"), "{text}");
    assert!(
        sent.ends_with(&[b'X', 0, 0, 0, 4]),
        "the connection was not terminated: {text}"
    );
}

#[test]
fn the_extended_cycle_carries_its_parameters_as_text() {
    let outcome = run(
        "m.ask_with",
        vec![
            Value::str("127.0.0.1"),
            Value::Int(5432),
            Value::str("hello"),
        ],
        vec![greeting(), extended_one("hello")],
    )
    .expect("the server refused nothing");
    assert_eq!(outcome.text, "hello");

    let text = sent_text(&outcome.sent);
    // Parse names the statement, Bind carries the value as text, and Sync ends the cycle.
    assert!(text.contains("select $1"), "{text}");
    assert!(text.contains("hello"), "{text}");
    assert_eq!(
        sent_text(&outcome.sent).matches("\x00P").count(),
        1,
        "the cycle sends one Parse: {text}"
    );
}

#[test]
fn a_refusal_comes_back_with_its_sqlstate_and_leaves_the_connection_usable() {
    let outcome = run(
        "m.refuse_then_ask",
        vec![Value::str("127.0.0.1"), Value::Int(5432)],
        vec![
            greeting(),
            refused("42P01", "relation \"nope\" does not exist"),
            select_one(),
        ],
    )
    .expect("the second query succeeded");
    assert!(
        outcome.text.contains("42P01") && outcome.text.ends_with("then 1"),
        "{}",
        outcome.text
    );
    let text = sent_text(&outcome.sent);
    assert!(text.contains("select nope"), "{text}");
    assert!(text.contains("select 1"), "{text}");
}

/// A serialization failure the server answers a `COMMIT` with comes back from `std.db` as its
/// SQLSTATE, so `is_retryable` sees it, and the retry begins a transaction of its own on the
/// connection the refusal left talking.
#[test]
fn a_serialization_failure_comes_back_as_40001_and_the_transaction_is_retried() {
    let outcome = run(
        "m.retried",
        vec![Value::str("postgres://ply@127.0.0.1:5432/ply")],
        vec![
            greeting(),
            completed("BEGIN", b'T'),
            inserted(),
            refused(
                "40001",
                "could not serialize access due to read/write dependencies among transactions",
            ),
            completed("BEGIN", b'T'),
            inserted(),
            completed("COMMIT", b'I'),
        ],
    )
    .expect("the server refused nothing the program did not handle");
    assert_eq!(outcome.text, "40001 then committed");

    let text = sent_text(&outcome.sent);
    assert_eq!(
        text.matches("begin isolation level serializable read write")
            .count(),
        2,
        "the retry did not begin a transaction of its own: {text:?}"
    );
    assert!(
        !text.contains("savepoint"),
        "the retry was taken for a transaction nested in the refused one: {text:?}"
    );
}

/// `std.db` tells the server a connection string's timeouts and name in the start-up message, so a
/// statement is bounded by the server without a `SET` the driver's reader refuses.
#[test]
fn a_connection_strings_settings_are_in_the_start_up_message() {
    let outcome = run(
        "m.told",
        vec![Value::str(
            "postgres://ply@127.0.0.1:5432/ply?statement_timeout=250\
             &idle_in_transaction_session_timeout=1000&application_name=desk",
        )],
        vec![greeting(), extended_one("3")],
    )
    .expect("the server refused nothing");
    assert_eq!(outcome.text, "rows");

    let sent = &outcome.sent;
    let declared = u32::from_be_bytes([sent[0], sent[1], sent[2], sent[3]]) as usize;
    let startup = sent_text(&sent[..declared]);
    assert!(
        startup.ends_with(
            "user\x00ply\x00database\x00ply\x00application_name\x00desk\x00\
             statement_timeout\x00250\x00idle_in_transaction_session_timeout\x001000\x00\x00"
        ),
        "{startup:?}"
    );
    assert!(
        !sent_text(&sent[declared..]).contains("statement_timeout"),
        "a setting went out as a statement rather than at start-up: {:?}",
        sent_text(sent)
    );
}

#[test]
fn the_client_answers_scram_and_checks_the_servers_proof() {
    let outcome = run(
        "m.ask_with_password",
        vec![
            Value::str("127.0.0.1"),
            Value::Int(5432),
            Value::str("user"),
            Value::str("pencil"),
            Value::str("rOprNGfwEbeRWgbNEkqO"),
        ],
        vec![scram_greeting(), select_one()],
    )
    .expect("SCRAM completed and the server's proof verified");
    assert_eq!(outcome.text, "1");

    let text = sent_text(&outcome.sent);
    assert!(text.contains("SCRAM-SHA-256"), "{text}");
    assert!(text.contains("n=user,r=rOprNGfwEbeRWgbNEkqO"), "{text}");
    assert!(
        text.contains("p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ="),
        "the proof the client sent is not the one RFC 7677 works out: {text}"
    );
}

/// A real cluster, whose TCP connections are configured to demand a password, so the handshake
/// is SCRAM against postgres rather than against a script. Skipped where postgres is not installed,
/// except under CI, where that fails.
#[test]
fn the_client_speaks_scram_to_a_real_server() {
    if !crate::support::cluster::available() {
        eprintln!("skipping: postgres is not installed here");
        return;
    }
    let cluster = crate::support::cluster::Cluster::start_with_password("ply", "pencil");
    let ran = run_over_tcp(
        "m.ask_with_password",
        vec![
            Value::str("127.0.0.1"),
            Value::Int(i64::from(cluster.port())),
            Value::str("ply"),
            Value::str("pencil"),
            Value::str("a-cluster-test-nonce"),
        ],
    );
    match ran {
        Ok(outcome) => assert_eq!(outcome.text, "1"),
        Err(why) => panic!("the client could not talk to a real server: {why}"),
    }
}

/// A real cluster that asks for the password in the clear, which the client sends sealed, by
/// `net.send_secret`; a wrong one is the server's refusal, not a connection.
#[test]
fn the_client_sends_a_clear_text_password_to_a_real_server() {
    if !crate::support::cluster::available() {
        eprintln!("skipping: postgres is not installed here");
        return;
    }
    let cluster = crate::support::cluster::Cluster::start_with_clear_text_password("ply", "pencil");
    let ask = |password: &str| {
        run_over_tcp(
            "m.ask_with_password",
            vec![
                Value::str("127.0.0.1"),
                Value::Int(i64::from(cluster.port())),
                Value::str("ply"),
                Value::str(password),
                Value::str("a-cluster-test-nonce"),
            ],
        )
    };
    match ask("pencil") {
        Ok(outcome) => assert_eq!(outcome.text, "1"),
        Err(why) => panic!("the client could not authenticate in the clear: {why}"),
    }
    match ask("crayon") {
        Ok(outcome) => panic!("a wrong password connected: {}", outcome.text),
        Err(why) => assert!(
            why.contains("28P01"),
            "the refusal is not the server's: {why}"
        ),
    }
}
