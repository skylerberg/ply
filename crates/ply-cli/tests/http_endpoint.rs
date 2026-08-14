//! The one test in this tree that opens a real socket.
//!
//! Everything else about W1 can be asserted in memory, and most of it should
//! be. This cannot: the claim is that a Ply program reached the host through
//! the effect system and came back with the right bytes, and the only witness
//! to that is a TCP connection made by something that is not Ply.
//!
//! Three things are checked together on purpose, because each one is only
//! meaningful beside the others: the request round trips under `--host`; a
//! malformed request is answered 400 rather than crashing the server; and the
//! *same source*, unchanged, is hermetic under `ply test` and refused with
//! `E0424` under `ply run` without the flag. A green round trip that also
//! passed hermetically would mean the binding did nothing.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

/// How long the server has to typecheck the program and bind. Generous: a cold
/// front-end cache in CI is slower than anything a laptop shows.
const STARTUP: Duration = Duration::from_secs(30);

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// A port nothing is listening on.
///
/// Bind, read the number, drop. The window between the drop and the server's
/// own bind is a race with every other process on the machine; it is also the
/// only option, because `ply run` takes no program arguments, so the port has
/// to be in the source before the process starts.
fn reserve_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("an ephemeral port");
    listener.local_addr().expect("a bound address").port()
}

/// The example, verbatim, with the two numbers a test needs to choose.
///
/// The substitution asserts it found what it was replacing. A silent miss
/// would leave the server listening on 8080 and answering somebody else's
/// test, which is exactly the sort of green-over-unexplored-space this
/// milestone exists to refuse.
fn project(port: u16, connections: u32) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a temp dir");
    let hello = std::fs::read_to_string(repo("examples/hello.ply")).expect("examples/hello.ply");

    let source = replace(
        &hello,
        "fn port() -> Int = 8080",
        &format!("fn port() -> Int = {port}"),
    );
    let source = replace(
        &source,
        "fn connections() -> Int = 64",
        &format!("fn connections() -> Int = {connections}"),
    );

    std::fs::write(dir.path().join("hello.ply"), source).unwrap();
    dir
}

fn replace(source: &str, from: &str, to: &str) -> String {
    assert!(
        source.contains(from),
        "`examples/hello.ply` no longer contains `{from}`; this test rewrites it and must be \
         updated with it rather than silently listening on the example's own port"
    );
    source.replace(from, to)
}

fn ply(dir: &std::path::Path) -> Command {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin("ply"));
    cmd.arg("--color").arg("never").current_dir(dir);
    cmd
}

/// Kills the server whatever the test does, including panicking out of an
/// assertion. A leaked `ply run` holds a port for the rest of the suite.
struct Server {
    child: Option<Child>,
    addr: SocketAddr,
}

impl Server {
    fn start(dir: &std::path::Path, port: u16) -> Server {
        let child = ply(dir)
            .args(["run", "--host"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("`ply run --host` starts");
        Server {
            child: Some(child),
            addr: SocketAddr::from(([127, 0, 0, 1], port)),
        }
    }

    fn running(&mut self) -> &mut Child {
        self.child.as_mut().expect("the server has not been reaped")
    }

    /// Connects once the server is listening. A child that exited first is
    /// reported with what it printed — a compile error there is far more
    /// useful than "connection refused" thirty seconds later.
    fn connect(&mut self) -> TcpStream {
        let deadline = Instant::now() + STARTUP;
        loop {
            if let Some(status) = self.running().try_wait().expect("the child is waitable") {
                let output = self.take();
                panic!("`ply run --host` exited {status} before listening:\n{output}");
            }
            match TcpStream::connect_timeout(&self.addr, Duration::from_millis(250)) {
                Ok(stream) => return stream,
                Err(e) if Instant::now() >= deadline => {
                    panic!("nothing listening on {} after {STARTUP:?}: {e}", self.addr)
                }
                Err(_) => std::thread::sleep(Duration::from_millis(25)),
            }
        }
    }

    /// The server was asked for a fixed number of connections and has been
    /// given them, so it must return on its own. One that has to be killed has
    /// not demonstrated that the host operation returned into the program.
    fn finish(mut self) {
        let deadline = Instant::now() + STARTUP;
        loop {
            match self.running().try_wait().expect("the child is waitable") {
                Some(status) if status.success() => return,
                Some(status) => {
                    let output = self.take();
                    panic!("the server exited {status} after answering:\n{output}");
                }
                None => assert!(
                    Instant::now() < deadline,
                    "the server was still running {STARTUP:?} after every connection it was asked \
                     for; `serve` should have returned"
                ),
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn take(&mut self) -> String {
        let Some(child) = self.child.take() else {
            return String::new();
        };
        let out: Output = child.wait_with_output().expect("the server's output");
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Writes a request and reads until the server closes. The endpoint answers
/// `Connection: close` and closes, so end-of-stream is the end of the response
/// and there is no framing to get wrong on this side.
fn exchange(mut stream: TcpStream, request: &[u8]) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.write_all(request).expect("the request is written");
    stream.flush().unwrap();
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .expect("the server answers and closes");
    String::from_utf8(response).expect("the response is UTF-8")
}

// --- The round trip ---------------------------------------------------------

#[test]
fn a_request_over_a_real_socket_is_answered_by_a_ply_program() {
    let port = reserve_port();
    let dir = project(port, 1);
    let mut server = Server::start(dir.path(), port);
    let stream = server.connect();

    let response = exchange(
        stream,
        b"GET /hello HTTP/1.1\r\nHost: 127.0.0.1\r\nUser-Agent: ply-test\r\n\r\n",
    );

    assert!(
        response.starts_with("HTTP/1.1 200 OK\r\n"),
        "got:\n{response}"
    );
    assert!(
        response.contains("Content-Length: 15\r\n"),
        "got:\n{response}"
    );
    assert!(
        response.contains("Connection: close\r\n"),
        "got:\n{response}"
    );
    assert!(
        response.ends_with("\r\n\r\nhello from ply\n"),
        "got:\n{response}"
    );

    server.finish();
}

/// A peer sending nonsense is the case a host boundary makes dangerous: the
/// bytes come from outside, and the response to them must be a 400 rather than
/// a panic that takes the listener with it.
#[test]
fn a_malformed_request_is_answered_400_and_the_server_survives_it() {
    let port = reserve_port();
    let dir = project(port, 2);
    let mut server = Server::start(dir.path(), port);

    let response = exchange(server.connect(), b"GET /\r\n\r\n");
    assert!(
        response.starts_with("HTTP/1.1 400 Bad Request\r\n"),
        "got:\n{response}"
    );
    assert!(
        response.ends_with("a request line is `METHOD TARGET VERSION`\n"),
        "got:\n{response}"
    );

    // The listener is still there, which is the half of the claim a single
    // request cannot make.
    let second = exchange(
        server.connect(),
        b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
    );
    assert!(second.starts_with("HTTP/1.1 200 OK\r\n"), "got:\n{second}");

    server.finish();
}

/// A request that arrives in pieces. `read_head` is the only reason this is
/// not the packet-boundary bug every hand-rolled HTTP server ships with, and
/// a loopback connection will not produce the split on its own.
#[test]
fn a_request_split_across_writes_is_read_to_its_terminator() {
    let port = reserve_port();
    let dir = project(port, 1);
    let mut server = Server::start(dir.path(), port);
    let mut stream = server.connect();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();

    for piece in [
        &b"GET / HT"[..],
        &b"TP/1.1\r\nHost: 127.0.0"[..],
        &b".1\r\n\r"[..],
        &b"\n"[..],
    ] {
        stream.write_all(piece).expect("a piece is written");
        stream.flush().unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }

    let mut response = Vec::new();
    stream.read_to_end(&mut response).expect("an answer");
    let response = String::from_utf8(response).expect("UTF-8");
    assert!(
        response.starts_with("HTTP/1.1 200 OK\r\n"),
        "got:\n{response}"
    );

    server.finish();
}

// --- The same source, hermetically ------------------------------------------

/// The point of the boundary: the program that just answered a socket is also
/// the program `ply test` runs with nothing bound, and there it touches
/// nothing at all.
#[test]
fn the_same_program_is_hermetic_under_ply_test() {
    let dir = project(reserve_port(), 1);
    let out = ply(dir.path())
        .arg("test")
        .output()
        .expect("`ply test` runs");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.status.code(), Some(0), "got:\n{text}");
    assert!(
        !text.contains("E0424"),
        "the tests handle every `net` operation themselves, so none reaches the boundary:\n{text}"
    );
}

/// Without `--host` the run is hermetic, and reaching the boundary there is
/// `E0424` rather than a socket. This is the assertion that makes the round
/// trip above mean something: the binding is what changed, not the source.
#[test]
fn without_the_flag_the_program_never_reaches_the_socket() {
    let port = reserve_port();
    let dir = project(port, 1);
    let out = ply(dir.path()).arg("run").output().expect("`ply run` runs");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_ne!(out.status.code(), Some(0), "got:\n{text}");
    assert!(text.contains("E0424"), "got:\n{text}");
    assert!(text.contains("net.listen"), "got:\n{text}");

    assert!(
        TcpStream::connect_timeout(
            &SocketAddr::from(([127, 0, 0, 1], port)),
            Duration::from_millis(250)
        )
        .is_err(),
        "a hermetic run bound port {port}"
    );
}

// --- `ply test` and the binding ----------------------------------------------

/// A test that reaches a socket, and the same project's whole cache lifecycle
/// around it.
///
/// The source declares `net` exactly as `crates/ply-host/ply/net.ply` does,
/// because a signature that drifts is `E0421` at bind time and this test would
/// then be asserting about a program that never bound rather than about one
/// that ran.
fn reaching_test(port: u16) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a temp dir");
    let declaration =
        std::fs::read_to_string(repo("crates/ply-host/ply/net.ply")).expect("the declaration");
    let effect = declaration
        .split_once("nondet effect net {")
        .map(|(_, rest)| rest.split_once('}').expect("the block closes").0)
        .expect("the declaration holds the effect");
    std::fs::write(
        dir.path().join("touch.ply"),
        format!(
            "nondet effect net {{{effect}}}\n\
             \n\
             fn touch() -> Int / {{net.write[listener]}} {{\n\
             \x20 let l = net.listen[listener]({port});\n\
             \x20 net.close[listener](l);\n\
             \x20 l\n\
             }}\n\
             \n\
             test/nondet \"reaches the host\" {{ assert(touch() > 0) }}\n"
        ),
    )
    .unwrap();
    dir
}

fn output(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// `ply test` binds the registry **without** binding it: nothing is reachable,
/// and the refusal still names the handler that `--host` would have used.
///
/// The distinction is the whole of ADR 0011 §5. `E0303` means inference should
/// have prevented the perform and did not — file a bug — and `E0424` means
/// inference was right and the run was configured hermetically — pass the flag
/// or write a test double. A consumer that cannot tell them apart does the
/// wrong one, and a runner that never bound the registry can only say `E0303`.
#[test]
fn a_hermetic_test_that_reaches_the_boundary_names_the_handler_it_did_not_use() {
    let dir = reaching_test(reserve_port());
    let out = ply(dir.path())
        .arg("test")
        .output()
        .expect("`ply test` runs");
    let text = output(&out);
    assert_ne!(out.status.code(), Some(0), "got:\n{text}");
    // The failure block prints the diagnostic's message rather than its code,
    // and E0424's message is the one sentence E0303 cannot produce.
    assert!(
        text.contains("reached the host boundary in a hermetic run"),
        "got:\n{text}"
    );
    assert!(text.contains("ply_host::tcp::listen"), "got:\n{text}");
    assert!(!text.contains("no handler for"), "got:\n{text}");
}

/// `--host` is not reporting: the test opens a real listener and goes green.
///
/// And the pass is never written, in either direction — the hermetic run after
/// it fails again. A cached pass earned over a socket is believed by every
/// later hermetic run and nothing about that run looks wrong, which is the
/// longest-lived failure this boundary can produce.
#[test]
fn a_host_backed_pass_is_never_cached_and_never_satisfies_a_hermetic_run() {
    let dir = reaching_test(reserve_port());

    for attempt in 0..2 {
        let out = ply(dir.path())
            .args(["test", "--host"])
            .output()
            .expect("`ply test --host` runs");
        let text = output(&out);
        assert_eq!(
            out.status.code(),
            Some(0),
            "attempt {attempt}, got:\n{text}"
        );
        assert!(text.contains("host-backed and not cached"), "got:\n{text}");
        assert!(
            !text.contains(", 1 cached"),
            "the second run believed the first:\n{text}"
        );
    }

    let out = ply(dir.path())
        .arg("test")
        .output()
        .expect("`ply test` runs");
    let text = output(&out);
    assert_ne!(
        out.status.code(),
        Some(0),
        "a pass earned over a socket satisfied a hermetic run:\n{text}"
    );
    assert!(
        text.contains("reached the host boundary in a hermetic run"),
        "got:\n{text}"
    );
}

/// The production scheduler, reached the only way a program can reach it.
///
/// `task.*` is answered by opening a region rather than by a handler — a task is
/// a suspended machine state and a handler sees only values — so a defect here
/// is `E0505` from `ply_host::sched`, not a wrong number.
#[test]
fn task_spawn_under_the_flag_runs_on_the_production_scheduler() {
    let dir = tempfile::tempdir().expect("a temp dir");
    std::fs::write(
        dir.path().join("tasks.ply"),
        "fn main() -> Int / {task.write} {\n\
         \x20 let a = task.spawn(|| 1);\n\
         \x20 let b = task.spawn(|| 2);\n\
         \x20 task.join(a) + task.join(b)\n\
         }\n",
    )
    .unwrap();

    let out = ply(dir.path())
        .args(["run", "--host"])
        .output()
        .expect("`ply run --host` runs");
    let text = output(&out);
    assert_eq!(out.status.code(), Some(0), "got:\n{text}");
    assert!(text.contains('3'), "got:\n{text}");

    // Lock 3: hermetically the same program reaches the boundary and is told
    // both remedies, rather than getting real threads by accident.
    let out = ply(dir.path()).arg("run").output().expect("`ply run` runs");
    let text = output(&out);
    assert_ne!(out.status.code(), Some(0), "got:\n{text}");
    assert!(text.contains("E0424"), "got:\n{text}");
    assert!(text.contains("task.spawn"), "got:\n{text}");
}
