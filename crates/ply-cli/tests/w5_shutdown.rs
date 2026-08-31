//! What a client sees when a service is asked to stop.
//!
//! Every case here drives the real `ply` binary over a real loopback socket and
//! sends it a real signal, because the questions a drain has to answer are not
//! answerable from inside the process: **did the in-flight request get its
//! response, did the exit code say whether anything was lost, and what did the
//! client read?** A test that asserted the coordinator's own flags would be the
//! driver's bookkeeping standing in as evidence about the driver.
//!
//! The server is one program and it is not rewritten between cases. ADR 0015
//! §4.3 claims a sequential accept loop drains with **no source change**, and the
//! way to check that claim is to change nothing: the loop below ends because
//! `net.accept` answered `0`, which is ADR 0013's existing rule doing W5's job.
//!
//! `SIGTERM` is sent with `kill`, so nothing here needs a `libc` dependency for
//! a `#[cfg(unix)]` path the rest of the suite does not have.

#![cfg(unix)]

use assert_cmd::cargo::CommandCargoExt;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// A sequential accept loop that reads one request, answers it, and closes.
///
/// `signal.stopping()` is in `body`'s row, which is the whole point of the
/// effect: `ply check --types` answers "which routes shed load when this
/// instance is draining" out of the type rather than out of a comment.
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
    b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: ",
    bytes_concat(bytes_of_string(int_to_string(bytes_len(payload))), b"\r\n\r\n"));
  let _ = net::send_all(c, bytes_concat(head, payload), 20000);
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

/// How many ports [`Server::start_with`] will try before it gives up. See the
/// note there for why one was not enough.
const PORT_ATTEMPTS: usize = 3;

/// A port nothing is listening on right now.
///
/// Racy by construction — it is released before the server takes it — and that
/// is why nothing here uses a fixed one: colliding with a developer's own
/// service would be the confusing failure. [`Server::start_with`] retries, so
/// the race is paid for in a respawn rather than in a red run.
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free port");
    let port = listener.local_addr().expect("an address").port();
    drop(listener);
    port
}

/// A running `ply run --host`, killed when this is dropped — including while a
/// panic unwinds, which is the case that would otherwise leak a server per
/// failing test.
struct Server {
    child: Child,
    port: u16,
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

    /// Starts the server, retrying on a fresh port if the one `free_port`
    /// handed over was taken before `ply run` could claim it.
    ///
    /// **Losing that race is expected; losing it silently for a minute was
    /// not.** `free_port` is racy by construction — its own comment says so —
    /// and `net.listen` on a taken port exits immediately, which
    /// `wait_until_listening` could not tell from a server that had not
    /// finished starting. So the failure cost the full 60-second budget and
    /// then reported the port and nothing else. Observed on `main` at
    /// 54a568d: one workspace run lost the race and spent **60.02s** failing,
    /// and the same binary passed in **2.07s** on the next run. Three ports
    /// rather than one because the retry is the fix; the budget was never the
    /// problem.
    fn start_with(source: &str, flags: &[&str]) -> Server {
        let mut refused = Vec::new();
        for _ in 0..PORT_ATTEMPTS {
            let mut server = Server::spawn(source, flags);
            match server.wait_until_listening() {
                Ok(()) => return server,
                Err(why) => refused.push(why),
            }
        }
        panic!(
            "`ply run --host` never answered a probe, on {PORT_ATTEMPTS} ports:\n\n{}",
            refused.join("\n\n")
        );
    }

    fn spawn(source: &str, flags: &[&str]) -> Server {
        let dir = tempfile::tempdir().expect("a temp dir");
        let port = free_port();
        write(dir.path(), &source.replace("PORT", &port.to_string()));
        let child = Command::cargo_bin("ply")
            .expect("the binary is built")
            .arg("--color")
            .arg("never")
            .arg("run")
            .arg("--host")
            .args(flags)
            .current_dir(dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("`ply run` starts");
        Server {
            child,
            port,
            _dir: dir,
        }
    }

    /// The probe is a whole request and response, not a bare connect. The
    /// server is a *sequential* accept loop, so a connection opened and
    /// abandoned would hold it inside `net.recv` for that operation's own
    /// deadline and every case below would be waiting on a probe.
    ///
    /// A child that has *exited* is reported at once rather than waited out.
    /// That is the difference between the two failures this can have — the
    /// port was taken, or `ply run --host` is broken — and the message that
    /// named only the port could not tell them apart.
    fn wait_until_listening(&mut self) -> Result<(), String> {
        let until = Instant::now() + Duration::from_secs(60);
        while Instant::now() < until {
            if let Some(status) = self.child.try_wait().expect("the child's status") {
                return Err(self.epitaph(status));
            }
            if let Ok(mut probe) =
                TcpStream::connect_timeout(&self.address(), Duration::from_millis(200))
            {
                let _ = probe.set_read_timeout(Some(Duration::from_secs(10)));
                if request(&mut probe).contains("200 OK") {
                    return Ok(());
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err(format!(
            "127.0.0.1:{}: the process was still running after 60s and never answered a probe",
            self.port
        ))
    }

    /// What the child said before it exited. The pipes are drained rather than
    /// summarised: a failure here is rare enough that the whole of what the
    /// binary printed is what a reader wants, and both pipes are at EOF
    /// already because the process is gone.
    fn epitaph(&mut self, status: ExitStatus) -> String {
        let mut out = String::new();
        let mut err = String::new();
        if let Some(mut pipe) = self.child.stdout.take() {
            let _ = pipe.read_to_string(&mut out);
        }
        if let Some(mut pipe) = self.child.stderr.take() {
            let _ = pipe.read_to_string(&mut err);
        }
        format!(
            "127.0.0.1:{}: `ply run --host` exited {status} without binding\n  stdout: \
             {}\n  stderr: {}",
            self.port,
            out.trim(),
            err.trim()
        )
    }

    fn address(&self) -> std::net::SocketAddr {
        format!("127.0.0.1:{}", self.port)
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
        let status = Command::new("kill")
            .arg(format!("-{name}"))
            .arg(self.child.id().to_string())
            .status()
            .expect("`kill` runs");
        assert!(status.success(), "`kill -{name}` failed");
    }

    /// The exit code and everything the run wrote, which is where `W0608` and
    /// the shutdown banner land.
    fn finish(mut self) -> (i32, String) {
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
        // `None` is a signal that killed the process rather than an exit code,
        // and a run that was killed is a run that did not drain.
        (status.code().unwrap_or(-1), format!("{out}{err}"))
    }
}

fn write(dir: &Path, source: &str) {
    std::fs::write(dir.join("main.ply"), source).unwrap();
}

/// One request and one response over an already-open connection.
fn request(stream: &mut TcpStream) -> String {
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
        .expect("the request is written");
    let mut answer = Vec::new();
    let _ = stream.read_to_end(&mut answer);
    String::from_utf8_lossy(&answer).to_string()
}

// ---------------------------------------------------------------------------

/// The exit criterion of ADR 0015 §4.3, end to end and with the source
/// unchanged: a request in flight when the signal arrives gets its response, the
/// accept loop then ends because `accept` answered `0`, and the run exits `0`.
///
/// The request is *held open* across the signal on purpose. A test that sent the
/// signal between requests would pass over a run that dropped everything in
/// flight, which is the failure this whole milestone exists to prevent.
#[test]
fn a_request_in_flight_at_the_signal_gets_its_response_and_the_run_exits_zero() {
    let server = Server::start(&["--drain-ms", "30000"]);
    let mut held = server.connect();

    // The server is inside `net.recv` on this connection with a 20s deadline,
    // so the signal lands with a request genuinely in flight.
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
    // ADR 0015 §6: every number an operator reads is a fact the run already
    // holds. These are the coordinator's own, and they used to be read before it
    // wrote them — `stop_accepting` is the instant `net.accept` answers `0`, so
    // this machine could be through the drain, the teardown and the banner while
    // the coordinator was still dialling its parked accepts, and an idle desk
    // printed `0 listener(s) closed · 0 connection(s) in flight` for a run that
    // had one of each. `0 transaction(s) open` is exactly the number somebody
    // deciding whether a restart was safe would trust.
    assert!(
        output.contains("1 listener(s) closed"),
        "the banner reports the listener the run actually closed\n\n{output}"
    );
    assert!(
        output.contains("1 connection(s) in flight"),
        "the banner reports the connection that was actually open\n\n{output}"
    );
}

/// A connection the run stopped accepting on is refused rather than half-served.
/// The lead phase is what a deployment uses to take an instance out of rotation,
/// and a connection that arrives after it is one a retry fixes.
#[test]
fn a_connection_opened_after_the_stop_gets_no_response() {
    let server = Server::start(&["--drain-ms", "30000"]);
    let mut first = server.connect();
    assert!(request(&mut first).contains("200 OK"));

    server.signal("TERM");
    // The accept loop is parked in `accept`; phase 2 dials it awake and it
    // answers `0`, so this connection is either refused outright or accepted and
    // closed. What it must never be is answered.
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

/// The drain deadline, and the honest answer W5 has for it: there is no
/// cancellation, so the task is not unwound and is not handed a `503`. The run
/// stops scheduling, tears down, reports `W0608` and exits `3` — and the client
/// sees a connection closed with no response.
///
/// The distinction between `3` and `0` is the whole product of §4: a rolling
/// restart that reported success while losing requests per instance is the
/// failure this makes visible.
#[test]
fn a_drain_that_expires_reports_w0608_and_exits_three() {
    let server = Server::start(&["--drain-ms", "300"]);
    let mut held = server.connect();
    // Held open and silent: the server is inside `net.recv` with a 20s deadline,
    // so this request cannot finish inside a 300ms drain.
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

/// A second signal means a person has decided to stop waiting. A process that
/// ignores it is a process people learn to `kill -9`, which abandons the
/// rollback the teardown exists for and is strictly worse.
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

/// The lead phase: accept keeps running so a readiness route can answer `503`
/// and a load balancer can take the instance out. `signal.stopping()` already
/// answers `true`, which is what makes the two distinguishable — a lead in which
/// the flag were not yet set would be a lead a program could not use.
#[test]
fn during_the_lead_the_run_still_accepts_and_already_says_it_is_stopping() {
    let server = Server::start(&["--drain-ms", "30000", "--drain-lead-ms", "2000"]);
    let mut warm = server.connect();
    assert!(request(&mut warm).ends_with("ok"), "not stopping yet");

    server.signal("TERM");
    std::thread::sleep(Duration::from_millis(300));

    // Inside the lead: a *new* connection is still accepted, and the route reads
    // the flag and sheds.
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

/// `signal` binds under `ply run --host` and is withheld under `ply test`, with
/// or without `--host` — a stop requested once ends every test after it, and a
/// suite whose verdicts depend on the terminal is the coupling the footprint
/// graph cannot see.
///
/// `E0424` and not `E0303`: inference was right and the row was legal, and the
/// two call for opposite responses.
#[test]
fn signal_is_withheld_under_ply_test_and_names_the_twin() {
    let dir = tempfile::tempdir().expect("a temp dir");
    write(
        dir.path(),
        r#"
import std.signal (signal)

pub fn shedding() -> Bool / {signal.read} = signal.stopping()

test/nondet "a stop reaches the program" {
  assert(!shedding())
}
"#,
    );
    // `--json` because the code is what a consumer acts on, and the human
    // projection renders the message rather than the number.
    for flags in [vec!["test", "--json"], vec!["test", "--host", "--json"]] {
        let out = Command::cargo_bin("ply")
            .expect("the binary is built")
            .arg("--color")
            .arg("never")
            .args(&flags)
            .current_dir(dir.path())
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

/// The same service with a task per connection, which is the shape a real one
/// has and the shape ADR 0015 §4.5 is about: `desk.ply`'s in-flight count at a
/// signal is exactly one, and a service that spawns per connection has N.
///
/// Written once and used by both of the cases below, because the difference
/// between them is what the *client* does and nothing about the program.
const CONCURRENT: &str = r#"
import std.net
import std.net (net)
import std.signal (signal)

fn answer(c: Int) -> Int / {net.write[conn], signal.read} = {
  let _ = net.recv[conn](c, 4096, 20000);
  let payload = if signal.stopping() { b"draining" } else { b"ok" };
  let head = bytes_concat(
    b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: ",
    bytes_concat(bytes_of_string(int_to_string(bytes_len(payload))), b"\r\n\r\n"));
  let _ = net::send_all(c, bytes_concat(head, payload), 20000);
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

/// Under the production scheduler, with a task per connection: a request in
/// flight at the signal finishes, the accept loop ends on `accept` answering
/// `0`, the root joins the task that was still running, and the run exits `0`.
///
/// The interesting difference from the sequential case is that the root is
/// parked in `accept` while another task is parked in `net.recv`, so the drain
/// has to get *both* out — and the only thing that ever resolves the accept is
/// phase 2.
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

/// **A task blocked on a host handler at shutdown must not hang the drain.**
///
/// The spawned task is inside `net.recv` with a twenty second deadline and its
/// client has gone silent, so nothing but that deadline will ever resolve it.
/// A drain that parked on the token would sit there for twenty seconds however
/// long ago `--drain-ms` elapsed, and the run would exit `0` having lost the
/// request anyway — reporting success for a shutdown that dropped it.
///
/// So the scheduler's park is bounded while a stop is in progress and the
/// deadline is checked between turns: the drain ends on time, `W0608` says what
/// was abandoned, and the exit code is `3`.
///
/// What W5 does **not** do is stated by the same test: the task is not
/// cancelled and not unwound. Its `net.recv` is still outstanding on a pool
/// thread when the process exits, which is why the client below reads nothing
/// rather than a `503`.
#[test]
fn a_task_blocked_on_a_host_handler_does_not_outlast_the_drain() {
    let server = Server::start_with(CONCURRENT, &["--drain-ms", "400"]);
    let mut held = server.connect();
    // Connected and silent: the spawned task is inside `net.recv` and will be
    // for twenty seconds.
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
    // `--drain-ms 400` plus the teardown's floor, plus slack. Bounded against
    // the numbers the operator set rather than against `net.recv`'s twenty
    // seconds: a stop whose length is a host operation's timeout is a stop
    // `--drain-ms` does not bound, which is exactly the shape a W5 audit found
    // the database teardown still had.
    assert!(
        waited < Duration::from_secs(5),
        "the drain waited {waited:?} — so it was hostage to the host operation rather than \
         bounded by `--drain-ms` plus the teardown's floor\n\n{output}"
    );
}
