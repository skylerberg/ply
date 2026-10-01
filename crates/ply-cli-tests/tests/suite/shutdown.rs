#![cfg(unix)]

use crate::harness::{Reservation, connect_when_ready, process, repo, write};
use serde_json::Value;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The status line and headers both programs answer with, as Ply source. The `HEAD` token in the
/// sources below is replaced with it, so an edit cannot leave the two answers disagreeing.
const HEAD: &str =
    r#"b"HTTP/1.1 200 OK\r\nConnection: close\r\nX-Test-Nonce: NONCE\r\nContent-Length: ""#;

const SERVER: &str = r#"
import std.net
import std.net (net)
import std.signal (signal)

// What a readiness probe reads. Its row says exactly what it consults, and a
// route whose row is empty is a route that checks nothing.
pub fn body() -> Bytes / {signal.read} =
  if signal.stopping() { b"draining" } else { b"ok" }

fn answer(c: Int) -> Unit / {net.write[conn], signal.read} = {
  let _ = net.recv[conn](c, 4096, 20000);
  let payload = body();
  let head = bytes_concat(
    HEAD,
    bytes_concat(bytes_of_string(int_to_string(bytes_len(payload))), b"\r\n\r\n"));
  let _ = net::send_all[conn](c, bytes_concat(head, payload), 20000);
  net.close[conn](c)
}

fn serve(l: Int, served: Int) -> Int / {net.write[listener], net.write[conn], signal.read} = {
  let c = net.accept[listener](l);
  if c == 0 {
    served
  } else {
    answer(c);
    serve(l, served + 1)
  }
}

fn main() -> Int / {net.write[listener], net.write[conn], signal.read} = {
  let l = net.listen[listener](PORT);
  let served = serve(l, 0);
  net.close[listener](l);
  served
}
"#;

const PORT_ATTEMPTS: usize = 3;

/// A token this test's server echoes in its answer, so a probe cannot read another test's server
/// as this one. The shutdown suite is one binary and its tests start together, and the kernel can
/// hand two of them the same ephemeral port; the one that loses the bind dies with `E0502`, and
/// without the token the winner's answer would satisfy the loser's probe.
fn nonce() -> String {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    format!(
        "{:x}-{:x}",
        std::process::id(),
        COUNT.fetch_add(1, Ordering::Relaxed)
    )
}

/// Whether the answer on the reserved address is *this* test's server: a `200 OK` on the port is
/// not proof, because the port is the kernel's to hand out twice.
fn ready(answer: &str, nonce: &str) -> bool {
    answer.contains(nonce)
}

/// Killed on drop, including during a panic, so a failing test does not leak a server.
struct Server {
    child: Child,
    reserved: Reservation,
    _dir: tempfile::TempDir,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Server {
    fn start(flags: &[&str]) -> Server {
        Server::start_with(SERVER, flags)
    }

    /// Retries on a fresh port if this one was taken before `ply run` could claim it.
    fn start_with(source: &str, flags: &[&str]) -> Server {
        let mut refused = Vec::new();
        for _ in 0..PORT_ATTEMPTS {
            let reserved = Reservation::take();
            let nonce = nonce();
            let dir = tempfile::tempdir().expect("a temp dir");
            write(
                dir.path(),
                "main.ply",
                &source
                    .replace("HEAD", HEAD)
                    .replace("PORT", &reserved.port().to_string())
                    .replace("NONCE", &nonce),
            );
            let mut server = Server::launch(dir, flags, reserved);
            match server.wait_until_listening(&nonce) {
                Ok(()) => return server,
                Err(why) => refused.push(why),
            }
        }
        panic!(
            "`ply run --host` never answered a probe, on {PORT_ATTEMPTS} ports:\n\n{}",
            refused.join("\n\n")
        );
    }

    /// `tests/fixtures/<name>.ply` as committed, told its port by the setting `port_key`. It cannot
    /// echo a token, so the reservation keeps the port this test's alone, and the connection that
    /// found it listening is handed back as the test's own.
    fn fixture(name: &str, port_key: &str, flags: &[&str]) -> (Server, TcpStream) {
        let file = format!("{name}.ply");
        let mut refused = Vec::new();
        for _ in 0..PORT_ATTEMPTS {
            let reserved = Reservation::take();
            let setting = format!("{port_key}={}", reserved.port());
            let dir = tempfile::tempdir().expect("a temp dir");
            std::fs::copy(
                repo().join("tests/fixtures").join(&file),
                dir.path().join(&file),
            )
            .expect("the fixture is copied");
            let mut server = Server::launch(dir, &[flags, &["--set", &setting]].concat(), reserved);
            let Server {
                child, reserved, ..
            } = &mut server;
            match connect_when_ready(reserved, child, Duration::from_secs(60), |_| true) {
                Ok(held) => return (server, held),
                Err(why) => refused.push(why),
            }
        }
        panic!(
            "`ply run --host` never listened, on {PORT_ATTEMPTS} ports:\n\n{}",
            refused.join("\n\n")
        );
    }

    fn launch(dir: tempfile::TempDir, flags: &[&str], reserved: Reservation) -> Server {
        let child = process(dir.path())
            .arg("run")
            .arg("--host")
            .args(flags)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("`ply run` starts");
        Server {
            child,
            reserved,
            _dir: dir,
        }
    }

    /// The probe is a whole request and response, and the answer has to carry this test's token:
    /// a bare connect, or a `200 OK`, can belong to another test's server on the same port.
    fn wait_until_listening(&mut self, nonce: &str) -> Result<(), String> {
        connect_when_ready(
            &mut self.reserved,
            &mut self.child,
            Duration::from_secs(60),
            |stream| ready(&request(stream), nonce),
        )
        .map(|_| ())
    }

    fn address(&self) -> std::net::SocketAddr {
        format!("127.0.0.1:{}", self.reserved.port())
            .parse()
            .expect("an address")
    }

    fn connect(&self) -> TcpStream {
        let stream = TcpStream::connect_timeout(&self.address(), Duration::from_secs(5))
            .expect("the server is listening");
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .expect("a read deadline");
        stream
    }

    fn signal(&self, name: &str) {
        let status = std::process::Command::new("kill")
            .arg(format!("-{name}"))
            .arg(self.child.id().to_string())
            .status()
            .expect("`kill` runs");
        assert!(status.success(), "`kill -{name}` failed");
    }

    /// The exit code and everything the run wrote, where `W0608` and the shutdown banner land.
    fn finish(self) -> (i32, String) {
        let (code, out, err) = self.wait();
        (code, format!("{out}{err}"))
    }

    /// The exit code, and what the run wrote to stdout and to stderr apart.
    fn wait(mut self) -> (i32, String, String) {
        let until = Instant::now() + Duration::from_secs(60);
        loop {
            match self.child.try_wait().expect("the child is ours") {
                Some(_) => break,
                None => {
                    assert!(
                        Instant::now() < until,
                        "the run never exited, so the drain hung rather than being bounded"
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
        let status = self.child.wait().expect("the child is ours");
        let mut out = String::new();
        if let Some(stdout) = self.child.stdout.as_mut() {
            let _ = stdout.read_to_string(&mut out);
        }
        let mut err = String::new();
        if let Some(stderr) = self.child.stderr.as_mut() {
            let _ = stderr.read_to_string(&mut err);
        }
        // `None` means a signal killed the process, and a killed run did not drain.
        (status.code().unwrap_or(-1), out, err)
    }
}

fn request(stream: &mut TcpStream) -> String {
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
        .expect("the request is written");
    let mut answer = Vec::new();
    let _ = stream.read_to_end(&mut answer);
    String::from_utf8_lossy(&answer).to_string()
}

/// Two tests in this binary can be handed the same port, and the one that loses the bind answers
/// the winner's probe with what looks like readiness. The token in the answer is what tells them
/// apart, so this holds `ready` to it rather than to the status line.
#[test]
fn a_200_ok_from_another_server_is_not_readiness() {
    let decoy = TcpListener::bind("127.0.0.1:0").expect("a decoy port");
    let address = decoy.local_addr().expect("a decoy address");
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = decoy.accept() {
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nok");
        }
    });

    let mut probe =
        TcpStream::connect_timeout(&address, Duration::from_secs(5)).expect("the decoy listens");
    let _ = probe.set_read_timeout(Some(Duration::from_secs(5)));
    let answer = request(&mut probe);
    assert!(
        answer.contains("200 OK"),
        "the decoy did not answer the probe, so this test proves nothing: {answer:?}"
    );
    assert!(
        !ready(&answer, "the-token-this-test-asked-for"),
        "a server that is not this test's read as ready:\n\n{answer}"
    );
}

#[test]
fn a_request_in_flight_at_the_signal_gets_its_response_and_the_run_exits_zero() {
    let server = Server::start(&["--drain-ms", "30000"]);
    let mut held = server.connect();

    // The server is inside `net.recv` with a 20s deadline, so the signal lands with a request in flight.
    std::thread::sleep(Duration::from_millis(200));
    server.signal("TERM");
    std::thread::sleep(Duration::from_millis(200));

    let answer = request(&mut held);
    assert!(
        answer.contains("200 OK"),
        "the in-flight request was dropped by the drain: {answer:?}"
    );
    assert!(
        answer.ends_with("draining"),
        "the route read the stop flag and answered `{answer}`, so `signal.stopping()` did not \
         reach the handler"
    );

    let (code, output) = server.finish();
    assert_eq!(code, 0, "a clean drain exits 0\n\n{output}");
    assert!(
        !output.contains("W0608"),
        "a drain that finished reported the deadline expiring\n\n{output}"
    );
    assert!(
        output.contains("stopping"),
        "a stopping service prints what it is doing\n\n{output}"
    );
    assert!(
        output.contains("1 listener(s) closed"),
        "the banner reports the listener the run actually closed\n\n{output}"
    );
    assert!(
        output.contains("1 connection(s) in flight"),
        "the banner reports the connection that was actually open\n\n{output}"
    );
}

#[test]
fn a_connection_opened_after_the_stop_gets_no_response() {
    let server = Server::start(&["--drain-ms", "30000"]);
    let mut first = server.connect();
    assert!(request(&mut first).contains("200 OK"));

    server.signal("TERM");
    // Phase 2 dials the parked `accept` awake and it answers `0`, so this connection is refused or accepted and closed.
    std::thread::sleep(Duration::from_millis(400));
    if let Ok(mut late) = TcpStream::connect_timeout(&server.address(), Duration::from_millis(500))
    {
        let _ = late.set_read_timeout(Some(Duration::from_secs(3)));
        let _ = late.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n");
        let mut answer = Vec::new();
        let _ = late.read_to_end(&mut answer);
        assert!(
            answer.is_empty(),
            "a connection opened after the run stopped accepting was answered {:?}",
            String::from_utf8_lossy(&answer)
        );
    }

    let (code, output) = server.finish();
    assert_eq!(code, 0, "{output}");
}

/// There is no cancellation, so the task is not unwound and is not handed a `503`.
#[test]
fn a_drain_that_expires_reports_w0608_and_exits_three() {
    let server = Server::start(&["--drain-ms", "300"]);
    let mut held = server.connect();
    // Held open and silent inside a 20s `net.recv`, so this request cannot finish inside a 300ms drain.
    std::thread::sleep(Duration::from_millis(200));
    server.signal("TERM");

    let mut answer = Vec::new();
    let _ = held.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = held.read_to_end(&mut answer);
    assert!(
        answer.is_empty(),
        "W5 has no cancellation, so a request live at the deadline gets nothing rather than a \
         partial response; this client read {:?}",
        String::from_utf8_lossy(&answer)
    );

    let (code, output) = server.finish();
    assert_eq!(
        code, 3,
        "a drain that dropped requests must not report success\n\n{output}"
    );
    assert!(
        output.contains("W0608"),
        "the run exited 3 and never said why\n\n{output}"
    );
    assert!(
        output.contains("drain-ms"),
        "`W0608` has to say what to do about it\n\n{output}"
    );
}

/// `tests/fixtures/drain_incomplete.ply`: a head announcing a body that never comes keeps its
/// request reading for up to the program's `body_timeout_ms`, and a 300ms drain cannot wait that out.
#[test]
fn the_drain_fixture_abandons_a_request_still_reading_its_body() {
    let (server, mut held) = Server::fixture(
        "drain_incomplete",
        "DRAIN_PORT",
        &["--json", "--drain-ms", "300"],
    );
    held.write_all(b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\n\r\n")
        .expect("the head is written");
    std::thread::sleep(Duration::from_millis(200));
    server.signal("TERM");

    let mut answer = Vec::new();
    let _ = held.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = held.read_to_end(&mut answer);
    assert!(
        answer.is_empty(),
        "a request live at the deadline is closed with no response; this client read {:?}",
        String::from_utf8_lossy(&answer)
    );

    let (code, out, err) = server.wait();
    let report: Value = serde_json::from_str(&out)
        .unwrap_or_else(|e| panic!("`--json` writes one document on stdout: {e}\n{out}{err}"));
    assert_eq!(code, 3, "{report}");
    assert_eq!(report["exit_code"], 3, "{report}");
    assert_eq!(report["shutdown"]["signal"], "TERM", "{report}");
    assert_eq!(report["shutdown"]["drain_ms"], 300, "{report}");
    let diagnostics = report["diagnostics"]
        .as_array()
        .unwrap_or_else(|| panic!("a diagnostics array: {report}"));
    assert!(
        diagnostics.iter().all(|d| d["severity"] != "error"),
        "the expired drain is the run's only failure: {report}"
    );
    let drained = diagnostics
        .iter()
        .find(|d| d["code"] == "W0608")
        .unwrap_or_else(|| panic!("the run exited 3 and never said why: {report}"));
    assert_eq!(drained["severity"], "warning", "{drained}");
    assert_eq!(
        drained["message"], "the drain deadline expired",
        "{drained}"
    );
    let notes: Vec<&str> = drained["notes"]
        .as_array()
        .map(|notes| notes.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    for note in [
        "1 connection(s) abandoned with no response written",
        "raise `--drain-ms` above the program's own body_timeout_ms + write_timeout_ms",
    ] {
        assert!(notes.contains(&note), "`W0608` lacks `{note}`: {drained}");
    }
}

#[test]
fn a_second_signal_exits_immediately_and_says_what_it_abandoned() {
    let server = Server::start(&["--drain-ms", "60000"]);
    let held = server.connect();
    let _ = held.set_read_timeout(Some(Duration::from_secs(2)));

    std::thread::sleep(Duration::from_millis(200));
    server.signal("TERM");
    std::thread::sleep(Duration::from_millis(300));
    server.signal("TERM");

    let (code, output) = server.finish();
    assert_eq!(
        code, 143,
        "a second SIGTERM exits 128+15, which is the number a supervisor would have read from a \
         process that never caught it\n\n{output}"
    );
    assert!(
        output.contains("abandoned"),
        "a second signal prints what it cost before it goes\n\n{output}"
    );
    let _ = held.shutdown(Shutdown::Both);
}

#[test]
fn during_the_lead_the_run_still_accepts_and_already_says_it_is_stopping() {
    let server = Server::start(&["--drain-ms", "30000", "--drain-lead-ms", "2000"]);
    let mut warm = server.connect();
    assert!(request(&mut warm).ends_with("ok"), "not stopping yet");

    server.signal("TERM");
    std::thread::sleep(Duration::from_millis(300));

    // Inside the lead a new connection is still accepted, and the route reads the flag and sheds.
    let mut during = server.connect();
    let answer = request(&mut during);
    assert!(
        answer.contains("200 OK") && answer.ends_with("draining"),
        "the lead phase stopped accepting, or the flag had not reached the program: {answer:?}"
    );

    let (code, output) = server.finish();
    assert_eq!(code, 0, "{output}");
    assert!(
        output.contains("lead 2000ms"),
        "the run has to say what it will do on a signal before it does it, so the number can be \
         compared by eye against the program's own body_timeout_ms + write_timeout_ms\n\n{output}"
    );
    assert!(output.contains("signals INT TERM"), "{output}");
}

/// A stop requested once would end every test after it: a coupling the footprint graph cannot see.
#[test]
fn signal_is_withheld_under_ply_test_and_names_the_twin() {
    let dir = tempfile::tempdir().expect("a temp dir");
    write(
        dir.path(),
        "main.ply",
        r#"
import std.signal (signal)

pub fn shedding() -> Bool / {signal.read} = signal.stopping()

test/nondet "a stop reaches the program" {
  assert(!shedding())
}
"#,
    );
    // `--json` because the human projection renders the message rather than the code.
    for flags in [vec!["test", "--json"], vec!["test", "--host", "--json"]] {
        let out = process(dir.path())
            .args(&flags)
            .output()
            .expect("`ply test` ran");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            text.contains("E0424"),
            "`ply {}` did not refuse `signal.stopping` at the boundary\n\n{text}",
            flags.join(" ")
        );
        assert!(
            text.contains("std.signal"),
            "the refusal has to name the twin a test handles it over\n\n{text}"
        );
        assert!(
            text.contains("ply_host::signal::stopping"),
            "the refusal has to name the handler that would have served it, which is the whole \
             difference between `E0424` and `E0303`\n\n{text}"
        );
    }
}

/// A task per connection: the shape the drain's unfinished-request case is about.
const CONCURRENT: &str = r#"
import std.net
import std.net (net)
import std.signal (signal)

fn answer(c: Int) -> Int / {net.write[conn], signal.read} = {
  let _ = net.recv[conn](c, 4096, 20000);
  let payload = if signal.stopping() { b"draining" } else { b"ok" };
  let head = bytes_concat(
    HEAD,
    bytes_concat(bytes_of_string(int_to_string(bytes_len(payload))), b"\r\n\r\n"));
  let _ = net::send_all[conn](c, bytes_concat(head, payload), 20000);
  net.close[conn](c);
  1
}

fn joined(ts: List<Task<Int>>) -> Int / {task.write} =
  match ts { [] -> 0, [t, ..rest] -> task.join(t) + joined(rest) }

fn serve(l: Int, running: List<Task<Int>>) -> Int
  / {net.write[listener], net.write[conn], signal.read, task.write} = {
  let c = net.accept[listener](l);
  if c == 0 {
    joined(running)
  } else {
    serve(l, push(running, task.spawn(|| answer(c))))
  }
}

fn main() -> Int / {net.write[listener], net.write[conn], signal.read, task.write} = {
  let l = net.listen[listener](PORT);
  let served = serve(l, []);
  net.close[listener](l);
  served
}
"#;

#[test]
fn a_spawned_task_still_serving_at_the_signal_finishes_and_the_run_exits_zero() {
    let server = Server::start_with(CONCURRENT, &["--drain-ms", "30000"]);
    let mut held = server.connect();
    std::thread::sleep(Duration::from_millis(200));

    server.signal("TERM");
    std::thread::sleep(Duration::from_millis(300));

    let answer = request(&mut held);
    assert!(
        answer.contains("200 OK"),
        "a task in flight at the signal was dropped: {answer:?}"
    );

    let (code, output) = server.finish();
    assert_eq!(code, 0, "a clean drain exits 0\n\n{output}");
    assert!(!output.contains("W0608"), "{output}");
}

#[test]
fn a_task_blocked_on_a_host_handler_does_not_outlast_the_drain() {
    let server = Server::start_with(CONCURRENT, &["--drain-ms", "400"]);
    let mut held = server.connect();
    // Connected and silent: the spawned task is inside `net.recv` and will be for twenty seconds.
    std::thread::sleep(Duration::from_millis(200));

    let signalled = Instant::now();
    server.signal("TERM");

    let mut answer = Vec::new();
    let _ = held.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = held.read_to_end(&mut answer);
    assert!(
        answer.is_empty(),
        "W5 has no cancellation, so the blocked task is not handed a 503: {:?}",
        String::from_utf8_lossy(&answer)
    );

    let (code, output) = server.finish();
    let waited = signalled.elapsed();
    assert_eq!(
        code, 3,
        "a drain that abandoned a request must not report success\n\n{output}"
    );
    assert!(
        output.contains("W0608"),
        "the run exited 3 and never said why\n\n{output}"
    );
    // `--drain-ms 400` plus the teardown's floor, plus slack.
    assert!(
        waited < Duration::from_secs(5),
        "the drain waited {waited:?} — so it was hostage to the host operation rather than \
         bounded by `--drain-ms` plus the teardown's floor\n\n{output}"
    );
}
