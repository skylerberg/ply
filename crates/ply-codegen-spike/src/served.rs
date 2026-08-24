//! The denominator: what one whole request costs, end to end, through the
//! shipped binary.
//!
//! A kernel speedup means nothing without it — a function that is a fifth of a
//! request cannot make a request more than a quarter faster however fast it
//! gets — so the spike measures the request as well as the function, with its
//! own client, against `examples/desk.ply` served over loopback exactly as
//! `examples/serve.sh --memory` serves it.

use anyhow::{Context, Result, bail};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The request the client sends, the shape W1 and W2 measured: one packet, a
/// complete head, no body.
pub const REQUEST: &[u8] =
    b"GET /items HTTP/1.1\r\nHost: 127.0.0.1\r\nUser-Agent: ply-bench\r\n\r\n";

/// The two rewrites `examples/serve.sh --memory` makes, and the same refusal to
/// guess: a silent miss would serve a program this measured something else.
const MAIN_ROW: (&str, &str) = (
    "fn main() -> Int / {Serving, config.read[server], net.write[conn], net.write[listener]} = {",
    "fn main() -> Int / {config.read[server], config.read[credentials], net.write[conn], net.write[listener]} = {",
);
const MAIN_CALL: (&str, &str) = ("    run(port, count)", "    run_memory(port, key, count)");

pub struct Server {
    child: Child,
    pub port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

pub fn project(root: &Path, out: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(out)?;
    let source = std::fs::read_to_string(root.join("examples/desk.ply"))
        .context("examples/desk.ply is where the served denominator comes from")?;
    let mut text = source;
    for (from, to) in [MAIN_ROW, MAIN_CALL] {
        if !text.contains(from) {
            bail!("examples/desk.ply no longer contains: {from}");
        }
        text = text.replace(from, to);
    }
    let path = out.join("desk.ply");
    std::fs::write(&path, text)?;
    Ok(path)
}

impl Server {
    pub fn start(binary: &Path, project: &Path, connections: u32) -> Result<Server> {
        let port = free_port()?;
        let child = Command::new(binary)
            .arg("run")
            .arg(project)
            .arg("--host")
            .args(["--config-schema", "desk.config"])
            .args(["--set", &format!("DESK_PORT={port}")])
            .args(["--set", &format!("DESK_CONNECTIONS={connections}")])
            .args(["--trace", "off"])
            .args(["--drain-ms", "30000"])
            .env("DESK_API_KEY", "spike-development-key")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("running the `ply` binary")?;
        let server = Server { child, port };
        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline {
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return Ok(server);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        bail!("the server did not bind {port} within two minutes")
    }
}

/// One response, read to its end so the next request on the connection begins
/// where this one stopped.
fn read_response(stream: &mut TcpStream, buffer: &mut Vec<u8>) -> Result<usize> {
    let mut scratch = [0u8; 8192];
    loop {
        if let Some(head_end) = find(buffer, b"\r\n\r\n") {
            let head = &buffer[..head_end];
            let length = content_length(head)?;
            let total = head_end + 4 + length;
            if buffer.len() >= total {
                buffer.drain(..total);
                return Ok(total);
            }
        }
        let n = stream.read(&mut scratch)?;
        if n == 0 {
            bail!("the server closed the connection mid-response");
        }
        buffer.extend_from_slice(&scratch[..n]);
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn content_length(head: &[u8]) -> Result<usize> {
    let text = String::from_utf8_lossy(head);
    for line in text.split("\r\n") {
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("content-length:") {
            return Ok(rest.trim().parse()?);
        }
    }
    bail!("the response carried no `Content-Length`")
}

pub struct Load {
    pub requests: u32,
    pub micros_per_request: f64,
    pub requests_per_second: f64,
    pub head_bytes: usize,
}

/// Sequential, one connection at a time. `desk.ply` answers one connection at a
/// time, so anything else would measure a queue.
pub fn drive(port: u16, connections: u32, per_connection: u32) -> Result<Load> {
    drive_with(port, connections, per_connection, REQUEST)
}

/// The same, over a head the caller chooses: W1's whole finding was that a
/// request's cost was a function of its head length, so one head length is one
/// point on a curve.
pub fn drive_with(
    port: u16,
    connections: u32,
    per_connection: u32,
    request: &[u8],
) -> Result<Load> {
    let mut buffer = Vec::with_capacity(64 * 1024);
    let mut answered = 0u32;
    let started = Instant::now();
    for _ in 0..connections {
        let mut stream = TcpStream::connect(("127.0.0.1", port))?;
        stream.set_nodelay(true)?;
        for _ in 0..per_connection {
            stream.write_all(request)?;
            read_response(&mut stream, &mut buffer)?;
            answered += 1;
        }
        drop(stream);
        buffer.clear();
    }
    let taken = started.elapsed().as_secs_f64();
    Ok(Load {
        requests: answered,
        micros_per_request: taken * 1e6 / f64::from(answered),
        requests_per_second: f64::from(answered) / taken,
        head_bytes: request.len(),
    })
}
