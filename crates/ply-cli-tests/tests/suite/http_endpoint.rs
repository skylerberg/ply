//! The one test in this tree that opens a real socket.

use crate::harness::{Reservation, connect_when_ready, process, repo};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::{Child, Output, Stdio};
use std::time::{Duration, Instant};

/// How long the server has to typecheck the program and bind.
const STARTUP: Duration = Duration::from_secs(30);

/// The example, verbatim: the port and the connection count a test chooses are the run's settings.
fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a temp dir");
    std::fs::copy(
        repo().join("examples/hello.ply"),
        dir.path().join("hello.ply"),
    )
    .expect("examples/hello.ply is copied");
    dir
}

/// Kills the server whatever the test does, including panicking out of an assertion.
struct Server {
    child: Option<Child>,
    reserved: Reservation,
}

impl Server {
    /// Listens on the reserved port and answers `connections` connections before it returns.
    fn start(dir: &std::path::Path, reserved: Reservation, connections: u32) -> Server {
        let child = process(dir)
            .args(["run", "--host", "--set"])
            .arg(format!("HELLO_PORT={}", reserved.port()))
            .arg("--set")
            .arg(format!("HELLO_CONNECTIONS={connections}"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("`ply run --host` starts");
        Server {
            child: Some(child),
            reserved,
        }
    }

    fn running(&mut self) -> &mut Child {
        self.child.as_mut().expect("the server has not been reaped")
    }

    fn connect(&mut self) -> TcpStream {
        let Server { child, reserved } = self;
        let child = child.as_mut().expect("the server has not been reaped");
        let port = reserved.port();
        connect_when_ready(reserved, child, STARTUP, |_| true)
            .unwrap_or_else(|why| panic!("{why}\nthe run was given `--set HELLO_PORT={port}`"))
    }

    /// Asked for a fixed number of connections and given them, the server must return on its own.
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

/// Writes a request and reads until the server closes.
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

#[test]
fn a_request_over_a_real_socket_is_answered_by_a_ply_program() {
    let reserved = Reservation::take();
    let dir = project();
    let mut server = Server::start(dir.path(), reserved, 1);
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

#[test]
fn a_malformed_request_is_answered_400_and_the_server_survives_it() {
    let reserved = Reservation::take();
    let dir = project();
    let mut server = Server::start(dir.path(), reserved, 2);

    let first = server.connect();
    let response = exchange(first, b"GET /\r\n\r\n");
    assert!(
        response.starts_with("HTTP/1.1 400 Bad Request\r\n"),
        "got:\n{response}"
    );
    assert!(
        response.ends_with("a request line is `METHOD TARGET VERSION`\n"),
        "got:\n{response}"
    );

    // The listener is still there, which is the half of the claim a single request cannot make.
    let second = exchange(
        server.connect(),
        b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
    );
    assert!(second.starts_with("HTTP/1.1 200 OK\r\n"), "got:\n{second}");

    server.finish();
}

#[test]
fn a_request_split_across_writes_is_read_to_its_terminator() {
    let reserved = Reservation::take();
    let dir = project();
    let mut server = Server::start(dir.path(), reserved, 1);
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

#[test]
fn the_same_program_is_hermetic_under_ply_test() {
    let dir = project();
    let out = process(dir.path())
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
        "the tests handle every `net` and `config` operation themselves, so none reaches the \
         boundary:\n{text}"
    );
}

#[test]
fn without_the_flag_the_program_never_reaches_the_socket() {
    let reserved = Reservation::take();
    let port = reserved.port();
    let dir = project();
    // `--set` needs `--host`; a run that read this environment would listen where the check looks.
    let out = process(dir.path())
        .env("HELLO_PORT", port.to_string())
        .env("HELLO_CONNECTIONS", "1")
        .arg("run")
        .output()
        .expect("`ply run` runs");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_ne!(out.status.code(), Some(0), "got:\n{text}");
    assert!(text.contains("E0424"), "got:\n{text}");
    assert!(
        text.contains("ply_host::config::get") && !text.contains("net.listen"),
        "the run should be refused reading its settings, before any socket; got:\n{text}"
    );

    assert!(
        TcpStream::connect_timeout(
            &SocketAddr::from(([127, 0, 0, 1], port)),
            Duration::from_millis(250)
        )
        .is_err(),
        "a hermetic run bound port {port}"
    );
}

fn reaching_test(port: u16) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a temp dir");
    std::fs::write(
        dir.path().join("touch.ply"),
        format!(
            "import std.net (net)\n\
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

#[test]
fn a_hermetic_test_that_reaches_the_boundary_names_the_handler_it_did_not_use() {
    let reserved = Reservation::take();
    let dir = reaching_test(reserved.port());
    let out = process(dir.path())
        .arg("test")
        .output()
        .expect("`ply test` runs");
    let text = output(&out);
    assert_ne!(out.status.code(), Some(0), "got:\n{text}");
    // The failure block prints the message, not the code, and E0424's message is one E0303 cannot produce.
    assert!(
        text.contains("reached the host boundary in a hermetic run"),
        "got:\n{text}"
    );
    assert!(text.contains("ply_host::tcp::listen"), "got:\n{text}");
    assert!(!text.contains("no handler for"), "got:\n{text}");
}

#[test]
fn a_host_backed_pass_is_never_cached_and_never_satisfies_a_hermetic_run() {
    let reserved = Reservation::take();
    let dir = reaching_test(reserved.port());

    for attempt in 0..2 {
        let out = process(dir.path())
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

    let out = process(dir.path())
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

    let out = process(dir.path())
        .args(["run", "--host"])
        .output()
        .expect("`ply run --host` runs");
    let text = output(&out);
    assert_eq!(out.status.code(), Some(0), "got:\n{text}");
    assert!(text.contains('3'), "got:\n{text}");

    // Hermetically, the program reaches the boundary and is told both remedies rather than getting real threads.
    let out = process(dir.path())
        .arg("run")
        .output()
        .expect("`ply run` runs");
    let text = output(&out);
    assert_ne!(out.status.code(), Some(0), "got:\n{text}");
    assert!(text.contains("E0424"), "got:\n{text}");
    assert!(text.contains("task.spawn"), "got:\n{text}");
}
