use crate::harness::{Reservation, connect_when_ready, json_of, ply, process, repo};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Output, Stdio};
use std::time::{Duration, Instant};

const STARTUP: Duration = Duration::from_secs(30);

/// The example, verbatim.
fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a temp dir");
    std::fs::copy(
        repo().join("examples/orders.ply"),
        dir.path().join("orders.ply"),
    )
    .expect("examples/orders.ply is copied");
    dir
}

/// The `main` the example deliberately lacks, as a module of the test's own beside it.
fn entry(dir: &Path, port: u16, connections: u32) {
    std::fs::write(
        dir.join("serve.ply"),
        format!(
            "import std.net (net)\n\
             import orders\n\
             \n\
             fn main() -> Int / {{net.write[listener], net.write[conn]}} =\n  \
               orders::listen_and_serve({port}, {connections})\n"
        ),
    )
    .unwrap();
}

/// The claim the socket test rests on: `ply show` finds no `fn` written for the codec a request is
/// decoded with or the one its answer is encoded with, and names the `derive` declaring each.
#[track_caller]
fn the_codecs_are_derived(dir: &Path) {
    for (codec, declaration) in [
        ("order_json", "derive json for Order"),
        ("reply_json", "derive json for Reply"),
    ] {
        let shown = json_of(
            &ply(dir)
                .args(["show", codec, "--json"])
                .output()
                .expect("`ply show` runs"),
        );
        let refusal = &shown["diagnostics"][0];
        assert!(
            refusal["code"] == "E0101" && refusal["labels"][0]["snippet"] == declaration,
            "`{codec}` should be declared by `{declaration}` and written by no `fn`; \
             `ply show` answered {shown}"
        );
    }
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
    let dir = project();
    the_codecs_are_derived(dir.path());
    let reserved = Reservation::take();
    entry(dir.path(), reserved.port(), 2);
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
