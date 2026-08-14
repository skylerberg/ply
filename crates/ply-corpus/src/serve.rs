//! What one request costs, and which layer it was spent in.
//!
//! W6 decides whether M9 comes forward, and the argument that deferred M9 was
//! that execution is a few percent of a warm test run. Serving inverts that
//! argument, so the number it turns on is per-request time with the layers
//! separated: a total says nothing about whether a faster interpreter would
//! move it.
//!
//! The separation is by **substitution rather than by instrumentation**. Every
//! rung runs the same endpoint — `examples/hello.ply`, verbatim — and changes
//! only what is underneath it:
//!
//! | rung | what is under the endpoint | what the step above it adds |
//! | --- | --- | --- |
//! | `answer` | nothing; a pure call | the HTTP parse and the response build |
//! | `ply-handler` | a `handle` written in Ply | performing an effect and dispatching a clause |
//! | `host-sim` | the `SimNet` host handler | the host boundary: resolve, footprint check, decode |
//! | `host-tcp` | the `TcpHost` host handler | the socket, the reactor and the blocking pool |
//! | `rust-floor` | no Ply at all | (the denominator: the same syscalls with no interpreter) |
//!
//! Each rung is the one below it plus exactly one layer, so a difference is that
//! layer's cost and nothing has to be trusted about a timer placed inside the
//! machine. `rust-floor` is what the same accept/recv/send/close costs a Rust
//! program driven by the same client, which is what says whether the socket
//! numbers above it are the socket or the boundary around it.
//!
//! Under load the client is a separate matter: [`load`] drives the real `ply`
//! binary over loopback and reports what a client observed, which is the only
//! number that is not an argument about where time went.

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

/// The request every rung answers. One packet, a complete head, no body — the
/// shape `curl` produces and the shape the endpoint is fastest on, so nothing
/// here is inflated by a pathological input.
pub const REQUEST: &[u8] =
    b"GET /hello HTTP/1.1\r\nHost: 127.0.0.1\r\nUser-Agent: ply-bench\r\n\r\n";

/// How long a client waits for a response before calling the server hung. W1
/// has no cancellation, so a hang is a real outcome and has to be reported as
/// one rather than waited on forever.
const CLIENT_TIMEOUT: Duration = Duration::from_secs(20);

/// How long a server has to typecheck its program and bind a port.
const STARTUP: Duration = Duration::from_secs(60);

fn micros(d: Duration) -> f64 {
    d.as_secs_f64() * 1e6
}

// ------------------------------------------------------------------ endpoint

/// `examples/hello.ply`, and the two definitions a measurement has to choose.
///
/// The example is read rather than copied so that what is measured is the
/// endpoint W1 shipped. The substitutions assert they found what they were
/// replacing: a silent miss would leave a server listening on the example's own
/// port and a benchmark reporting somebody else's numbers.
pub struct Endpoint {
    source: String,
    /// The example with everything from the simulated-socket section onwards
    /// removed. The tests there are `det` and handle `net` themselves, so they
    /// are noise in a project a benchmark is about to typecheck repeatedly —
    /// and the concurrent variant cannot keep them at all, because spawning
    /// puts `task.write` in `listen_and_serve`'s row and `E0412` is then
    /// correct.
    server_only: String,
}

/// Where the endpoint's simulated twin begins. Everything above it is the
/// program a request goes through.
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
    pub fn benchable(&self) -> String {
        format!("{}{PLY_HANDLER_DRIVER}", self.server_only)
    }

    /// The example with a chosen port and connection count, tests included.
    pub fn sequential(&self, port: u16, connections: u32) -> Result<String> {
        settings(&self.source, port, connections)
    }

    /// The same endpoint with a task spawned per connection.
    ///
    /// Only `serve` changes. `serve_one`, the parser and the response builder
    /// are the same definitions the sequential program runs, so the difference
    /// between the two is the scheduler and nothing else.
    pub fn concurrent(&self, port: u16, connections: u32) -> Result<String> {
        const OLD: &str = "\
fn serve(server: Int, count: Int) -> Int / {net.write[listener], net.write[conn]} =
  if count <= 0 {
    0
  } else {
    serve_one(net.accept[listener](server));
    1 + serve(server, count - 1)
  }";
        // Accept, spawn, and go straight back to accepting: the joins unwind at
        // the end, so up to `count` handlers are in flight at once and the
        // accept loop is never waiting on one of them.
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
        let source = replace(&self.server_only, OLD, NEW)?;
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
///
/// Bind, read the number, drop. The window between the drop and the server's
/// own bind is a race with every other process on the machine; it is also the
/// only option, because `ply run` takes no program arguments, so a port has to
/// be in the source before the process starts.
pub fn reserve_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context("reserving an ephemeral port")?;
    Ok(listener.local_addr()?.port())
}

// -------------------------------------------------------------------- rungs

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
    /// That layer as a share of the top rung, which is what decides where a
    /// speedup would have to come from.
    pub layer_share: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Ladder {
    pub head_bytes: usize,
    pub rungs: Vec<Rung>,
    /// `host-tcp` over `rust-floor`: how many times a Ply request costs what
    /// the same syscalls cost with no interpreter under them.
    pub over_floor: f64,
    /// Everything below `host-tcp` that is not the socket — the share of a
    /// request a faster interpreter could address.
    pub interpreter_share: f64,
}

/// The in-process ladder. One thread, the machine engine, no CLI, no child
/// process, and the same endpoint under all five.
pub fn ladder(repo: &Path, requests: u32, repeats: usize) -> Result<Ladder> {
    let endpoint = Endpoint::open(repo)?;
    let dir = tempfile::tempdir().context("a temp dir for the benchmark project")?;
    std::fs::write(dir.path().join("hello.ply"), endpoint.benchable())?;
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
        // `rust-floor` is not a rung above `host-tcp`; it is the denominator, so
        // it contributes no layer.
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
        head_bytes: REQUEST.len(),
        over_floor: tcp.as_secs_f64() / floor.as_secs_f64(),
        interpreter_share: (total - socket_layer) / total,
        rungs,
    })
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

// ------------------------------------------------------- the in-process rungs

/// A checked endpoint, ready to be called with whatever is underneath it.
struct Program {
    program: ply_syntax::ast::Program,
    resolved: ply_syntax::resolve::Resolved,
    check: CheckOutput,
}

impl Program {
    fn load(root: &Path) -> Result<Program> {
        let path = root.join("hello.ply");
        let text = std::fs::read_to_string(&path)?;
        let mut sources = ply_span::SourceMap::new();
        let id = sources.add(&path, text.clone());
        let name = ModuleName::from_relative_path(Path::new("hello.ply"))
            .map_err(|d| anyhow::anyhow!("{}", d.message))?;
        let program = ply_syntax::parse_program([(id, name, text.as_str())])
            .map_err(|d| diagnostics("parsing the endpoint", &d))?;
        let resolved = ply_syntax::resolve::resolve(&program)
            .map_err(|d| diagnostics("resolving the endpoint", &d))?;
        let check = ply_core::check_program(&program, &resolved)
            .map_err(|d| diagnostics("checking the endpoint", &d))?;
        Ok(Program {
            program,
            resolved,
            check,
        })
    }

    fn machine(&self) -> Machine<'_> {
        Machine::new(&self.program, &self.resolved, &self.check)
    }

    fn footprint(&self, name: &str) -> Option<Footprint> {
        self.check
            .defs
            .values()
            .find(|d| d.simple_name.as_str() == name)
            .map(|d| d.footprint.clone())
    }

    /// `Machine::call` takes the program-wide name, so a simple one has to be
    /// looked up rather than guessed at from the file name.
    fn full(&self, simple: &str) -> Result<String> {
        self.check
            .defs
            .values()
            .find(|d| d.simple_name.as_str() == simple)
            .map(|d| d.name.to_string())
            .with_context(|| format!("the endpoint declares no `{simple}`"))
    }

    /// Rung 1. `answer(head)` — no effect performed, so nothing but the parse
    /// and the response build.
    fn answer_only(&self, requests: u32) -> Result<Duration> {
        let name = self.full("answer")?;
        let mut machine = self.machine();
        let head = Value::bytes(REQUEST);
        let started = Instant::now();
        for _ in 0..requests {
            machine
                .call(&name, vec![head.clone()], Span::DUMMY)
                .map_err(|d| anyhow::anyhow!("`answer` raised: {}", d.message))?;
        }
        Ok(started.elapsed())
    }

    /// Rung 2. The same accept loop, answered by a `handle` written in Ply.
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

    /// Rung 3. The same connection through the host boundary, served by the
    /// simulated twin. Everything rung 4 has except the syscalls.
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

    /// Rung 4. A real listener, a real client, real syscalls — in this process,
    /// so nothing here is the CLI's start-up or a child's scheduling.
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

        let client = Client::spawn(port, requests, 1);
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
///
/// It calls `listen_and_serve` — the same entry point, the same connection
/// count and therefore the same operation sequence the two host rungs perform —
/// so the only thing that changes between this rung and `host-sim` is what
/// answers `net.*`. That is precisely ADR 0008 §5's twin comparison, priced.
///
/// The clauses do as little as a clause can: one `recv` returns the whole head,
/// so `read_head` stops after it exactly as it does against a socket that
/// delivered the request in one packet. A scripted queue in a cell would put a
/// `match`, two cell operations and a list allocation inside the layer being
/// measured, and the rung would read as the host boundary being cheap rather
/// than as an in-memory handler being dear.
const PLY_HANDLER_DRIVER: &str = r#"

fn bench_request() -> Bytes = b"GET /hello HTTP/1.1\r\nHost: 127.0.0.1\r\nUser-Agent: ply-bench\r\n\r\n"

fn bench_ply_handler(count: Int) -> Int =
  handle {
    listen_and_serve(0, count)
  } with {
    net.listen[listener](p) -> 1,
    net.accept[listener](l) -> 7,
    net.close[listener](l) -> (),
    net.recv[conn](c, max) -> bench_request(),
    net.send[conn](c, payload) -> bytes_len(payload),
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

// -------------------------------------------------------------- the rust floor

/// The same accept/recv/send/close, in Rust, answering the same bytes.
///
/// This is the denominator the interpreter numbers mean nothing without: it is
/// what the syscalls cost on this machine with this client and no Ply at all.
/// Deliberately as naive as the endpoint — one connection at a time, one read,
/// one write, close — because a floor that used a smarter I/O strategy would be
/// measuring the strategy.
fn rust_floor(requests: u32) -> Result<Duration> {
    let listener = TcpListener::bind("127.0.0.1:0").context("binding the floor's listener")?;
    let port = listener.local_addr()?.port();
    let response = floor_response();

    let client = Client::spawn(port, requests, 1);
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

/// Byte-identical to what the endpoint answers, so the two are writing the same
/// number of bytes to the same kind of socket.
fn floor_response() -> Vec<u8> {
    let body = "hello from ply\n";
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

// -------------------------------------------------------------------- clients

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

    /// A benchmark over a server that answered half the requests is not a
    /// benchmark, so a shortfall is an error rather than a smaller denominator.
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
        // Nearest-rank. With a few thousand samples the interpolated form
        // differs in the third digit and needs a tie-break rule that is one more
        // thing to get wrong.
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
    ///
    /// The threads start before the server is listening, so each connects in a
    /// retry loop up to [`STARTUP`]: a benchmark that raced the bind would
    /// report a connection refused as a server defect.
    fn spawn(port: u16, requests: u32, concurrency: u32) -> Client {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let concurrency = concurrency.max(1);
        let threads = (0..concurrency)
            .map(|i| {
                // The remainder goes to the low-numbered threads, so the total
                // is exact whatever the two numbers are.
                let mine = requests / concurrency + u32::from(i < requests % concurrency);
                std::thread::spawn(move || one_client(addr, mine))
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

fn one_client(addr: SocketAddr, requests: u32) -> Sample {
    let mut sample = Sample::default();
    for _ in 0..requests {
        let started = Instant::now();
        match exchange(addr) {
            Ok(()) => sample.latencies.push(started.elapsed()),
            Err(e) => sample.failures.push(e.to_string()),
        }
    }
    sample
}

/// One request, one response, one connection. The endpoint answers `Connection:
/// close` and closes, so end of stream is the end of the response and there is
/// no framing to get wrong on this side.
fn exchange(addr: SocketAddr) -> Result<()> {
    // Blocking `connect` on the fast path and `connect_timeout` only while
    // retrying. They are not the same syscall sequence: `connect_timeout` sets
    // the socket non-blocking and polls it, which costs a measurable fraction of
    // a loopback request and would be charged to the server.
    let mut stream = match TcpStream::connect(addr) {
        Ok(stream) => stream,
        Err(_) => retry_connect(addr)?,
    };
    stream.set_read_timeout(Some(CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CLIENT_TIMEOUT))?;
    stream.set_nodelay(true)?;
    stream.write_all(REQUEST)?;
    stream.flush()?;
    // Sized up front: `read_to_end` over an empty `Vec` probes and grows, which
    // is three or four extra reads per request charged to a server that sent
    // one response.
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

/// The slow path: a server that has not bound yet, or one whose accept backlog
/// is momentarily full.
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

// ----------------------------------------------------------------- under load

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
///
/// A child process rather than an in-process machine, because this is the
/// number a user gets: it includes the CLI's load and typecheck (excluded from
/// the timed window, which starts at the first connection) and the process
/// boundary the reactor's threads actually live behind.
pub fn load(
    repo: &Path,
    ply: &Path,
    shape: Shape,
    concurrency: u32,
    requests: u32,
) -> Result<LoadPoint> {
    let endpoint = Endpoint::open(repo)?;
    let port = reserve_port()?;
    // One more connection than the load: the probe below spends it proving the
    // server is listening and answering, so the timed window starts at a warm
    // server rather than at a race with its typecheck.
    let source = match shape {
        Shape::Sequential => endpoint.sequential(port, requests + 1)?,
        Shape::Concurrent => endpoint.concurrent(port, requests + 1)?,
    };
    let dir = tempfile::tempdir().context("a temp dir for the served project")?;
    std::fs::write(dir.path().join("hello.ply"), source)?;

    let mut server = Server::start(ply, dir.path())?;
    server.probe(port)?;

    let client = Client::spawn(port, requests, concurrency);
    let started = Instant::now();
    let sample = client.join()?;
    let seconds = started.elapsed().as_secs_f64();
    server.finish()?;
    sample.require(requests)?;

    Ok(LoadPoint {
        shape,
        server: shape.label(),
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

/// The same load against the Rust floor, so a concurrency sweep has a shape to
/// be compared with rather than only a slope.
pub fn load_floor(concurrency: u32, requests: u32) -> Result<LoadPoint> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let response = floor_response();
    let stop = Arc::new(AtomicBool::new(false));

    // One accept loop, one thread per connection: the floor is allowed to be
    // concurrent, because what it prices is the socket and not a serving
    // strategy.
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

    let client = Client::spawn(port, requests, concurrency);
    let started = Instant::now();
    let sample = client.join()?;
    let seconds = started.elapsed().as_secs_f64();
    stop.store(true, Ordering::Relaxed);
    // One more connection so the accept loop observes the flag rather than
    // blocking on a peer that will never arrive.
    let _ = TcpStream::connect_timeout(
        &SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(250),
    );
    let _ = server.join();
    sample.require(requests)?;

    Ok(LoadPoint {
        shape: Shape::Concurrent,
        server: "rust-floor",
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
struct Server {
    child: Option<Child>,
}

impl Server {
    fn start(ply: &Path, dir: &Path) -> Result<Server> {
        let child = Command::new(ply)
            .args(["run", "--host", "--color", "never"])
            .current_dir(dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("starting `{} run --host`", ply.display()))?;
        Ok(Server { child: Some(child) })
    }

    /// One real request, so the timed window starts at a server that has
    /// already answered rather than at a race with its typecheck.
    ///
    /// A probe that only tested the bind would be cheaper and wrong twice: it
    /// would have to hold the port to test it, racing the server's own bind, and
    /// it would not have shown that a request comes back — which is the
    /// difference between a slow benchmark and one measuring a server that is
    /// about to fail every request.
    fn probe(&mut self, port: u16) -> Result<()> {
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
            match exchange(addr) {
                Ok(()) => return Ok(()),
                Err(e) if Instant::now() >= deadline => {
                    bail!("nothing answering on {addr} after {STARTUP:?}: {e}")
                }
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    }

    /// The server was asked for a fixed number of connections and has been given
    /// them, so it must return on its own. One that has to be killed has not
    /// demonstrated that the host operation returned into the program.
    fn finish(mut self) -> Result<()> {
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
///
/// Siblings in the same profile directory, so a `--release` measurement drives a
/// release server and a debug one does not silently measure a debug build.
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

// ------------------------------------------------------------------ reporting

#[derive(Clone, Debug, Serialize)]
pub struct Measurements {
    pub ladder: Option<Ladder>,
    pub load: Vec<LoadPoint>,
}

pub fn render(m: &Measurements) -> String {
    let mut s = String::new();

    if let Some(l) = &m.ladder {
        s.push_str(&format!(
            "per request — one thread, machine engine, a {}-byte head\n",
            l.head_bytes
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

    if !m.load.is_empty() {
        s.push_str("under load — the `ply` binary over loopback, client-observed\n");
        s.push_str(&format!(
            "  {:<14} {:>6} {:>8} {:>10} {:>10} {:>10} {:>10} {:>10}\n",
            "server", "conns", "reqs", "req/s", "p50 µs", "p95 µs", "p99 µs", "max µs"
        ));
        for p in &m.load {
            s.push_str(&format!(
                "  {:<14} {:>6} {:>8} {:>10.0} {:>10.0} {:>10.0} {:>10.0} {:>10.0}\n",
                p.server,
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

    /// The harness rewrites the example, and a rewrite that silently matched
    /// nothing would measure a server on the example's own port. Both variants
    /// are checked here rather than discovered as a hang.
    #[test]
    fn both_shapes_are_produced_from_the_example_and_typecheck() {
        let endpoint = Endpoint::open(&repo()).expect("the example is where it was");
        for source in [
            endpoint.sequential(19000, 3).unwrap(),
            endpoint.concurrent(19001, 3).unwrap(),
        ] {
            assert!(source.contains("fn connections() -> Int = 3"));
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("hello.ply"), &source).unwrap();
            Program::load(dir.path()).expect("the rewritten endpoint typechecks");
        }
    }

    /// The concurrent variant must differ from the sequential one in `serve` and
    /// in the three rows above it, and nowhere else: if it changed `serve_one`
    /// or the parser, the two would not be measuring one endpoint.
    #[test]
    fn the_concurrent_variant_changes_only_the_accept_loop() {
        let endpoint = Endpoint::open(&repo()).unwrap();
        let sequential = endpoint.sequential(19002, 1).unwrap();
        let concurrent = endpoint.concurrent(19002, 1).unwrap();
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

    /// Nearest-rank, and the tail is the number this milestone reports, so an
    /// off-by-one here is an off-by-one in the answer.
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

    /// A server that answered some of the requests is not a slower server, it is
    /// a different measurement, so the shortfall has to stop the run.
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

    /// The floor is the denominator every interpreter number is read against,
    /// so it has to actually serve.
    #[test]
    fn the_rust_floor_answers_every_request() {
        assert!(rust_floor(8).unwrap() > Duration::ZERO);
    }
}
