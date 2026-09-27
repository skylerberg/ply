use crate::harness::{Reservation, connect_when_ready, process, repo};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Output, Stdio};
use std::time::{Duration, Instant};

const STARTUP: Duration = Duration::from_secs(30);

/// Plus the `main` the example deliberately lacks: `examples/hello.ply` holds the only one under `examples/`.
fn project(port: u16, connections: u32) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a temp dir");
    let orders =
        std::fs::read_to_string(repo().join("examples/orders.ply")).expect("examples/orders.ply");
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

struct Server {
    child: Option<Child>,
    reserved: Reservation,
}

impl Server {
    fn start(dir: &std::path::Path, reserved: Reservation) -> Server {
        let child = process(dir)
            .args(["run", "--host"])
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
        connect_when_ready(reserved, child, STARTUP, |_| true).unwrap_or_else(|why| panic!("{why}"))
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
    let reserved = Reservation::take();
    let dir = project(reserved.port(), 2);
    let mut server = Server::start(dir.path(), reserved);

    let first = server.connect();
    let accepted = exchange(
        first,
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
    // 3 x 1.05 exactly, and rendered at the scale the arithmetic produced.
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
