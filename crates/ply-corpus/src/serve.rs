//! What one request costs, and which layer it was spent in.

use anyhow::{Context, Result, bail};
use ply_eval::host::HostRuntime;
use ply_eval::{Machine, Value};
use ply_host::tcp::{Net, SimNet};
use ply_span::Span;
use ply_syntax::ast::ModuleName;
use ply_ty::CheckOutput;
use ply_ty::ty::Footprint;
use serde::Serialize;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const REQUEST: &[u8] =
    b"GET /hello HTTP/1.1\r\nHost: 127.0.0.1\r\nUser-Agent: ply-bench\r\n\r\n";

/// How long a client waits for a response before calling the server hung.
const CLIENT_TIMEOUT: Duration = Duration::from_secs(20);

/// How long a server has to typecheck its program and bind a port.
const STARTUP: Duration = Duration::from_secs(60);

fn micros(d: Duration) -> f64 {
    d.as_secs_f64() * 1e6
}

/// Which implementation of the endpoint's three scans is under measurement.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Parser {
    /// `bytes_index_of`, `bytes_scan` and `bytes_scan_until`.
    Native,
    /// `fold`-over-`range` scans.
    W1Folds,
}

impl Parser {
    pub fn label(self) -> &'static str {
        match self {
            Parser::Native => "native",
            Parser::W1Folds => "w1-folds",
        }
    }

    pub fn all() -> [Parser; 2] {
        [Parser::W1Folds, Parser::Native]
    }
}

/// Each native scan paired with its fold replacement.
const W1_SCANS: [(&str, &str); 3] = [
    (
        "\
fn index_of(hay: Bytes, set: Bytes, from: Int) -> Int =
  bytes_scan_until(hay, from, set, bytes_len(hay))",
        "\
fn index_of(hay: Bytes, set: Bytes, from: Int) -> Int =
  fold(range(0, bytes_len(hay)), bytes_len(hay), |best: Int, i: Int|
    if best < bytes_len(hay) || i < from || bytes_at(hay, i) != bytes_at(set, 0) {
      best
    } else {
      i
    })",
    ),
    (
        "\
fn head_end(head: Bytes) -> Int =
  match bytes_index_of(head, b\"\\r\\n\\r\\n\") {
    Some(at) -> at + 4,
    None -> -1,
  }",
        "\
fn head_end(head: Bytes) -> Int =
  fold(range(0, bytes_len(head)), -1, |best: Int, i: Int|
    if best >= 0 || i + 3 >= bytes_len(head) {
      best
    } else if bytes_at(head, i) == 13 && bytes_at(head, i + 1) == 10
           && bytes_at(head, i + 2) == 13 && bytes_at(head, i + 3) == 10 {
      i + 4
    } else {
      best
    })",
    ),
    (
        "\
fn all_upper(b: Bytes) -> Bool =
  bytes_scan(b, 0, b\"ABCDEFGHIJKLMNOPQRSTUVWXYZ\", bytes_len(b)) == bytes_len(b)",
        "\
fn all_upper(b: Bytes) -> Bool =
  fold(range(0, bytes_len(b)), true, |ok: Bool, i: Int|
    ok && bytes_at(b, i) >= 65 && bytes_at(b, i) <= 90)",
    ),
];

/// `examples/hello.ply`.
pub struct Endpoint {
    source: String,
    /// The example cut at `TESTS_MARKER`.
    server_only: String,
}

/// Where the endpoint's simulated twin begins.
const TESTS_MARKER: &str = "// --- The simulated socket";

impl Endpoint {
    pub fn open(repo: &Path) -> Result<Endpoint> {
        let path = repo.join("examples/hello.ply");
        let source = std::fs::read_to_string(&path)
            .with_context(|| format!("reading `{}`", path.display()))?;
        let Some(cut) = source.find(TESTS_MARKER) else {
            bail!(
                "`{}` no longer contains `{TESTS_MARKER}`; this harness splits the example there \
                 and must be updated with it rather than measuring a program it guessed at",
                path.display()
            );
        };
        let server_only = source[..cut].to_string();
        Ok(Endpoint {
            source,
            server_only,
        })
    }

    /// The server half plus the driver the in-process rungs call into.
    pub fn benchable(&self, parser: Parser) -> Result<String> {
        Ok(format!("{}{PLY_HANDLER_DRIVER}", self.scans(parser)?))
    }

    pub fn whole(&self, parser: Parser) -> Result<String> {
        Self::retarget(&self.source, parser)
    }

    fn scans(&self, parser: Parser) -> Result<String> {
        Self::retarget(&self.server_only, parser)
    }

    fn retarget(source: &str, parser: Parser) -> Result<String> {
        match parser {
            Parser::Native => Ok(source.to_string()),
            Parser::W1Folds => W1_SCANS
                .iter()
                .try_fold(source.to_string(), |acc, (from, to)| {
                    replace(&acc, from, to)
                }),
        }
    }

    /// The example with a chosen port and connection count, tests included.
    pub fn sequential(&self, parser: Parser, port: u16, connections: u32) -> Result<String> {
        settings(&self.whole(parser)?, port, connections)
    }

    /// The same endpoint with a task spawned per connection.
    pub fn concurrent(&self, parser: Parser, port: u16, connections: u32) -> Result<String> {
        const OLD: &str = "\
fn serve(server: Int, count: Int) -> Int / {net.write[listener], net.write[conn]} =
  if count <= 0 {
    0
  } else {
    serve_one(net.accept[listener](server));
    1 + serve(server, count - 1)
  }";
        // The joins unwind at the end, so up to `count` handlers are in flight at once.
        const NEW: &str = "\
fn serve(server: Int, count: Int) -> Int
  / {task.write, net.write[listener], net.write[conn]} =
  if count <= 0 {
    0
  } else {
    let c = net.accept[listener](server);
    let t = task.spawn(|| serve_one(c));
    let rest = serve(server, count - 1);
    task.join(t);
    1 + rest
  }";
        let source = replace(&self.scans(parser)?, OLD, NEW)?;
        let source = replace(
            &source,
            "fn listen_and_serve(port: Int, count: Int) -> Int\n  / {net.write[listener], net.write[conn]} {",
            "fn listen_and_serve(port: Int, count: Int) -> Int\n  / {task.write, net.write[listener], net.write[conn]} {",
        )?;
        let source = replace(
            &source,
            "fn main() -> Int / {net.write[listener], net.write[conn]} =",
            "fn main() -> Int / {task.write, net.write[listener], net.write[conn]} =",
        )?;
        settings(&source, port, connections)
    }
}

fn settings(source: &str, port: u16, connections: u32) -> Result<String> {
    let source = replace(
        source,
        "fn port() -> Int = 8080",
        &format!("fn port() -> Int = {port}"),
    )?;
    replace(
        &source,
        "fn connections() -> Int = 64",
        &format!("fn connections() -> Int = {connections}"),
    )
}

fn replace(source: &str, from: &str, to: &str) -> Result<String> {
    if !source.contains(from) {
        bail!(
            "`examples/hello.ply` no longer contains:\n{from}\n\
             this harness rewrites it and must be updated with it rather than measuring a program \
             it guessed at"
        );
    }
    Ok(source.replace(from, to))
}

/// A port nothing is listening on.
pub fn reserve_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context("reserving an ephemeral port")?;
    Ok(listener.local_addr()?.port())
}

#[derive(Clone, Debug, Serialize)]
pub struct Rung {
    pub name: &'static str,
    /// What the rung adds over the one before it, in prose.
    pub adds: &'static str,
    pub requests: u32,
    pub per_request_micros: f64,
    pub per_second: f64,
    /// Cost above the previous rung.
    pub layer_micros: f64,
    /// That layer as a share of the top rung.
    pub layer_share: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Ladder {
    pub parser: &'static str,
    pub head_bytes: usize,
    pub rungs: Vec<Rung>,
    /// `host-tcp` over `rust-floor`.
    pub over_floor: f64,
    /// The share of a request that is not the socket.
    pub interpreter_share: f64,
}

pub fn ladder(repo: &Path, parser: Parser, requests: u32, repeats: usize) -> Result<Ladder> {
    let endpoint = Endpoint::open(repo)?;
    let dir = tempfile::tempdir().context("a temp dir for the benchmark project")?;
    std::fs::write(dir.path().join("hello.ply"), endpoint.benchable(parser)?)?;
    let program = Program::load(dir.path())?;

    let answer = best_of(repeats, || program.answer_only(requests))?;
    let handler = best_of(repeats, || program.through_ply_handler(requests))?;
    let sim = best_of(repeats, || program.through_host(requests))?;
    let tcp = best_of(repeats, || program.through_socket(requests))?;
    let floor = best_of(repeats, || rust_floor(requests))?;

    let names: [(&'static str, &'static str, Duration); 5] = [
        (
            "answer",
            "the HTTP parse and the response build, as ordinary Ply",
            answer,
        ),
        (
            "ply-handler",
            "performing `net.*` and dispatching a Ply `handle` clause",
            handler,
        ),
        (
            "host-sim",
            "the host boundary: resolve, footprint check, decode",
            sim,
        ),
        (
            "host-tcp",
            "the socket, the reactor and the blocking pool",
            tcp,
        ),
        (
            "rust-floor",
            "nothing; the same syscalls with no interpreter",
            floor,
        ),
    ];

    let total = micros(tcp) / requests as f64;
    let mut rungs = Vec::new();
    let mut previous = 0.0;
    for (name, adds, taken) in names {
        let per = micros(taken) / requests as f64;
        // `rust-floor` is the denominator, not a rung, so it adds no layer.
        let layer = if name == "rust-floor" {
            0.0
        } else {
            per - previous
        };
        rungs.push(Rung {
            name,
            adds,
            requests,
            per_request_micros: per,
            per_second: 1e6 / per,
            layer_micros: layer,
            layer_share: layer / total,
        });
        if name != "rust-floor" {
            previous = per;
        }
    }

    let socket_layer = micros(tcp - sim) / requests as f64;
    Ok(Ladder {
        parser: parser.label(),
        head_bytes: REQUEST.len(),
        over_floor: tcp.as_secs_f64() / floor.as_secs_f64(),
        interpreter_share: (total - socket_layer) / total,
        rungs,
    })
}

/// One head length, and what `answer` cost over it.
#[derive(Clone, Debug, Serialize)]
pub struct HeadPoint {
    pub parser: &'static str,
    pub head_bytes: usize,
    /// Header lines above the blank one.
    pub headers: usize,
    pub requests: u32,
    pub per_request_micros: f64,
    pub per_byte_micros: f64,
    pub per_second: f64,
}

/// Whether a request's cost scales with head bytes or with fields parsed.
pub fn head_sweep(
    repo: &Path,
    parser: Parser,
    requests: u32,
    repeats: usize,
) -> Result<Vec<HeadPoint>> {
    let endpoint = Endpoint::open(repo)?;
    let dir = tempfile::tempdir().context("a temp dir for the head sweep")?;
    std::fs::write(dir.path().join("hello.ply"), endpoint.benchable(parser)?)?;
    let program = Program::load(dir.path())?;

    let mut out = Vec::new();
    for headers in [0usize, 1, 2, 4, 8, 16, 32] {
        let head = padded_head(headers);
        // Past `max_head` the endpoint refuses, which would measure a different path.
        if head.len() > 2048 {
            break;
        }
        // The fold parser is far slower on a long head; fewer requests say the same.
        let requests = match parser {
            Parser::Native => requests,
            Parser::W1Folds => (requests / 10).max(50),
        };
        let taken = best_of(repeats, || program.answer_over(&head, requests))?;
        let per = micros(taken) / requests as f64;
        out.push(HeadPoint {
            parser: parser.label(),
            head_bytes: head.len(),
            headers,
            requests,
            per_request_micros: per,
            per_byte_micros: per / head.len() as f64,
            per_second: 1e6 / per,
        });
    }
    Ok(out)
}

/// The benchmark request line with `headers` filler lines under it.
pub fn padded_head(headers: usize) -> Vec<u8> {
    let mut head = b"GET /hello HTTP/1.1\r\n".to_vec();
    for i in 0..headers {
        head.extend_from_slice(
            format!("X-Pad-{i:02}: {}\r\n", "0123456789abcdef".repeat(3)).as_bytes(),
        );
    }
    head.extend_from_slice(b"\r\n");
    head
}

fn best_of(repeats: usize, mut run: impl FnMut() -> Result<Duration>) -> Result<Duration> {
    let mut best: Option<Duration> = None;
    for _ in 0..repeats.max(1) {
        let taken = run()?;
        if best.is_none_or(|b| taken < b) {
            best = Some(taken);
        }
    }
    Ok(best.expect("at least one attempt always runs"))
}

/// A checked endpoint, ready to be called with whatever is underneath it.
pub struct Program {
    check: CheckOutput,
    /// The tier is built from this rather than from a second front end.
    port: ply_ty::Front,
    sources: ply_span::SourceMap,
}

impl Program {
    pub fn load(root: &Path) -> Result<Program> {
        let path = root.join("hello.ply");
        let text = std::fs::read_to_string(&path)?;
        let mut sources = ply_span::SourceMap::new();
        let id = sources.add(&path, text.clone());
        let name = ModuleName::from_relative_path(Path::new("hello.ply"))
            .map_err(|d| anyhow::anyhow!("{}", d.message))?;
        // The endpoint imports `std.net`.
        let mut inputs = vec![(id, name, text.as_str())];
        for (module, source) in ply_std::sources() {
            let module = ModuleName::from_dotted(module);
            let id = sources.add(ply_std::pseudo_path(&module), source.to_string());
            inputs.push((id, module, source));
        }
        let ordered: Vec<(String, String)> = inputs
            .iter()
            .map(|(_, m, s)| (m.to_string(), s.to_string()))
            .collect();
        let ids: Vec<ply_span::SourceId> = inputs.iter().map(|(id, _, _)| *id).collect();
        let port = ply_codegen::c::producer::checked_front(&ordered, &ids)
            .map_err(|e| anyhow::anyhow!("checking the endpoint: {e}"))?;
        Ok(Program {
            check: port.check.clone(),
            port,
            sources,
        })
    }

    fn machine(&self) -> Machine<'_> {
        crate::tier_machine(&self.port, &self.sources)
    }

    fn footprint(&self, name: &str) -> Option<Footprint> {
        self.check
            .defs
            .values()
            .find(|d| d.simple_name.as_str() == name)
            .map(|d| d.footprint.clone())
    }

    /// The program-wide name `Machine::call` takes.
    fn full(&self, simple: &str) -> Result<String> {
        self.check
            .defs
            .values()
            .find(|d| d.simple_name.as_str() == simple)
            .map(|d| d.name.to_string())
            .with_context(|| format!("the endpoint declares no `{simple}`"))
    }

    fn answer_only(&self, requests: u32) -> Result<Duration> {
        self.answer_over(REQUEST, requests)
    }

    fn answer_over(&self, head: &[u8], requests: u32) -> Result<Duration> {
        let name = self.full("answer")?;
        let mut machine = self.machine();
        let head = Value::bytes(head);
        let started = Instant::now();
        for _ in 0..requests {
            machine
                .call(&name, vec![head.clone()], Span::DUMMY)
                .map_err(|d| anyhow::anyhow!("`answer` raised: {}", d.message))?;
        }
        Ok(started.elapsed())
    }

    fn through_ply_handler(&self, requests: u32) -> Result<Duration> {
        let name = self.full("bench_ply_handler")?;
        let mut machine = self.machine();
        let started = Instant::now();
        let served = machine
            .call(&name, vec![Value::Int(requests as i64)], Span::DUMMY)
            .map_err(|d| anyhow::anyhow!("the handler rung raised: {}", d.message))?;
        let taken = started.elapsed();
        expect_served(&served, requests)?;
        Ok(taken)
    }

    fn through_host(&self, requests: u32) -> Result<Duration> {
        let script: Vec<Vec<Vec<u8>>> = (0..requests).map(|_| vec![REQUEST.to_vec()]).collect();
        let net: Arc<dyn Net> = Arc::new(SimNet::new(script));
        let binding = ply_host::tcp::registry(net)
            .bind(&self.check)
            .map_err(|d| diagnostics("binding the simulated network", &d))?;

        let name = self.full("listen_and_serve")?;
        let mut machine = self.machine();
        machine.set_host_binding(Arc::new(binding));
        if let Some(declared) = self.footprint("listen_and_serve") {
            machine.set_declared_footprint(declared);
        }
        let started = Instant::now();
        let served = machine
            .call(
                &name,
                vec![Value::Int(0), Value::Int(requests as i64)],
                Span::DUMMY,
            )
            .map_err(|d| anyhow::anyhow!("the host rung raised: {}", d.message))?;
        let taken = started.elapsed();
        expect_served(&served, requests)?;
        Ok(taken)
    }

    fn through_socket(&self, requests: u32) -> Result<Duration> {
        let port = reserve_port()?;
        let host = Arc::new(ply_host::Host::new());
        let binding = host
            .registry()
            .bind(&self.check)
            .map_err(|d| diagnostics("binding the network", &d))?;

        let name = self.full("listen_and_serve")?;
        let mut machine = self.machine();
        machine.set_host_binding(Arc::new(binding));
        let runtime: Rc<dyn HostRuntime> = host.runtime();
        machine.set_host_runtime(runtime);
        if let Some(declared) = self.footprint("listen_and_serve") {
            machine.set_declared_footprint(declared);
        }

        let client = Client::spawn(port, REQUEST.into(), requests, 1);
        let started = Instant::now();
        let served = machine.call(
            &name,
            vec![Value::Int(port as i64), Value::Int(requests as i64)],
            Span::DUMMY,
        );
        let taken = started.elapsed();
        let sample = client.join()?;
        served.map_err(|d| anyhow::anyhow!("the socket rung raised: {}", d.message))?;
        sample.require(requests)?;
        Ok(taken)
    }
}

/// The Ply-handler rung's driver, appended to the endpoint.
const PLY_HANDLER_DRIVER: &str = r#"

fn bench_request() -> Bytes = b"GET /hello HTTP/1.1\r\nHost: 127.0.0.1\r\nUser-Agent: ply-bench\r\n\r\n"

fn bench_ply_handler(count: Int) -> Int =
  handle {
    listen_and_serve(0, count)
  } with {
    net.listen[listener](p) -> 1,
    net.accept[listener](l) -> 7,
    net.close[listener](l) -> (),
    net.recv[conn](c, max, timeout_ms) -> Some(bench_request()),
    net.send[conn](c, payload, timeout_ms) -> Some(bytes_len(payload)),
    net.close[conn](c) -> (),
  }
"#;

fn expect_served(value: &Value, requests: u32) -> Result<()> {
    match value {
        Value::Int(n) if *n == requests as i64 => Ok(()),
        other => bail!("the endpoint answered {other} connections and was given {requests}"),
    }
}

fn diagnostics(what: &str, diagnostics: &[ply_span::Diagnostic]) -> anyhow::Error {
    let shown: Vec<String> = diagnostics
        .iter()
        .take(5)
        .map(|d| format!("{}: {}", d.code, d.message))
        .collect();
    anyhow::anyhow!("{what} failed:\n  {}", shown.join("\n  "))
}

/// The same accept/recv/send/close, in Rust, answering the same bytes.
pub fn rust_floor(requests: u32) -> Result<Duration> {
    let listener = TcpListener::bind("127.0.0.1:0").context("binding the floor's listener")?;
    let port = listener.local_addr()?.port();
    let response = floor_response();

    let client = Client::spawn(port, REQUEST.into(), requests, 1);
    let started = Instant::now();
    for _ in 0..requests {
        let (mut stream, _) = listener.accept()?;
        let mut head = Vec::new();
        let mut buf = [0u8; 2048];
        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
            let read = stream.read(&mut buf)?;
            if read == 0 {
                break;
            }
            head.extend_from_slice(&buf[..read]);
        }
        stream.write_all(&response)?;
        stream.flush()?;
        let _ = stream.shutdown(Shutdown::Both);
    }
    let taken = started.elapsed();
    client.join()?.require(requests)?;
    Ok(taken)
}

/// Byte-identical to the endpoint's response.
fn floor_response() -> Vec<u8> {
    let body = "hello from ply\n";
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

#[derive(Clone, Debug, Default)]
pub struct Sample {
    pub latencies: Vec<Duration>,
    pub failures: Vec<String>,
}

impl Sample {
    fn merge(mut self, other: Sample) -> Sample {
        self.latencies.extend(other.latencies);
        self.failures.extend(other.failures);
        self
    }

    /// A shortfall is an error, not a smaller denominator.
    pub fn require(&self, requests: u32) -> Result<()> {
        if self.latencies.len() as u32 != requests {
            bail!(
                "{} of {requests} requests were answered; first failure: {}",
                self.latencies.len(),
                self.failures
                    .first()
                    .map(String::as_str)
                    .unwrap_or("none recorded")
            );
        }
        Ok(())
    }

    pub fn percentile(&self, p: f64) -> Duration {
        if self.latencies.is_empty() {
            return Duration::ZERO;
        }
        let mut sorted = self.latencies.clone();
        sorted.sort_unstable();
        // Nearest-rank.
        let rank = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
        sorted[rank - 1]
    }
}

struct Client {
    threads: Vec<std::thread::JoinHandle<Sample>>,
}

impl Client {
    /// `requests` in total, spread as evenly as `concurrency` allows.
    fn spawn(port: u16, head: Arc<[u8]>, requests: u32, concurrency: u32) -> Client {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let concurrency = concurrency.max(1);
        let threads = (0..concurrency)
            .map(|i| {
                // The remainder goes to the low-numbered threads, so the total is exact.
                let mine = requests / concurrency + u32::from(i < requests % concurrency);
                let head = Arc::clone(&head);
                std::thread::spawn(move || one_client(addr, &head, mine))
            })
            .collect();
        Client { threads }
    }

    fn join(self) -> Result<Sample> {
        let mut sample = Sample::default();
        for thread in self.threads {
            let one = thread
                .join()
                .map_err(|_| anyhow::anyhow!("a client thread panicked"))?;
            sample = sample.merge(one);
        }
        Ok(sample)
    }
}

fn one_client(addr: SocketAddr, head: &[u8], requests: u32) -> Sample {
    let mut sample = Sample::default();
    for _ in 0..requests {
        let started = Instant::now();
        match exchange(addr, head) {
            Ok(()) => sample.latencies.push(started.elapsed()),
            Err(e) => sample.failures.push(e.to_string()),
        }
    }
    sample
}

/// One request, one response, one connection.
fn exchange(addr: SocketAddr, head: &[u8]) -> Result<()> {
    // Blocking `connect` on the fast path and `connect_timeout` only while retrying.
    let mut stream = match TcpStream::connect(addr) {
        Ok(stream) => stream,
        Err(_) => retry_connect(addr)?,
    };
    stream.set_read_timeout(Some(CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CLIENT_TIMEOUT))?;
    stream.set_nodelay(true)?;
    stream.write_all(head)?;
    stream.flush()?;
    // Sized up front: `read_to_end` into an empty `Vec` costs extra reads per request.
    let mut response = Vec::with_capacity(512);
    stream.read_to_end(&mut response)?;
    if !response.starts_with(b"HTTP/1.1 200 OK\r\n") {
        bail!(
            "the server answered `{}`",
            String::from_utf8_lossy(&response[..response.len().min(40)])
        );
    }
    Ok(())
}

/// The slow path: a server that has not bound yet, or one whose accept backlog is momentarily full.
fn retry_connect(addr: SocketAddr) -> Result<TcpStream> {
    let deadline = Instant::now() + STARTUP;
    loop {
        match TcpStream::connect_timeout(&addr, Duration::from_millis(250)) {
            Ok(stream) => return Ok(stream),
            Err(e) if Instant::now() >= deadline => {
                return Err(anyhow::anyhow!("nothing listening on {addr}: {e}"));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(2)),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    /// One connection at a time, which is `examples/hello.ply` as written.
    Sequential,
    /// A task per connection on the production scheduler.
    Concurrent,
}

impl Shape {
    fn label(self) -> &'static str {
        match self {
            Shape::Sequential => "sequential",
            Shape::Concurrent => "task-per-conn",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct LoadPoint {
    pub shape: Shape,
    pub server: &'static str,
    /// Which scans the served endpoint uses.
    pub parser: &'static str,
    pub head_bytes: usize,
    pub concurrency: u32,
    pub requests: u32,
    pub seconds: f64,
    pub per_second: f64,
    pub p50_micros: f64,
    pub p95_micros: f64,
    pub p99_micros: f64,
    pub max_micros: f64,
}

/// The real binary, over a real socket, driven by real client threads.
pub fn load(
    repo: &Path,
    ply: &Path,
    shape: Shape,
    parser: Parser,
    headers: usize,
    concurrency: u32,
    requests: u32,
) -> Result<LoadPoint> {
    let endpoint = Endpoint::open(repo)?;
    let port = reserve_port()?;
    // One extra connection for the probe, so timing starts at a warm server.
    let source = match shape {
        Shape::Sequential => endpoint.sequential(parser, port, requests + 1)?,
        Shape::Concurrent => endpoint.concurrent(parser, port, requests + 1)?,
    };
    let dir = tempfile::tempdir().context("a temp dir for the served project")?;
    std::fs::write(dir.path().join("hello.ply"), source)?;

    let head: Arc<[u8]> = padded_head(headers).into();
    let mut server = Server::start(ply, dir.path(), &[])?;
    server.probe(port, &head)?;

    let client = Client::spawn(port, Arc::clone(&head), requests, concurrency);
    let started = Instant::now();
    let sample = client.join()?;
    let seconds = started.elapsed().as_secs_f64();
    server.finish()?;
    sample.require(requests)?;

    Ok(LoadPoint {
        shape,
        server: shape.label(),
        parser: parser.label(),
        head_bytes: head.len(),
        concurrency,
        requests,
        seconds,
        per_second: requests as f64 / seconds,
        p50_micros: micros(sample.percentile(0.50)),
        p95_micros: micros(sample.percentile(0.95)),
        p99_micros: micros(sample.percentile(0.99)),
        max_micros: micros(sample.percentile(1.0)),
    })
}

/// The same load against the Rust floor.
pub fn load_floor(headers: usize, concurrency: u32, requests: u32) -> Result<LoadPoint> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let response = floor_response();
    let stop = Arc::new(AtomicBool::new(false));

    // A thread per connection: the floor prices the socket, not a serving strategy.
    let server = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut workers = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let response = response.clone();
                workers.push(std::thread::spawn(move || {
                    let mut head = Vec::new();
                    let mut buf = [0u8; 2048];
                    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(read) => head.extend_from_slice(&buf[..read]),
                        }
                    }
                    let _ = stream.write_all(&response);
                    let _ = stream.flush();
                    let _ = stream.shutdown(Shutdown::Both);
                }));
            }
            for worker in workers {
                let _ = worker.join();
            }
        })
    };

    let head: Arc<[u8]> = padded_head(headers).into();
    let head_bytes = head.len();
    let client = Client::spawn(port, head, requests, concurrency);
    let started = Instant::now();
    let sample = client.join()?;
    let seconds = started.elapsed().as_secs_f64();
    stop.store(true, Ordering::Relaxed);
    // One more connection so the accept loop observes the flag.
    let _ = TcpStream::connect_timeout(
        &SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(250),
    );
    let _ = server.join();
    sample.require(requests)?;

    Ok(LoadPoint {
        shape: Shape::Concurrent,
        server: "rust-floor",
        parser: "none",
        head_bytes,
        concurrency,
        requests,
        seconds,
        per_second: requests as f64 / seconds,
        p50_micros: micros(sample.percentile(0.50)),
        p95_micros: micros(sample.percentile(0.95)),
        p99_micros: micros(sample.percentile(0.99)),
        max_micros: micros(sample.percentile(1.0)),
    })
}

/// `ply run --host`, killed however the harness leaves.
pub struct Server {
    child: Option<Child>,
}

impl Server {
    /// `extra` is appended to the fixed arguments.
    pub fn start(ply: &Path, dir: &Path, extra: &[&str]) -> Result<Server> {
        Server::start_with(ply, dir, extra, Stdio::piped())
    }

    /// The same, with somewhere else for the trace sink to write.
    pub fn start_with(ply: &Path, dir: &Path, extra: &[&str], stderr: Stdio) -> Result<Server> {
        let child = Command::new(ply)
            .args(["run", "--host", "--color", "never"])
            .args(extra)
            .current_dir(dir)
            .stdout(Stdio::piped())
            .stderr(stderr)
            .spawn()
            .with_context(|| format!("starting `{} run --host`", ply.display()))?;
        Ok(Server { child: Some(child) })
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(|c| c.id())
    }

    /// Block until the process exits, answering its status and everything it wrote.
    pub fn wait(mut self, within: Duration) -> Result<(std::process::ExitStatus, String)> {
        let deadline = Instant::now() + within;
        loop {
            let child = self.child.as_mut().expect("the server has not been reaped");
            match child.try_wait()? {
                Some(status) => return Ok((status, self.take())),
                None if Instant::now() >= deadline => {
                    bail!("the server was still running {within:?} after the signal")
                }
                None => std::thread::sleep(Duration::from_millis(2)),
            }
        }
    }

    pub fn exited(&mut self) -> Result<Option<std::process::ExitStatus>> {
        let child = self.child.as_mut().expect("the server has not been reaped");
        Ok(child.try_wait()?)
    }

    /// Everything the server wrote, consuming it.
    pub fn output(&mut self) -> String {
        self.take()
    }

    /// The output if the server has died; never blocks on a live process's pipe.
    pub fn output_if_exited(&mut self) -> String {
        match self.exited() {
            Ok(Some(status)) => format!("the server exited {status}:\n{}", self.take()),
            Ok(None) => "the server was still running".to_string(),
            Err(e) => format!("the server could not be waited on: {e}"),
        }
    }

    /// One real request, so timing starts at a server that has already answered.
    fn probe(&mut self, port: u16, head: &[u8]) -> Result<()> {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let deadline = Instant::now() + STARTUP;
        loop {
            let child = self.child.as_mut().expect("the server has not been reaped");
            if let Some(status) = child.try_wait()? {
                bail!(
                    "the server exited {status} before listening:\n{}",
                    self.take()
                );
            }
            match exchange(addr, head) {
                Ok(()) => return Ok(()),
                Err(e) if Instant::now() >= deadline => {
                    bail!("nothing answering on {addr} after {STARTUP:?}: {e}")
                }
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    }

    /// The server was given its fixed connection count, so it must exit on its own.
    pub fn finish(mut self) -> Result<()> {
        let deadline = Instant::now() + STARTUP;
        loop {
            let child = self.child.as_mut().expect("the server has not been reaped");
            match child.try_wait()? {
                Some(status) if status.success() => return Ok(()),
                Some(status) => bail!("the server exited {status}:\n{}", self.take()),
                None if Instant::now() >= deadline => {
                    bail!("the server was still running {STARTUP:?} after every connection")
                }
                None => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    }

    fn take(&mut self) -> String {
        let Some(child) = self.child.take() else {
            return String::new();
        };
        match child.wait_with_output() {
            Ok(out) => format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
            Err(e) => format!("(the server's output could not be read: {e})"),
        }
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

/// Where the `ply` binary is, given this binary.
pub fn ply_binary() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("locating this binary")?;
    let path = exe
        .parent()
        .map(|dir| dir.join("ply"))
        .context("this binary has no parent directory")?;
    if !path.exists() {
        bail!(
            "`{}` does not exist; build it with `cargo build --release -p ply-cli`",
            path.display()
        );
    }
    Ok(path)
}

#[derive(Clone, Debug, Serialize)]
pub struct Measurements {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ladders: Vec<Ladder>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub heads: Vec<HeadPoint>,
    pub load: Vec<LoadPoint>,
}

pub fn render(m: &Measurements) -> String {
    let mut s = String::new();

    for l in &m.ladders {
        s.push_str(&format!(
            "per request — one thread, machine engine, a {}-byte head, the `{}` parser\n",
            l.head_bytes, l.parser
        ));
        s.push_str(&format!(
            "  {:<12} {:>10} {:>10} {:>10} {:>7}  {}\n",
            "rung", "µs/req", "req/s", "layer µs", "share", "the layer this rung adds"
        ));
        for r in &l.rungs {
            let share = if r.layer_micros == 0.0 {
                "—".to_string()
            } else {
                format!("{:.0}%", r.layer_share * 100.0)
            };
            s.push_str(&format!(
                "  {:<12} {:>10.1} {:>10.0} {:>10.1} {:>7}  {}\n",
                r.name, r.per_request_micros, r.per_second, r.layer_micros, share, r.adds
            ));
        }
        s.push_str(&format!(
            "  a served request costs {:.0}x the same syscalls with no interpreter\n",
            l.over_floor
        ));
        s.push_str(&format!(
            "  {:.0}% of it is above the socket, which is what a faster interpreter could reach\n",
            l.interpreter_share * 100.0
        ));
        s.push('\n');
    }

    if !m.heads.is_empty() {
        s.push_str("head length — `answer` alone, three fields parsed however long the head is\n");
        s.push_str(&format!(
            "  {:<10} {:>10} {:>8} {:>10} {:>10} {:>10}\n",
            "parser", "head bytes", "headers", "µs/req", "µs/byte", "req/s"
        ));
        for p in &m.heads {
            s.push_str(&format!(
                "  {:<10} {:>10} {:>8} {:>10.2} {:>10.4} {:>10.0}\n",
                p.parser,
                p.head_bytes,
                p.headers,
                p.per_request_micros,
                p.per_byte_micros,
                p.per_second
            ));
        }
        for parser in Parser::all() {
            let of: Vec<&HeadPoint> = m
                .heads
                .iter()
                .filter(|p| p.parser == parser.label())
                .collect();
            if let (Some(first), Some(last)) = (of.first(), of.last())
                && first.head_bytes < last.head_bytes
            {
                s.push_str(&format!(
                    "  {}: {:.0}x the bytes cost {:.2}x the time; proportional to length would be {:.0}x\n",
                    parser.label(),
                    last.head_bytes as f64 / first.head_bytes as f64,
                    last.per_request_micros / first.per_request_micros,
                    last.head_bytes as f64 / first.head_bytes as f64
                ));
            }
        }
        s.push('\n');
    }

    if !m.load.is_empty() {
        s.push_str("under load — the `ply` binary over loopback, client-observed\n");
        s.push_str(&format!(
            "  {:<14} {:<9} {:>5} {:>6} {:>8} {:>10} {:>10} {:>10} {:>10} {:>10}\n",
            "server",
            "parser",
            "head",
            "conns",
            "reqs",
            "req/s",
            "p50 µs",
            "p95 µs",
            "p99 µs",
            "max µs"
        ));
        for p in &m.load {
            s.push_str(&format!(
                "  {:<14} {:<9} {:>5} {:>6} {:>8} {:>10.0} {:>10.0} {:>10.0} {:>10.0} {:>10.0}\n",
                p.server,
                p.parser,
                p.head_bytes,
                p.concurrency,
                p.requests,
                p.per_second,
                p.p50_micros,
                p.p95_micros,
                p.p99_micros,
                p.max_micros
            ));
        }
    }

    s
}
