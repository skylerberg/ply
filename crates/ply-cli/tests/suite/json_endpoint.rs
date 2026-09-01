//! `examples/orders.ply` over a real socket, with a derived codec on both ends.
//!
//! `http_endpoint.rs` proves a Ply program can reach the host at all. This one
//! proves the thing W2 exists for: the bytes a client reads back were written by
//! a definition nobody wrote. `derive json for Receipt` generates the encoder,
//! `derive json for Order` generates the decoder, and the assertions below are
//! written against the wire rather than against a Ply value — a codec that only
//! round-trips inside the language has demonstrated nothing about a payload.
//!
//! The rejected case is here for the same reason the 400 is in `http_endpoint`:
//! a client sends what it likes, and `$.lines[0].qty: expected a number, found a
//! string` is a message built by `std.json` from a path the deriver emitted, so
//! it is the part of the format most likely to be quietly wrong.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const STARTUP: Duration = Duration::from_secs(30);

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn reserve_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("an ephemeral port");
    listener.local_addr().expect("a bound address").port()
}

/// The example verbatim, plus the entry point it deliberately does not carry:
/// `examples/hello.ply` holds the only `main` under `examples/`, so `ply run
/// examples` has one. Copying rather than editing is what keeps this test from
/// being a test of a file only this test has ever seen.
fn project(port: u16, connections: u32) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a temp dir");
    let orders = std::fs::read_to_string(repo("examples/orders.ply")).expect("examples/orders.ply");
    assert!(
        orders.contains("derive json for Order"),
        "`examples/orders.ply` no longer derives its codec, which is the whole claim here"
    );
    std::fs::write(dir.path().join("orders.ply"), orders).unwrap();
    std::fs::write(
        dir.path().join("serve.ply"),
        format!(
            "import std.net (net)\n\
             import orders\n\
             \n\
             fn main() -> Int / {{net.write[listener], net.write[conn]}} =\n  \
               orders::listen_and_serve({port}, {connections})\n"
        ),
    )
    .unwrap();
    dir
}

fn ply(dir: &std::path::Path) -> Command {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin("ply"));
    cmd.arg("--color").arg("never").current_dir(dir);
    cmd
}

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

fn post(body: &str) -> Vec<u8> {
    format!(
        "POST /orders HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

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

fn body_of(response: &str) -> &str {
    response
        .split_once("\r\n\r\n")
        .expect("a response has a blank line")
        .1
}

#[test]
fn a_json_payload_over_a_real_socket_is_decoded_and_answered_by_a_derived_codec() {
    let port = reserve_port();
    let dir = project(port, 2);
    let mut server = Server::start(dir.path(), port);

    let accepted = exchange(
        server.connect(),
        &post(r#"{"customer":"ada","lines":[{"sku":"widget","qty":3,"unit_price":1.05}]}"#),
    );
    assert!(
        accepted.starts_with("HTTP/1.1 200 OK\r\n"),
        "got:\n{accepted}"
    );
    assert!(
        accepted.contains("Content-Type: application/json\r\n"),
        "got:\n{accepted}"
    );
    // 3 x 1.05 exactly, and rendered at the scale the arithmetic produced. A
    // codec that had routed the price through binary64 would answer
    // 3.1500000000000004 here.
    assert_eq!(
        body_of(&accepted),
        r#"{"tag":"Accepted","values":[{"customer":"ada","items":3,"total":3.15}]}"#
    );

    let rejected = exchange(
        server.connect(),
        &post(r#"{"customer":"ada","lines":[{"sku":"w","qty":"three","unit_price":1.05}]}"#),
    );
    assert!(
        rejected.starts_with("HTTP/1.1 400 Bad Request\r\n"),
        "got:\n{rejected}"
    );
    assert_eq!(
        body_of(&rejected),
        r#"{"tag":"Rejected","values":["$.lines[0].qty: expected a number, found a string"]}"#
    );

    server.finish();
}
