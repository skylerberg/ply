//! What one request costs, and which layer it was spent in.

use anyhow::{Context, Result, bail};
use ply_core::CheckOutput;
use ply_core::ty::Footprint;
use ply_eval::host::HostRuntime;
use ply_eval::{Machine, Value};
use ply_host::tcp::{Net, SimNet};
use ply_span::Span;
use ply_syntax::ast::ModuleName;
use serde::Serialize;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The request every rung answers.
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
    /// `bytes_index_of`, `bytes_scan` and `bytes_scan_until` — what W2 ships.
    Native,
    /// The `fold`-over-`range` scans W1 shipped.
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

/// The three scans, and what W1 wrote in their place.
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

/// `examples/hello.ply`, and the two definitions a measurement has to choose.
pub struct Endpoint {
    source: String,
    /// The example with everything from the simulated-socket section onwards removed.
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

    /// The example's whole source with the chosen scans in it, tests included.
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
        // Accept, spawn, and go straight back to accepting: the joins unwind at the end, so up to
        // `count` handlers are in flight at once and the accept loop is never waiting on one of
        // them.
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
    /// Requests per second one thread sustains at this rung.
    pub per_second: f64,
    /// What this rung costs above the previous one — the layer's own price.
    pub layer_micros: f64,
    /// That layer as a share of the top rung, which is what decides where a speedup would have to
    /// come from.
    pub layer_share: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Ladder {
    pub parser: &'static str,
    pub head_bytes: usize,
    pub rungs: Vec<Rung>,
    /// `host-tcp` over `rust-floor`: how many times a Ply request costs what the same syscalls cost
    /// with no interpreter under them.
    pub over_floor: f64,
    /// Everything below `host-tcp` that is not the socket — the share of a request a faster
    /// interpreter could address.
    pub interpreter_share: f64,
}

/// The in-process ladder.
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
        // `rust-floor` is not a rung above `host-tcp`; it is the denominator, so it contributes no
        // layer.
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

/// The exit criterion the byte builtins states: whether a request's cost is a function of how many bytes
/// the head is or of how many fields were parsed.
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
        // The endpoint refuses a head over `max_head`, and a sweep that measured the refusal path
        // would be measuring a different program.
        if head.len() > 2048 {
            break;
        }
        // The fold parser is two orders of magnitude slower on a long head, so a sweep that gave it
        // the native parser's request count would spend minutes proving something one tenth of them
        // says.
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
struct Program {
    program: ply_syntax::ast::Program,
    resolved: ply_syntax::resolve::Resolved,
    check: CheckOutput,
    /// One answer about this program's regions for every rung below, rather than one per rung's
    /// machine.
    region_kinds: ply_eval::region_kind::Kinds,
}

impl Program {
    fn load(root: &Path) -> Result<Program> {
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
        let mut program = ply_syntax::parse_program(inputs)
            .map_err(|d| diagnostics("parsing the endpoint", &d))?;
        // Before resolution, as the driver does: what resolution sees is ordinary definitions.
        let expanded = ply_derive::expand_program(&mut program);
        if !expanded.is_empty() {
            return Err(diagnostics("expanding a `derive`", &expanded));
        }
        let resolved = ply_syntax::resolve::resolve(&mut program)
            .map_err(|d| diagnostics("resolving the endpoint", &d))?;
        let check = ply_core::check_program(&program, &resolved)
            .map_err(|d| diagnostics("checking the endpoint", &d))?;
        Ok(Program {
            program,
            resolved,
            check,
            region_kinds: ply_eval::region_kind::Kinds::default(),
        })
    }

    fn machine(&self) -> Machine<'_> {
        let mut machine = Machine::new(&self.program, &self.resolved, &self.check);
        machine.share_region_kinds(ply_eval::region_kind::Kinds::clone(&self.region_kinds));
        machine
    }

    fn footprint(&self, name: &str) -> Option<Footprint> {
        self.check
            .defs
            .values()
            .find(|d| d.simple_name.as_str() == name)
            .map(|d| d.footprint.clone())
    }

    /// `Machine::call` takes the program-wide name, so a simple one has to be looked up rather than
    /// guessed at from the file name.
    fn full(&self, simple: &str) -> Result<String> {
        self.check
            .defs
            .values()
            .find(|d| d.simple_name.as_str() == simple)
            .map(|d| d.name.to_string())
            .with_context(|| format!("the endpoint declares no `{simple}`"))
    }

    /// Rung 1.
    fn answer_only(&self, requests: u32) -> Result<Duration> {
        self.answer_over(REQUEST, requests)
    }

    /// The same rung over a chosen head, which is what the length sweep varies.
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

    /// Rung 2.
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

    /// Rung 3.
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

    /// Rung 4.
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
fn rust_floor(requests: u32) -> Result<Duration> {
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

/// Byte-identical to what the endpoint answers, so the two are writing the same number of bytes to
/// the same kind of socket.
fn floor_response() -> Vec<u8> {
    let body = "hello from ply\n";
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// Every request one client thread completed, with the latency of each.
#[derive(Clone, Debug, Default)]
struct Sample {
    latencies: Vec<Duration>,
    failures: Vec<String>,
}

impl Sample {
    fn merge(mut self, other: Sample) -> Sample {
        self.latencies.extend(other.latencies);
        self.failures.extend(other.failures);
        self
    }

    /// A benchmark over a server that answered half the requests is not a benchmark, so a shortfall
    /// is an error rather than a smaller denominator.
    fn require(&self, requests: u32) -> Result<()> {
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

    fn percentile(&self, p: f64) -> Duration {
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

/// Client threads, running until they have made their requests.
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
                // The remainder goes to the low-numbered threads, so the total is exact whatever
                // the two numbers are.
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
    // Sized up front: `read_to_end` over an empty `Vec` probes and grows, which is three or four
    // extra reads per request charged to a server that sent one response.
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
    /// What the client sent.
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
    // One more connection than the load: the probe below spends it proving the server is listening
    // and answering, so the timed window starts at a warm server rather than at a race with its
    // typecheck.
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

/// The same load against the Rust floor, so a concurrency sweep has a shape to be compared with
/// rather than only a slope.
pub fn load_floor(headers: usize, concurrency: u32, requests: u32) -> Result<LoadPoint> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let response = floor_response();
    let stop = Arc::new(AtomicBool::new(false));

    // One accept loop, one thread per connection: the floor is allowed to be concurrent, because
    // what it prices is the socket and not a serving strategy.
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
    // One more connection so the accept loop observes the flag rather than blocking on a peer that
    // will never arrive.
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
    /// `extra` is appended to the fixed arguments — `--tls NAME=CERT,KEY` and nothing else so far.
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

    /// The process id, so a harness can deliver a signal to it.
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

    /// The status if the server has already exited, which is what a probe loop checks before
    /// waiting again on a process that is gone.
    pub fn exited(&mut self) -> Result<Option<std::process::ExitStatus>> {
        let child = self.child.as_mut().expect("the server has not been reaped");
        Ok(child.try_wait()?)
    }

    /// Everything the server wrote, consuming it.
    pub fn output(&mut self) -> String {
        self.take()
    }

    /// The server's output when it has died, and a note when it is still up — so a failure message
    /// never blocks on a pipe belonging to a live process.
    pub fn output_if_exited(&mut self) -> String {
        match self.exited() {
            Ok(Some(status)) => format!("the server exited {status}:\n{}", self.take()),
            Ok(None) => "the server was still running".to_string(),
            Err(e) => format!("the server could not be waited on: {e}"),
        }
    }

    /// One real request, so the timed window starts at a server that has already answered rather
    /// than at a race with its typecheck.
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

    /// The server was asked for a fixed number of connections and has been given them, so it must
    /// return on its own.
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
    /// One per parser measured, so the byte builtins' before and after sit on one table taken on
    /// one machine in one run.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// The example's code with its `//` comments removed, so that a claim about which builtins a
    /// variant *calls* is not answered by prose describing them.
    fn code_only(source: &str) -> String {
        let mut out = String::with_capacity(source.len());
        for line in source.lines() {
            let bytes = line.as_bytes();
            let mut in_string = false;
            let mut cut = line.len();
            let mut i = 0;
            while i < bytes.len() {
                match bytes[i] {
                    b'\\' if in_string => i += 1,
                    b'"' => in_string = !in_string,
                    b'/' if !in_string && bytes.get(i + 1) == Some(&b'/') => {
                        cut = i;
                        break;
                    }
                    _ => {}
                }
                i += 1;
            }
            out.push_str(&line[..cut]);
            out.push('\n');
        }
        out
    }

    /// The harness rewrites the example, and a rewrite that silently matched nothing would measure
    /// a server on the example's own port.
    #[test]
    fn the_w1_reconstruction_replaces_every_scan_and_nothing_else() {
        let endpoint = Endpoint::open(&repo()).unwrap();
        let native = endpoint.benchable(Parser::Native).unwrap();
        let folds = endpoint.benchable(Parser::W1Folds).unwrap();
        let folds_code = code_only(&folds);
        for builtin in [
            "bytes_scan",
            "bytes_index_of",
            "bytes_starts_with",
            "bytes_ends_with",
            "bytes_split",
            "bytes_position",
        ] {
            assert!(
                !folds_code.contains(builtin),
                "`{builtin}` survived the rewrite"
            );
        }
        assert!(
            code_only(&native).contains("bytes_scan"),
            "the native variant stopped calling the builtins it is measuring"
        );
        for shared in [
            "fn parse(head: Bytes) -> Parsed {",
            "fn request_line(line: Bytes) -> Parsed {",
            "fn answer(head: Bytes) -> Bytes =",
            "fn response(status: String, body: String) -> Bytes =",
        ] {
            assert!(native.contains(shared), "{shared}");
            assert!(folds.contains(shared), "{shared}");
        }

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hello.ply"), &folds).unwrap();
        Program::load(dir.path()).expect("the reconstructed endpoint typechecks");
    }

    #[test]
    fn both_shapes_are_produced_from_the_example_and_typecheck() {
        let endpoint = Endpoint::open(&repo()).expect("the example is where it was");
        // Both parsers, because the load table serves the reconstruction too: W2's claim about
        // requests per second needs a before taken the same way as its after, not a before quoted
        // from a milestone ago.
        for parser in Parser::all() {
            for source in [
                endpoint.sequential(parser, 19000, 3).unwrap(),
                endpoint.concurrent(parser, 19001, 3).unwrap(),
            ] {
                assert!(source.contains("fn connections() -> Int = 3"));
                let dir = tempfile::tempdir().unwrap();
                std::fs::write(dir.path().join("hello.ply"), &source).unwrap();
                Program::load(dir.path()).expect("the rewritten endpoint typechecks");
            }
        }
    }

    /// The concurrent variant must differ from the sequential one in `serve` and in the three rows
    /// above it, and nowhere else: if it changed `serve_one` or the parser, the two would not be
    /// measuring one endpoint.
    #[test]
    fn the_concurrent_variant_changes_only_the_accept_loop() {
        let endpoint = Endpoint::open(&repo()).unwrap();
        let sequential = endpoint.sequential(Parser::Native, 19002, 1).unwrap();
        let concurrent = endpoint.concurrent(Parser::Native, 19002, 1).unwrap();
        assert!(concurrent.contains("task.spawn(|| serve_one(c))"));
        for shared in [
            "fn serve_one(c: Int) -> Unit / {net.write[conn]} {",
            "fn answer(head: Bytes) -> Bytes =",
            "fn head_end(head: Bytes) -> Int =",
        ] {
            assert!(sequential.contains(shared), "{shared}");
            assert!(concurrent.contains(shared), "{shared}");
        }
    }

    /// Nearest-rank, and the tail is the number this milestone reports, so an off-by-one here is an
    /// off-by-one in the answer.
    #[test]
    fn percentiles_are_nearest_rank_over_the_sample() {
        let sample = Sample {
            latencies: (1..=100).map(Duration::from_micros).collect(),
            failures: Vec::new(),
        };
        assert_eq!(sample.percentile(0.50), Duration::from_micros(50));
        assert_eq!(sample.percentile(0.95), Duration::from_micros(95));
        assert_eq!(sample.percentile(0.99), Duration::from_micros(99));
        assert_eq!(sample.percentile(1.0), Duration::from_micros(100));

        let one = Sample {
            latencies: vec![Duration::from_micros(7)],
            failures: Vec::new(),
        };
        assert_eq!(one.percentile(0.99), Duration::from_micros(7));
        assert_eq!(Sample::default().percentile(0.5), Duration::ZERO);
    }

    /// A server that answered some of the requests is not a slower server, it is a different
    /// measurement, so the shortfall has to stop the run.
    #[test]
    fn a_short_sample_is_an_error_rather_than_a_smaller_denominator() {
        let sample = Sample {
            latencies: vec![Duration::from_micros(1); 3],
            failures: vec!["connection reset".to_string()],
        };
        let err = sample.require(4).unwrap_err().to_string();
        assert!(err.contains("3 of 4"), "{err}");
        assert!(err.contains("connection reset"), "{err}");
        assert!(sample.require(3).is_ok());
    }

    /// The floor is the denominator every interpreter number is read against, so it has to actually
    /// serve.
    #[test]
    fn the_rust_floor_answers_every_request() {
        assert!(rust_floor(8).unwrap() > Duration::ZERO);
    }
}
