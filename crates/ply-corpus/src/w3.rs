//! What W3 costs: a multi-route service, real framing, keep-alive and TLS.
//!
//! `serve` priced one endpoint answering one fixed head with no body, no
//! routing and no keep-alive. W3 put four things on that path — HTTP/1.1
//! framing, a route table, connection reuse and TLS — and each of them costs
//! something. This module measures how much, and it measures the one property
//! W2 bought that full framing is most likely to have taken back: that a
//! request's cost is a function of the **fields** it parses rather than of the
//! **bytes** it receives.
//!
//! Seven sections, each answering one question:
//!
//! | section | question |
//! | --- | --- |
//! | [`routes`] | throughput and tail latency at several concurrencies, against W2's single endpoint taken on the same machine |
//! | [`stages`] | where one request's time goes, by substitution: the table, the match, the framing, and everything else |
//! | [`per_route`] | what each of the ten routes costs, so a mix has a decomposition rather than an average |
//! | [`shape`] | whether cost tracks fields or bytes, on three axes: header bytes at fixed field count, header count, and body bytes |
//! | [`keep_alive`] | throughput against requests per connection, and where connection reuse stops paying |
//! | [`tls`] | the same route over HTTP and HTTPS, handshake priced apart from steady state |
//! | [`aliases`] | that `/ {Desk}` and its expansion are one definition, byte for byte, over a whole real program |
//!
//! The load sections drive the real `ply` binary over loopback and report what
//! a client observed. The shape and per-route sections run in process over
//! [`SimNet`], because what they price is the parse, the route and the encode,
//! and a syscall in the middle of that is noise with a bigger variance than the
//! thing being measured.

use anyhow::{Context, Result, bail};
use ply_core::CheckOutput;
use ply_core::ty::Footprint;
use ply_eval::{Machine, Value};
use ply_hash::DefHash;
use ply_host::tcp::{Net, SimNet};
use ply_span::{Span, Symbol};
use ply_syntax::ast::ModuleName;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::serve::{Server, reserve_port};

/// Where `examples/desk.ply` stops being the service and starts being its
/// tests. Everything above it is what a request goes through.
const TESTS_MARKER: &str = "// --- Tests: the business, which needs no handler at all";

/// The credential name the TLS runs are configured under. It is a name in the
/// program and a `--tls NAME=CERT,KEY` beside the run; the two have to agree
/// and this constant is where they do.
const CREDENTIAL: &str = "desk";

/// `main`'s declared row in `examples/desk.ply`, and the same row once the
/// accept loop spawns.
const MAIN_ROW: &str =
    "fn main() -> Int / {Serving, config.read[server], net.write[conn], net.write[listener]} = {";
const MAIN_ROW_SPAWNING: &str = "fn main() -> Int / {Serving, config.read[server], task.write, net.write[conn], net.write[listener]} = {";

/// How long a client waits on one response.
const CLIENT_TIMEOUT: Duration = Duration::from_secs(20);

/// Connections one measured point may open.
///
/// A point that reuses nothing needs one connection per request, and the
/// ephemeral port range is about sixteen thousand wide with a `TIME_WAIT` of
/// tens of seconds on top. Beyond this the run stops measuring a server and
/// starts measuring the client's socket table, so the request count of a
/// low-reuse point is cut rather than its connection count.
const MAX_CONNECTIONS: u32 = 1000;

/// Connections per thread for a point, and the requests that implies.
pub fn share(concurrency: u32, per_conn: u32, requests_per_point: u32) -> u32 {
    let wanted = requests_per_point.div_ceil(concurrency * per_conn).max(1);
    wanted.min((MAX_CONNECTIONS / concurrency).max(1))
}

fn micros(d: Duration) -> f64 {
    d.as_secs_f64() * 1e6
}

// --- The service ------------------------------------------------------------

/// `examples/desk.ply`, split where its tests begin.
///
/// The example is read rather than copied, so what is measured is the service
/// W3 shipped. Every rewrite below asserts it found what it was replacing: a
/// silent miss would leave the harness measuring a program it guessed at.
pub struct Service {
    server_only: String,
}

/// Which accept loop the served program runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Variant {
    /// `examples/desk.ply` as written: one connection at a time.
    Sequential,
    /// A task per connection on the production scheduler. Only `serve` and the
    /// four rows above it change, so the difference between the two is the
    /// scheduler and nothing else.
    TaskPerConn,
}

impl Variant {
    pub fn label(self) -> &'static str {
        match self {
            Variant::Sequential => "sequential",
            Variant::TaskPerConn => "task-per-conn",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Http,
    Https,
}

impl Transport {
    pub fn label(self) -> &'static str {
        match self {
            Transport::Http => "http",
            Transport::Https => "https",
        }
    }
}

impl Service {
    pub fn open(repo: &Path) -> Result<Service> {
        let path = repo.join("examples/desk.ply");
        let source = std::fs::read_to_string(&path)
            .with_context(|| format!("reading `{}`", path.display()))?;
        let Some(cut) = source.find(TESTS_MARKER) else {
            bail!(
                "`{}` no longer contains `{TESTS_MARKER}`; this harness splits the service there \
                 and must be updated with it rather than measuring a program it guessed at",
                path.display()
            );
        };
        Ok(Service {
            server_only: source[..cut].to_string(),
        })
    }

    /// The service alone, without its tests.
    ///
    /// The tests are cut for both variants rather than only for the concurrent
    /// one, so that the two programs a load run typechecks are the same size.
    pub fn source(&self, variant: Variant) -> Result<String> {
        match variant {
            Variant::Sequential => Ok(self.server_only.clone()),
            Variant::TaskPerConn => self.task_per_connection(),
        }
    }

    /// The accept loop, rewritten to spawn.
    ///
    /// Accept, spawn, and go straight back to accepting: the joins unwind at
    /// the end, so every accepted connection is in flight at once and the
    /// accept loop never waits on one of them. `serve_connection`, the parser,
    /// the router and every endpoint are the same definitions the sequential
    /// program runs.
    fn task_per_connection(&self) -> Result<String> {
        const OLD_SERVE: &str = "\
pub fn serve(listener: Int, l: http::Limits, count: Int) -> Int
  / {Serving, net.write[conn], net.write[listener]} =
  if count <= 0 {
    0
  } else {
    let c = net.accept[listener](listener);
    if c == 0 {
      0
    } else {
      serve_connection(c, l);
      1 + serve(listener, l, count - 1)
    }
  }";
        const NEW_SERVE: &str = "\
pub fn serve(listener: Int, l: http::Limits, count: Int) -> Int
  / {Serving, task.write, net.write[conn], net.write[listener]} =
  if count <= 0 {
    0
  } else {
    let c = net.accept[listener](listener);
    if c == 0 {
      0
    } else {
      let t = task.spawn(|| serve_connection(c, l));
      let rest = serve(listener, l, count - 1);
      task.join(t);
      1 + rest
    }
  }";
        let source = replace(&self.server_only, OLD_SERVE, NEW_SERVE)?;
        // The rows above and below `serve`, each widened by the one atom
        // spawning adds. Written out rather than pattern-matched, because a row
        // this harness got wrong would be a program that fails to typecheck at
        // the start of a load run rather than a wrong number at the end of one.
        let widenings: [(&str, &str); 8] = [
            (
                "pub fn listen_and_serve(port: Int, count: Int) -> Int\n  / {Serving, net.write[conn], net.write[listener]} {",
                "pub fn listen_and_serve(port: Int, count: Int) -> Int\n  / {Serving, task.write, net.write[conn], net.write[listener]} {",
            ),
            (
                "pub fn listen_and_serve_tls(port: Int, credential: String, count: Int) -> Int\n  / {Serving, net.write[conn], net.write[listener]} {",
                "pub fn listen_and_serve_tls(port: Int, credential: String, count: Int) -> Int\n  / {Serving, task.write, net.write[conn], net.write[listener]} {",
            ),
            (
                "pub fn run(port: Int, count: Int) -> Int\n  / {Serving, net.write[conn], net.write[listener]} =",
                "pub fn run(port: Int, count: Int) -> Int\n  / {Serving, task.write, net.write[conn], net.write[listener]} =",
            ),
            (
                "pub fn run_tls(port: Int, credential: String, count: Int) -> Int\n  / {Serving, net.write[conn], net.write[listener]} =",
                "pub fn run_tls(port: Int, credential: String, count: Int) -> Int\n  / {Serving, task.write, net.write[conn], net.write[listener]} =",
            ),
            // The twin's entry points are the ones this harness drives, so they
            // are widened with the rest rather than left behind at the row the
            // sequential accept loop published.
            (
                "pub fn run_memory(port: Int, api: Option<Secret<String>>, count: Int) -> Int\n  / {net.write[conn], net.write[listener]} =",
                "pub fn run_memory(port: Int, api: Option<Secret<String>>, count: Int) -> Int\n  / {task.write, net.write[conn], net.write[listener]} =",
            ),
            (
                "pub fn run_memory_tls(port: Int, tls: String, api: Option<Secret<String>>, count: Int) -> Int\n  / {net.write[conn], net.write[listener]} =",
                "pub fn run_memory_tls(port: Int, tls: String, api: Option<Secret<String>>, count: Int) -> Int\n  / {task.write, net.write[conn], net.write[listener]} =",
            ),
            (
                "fn memory_serving(port: Int, tls: String, api: Option<Secret<String>>, count: Int) -> Int\n  / {net.write[conn], net.write[listener]} =",
                "fn memory_serving(port: Int, tls: String, api: Option<Secret<String>>, count: Int) -> Int\n  / {task.write, net.write[conn], net.write[listener]} =",
            ),
            (MAIN_ROW, MAIN_ROW_SPAWNING),
        ];
        widenings
            .iter()
            .try_fold(source, |acc, (from, to)| replace(&acc, from, to))
    }

    /// A project directory `ply run --host` can be pointed at.
    pub fn project(
        &self,
        dir: &Path,
        variant: Variant,
        transport: Transport,
        port: u16,
        connections: u32,
    ) -> Result<()> {
        // `desk.ply` declares its own `main`, so the entry point is rewritten in
        // place rather than written beside it: two modules declaring `main` is
        // `E0112` and a directory is a whole program.
        let source = self.source(variant)?;
        let header = match variant {
            Variant::Sequential => MAIN_ROW,
            Variant::TaskPerConn => MAIN_ROW_SPAWNING,
        };
        let spawning = match variant {
            Variant::Sequential => "",
            Variant::TaskPerConn => "task.write, ",
        };
        // The twin, not postgres. This measures the cost of HTTP — parse,
        // route, encode, write — and a database on the other side of it would
        // put a query planner into the number. `run_memory` discharges every
        // `db`, `trace`, `config` and `signal` atom against a value in a
        // region-scoped cell, so the run needs no `--db`, no `--config-schema`
        // and no credential — the entry point takes `None` and the desk refuses
        // the one route that asks for a key, which no measured call is. Over
        // TLS there is no twin entry point that takes a credential name, so
        // `run_memory_tls` is it.
        let call = match transport {
            Transport::Http => format!("run_memory({port}, None, {connections})"),
            Transport::Https => {
                format!("run_memory_tls({port}, \"{CREDENTIAL}\", None, {connections})")
            }
        };
        let source = replace_entry_point(
            &source,
            header,
            &format!(
                "fn main() -> Int / {{{spawning}net.write[conn], net.write[listener]}} =\n  {call}"
            ),
        )?;
        std::fs::write(dir.join("desk.ply"), source)?;
        Ok(())
    }

    /// Every `/ {Desk}` and `/ {Desk, ..}` row written out as the six atoms
    /// the set expands to.
    ///
    /// The point of comparison for [`aliases`]: two spellings of one service,
    /// which must be one program.
    pub fn explicit_rows(&self) -> Result<(String, usize)> {
        const EXPANSION: &str = "db.read[items], db.write[items], \
             db.read[orders], db.write[orders], db.write, \
             trace.write[orders], trace.write[items]";
        let mut out = String::with_capacity(self.server_only.len() + 4096);
        let mut rewritten = 0usize;
        let mut rest = self.server_only.as_str();
        // Only inside a row. The declaration `effect set Desk = {` is left
        // where it is: an unused set moves no hash, and removing it would make
        // this two edits rather than one.
        while let Some(at) = rest.find("/ {Desk") {
            let (before, from) = rest.split_at(at);
            out.push_str(before);
            let tail = &from["/ {Desk".len()..];
            if let Some(rest_of_row) = tail.strip_prefix('}') {
                out.push_str("/ {");
                out.push_str(EXPANSION);
                out.push('}');
                rest = rest_of_row;
            } else if let Some(rest_of_row) = tail.strip_prefix(',') {
                out.push_str("/ {");
                out.push_str(EXPANSION);
                out.push(',');
                rest = rest_of_row;
            } else {
                bail!(
                    "a row beginning `/ {{Desk` is neither `/ {{Desk}}` nor `/ {{Desk, ..`; this \
                     harness rewrites the service's rows and must be updated with it"
                );
            }
            rewritten += 1;
        }
        out.push_str(rest);
        if rewritten == 0 {
            bail!("`examples/desk.ply` no longer annotates anything with `/ {{Desk`");
        }
        Ok((out, rewritten))
    }
}

/// The whole of `main`, from its declaration to the `}` that closes it,
/// replaced by an entry point that drives the twin.
///
/// The body is bounded by the first `\n}\n` after the declaration rather than
/// written out here: `main`'s body carries prose about where configuration is
/// read, and a needle holding a paragraph would stop matching the first time
/// somebody reworded it — which is the failure this whole harness is written to
/// avoid.
fn replace_entry_point(source: &str, header: &str, to: &str) -> Result<String> {
    let at = source.find(header).with_context(|| {
        format!(
            "`examples/desk.ply` no longer contains:\n{header}\nthis harness rewrites its entry \
             point and must be updated with it rather than measuring a program it guessed at"
        )
    })?;
    let body = &source[at + header.len()..];
    let close = body
        .find("\n}\n")
        .context("`desk.ply`'s `main` has no closing brace at column zero")?;
    let mut out = String::with_capacity(source.len());
    out.push_str(&source[..at]);
    out.push_str(to);
    out.push_str(&body[close + "\n}".len()..]);
    Ok(out)
}

fn replace(source: &str, from: &str, to: &str) -> Result<String> {
    if !source.contains(from) {
        bail!(
            "`examples/desk.ply` no longer contains:\n{from}\n\
             this harness rewrites it and must be updated with it rather than measuring a program \
             it guessed at"
        );
    }
    Ok(source.replace(from, to))
}

// --- The in-process program -------------------------------------------------

/// A checked service, callable with whatever answers `net`.
pub struct Loaded {
    pub program: ply_syntax::ast::Program,
    pub resolved: ply_syntax::resolve::Resolved,
    pub check: CheckOutput,
}

impl Loaded {
    /// One `.ply` source as the module `desk`, plus the shipped stdlib.
    pub fn parse(desk: &str) -> Result<Loaded> {
        let mut sources = ply_span::SourceMap::new();
        let id = sources.add(Path::new("desk.ply"), desk.to_string());
        let name = ModuleName::from_relative_path(Path::new("desk.ply"))
            .map_err(|d| anyhow::anyhow!("{}", d.message))?;
        let mut inputs = vec![(id, name, desk)];
        let shipped: Vec<(ModuleName, &'static str)> = ply_std::sources()
            .map(|(module, source)| (ModuleName::from_dotted(module), source))
            .collect();
        for (module, source) in &shipped {
            let id = sources.add(ply_std::pseudo_path(module), source.to_string());
            inputs.push((id, module.clone(), source));
        }
        let mut program = ply_syntax::parse_program(inputs)
            .map_err(|d| diagnostics("parsing the service", &d))?;
        let expanded = ply_derive::expand_program(&mut program);
        if !expanded.is_empty() {
            return Err(diagnostics("expanding a `derive`", &expanded));
        }
        let resolved = ply_syntax::resolve::resolve(&program)
            .map_err(|d| diagnostics("resolving the service", &d))?;
        let check = ply_core::check_program(&program, &resolved)
            .map_err(|d| diagnostics("checking the service", &d))?;
        Ok(Loaded {
            program,
            resolved,
            check,
        })
    }

    pub fn full(&self, simple: &str) -> Result<String> {
        self.full_in("desk", simple)
    }

    /// `Machine::call` takes the program-wide name, and two modules may declare
    /// the same simple one — `desk::limits` and `std.http::default_limits` are
    /// one edit apart from colliding — so the module is named rather than
    /// guessed at from whichever definition sorts first.
    pub fn full_in(&self, module: &str, simple: &str) -> Result<String> {
        self.check
            .defs
            .values()
            .find(|d| d.simple_name.as_str() == simple && d.module.to_string() == module)
            .map(|d| d.name.to_string())
            .with_context(|| format!("`{module}` declares no `{simple}`"))
    }

    /// One pure call, timed.
    pub fn pure_call(&self, name: &str, args: Vec<Value>, calls: u32) -> Result<(Duration, Value)> {
        let mut machine = Machine::new(&self.program, &self.resolved, &self.check);
        let mut last = Value::Unit;
        let started = Instant::now();
        for _ in 0..calls {
            last = machine
                .call(name, args.clone(), Span::DUMMY)
                .map_err(|d| anyhow::anyhow!("`{name}` raised: {}", d.message))?;
        }
        Ok((started.elapsed(), last))
    }

    /// What the service wrote back on one scripted connection.
    ///
    /// Handles ascend from 1 and the listener takes the first, so the one
    /// connection this opens is 2.
    pub fn response_over_sim(&self, request: &[u8]) -> Result<Vec<u8>> {
        let sim = Arc::new(SimNet::new(vec![vec![request.to_vec()]]));
        let net: Arc<dyn Net> = Arc::clone(&sim) as Arc<dyn Net>;
        let binding = ply_host::tcp::registry(net)
            .bind(&self.check)
            .map_err(|d| diagnostics("binding the simulated network", &d))?;
        let name = self.full("run_memory")?;
        let mut machine = Machine::new(&self.program, &self.resolved, &self.check);
        machine.set_host_binding(Arc::new(binding));
        if let Some(declared) = self.footprint("run_memory") {
            machine.set_declared_footprint(declared);
        }
        machine
            .call(&name, twin_arguments(1), Span::DUMMY)
            .map_err(|d| anyhow::anyhow!("the service raised: {}", d.message))?;
        Ok(sim.sent(2))
    }

    pub fn footprint(&self, simple: &str) -> Option<Footprint> {
        self.check
            .defs
            .values()
            .find(|d| d.simple_name.as_str() == simple)
            .map(|d| d.footprint.clone())
    }

    /// `desk::run` over a scripted network: the whole service — read, frame,
    /// route, dispatch, encode, write — with no syscall in it.
    ///
    /// Each element of `script` is one connection, and each element of that is
    /// one chunk `recv` answers with. So a connection carrying three requests
    /// is three chunks, which is what a keep-alive measurement wants.
    pub fn over_sim(&self, script: Vec<Vec<Vec<u8>>>) -> Result<(Duration, usize)> {
        let connections = script.len();
        let net: Arc<dyn Net> = Arc::new(SimNet::new(script));
        let binding = ply_host::tcp::registry(net)
            .bind(&self.check)
            .map_err(|d| diagnostics("binding the simulated network", &d))?;
        let name = self.full("run_memory")?;
        let mut machine = Machine::new(&self.program, &self.resolved, &self.check);
        machine.set_host_binding(Arc::new(binding));
        if let Some(declared) = self.footprint("run_memory") {
            machine.set_declared_footprint(declared);
        }
        let started = Instant::now();
        let served = machine
            .call(&name, twin_arguments(connections as i64), Span::DUMMY)
            .map_err(|d| anyhow::anyhow!("the service raised: {}", d.message))?;
        let taken = started.elapsed();
        match served {
            Value::Int(n) if n == connections as i64 => Ok((taken, connections)),
            other => bail!("the service answered {other} connections and was given {connections}"),
        }
    }
}

/// `desk::run_memory(port, api, count)` for a simulated network: port `0`,
/// because `SimNet` answers whatever it is asked to listen on, and no API key,
/// because no measured request presents one.
fn twin_arguments(connections: i64) -> Vec<Value> {
    vec![
        Value::Int(0),
        Value::ctor("None", Vec::new()),
        Value::Int(connections),
    ]
}

fn diagnostics(what: &str, diagnostics: &[ply_span::Diagnostic]) -> anyhow::Error {
    let shown: Vec<String> = diagnostics
        .iter()
        .take(5)
        .map(|d| format!("{}: {}", d.code, d.message))
        .collect();
    anyhow::anyhow!("{what} failed:\n  {}", shown.join("\n  "))
}

// --- Requests ---------------------------------------------------------------

/// One request the harness sends, by the name a table prints for it.
///
/// Two spellings, because the last request on a connection carries
/// `Connection: close` and the others do not. That is what a client does, and
/// it is also what keeps a load run from running out of ephemeral ports: a
/// client that closes first holds every one of its ports in `TIME_WAIT` for
/// twice the segment lifetime, and a few thousand connections a second exhausts
/// the range in seconds. Asking the server to close moves that state to the
/// side of the connection that has one port rather than sixteen thousand.
#[derive(Clone, Debug)]
pub struct Call {
    pub name: &'static str,
    pub bytes: Arc<[u8]>,
    pub last: Arc<[u8]>,
}

/// A request head, with an optional body and optional padding.
///
/// `pad_value` grows one header's *value*, which adds bytes without adding a
/// field. `pad_fields` adds header lines, which adds both. The two axes apart
/// is the whole of [`shape`].
pub fn request(
    method: &str,
    target: &str,
    body: Option<&[u8]>,
    close: bool,
    pad_value: usize,
    pad_fields: usize,
) -> Vec<u8> {
    let mut head = format!("{method} {target} HTTP/1.1\r\nHost: 127.0.0.1\r\n");
    if close {
        head.push_str("Connection: close\r\n");
    }
    if pad_value > 0 {
        head.push_str("X-Pad: ");
        head.push_str(&"0123456789abcdef".repeat(pad_value.div_ceil(16))[..pad_value]);
        head.push_str("\r\n");
    }
    for i in 0..pad_fields {
        head.push_str(&format!(
            "X-Pad-{i:02}: 0123456789abcdef0123456789abcdef\r\n"
        ));
    }
    if let Some(body) = body {
        head.push_str("Content-Type: application/json\r\n");
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");
    let mut out = head.into_bytes();
    if let Some(body) = body {
        out.extend_from_slice(body);
    }
    out
}

fn get(target: &str) -> Vec<u8> {
    request("GET", target, None, false, 0, 0)
}

fn call(name: &'static str, target: &str) -> Call {
    Call {
        name,
        bytes: get(target).into(),
        last: request("GET", target, None, true, 0, 0).into(),
    }
}

/// A placement whose `customer` string is `pad` bytes long. One `bolt` at a
/// time, because the shelf holds five hundred and a body sweep that emptied it
/// would start measuring the refusal path halfway down the column.
pub fn placement(pad: usize) -> Vec<u8> {
    let customer = "a".repeat(pad.max(3));
    format!("{{\"customer\":\"{customer}\",\"lines\":[{{\"sku\":\"bolt\",\"qty\":1}}]}}")
        .into_bytes()
}

/// The read-only routes a mixed load cycles through.
///
/// `POST /orders` and `DELETE /orders/{id}` are left out on purpose and priced
/// in [`per_route`] instead: both move the store, so a sustained load over them
/// would empty the shelf and start measuring a 409 rather than an order.
pub fn read_mix() -> Vec<Call> {
    let paths: [(&'static str, &'static str); 8] = [
        ("health", "/health"),
        ("items", "/items"),
        ("featured", "/items/featured"),
        ("item", "/items/bolt"),
        ("orders", "/orders"),
        ("order", "/orders/1"),
        ("docs", "/docs/orders/placing"),
        ("receipt", "/orders/1/receipt"),
    ];
    paths.iter().map(|(name, path)| call(name, path)).collect()
}

// --- The client -------------------------------------------------------------

trait Stream: Read + Write + Send {}
impl<T: Read + Write + Send> Stream for T {}

/// One connection, and the leftover bytes of the last response read off it.
struct Conn {
    io: Box<dyn Stream>,
    buf: Vec<u8>,
    at: usize,
}

impl Conn {
    fn plain(addr: SocketAddr) -> Result<(Conn, Duration)> {
        let started = Instant::now();
        let socket = TcpStream::connect_timeout(&addr, CLIENT_TIMEOUT)?;
        let connected = started.elapsed();
        socket.set_read_timeout(Some(CLIENT_TIMEOUT))?;
        socket.set_write_timeout(Some(CLIENT_TIMEOUT))?;
        socket.set_nodelay(true)?;
        Ok((
            Conn {
                io: Box::new(socket),
                buf: Vec::with_capacity(8192),
                at: 0,
            },
            connected,
        ))
    }

    /// A TLS connection, with the handshake driven to completion here rather
    /// than left to the first write — which is what lets [`tls`] report it as
    /// its own number.
    fn tls(addr: SocketAddr, config: Arc<ClientConfig>) -> Result<(Conn, Duration, Duration)> {
        let started = Instant::now();
        let socket = TcpStream::connect_timeout(&addr, CLIENT_TIMEOUT)?;
        let connected = started.elapsed();
        socket.set_read_timeout(Some(CLIENT_TIMEOUT))?;
        socket.set_write_timeout(Some(CLIENT_TIMEOUT))?;
        socket.set_nodelay(true)?;
        let name = ServerName::try_from("localhost").context("`localhost` as a server name")?;
        let connection = ClientConnection::new(config, name).context("a rustls client")?;
        let mut stream = StreamOwned::new(connection, socket);
        let shook = Instant::now();
        while stream.conn.is_handshaking() {
            let (read, written) = stream.conn.complete_io(&mut stream.sock)?;
            if read == 0 && written == 0 {
                bail!("the TLS handshake stalled");
            }
        }
        let handshake = shook.elapsed();
        Ok((
            Conn {
                io: Box::new(stream),
                buf: Vec::with_capacity(8192),
                at: 0,
            },
            connected,
            handshake,
        ))
    }

    fn send(&mut self, request: &[u8]) -> Result<()> {
        self.io.write_all(request)?;
        self.io.flush()?;
        Ok(())
    }

    fn fill(&mut self) -> Result<usize> {
        let mut chunk = [0u8; 8192];
        let read = self.io.read(&mut chunk)?;
        self.buf.extend_from_slice(&chunk[..read]);
        Ok(read)
    }

    /// Read one whole response, framed the way the server framed it.
    ///
    /// A client that guessed at the framing would be a second implementation of
    /// the thing under measurement, so both forms the service produces are read
    /// here: `Content-Length` for every buffered route and chunked for the
    /// streamed receipt.
    fn response(&mut self) -> Result<u16> {
        let head_end = loop {
            if let Some(at) = find(&self.buf[self.at..], b"\r\n\r\n") {
                break self.at + at + 4;
            }
            if self.fill()? == 0 {
                bail!("the connection closed before a response head arrived");
            }
        };
        let head = String::from_utf8_lossy(&self.buf[self.at..head_end]).to_string();
        let status: u16 = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .with_context(|| format!("no status in `{}`", head.lines().next().unwrap_or("")))?;
        let mut length: Option<usize> = None;
        let mut chunked = false;
        for line in head.lines().skip(1) {
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            match name.trim().to_ascii_lowercase().as_str() {
                "content-length" => length = value.trim().parse().ok(),
                "transfer-encoding" if value.trim().eq_ignore_ascii_case("chunked") => {
                    chunked = true;
                }
                _ => {}
            }
        }
        self.at = head_end;
        if chunked {
            self.chunked_body()?;
        } else {
            let want = length.unwrap_or(0);
            while self.buf.len() - self.at < want {
                if self.fill()? == 0 {
                    bail!("the connection closed inside a {want}-byte body");
                }
            }
            self.at += want;
        }
        // The buffer is rewound rather than grown for the life of a connection
        // that may serve a hundred requests.
        if self.at == self.buf.len() {
            self.buf.clear();
            self.at = 0;
        }
        Ok(status)
    }

    fn chunked_body(&mut self) -> Result<()> {
        loop {
            let line_end = loop {
                if let Some(at) = find(&self.buf[self.at..], b"\r\n") {
                    break self.at + at;
                }
                if self.fill()? == 0 {
                    bail!("the connection closed inside a chunk size");
                }
            };
            let text = String::from_utf8_lossy(&self.buf[self.at..line_end]).to_string();
            let size = usize::from_str_radix(text.split(';').next().unwrap_or("").trim(), 16)
                .with_context(|| format!("a chunk size that is not hex: `{text}`"))?;
            let need = line_end + 2 + size + 2;
            while self.buf.len() < need {
                if self.fill()? == 0 {
                    bail!("the connection closed inside a {size}-byte chunk");
                }
            }
            self.at = need;
            if size == 0 {
                return Ok(());
            }
        }
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// What one client thread observed.
#[derive(Clone, Debug, Default)]
struct Sample {
    latencies: Vec<Duration>,
    connects: Vec<Duration>,
    handshakes: Vec<Duration>,
    statuses: BTreeMap<u16, u32>,
    failures: Vec<String>,
}

impl Sample {
    fn merge(mut self, other: Sample) -> Sample {
        self.latencies.extend(other.latencies);
        self.connects.extend(other.connects);
        self.handshakes.extend(other.handshakes);
        for (status, n) in other.statuses {
            *self.statuses.entry(status).or_default() += n;
        }
        self.failures.extend(other.failures);
        self
    }

    /// A run over a server that answered some of the requests is not a slower
    /// server, it is a different measurement.
    fn require(&self, requests: u32) -> Result<()> {
        if !self.failures.is_empty() || self.latencies.len() as u32 != requests {
            bail!(
                "{} of {requests} requests were answered ({} failures); first: {}",
                self.latencies.len(),
                self.failures.len(),
                self.failures
                    .first()
                    .map(String::as_str)
                    .unwrap_or("none recorded")
            );
        }
        let bad: Vec<String> = self
            .statuses
            .iter()
            .filter(|(status, _)| **status != 200)
            .map(|(status, n)| format!("{n}x {status}"))
            .collect();
        if !bad.is_empty() {
            bail!("the server answered {}", bad.join(", "));
        }
        Ok(())
    }

    fn percentile(of: &[Duration], p: f64) -> Duration {
        if of.is_empty() {
            return Duration::ZERO;
        }
        let mut sorted = of.to_vec();
        sorted.sort_unstable();
        let rank = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
        sorted[rank - 1]
    }
}

/// What the client threads should do.
#[derive(Clone)]
struct Plan {
    addr: SocketAddr,
    calls: Vec<Call>,
    /// Requests one connection carries before it is closed and another opened.
    per_conn: u32,
    /// Connections one thread opens.
    conns_per_thread: u32,
    tls: Option<Arc<ClientConfig>>,
}

impl Plan {
    fn requests(&self, threads: u32) -> u32 {
        threads * self.conns_per_thread * self.per_conn
    }

    fn connections(&self, threads: u32) -> u32 {
        threads * self.conns_per_thread
    }
}

fn drive(plan: &Plan, thread: u32) -> Sample {
    let mut sample = Sample::default();
    // Each thread starts at a different point in the mix, so that at
    // concurrency 8 over eight routes the server is not answering eight copies
    // of one request at a time.
    let mut next = thread as usize;
    for _ in 0..plan.conns_per_thread {
        let opened = match &plan.tls {
            None => Conn::plain(plan.addr).map(|(c, t)| (c, t, Duration::ZERO)),
            Some(config) => Conn::tls(plan.addr, Arc::clone(config)),
        };
        let (mut conn, connected, handshake) = match opened {
            Ok(triple) => triple,
            Err(e) => {
                sample.failures.push(format!("connecting: {e}"));
                return sample;
            }
        };
        sample.connects.push(connected);
        if plan.tls.is_some() {
            sample.handshakes.push(handshake);
        }
        for i in 0..plan.per_conn {
            let call = &plan.calls[next % plan.calls.len()];
            next += 1;
            let body = if i + 1 == plan.per_conn {
                &call.last
            } else {
                &call.bytes
            };
            let started = Instant::now();
            let answered = conn.send(body).and_then(|()| conn.response());
            match answered {
                Ok(status) => {
                    sample.latencies.push(started.elapsed());
                    *sample.statuses.entry(status).or_default() += 1;
                }
                Err(e) => {
                    sample.failures.push(format!("{}: {e}", call.name));
                    return sample;
                }
            }
        }
    }
    sample
}

fn run_plan(plan: &Plan, threads: u32) -> Result<(Sample, Duration)> {
    let started = Instant::now();
    let handles: Vec<_> = (0..threads)
        .map(|i| {
            let plan = plan.clone();
            std::thread::spawn(move || drive(&plan, i))
        })
        .collect();
    let mut sample = Sample::default();
    for handle in handles {
        sample = sample.merge(
            handle
                .join()
                .map_err(|_| anyhow::anyhow!("a client thread panicked"))?,
        );
    }
    Ok((sample, started.elapsed()))
}

// --- One measured point -----------------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct LoadPoint {
    pub variant: &'static str,
    pub transport: &'static str,
    pub label: String,
    pub concurrency: u32,
    /// Requests one connection carried. 1 is a fresh connection per request.
    pub per_conn: u32,
    pub connections: u32,
    pub requests: u32,
    pub seconds: f64,
    pub per_second: f64,
    pub p50_micros: f64,
    pub p95_micros: f64,
    pub p99_micros: f64,
    pub max_micros: f64,
    /// Connect and handshake, which a per-request latency does not contain.
    pub connect_p50_micros: f64,
    pub handshake_p50_micros: f64,
    pub handshake_p99_micros: f64,
}

/// A server, started once and driven through several points.
///
/// One `ply run --host` per point would spend more wall clock typechecking
/// `desk.ply` than measuring it, so the connection budget is the sum over the
/// points a table takes and the server is required to exit cleanly at the end —
/// which is also the check that it served exactly the connections it was given
/// and no more.
struct Bench {
    _dir: tempfile::TempDir,
    server: Server,
    port: u16,
    variant: Variant,
    transport: Transport,
    tls: Option<Arc<ClientConfig>>,
}

impl Bench {
    fn start(
        service: &Service,
        ply: &Path,
        variant: Variant,
        transport: Transport,
        connections: u32,
    ) -> Result<Bench> {
        let dir = tempfile::tempdir().context("a temp dir for the served project")?;
        let port = reserve_port()?;
        // One connection for the probe below, which proves the server is
        // answering before anything is timed.
        service.project(dir.path(), variant, transport, port, connections + 1)?;
        let (server, tls) = match transport {
            Transport::Http => (Server::start(ply, dir.path(), &[])?, None),
            Transport::Https => {
                let material = credential(dir.path())?;
                let arg = format!(
                    "{CREDENTIAL}={},{}",
                    material.certificate.display(),
                    material.key.display()
                );
                let server = Server::start(ply, dir.path(), &["--tls", &arg])?;
                (server, Some(Arc::new(client_config(&material.der)?)))
            }
        };
        let mut bench = Bench {
            _dir: dir,
            server,
            port,
            variant,
            transport,
            tls,
        };
        bench.probe()?;
        Ok(bench)
    }

    fn addr(&self) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], self.port))
    }

    /// One real request over the real transport, so the first timed point does
    /// not race the server's typecheck.
    fn probe(&mut self) -> Result<()> {
        let plan = Plan {
            addr: self.addr(),
            calls: vec![call("health", "/health")],
            per_conn: 1,
            conns_per_thread: 1,
            tls: self.tls.clone(),
        };
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            if let Some(status) = self.server.exited()? {
                bail!(
                    "the server exited {status} before listening:\n{}",
                    self.server.output()
                );
            }
            let sample = drive(&plan, 0);
            if sample.require(1).is_ok() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!(
                    "nothing answering on {} after two minutes: {}",
                    self.addr(),
                    sample
                        .failures
                        .first()
                        .map(String::as_str)
                        .unwrap_or("no failure recorded")
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn point(
        &mut self,
        label: impl Into<String>,
        calls: &[Call],
        concurrency: u32,
        per_conn: u32,
        conns_per_thread: u32,
    ) -> Result<LoadPoint> {
        let plan = Plan {
            addr: self.addr(),
            calls: calls.to_vec(),
            per_conn,
            conns_per_thread,
            tls: self.tls.clone(),
        };
        let requests = plan.requests(concurrency);
        let (sample, taken) = run_plan(&plan, concurrency)?;
        sample.require(requests).with_context(|| {
            format!(
                "at concurrency {concurrency}, {per_conn} requests per connection\n{}",
                self.server.output_if_exited()
            )
        })?;
        let seconds = taken.as_secs_f64();
        Ok(LoadPoint {
            variant: self.variant.label(),
            transport: self.transport.label(),
            label: label.into(),
            concurrency,
            per_conn,
            connections: plan.connections(concurrency),
            requests,
            seconds,
            per_second: requests as f64 / seconds,
            p50_micros: micros(Sample::percentile(&sample.latencies, 0.50)),
            p95_micros: micros(Sample::percentile(&sample.latencies, 0.95)),
            p99_micros: micros(Sample::percentile(&sample.latencies, 0.99)),
            max_micros: micros(Sample::percentile(&sample.latencies, 1.0)),
            connect_p50_micros: micros(Sample::percentile(&sample.connects, 0.50)),
            handshake_p50_micros: micros(Sample::percentile(&sample.handshakes, 0.50)),
            handshake_p99_micros: micros(Sample::percentile(&sample.handshakes, 0.99)),
        })
    }

    fn finish(self) -> Result<()> {
        self.server.finish()
    }
}

// --- The client, for a harness that starts its own server -------------------
//
// `Bench` above owns the project it serves, which W4 cannot use: its server is
// started with a `--db` and rewritten to a different `main`. These two are the
// halves of `Bench` that are about the *client*, exposed so there is one HTTP
// client in this crate rather than two.

/// Block until the server answers one real request over the real transport, so
/// the first timed point does not race its typecheck.
pub fn wait_until_serving(server: &mut Server, addr: SocketAddr) -> Result<()> {
    wait_until_serving_over(server, addr, None)
}

/// The same over whichever transport the server was started with.
pub fn wait_until_serving_over(
    server: &mut Server,
    addr: SocketAddr,
    tls: Option<Arc<ClientConfig>>,
) -> Result<()> {
    let plan = Plan {
        addr,
        calls: vec![call("health", "/health")],
        per_conn: 1,
        conns_per_thread: 1,
        tls,
    };
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if let Some(status) = server.exited()? {
            bail!(
                "the server exited {status} before listening:\n{}",
                server.output()
            );
        }
        if drive(&plan, 0).require(1).is_ok() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("nothing answering on {addr} after three minutes");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One measured point against a server the caller started.
#[allow(clippy::too_many_arguments)]
pub fn load_point(
    server: &mut Server,
    addr: SocketAddr,
    variant: &'static str,
    label: &'static str,
    path: &str,
    concurrency: u32,
    per_conn: u32,
    conns_per_thread: u32,
) -> Result<LoadPoint> {
    load_point_over(
        server,
        addr,
        None,
        variant,
        label,
        path,
        concurrency,
        per_conn,
        conns_per_thread,
    )
}

/// The same over whichever transport the server was started with.
#[allow(clippy::too_many_arguments)]
pub fn load_point_over(
    server: &mut Server,
    addr: SocketAddr,
    tls: Option<Arc<ClientConfig>>,
    variant: &'static str,
    label: &'static str,
    path: &str,
    concurrency: u32,
    per_conn: u32,
    conns_per_thread: u32,
) -> Result<LoadPoint> {
    let transport = if tls.is_some() {
        Transport::Https
    } else {
        Transport::Http
    };
    let plan = Plan {
        addr,
        calls: vec![call(label, path)],
        per_conn,
        conns_per_thread,
        tls,
    };
    let requests = plan.requests(concurrency);
    let (sample, taken) = run_plan(&plan, concurrency)?;
    sample.require(requests).with_context(|| {
        format!(
            "{label} at concurrency {concurrency}\n{}",
            server.output_if_exited()
        )
    })?;
    let seconds = taken.as_secs_f64();
    Ok(LoadPoint {
        variant,
        transport: transport.label(),
        label: label.to_string(),
        concurrency,
        per_conn,
        connections: plan.connections(concurrency),
        requests,
        seconds,
        per_second: requests as f64 / seconds,
        p50_micros: micros(Sample::percentile(&sample.latencies, 0.50)),
        p95_micros: micros(Sample::percentile(&sample.latencies, 0.95)),
        p99_micros: micros(Sample::percentile(&sample.latencies, 0.99)),
        max_micros: micros(Sample::percentile(&sample.latencies, 1.0)),
        connect_p50_micros: micros(Sample::percentile(&sample.connects, 0.50)),
        handshake_p50_micros: micros(Sample::percentile(&sample.handshakes, 0.50)),
        handshake_p99_micros: micros(Sample::percentile(&sample.handshakes, 0.99)),
    })
}

// --- TLS material -----------------------------------------------------------

pub struct Material {
    pub certificate: PathBuf,
    pub key: PathBuf,
    pub der: CertificateDer<'static>,
}

/// Generated per run rather than checked in. A private key in a repository is a
/// private key that leaks, and this one exists to exercise a handshake.
pub fn credential(dir: &Path) -> Result<Material> {
    let issued = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
        .context("generating a self-signed certificate")?;
    let certificate = dir.join("desk.pem");
    let key = dir.join("desk.key");
    std::fs::write(&certificate, issued.cert.pem())?;
    std::fs::write(&key, issued.signing_key.serialize_pem())?;
    Ok(Material {
        certificate,
        key,
        der: issued.cert.der().clone(),
    })
}

/// A client that trusts exactly the certificate this run generated, so the
/// handshake being measured is a real one rather than one with verification
/// switched off.
pub fn client_config(der: &CertificateDer<'static>) -> Result<ClientConfig> {
    let mut roots = RootCertStore::empty();
    roots.add(der.clone()).context("trusting the certificate")?;
    let mut config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .context("the provider supports both versions")?
            .with_root_certificates(roots)
            .with_no_client_auth();
    config.alpn_protocols = ply_host::tls::ALPN
        .iter()
        .map(|p| p.as_bytes().to_vec())
        .collect();
    Ok(config)
}

// --- Section 1: the multi-route service under load --------------------------

/// Throughput and tail latency for the mixed read load, at each concurrency.
///
/// Keep-alive is on and every connection carries `per_conn` requests, because
/// that is what a client does and because a connection per request would price
/// `connect` rather than the service.
pub fn routes(
    repo: &Path,
    ply: &Path,
    variant: Variant,
    concurrencies: &[u32],
    per_conn: u32,
    requests_per_point: u32,
) -> Result<Vec<LoadPoint>> {
    let service = Service::open(repo)?;
    let calls = read_mix();
    // Every point gets about the same number of requests, so a p99 at
    // concurrency 1 rests on as many samples as one at concurrency 64.
    let shares: Vec<(u32, u32)> = concurrencies
        .iter()
        .map(|&c| (c, share(c, per_conn, requests_per_point)))
        .collect();
    let budget: u32 = shares.iter().map(|(c, conns)| c * conns).sum();
    let mut bench = Bench::start(&service, ply, variant, Transport::Http, budget)?;
    let mut out = Vec::new();
    for (concurrency, conns_per_thread) in shares {
        out.push(bench.point("read mix", &calls, concurrency, per_conn, conns_per_thread)?);
    }
    bench.finish()?;
    Ok(out)
}

// --- Section 2: what each route costs ---------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct RoutePoint {
    pub route: String,
    pub request_bytes: usize,
    pub response_bytes: usize,
    pub requests: u32,
    pub per_request_micros: f64,
    pub per_second: f64,
}

/// Every route, one at a time, in process over a scripted network.
///
/// No syscall, so what this prices is the read loop, the framing, the route
/// match, the endpoint and the encode — which is what a mixed load's average is
/// made of and what an average hides.
pub fn per_route(repo: &Path, requests: u32, repeats: usize) -> Result<Vec<RoutePoint>> {
    let service = Service::open(repo)?;
    let loaded = Loaded::parse(&service.source(Variant::Sequential)?)?;

    // `POST /orders` draws one `bolt` off a shelf of five hundred, so its
    // request count is capped rather than shared with the read routes'.
    let writes = requests.min(400);
    let mut cases: Vec<(String, Vec<u8>, u32)> = read_mix()
        .into_iter()
        .map(|c| {
            (
                format!("GET {}", route_path(c.name)),
                c.bytes.to_vec(),
                requests,
            )
        })
        .collect();
    cases.push(("GET /nope (404)".to_string(), get("/nope"), requests));
    cases.push((
        "POST /orders".to_string(),
        request("POST", "/orders", Some(&placement(3)), false, 0, 0),
        writes,
    ));

    let mut out = Vec::new();
    for (route, bytes, requests) in cases {
        let response_bytes = loaded.response_over_sim(&bytes)?.len();
        let taken = best_of(repeats, || {
            let script: Vec<Vec<Vec<u8>>> = (0..requests).map(|_| vec![bytes.clone()]).collect();
            loaded.over_sim(script)
        })?;
        let per = micros(taken) / requests as f64;
        out.push(RoutePoint {
            route,
            request_bytes: bytes.len(),
            response_bytes,
            requests,
            per_request_micros: per,
            per_second: 1e6 / per,
        });
    }
    Ok(out)
}

// --- Where a request's time goes --------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct StagePoint {
    pub stage: &'static str,
    pub what: &'static str,
    pub per_request_micros: f64,
    /// This piece as a share of a whole request. The pieces overlap — a route
    /// match contains the table build — so the column does not sum to one and
    /// is not meant to.
    pub share: f64,
}

/// The pieces of one request, each priced on its own.
///
/// Every piece but the last is a **pure** call — `table`, `route_of`,
/// `parse_head`, `health` and `encode` all publish the empty row — so each is
/// timed by calling it, with nothing instrumented and no timer inside the
/// machine to trust. They are not a ladder: `route_of` contains `table`, and
/// `parse_head` is beside both rather than above them, so the shares overlap
/// and the table says so.
///
/// The reason this exists rather than an average: `table()` is an ordinary
/// function, so the route table is rebuilt from its ten pattern strings on
/// every request. Whether that is the bill or a rounding error is not a
/// question a mixed-load number can answer.
pub fn stages(repo: &Path, requests: u32, repeats: usize) -> Result<Vec<StagePoint>> {
    let service = Service::open(repo)?;
    let source = format!("{}{STAGE_DRIVER}", service.source(Variant::Sequential)?);
    let loaded = Loaded::parse(&source)?;
    let head = get("/items");

    let limits = loaded.full("limits")?;
    let (_, limits_value) = loaded.pure_call(&limits, Vec::new(), 1)?;
    let health = loaded.full("health")?;
    let (_, health_value) = loaded.pure_call(&health, Vec::new(), 1)?;
    let (_, method_value) = loaded.pure_call(&loaded.full("bench_method")?, Vec::new(), 1)?;
    let (_, version_value) = loaded.pure_call(&loaded.full("bench_version")?, Vec::new(), 1)?;

    let pieces: [(&'static str, &'static str, String, Vec<Value>); 5] = [
        (
            "table()",
            "building the ten-route table from its pattern strings",
            loaded.full("table")?,
            Vec::new(),
        ),
        (
            "route_of()",
            "that table, and matching one path against it",
            loaded.full("route_of")?,
            vec![method_value.clone(), Value::str("/items")],
        ),
        (
            "parse_head()",
            "framing one head: request line, fields, host, length",
            loaded.full_in("std.http", "parse_head")?,
            vec![Value::bytes(&head), limits_value],
        ),
        (
            "health()",
            "one endpoint: a record through a derived JSON encoder",
            health,
            Vec::new(),
        ),
        (
            "encode()",
            "the response bytes: status line, fields, framing, body",
            loaded.full_in("std.http", "encode")?,
            vec![method_value, version_value, Value::Bool(true), health_value],
        ),
    ];

    let whole = best_of(repeats, || {
        let script: Vec<Vec<Vec<u8>>> = (0..requests).map(|_| vec![head.clone()]).collect();
        loaded.over_sim(script)
    })?;
    let total = micros(whole) / requests as f64;

    let mut out = Vec::new();
    for (stage, what, name, args) in pieces {
        let mut best = f64::MAX;
        for _ in 0..repeats.max(1) {
            let (taken, _) = loaded.pure_call(&name, args.clone(), requests)?;
            best = best.min(micros(taken) / requests as f64);
        }
        out.push(StagePoint {
            stage,
            what,
            per_request_micros: best,
            share: best / total,
        });
    }
    out.push(StagePoint {
        stage: "whole request",
        what: "the read loop, the body, dispatch, the endpoint, encode and write",
        per_request_micros: total,
        share: 1.0,
    });
    Ok(out)
}

/// A constructor value the machine will match on has to come from the program:
/// a pattern resolves its constructor to a module-qualified name, so a value
/// synthesized in Rust from the bare one matches nothing. Two lines of Ply is
/// the whole fix, and it means these tables can never disagree with what the
/// service passes `encode` at run time.
const STAGE_DRIVER: &str = r#"

fn bench_method() -> http::Method = http::Get

fn bench_version() -> http::Version = http::Http11
"#;

fn route_path(name: &str) -> &'static str {
    match name {
        "health" => "/health",
        "items" => "/items",
        "featured" => "/items/featured",
        "item" => "/items/bolt",
        "orders" => "/orders",
        "order" => "/orders/1",
        "docs" => "/docs/orders/placing",
        _ => "/orders/1/receipt",
    }
}

fn best_of(repeats: usize, mut run: impl FnMut() -> Result<(Duration, usize)>) -> Result<Duration> {
    let mut best: Option<Duration> = None;
    for _ in 0..repeats.max(1) {
        let (taken, _) = run()?;
        if best.is_none_or(|b| taken < b) {
            best = Some(taken);
        }
    }
    Ok(best.expect("at least one attempt runs"))
}

// --- Section 3: fields or bytes ---------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct ShapePoint {
    /// Which axis is being grown.
    pub axis: &'static str,
    /// What the axis says about the request: how many header fields it carries
    /// and how many bytes it is.
    pub fields: usize,
    pub request_bytes: usize,
    pub requests: u32,
    pub per_request_micros: f64,
    pub per_byte_micros: f64,
    pub per_second: f64,
}

/// Three sweeps, and the only one that matters is the first.
///
/// - `head-bytes`: one header's *value* is grown. Every point parses the same
///   three fields, so a rising `µs/req` here would mean full framing put a
///   per-byte cost back on the request path — which is the finding this
///   section exists to look for.
/// - `head-fields`: header *lines* are added. Cost should rise, and roughly
///   linearly: parsing a field is work and there is no claim otherwise.
/// - `body-bytes`: a body the service frames and reads but does not decode,
///   because the target is a 404. A body must be crossed once; the claim is
///   that it is crossed a bounded number of times.
pub fn shape(repo: &Path, requests: u32, repeats: usize) -> Result<Vec<ShapePoint>> {
    let service = Service::open(repo)?;
    let loaded = Loaded::parse(&service.source(Variant::Sequential)?)?;
    let mut out = Vec::new();

    // `max_header_bytes` is 16384 and `max_header_count` is 64; both sweeps
    // stop below their bound, because measuring a refusal is measuring a
    // different program.
    for pad in [0usize, 64, 256, 1024, 4096, 12288] {
        let bytes = request("GET", "/items", None, false, pad, 0);
        out.push(point(
            &loaded,
            "head-bytes",
            if pad == 0 { 2 } else { 3 },
            &bytes,
            requests,
            repeats,
        )?);
    }
    for fields in [0usize, 2, 4, 8, 16, 32, 60] {
        let bytes = request("GET", "/items", None, false, 0, fields);
        out.push(point(
            &loaded,
            "head-fields",
            2 + fields,
            &bytes,
            requests,
            repeats,
        )?);
    }
    for pad in [0usize, 256, 1024, 4096, 16384, 61440] {
        let body = placement(pad);
        let bytes = request("POST", "/nope", Some(&body), false, 0, 0);
        out.push(point(&loaded, "body-bytes", 4, &bytes, requests, repeats)?);
    }
    Ok(out)
}

fn point(
    loaded: &Loaded,
    axis: &'static str,
    fields: usize,
    bytes: &[u8],
    requests: u32,
    repeats: usize,
) -> Result<ShapePoint> {
    let taken = best_of(repeats, || {
        let script: Vec<Vec<Vec<u8>>> = (0..requests).map(|_| vec![bytes.to_vec()]).collect();
        loaded.over_sim(script)
    })?;
    let per = micros(taken) / requests as f64;
    Ok(ShapePoint {
        axis,
        fields,
        request_bytes: bytes.len(),
        requests,
        per_request_micros: per,
        per_byte_micros: per / bytes.len() as f64,
        per_second: 1e6 / per,
    })
}

// --- Section 4: keep-alive --------------------------------------------------

/// The same total work, spread over fewer and fewer connections.
///
/// `max_keep_alive` is 100 in `desk.ply`'s own limits, so 100 is the top of the
/// sweep rather than a number chosen for the table.
pub fn keep_alive(
    repo: &Path,
    ply: &Path,
    variant: Variant,
    concurrency: u32,
    requests_per_point: u32,
) -> Result<Vec<LoadPoint>> {
    let service = Service::open(repo)?;
    let calls = read_mix();
    let ladder: [u32; 5] = [1, 2, 8, 32, 100];
    let shares: Vec<(u32, u32)> = ladder
        .iter()
        .map(|&per| (per, share(concurrency, per, requests_per_point)))
        .collect();
    let budget: u32 = shares.iter().map(|(_, conns)| concurrency * conns).sum();
    let mut bench = Bench::start(&service, ply, variant, Transport::Http, budget)?;
    let mut out = Vec::new();
    for (per_conn, conns_per_thread) in shares {
        out.push(bench.point(
            format!("{per_conn} req/conn"),
            &calls,
            concurrency,
            per_conn,
            conns_per_thread,
        )?);
    }
    bench.finish()?;
    Ok(out)
}

// --- Section 5: TLS ---------------------------------------------------------

/// One route over HTTP and over HTTPS, at three degrees of connection reuse.
///
/// A handshake is a fixed cost per connection, so its share of a request falls
/// as the connection carries more, and the difference between the two
/// transports at `1 req/conn` and at `32 req/conn` is the handshake and the
/// record layer respectively.
///
/// **Every rung is run at concurrency 1 as well, and that column is the one to
/// read the handshake off.** `desk.ply` serves one connection at a time and the
/// TLS handshake is completed lazily on the first `recv`, so a client that
/// connected while the server was busy measures the queue rather than the
/// cryptography. At concurrency 1 there is no queue.
pub fn tls(
    repo: &Path,
    ply: &Path,
    variant: Variant,
    concurrency: u32,
    requests_per_point: u32,
) -> Result<Vec<LoadPoint>> {
    let service = Service::open(repo)?;
    let calls = vec![call("items", "/items")];
    let ladder: [u32; 3] = [1, 8, 32];
    let concurrencies: Vec<u32> = if concurrency == 1 {
        vec![1]
    } else {
        vec![1, concurrency]
    };
    let shares: Vec<(u32, u32, u32)> = concurrencies
        .iter()
        .flat_map(|&c| {
            ladder
                .iter()
                .map(move |&per| (c, per, share(c, per, requests_per_point)))
        })
        .collect();
    let budget: u32 = shares.iter().map(|(c, _, conns)| c * conns).sum();
    let mut out = Vec::new();
    for transport in [Transport::Http, Transport::Https] {
        let mut bench = Bench::start(&service, ply, variant, transport, budget)?;
        for &(concurrency, per_conn, conns_per_thread) in &shares {
            out.push(bench.point(
                format!("{per_conn} req/conn"),
                &calls,
                concurrency,
                per_conn,
                conns_per_thread,
            )?);
        }
        bench.finish()?;
    }
    Ok(out)
}

// --- Section 6: an alias costs nothing --------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct AliasReport {
    /// Rows in `desk.ply` written `/ {Desk..}` and rewritten to the expansion.
    pub rows_rewritten: usize,
    /// Source bytes the two spellings differ by. The alias is shorter, and that
    /// is the whole of what it buys.
    pub source_bytes_aliased: usize,
    pub source_bytes_explicit: usize,
    pub definitions: usize,
    /// Definitions whose hash differs between the two spellings. Must be zero.
    pub hash_differences: usize,
    /// Definitions whose stored body bytes differ. Must be zero: a hash is
    /// `blake3` of exactly these bytes, so this is the stronger claim.
    pub body_differences: usize,
    pub stored_bytes_aliased: usize,
    pub stored_bytes_explicit: usize,
    /// Definitions whose published footprint differs, atom for atom.
    pub footprint_differences: usize,
    /// Definitions carrying a `Desk` in their `row_aliases` provenance. The
    /// spelling *is* recorded — as namespace metadata, which enters no hash.
    pub definitions_naming_the_set: usize,
    /// Definitions whose declared row is wider than what their body performs.
    /// ADR 0013 §1.6's cost, counted rather than worried about.
    pub declared_not_performed: Vec<String>,
}

/// The two spellings of one service, compared where it counts.
pub fn aliases(repo: &Path) -> Result<AliasReport> {
    let service = Service::open(repo)?;
    let aliased = service.source(Variant::Sequential)?;
    let (explicit, rows_rewritten) = service.explicit_rows()?;
    if aliased == explicit {
        bail!("the rewrite changed nothing, so the two spellings are one file");
    }

    let left = Loaded::parse(&aliased)?;
    let right = Loaded::parse(&explicit)?;
    let (left_hashes, left_bodies) =
        ply_hash::hash_program_with_bodies(&left.program, &left.resolved)
            .map_err(|d| diagnostics("hashing the aliased service", &d))?;
    let (right_hashes, right_bodies) =
        ply_hash::hash_program_with_bodies(&right.program, &right.resolved)
            .map_err(|d| diagnostics("hashing the explicit service", &d))?;

    let mut hash_differences = 0;
    let mut body_differences = 0;
    for (name, hash) in &left_hashes.defs {
        let other = right_hashes.defs.get(name).copied();
        if other != Some(*hash) {
            hash_differences += 1;
            continue;
        }
        let mine = left_bodies.get(*hash).map(|b| b.as_bytes().to_vec());
        let theirs = right_bodies.get(*hash).map(|b| b.as_bytes().to_vec());
        if mine != theirs {
            body_differences += 1;
        }
    }
    // A definition present on one side and not the other is a difference too,
    // and would otherwise be invisible to the loop above.
    hash_differences += right_hashes
        .defs
        .keys()
        .filter(|name| !left_hashes.defs.contains_key(*name))
        .count();

    let desk = Symbol::new("Desk");
    let mut footprint_differences = 0;
    let mut naming = 0;
    let mut declared_not_performed = Vec::new();
    for (name, info) in &left.check.defs {
        if info.row_aliases.contains(&desk) {
            naming += 1;
        }
        if info.module.to_string() == "desk" && info.footprint != info.performed {
            declared_not_performed.push(info.simple_name.to_string());
        }
        match right.check.defs.get(name) {
            Some(other) if other.footprint == info.footprint => {}
            _ => footprint_differences += 1,
        }
    }
    declared_not_performed.sort();

    Ok(AliasReport {
        rows_rewritten,
        source_bytes_aliased: aliased.len(),
        source_bytes_explicit: explicit.len(),
        definitions: left_hashes.defs.len(),
        hash_differences,
        body_differences,
        stored_bytes_aliased: stored_bytes(&left_bodies, &left_hashes),
        stored_bytes_explicit: stored_bytes(&right_bodies, &right_hashes),
        footprint_differences,
        definitions_naming_the_set: naming,
        declared_not_performed,
    })
}

fn stored_bytes(bodies: &ply_hash::body::BodySet, hashes: &ply_hash::HashOutput) -> usize {
    hashes
        .defs
        .values()
        .filter_map(|h: &DefHash| bodies.get(*h))
        .map(|b| b.len())
        .sum()
}

// --- Reporting --------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize)]
pub struct Measurements {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<LoadPoint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub w2_baseline: Vec<crate::serve::LoadPoint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stages: Vec<StagePoint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub per_route: Vec<RoutePoint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shape: Vec<ShapePoint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keep_alive: Vec<LoadPoint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tls: Vec<LoadPoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aliases: Option<AliasReport>,
}

fn load_table(title: &str, points: &[LoadPoint], out: &mut String) {
    if points.is_empty() {
        return;
    }
    out.push_str(title);
    out.push('\n');
    out.push_str(&format!(
        "  {:<14} {:<6} {:<12} {:>5} {:>8} {:>7} {:>9} {:>9} {:>9} {:>9} {:>9}\n",
        "variant",
        "wire",
        "shape",
        "conns",
        "reqs",
        "opened",
        "req/s",
        "p50 µs",
        "p95 µs",
        "p99 µs",
        "max µs"
    ));
    for p in points {
        out.push_str(&format!(
            "  {:<14} {:<6} {:<12} {:>5} {:>8} {:>7} {:>9.0} {:>9.0} {:>9.0} {:>9.0} {:>9.0}\n",
            p.variant,
            p.transport,
            p.label,
            p.concurrency,
            p.requests,
            p.connections,
            p.per_second,
            p.p50_micros,
            p.p95_micros,
            p.p99_micros,
            p.max_micros
        ));
    }
    out.push('\n');
}

pub fn render(m: &Measurements) -> String {
    let mut s = String::new();

    load_table(
        "the multi-route service under load — `ply run --host`, client-observed",
        &m.routes,
        &mut s,
    );

    if !m.w2_baseline.is_empty() {
        s.push_str("W2's single endpoint, taken on this machine for comparison\n");
        s.push_str(&format!(
            "  {:<14} {:>5} {:>8} {:>9} {:>9} {:>9} {:>9}\n",
            "server", "conns", "reqs", "req/s", "p50 µs", "p95 µs", "p99 µs"
        ));
        for p in &m.w2_baseline {
            s.push_str(&format!(
                "  {:<14} {:>5} {:>8} {:>9.0} {:>9.0} {:>9.0} {:>9.0}\n",
                p.server,
                p.concurrency,
                p.requests,
                p.per_second,
                p.p50_micros,
                p.p95_micros,
                p.p99_micros
            ));
        }
        s.push('\n');
    }

    if !m.stages.is_empty() {
        s.push_str(
            "where one request goes — `GET /items`, in process; the pieces overlap and do not sum\n",
        );
        s.push_str(&format!(
            "  {:<16} {:>10} {:>7}  {}\n",
            "piece", "µs/req", "of req", "what it is"
        ));
        for p in &m.stages {
            s.push_str(&format!(
                "  {:<16} {:>10.2} {:>6.0}%  {}\n",
                p.stage,
                p.per_request_micros,
                p.share * 100.0,
                p.what
            ));
        }
        s.push('\n');
    }

    if !m.per_route.is_empty() {
        s.push_str("per route — in process over a scripted network, no syscall\n");
        s.push_str(&format!(
            "  {:<20} {:>8} {:>9} {:>8} {:>10} {:>10}\n",
            "route", "req B", "resp B", "reqs", "µs/req", "req/s"
        ));
        for p in &m.per_route {
            s.push_str(&format!(
                "  {:<20} {:>8} {:>9} {:>8} {:>10.2} {:>10.0}\n",
                p.route,
                p.request_bytes,
                p.response_bytes,
                p.requests,
                p.per_request_micros,
                p.per_second
            ));
        }
        s.push('\n');
    }

    if !m.shape.is_empty() {
        s.push_str("fields or bytes — the same route, grown along three axes\n");
        s.push_str(&format!(
            "  {:<12} {:>7} {:>9} {:>8} {:>10} {:>10} {:>10}\n",
            "axis", "fields", "req bytes", "reqs", "µs/req", "µs/byte", "req/s"
        ));
        for p in &m.shape {
            s.push_str(&format!(
                "  {:<12} {:>7} {:>9} {:>8} {:>10.2} {:>10.4} {:>10.0}\n",
                p.axis,
                p.fields,
                p.request_bytes,
                p.requests,
                p.per_request_micros,
                p.per_byte_micros,
                p.per_second
            ));
        }
        for axis in ["head-bytes", "head-fields", "body-bytes"] {
            let of: Vec<&ShapePoint> = m.shape.iter().filter(|p| p.axis == axis).collect();
            if let (Some(first), Some(last)) = (of.first(), of.last())
                && first.request_bytes < last.request_bytes
            {
                s.push_str(&format!(
                    "  {axis}: {:.0}x the bytes and {:.1}x the fields cost {:.2}x the time\n",
                    last.request_bytes as f64 / first.request_bytes as f64,
                    last.fields as f64 / first.fields.max(1) as f64,
                    last.per_request_micros / first.per_request_micros,
                ));
            }
        }
        s.push('\n');
    }

    load_table(
        "keep-alive — the same work over fewer connections",
        &m.keep_alive,
        &mut s,
    );

    if !m.tls.is_empty() {
        load_table("TLS — one route, two transports", &m.tls, &mut s);
        s.push_str(
            "  connect and handshake, timed apart from the request — read the concurrency-1 rows:\n\
             \x20 the handshake completes on the server's first `recv`, so a client that connected\n\
             \x20 while the server was busy times the queue rather than the cryptography\n",
        );
        s.push_str(&format!(
            "  {:<6} {:<12} {:>5} {:>12} {:>12} {:>12}\n",
            "wire", "shape", "conns", "connect p50", "hshake p50", "hshake p99"
        ));
        for p in &m.tls {
            s.push_str(&format!(
                "  {:<6} {:<12} {:>5} {:>12.0} {:>12.0} {:>12.0}\n",
                p.transport,
                p.label,
                p.concurrency,
                p.connect_p50_micros,
                p.handshake_p50_micros,
                p.handshake_p99_micros
            ));
        }
        s.push('\n');
    }

    if let Some(a) = &m.aliases {
        s.push_str(
            "an alias costs nothing — `/ {Desk}` against its expansion, over the whole service\n",
        );
        s.push_str(&format!(
            "  {:<34} {:>12} {:>12}\n",
            "", "/ {Desk}", "expanded"
        ));
        s.push_str(&format!(
            "  {:<34} {:>12} {:>12}\n",
            "source bytes", a.source_bytes_aliased, a.source_bytes_explicit
        ));
        s.push_str(&format!(
            "  {:<34} {:>12} {:>12}\n",
            "stored definition bytes", a.stored_bytes_aliased, a.stored_bytes_explicit
        ));
        s.push_str(&format!(
            "  {:<34} {:>12} {:>12}\n",
            "definitions", a.definitions, a.definitions
        ));
        s.push_str(&format!(
            "  rows rewritten: {}   definitions naming the set: {}\n",
            a.rows_rewritten, a.definitions_naming_the_set
        ));
        s.push_str(&format!(
            "  differing definition hashes: {}   differing stored bodies: {}   differing footprints: {}\n",
            a.hash_differences, a.body_differences, a.footprint_differences
        ));
        s.push_str(&format!(
            "  declared but not performed: {}\n",
            if a.declared_not_performed.is_empty() {
                "none".to_string()
            } else {
                a.declared_not_performed.join(", ")
            }
        ));
    }

    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// Both variants have to be programs, and a rewrite that silently matched
    /// nothing would leave a load run measuring the sequential loop twice.
    #[test]
    fn both_variants_are_produced_from_the_example_and_typecheck() {
        let service = Service::open(&repo()).expect("the example is where it was");
        for variant in [Variant::Sequential, Variant::TaskPerConn] {
            let source = service.source(variant).unwrap();
            Loaded::parse(&source).expect("the rewritten service typechecks");
        }
        let concurrent = service.source(Variant::TaskPerConn).unwrap();
        assert!(concurrent.contains("task.spawn(|| serve_connection(c, l))"));
        // Everything below the accept loop is the same program.
        for shared in [
            "pub fn serve_connection(c: Int, l: http::Limits) -> Unit / {Serving, net.write[conn]} = {",
            "pub fn answer(req: http::Request) -> Reply / {Serving} =",
            "pub fn table() -> List<router::Route<Endpoint>> = [",
        ] {
            assert!(concurrent.contains(shared), "{shared}");
        }
    }

    /// The served project has to be a program too, and it has to reach the twin
    /// rather than postgres: a `main` still calling `run` would have a load run
    /// refuse to start for want of a `--db` two minutes into its probe loop.
    #[test]
    fn the_served_project_typechecks_and_drives_the_twin() {
        let service = Service::open(&repo()).unwrap();
        for variant in [Variant::Sequential, Variant::TaskPerConn] {
            for (transport, entry) in [
                (Transport::Http, "run_memory(8137, None, 9)"),
                (Transport::Https, "run_memory_tls(8137, \"desk\", None, 9)"),
            ] {
                let dir = tempfile::tempdir().unwrap();
                service
                    .project(dir.path(), variant, transport, 8137, 9)
                    .unwrap();
                let source = std::fs::read_to_string(dir.path().join("desk.ply")).unwrap();
                assert!(source.contains(entry), "{entry} in {variant:?}");
                assert!(!source.contains("config.get[server]"), "{variant:?}");
                Loaded::parse(&source).expect("the served project typechecks");
            }
        }
    }

    /// The expansion has to be a program too, and it has to have replaced every
    /// row rather than the first one.
    #[test]
    fn the_explicit_spelling_typechecks_and_names_no_set() {
        let service = Service::open(&repo()).unwrap();
        let (explicit, rewritten) = service.explicit_rows().unwrap();
        assert!(rewritten >= 8, "only {rewritten} rows were rewritten");
        assert!(!explicit.contains("/ {Desk"), "a `/ {{Desk` row survived");
        Loaded::parse(&explicit).expect("the expanded service typechecks");
    }

    /// The headline claim of section 6, as a test rather than only as a number
    /// in a table nobody re-runs.
    #[test]
    fn an_alias_and_its_expansion_are_one_program() {
        let report = aliases(&repo()).unwrap();
        assert_eq!(report.hash_differences, 0);
        assert_eq!(report.body_differences, 0);
        assert_eq!(report.footprint_differences, 0);
        assert_eq!(report.stored_bytes_aliased, report.stored_bytes_explicit);
        assert!(report.definitions_naming_the_set >= 8);
        assert!(report.source_bytes_explicit > report.source_bytes_aliased);
    }

    /// A response the client mis-frames is a client bug reported as a server
    /// number, so both framings the service produces are read here.
    #[test]
    fn the_client_reads_both_framings_the_service_produces() {
        let service = Service::open(&repo()).unwrap();
        let loaded = Loaded::parse(&service.source(Variant::Sequential).unwrap()).unwrap();
        // One connection carrying a buffered route and the streamed one, which
        // is also the pipelining case: the client must find the second response
        // beginning exactly where the first ended.
        let script = vec![vec![get("/items"), get("/orders/1/receipt")]];
        let (_, connections) = loaded.over_sim(script).unwrap();
        assert_eq!(connections, 1);
    }

    #[test]
    fn percentiles_are_nearest_rank() {
        let of: Vec<Duration> = (1..=100).map(Duration::from_micros).collect();
        assert_eq!(Sample::percentile(&of, 0.50), Duration::from_micros(50));
        assert_eq!(Sample::percentile(&of, 0.99), Duration::from_micros(99));
        assert_eq!(Sample::percentile(&of, 1.0), Duration::from_micros(100));
        assert_eq!(Sample::percentile(&[], 0.5), Duration::ZERO);
    }

    /// A padded head has to grow bytes without growing fields, or the first
    /// sweep is measuring the second one's axis.
    #[test]
    fn padding_a_value_adds_bytes_and_no_fields() {
        let small = request("GET", "/items", None, false, 0, 0);
        let big = request("GET", "/items", None, false, 4096, 0);
        assert!(big.len() > small.len() * 40);
        assert_eq!(count(&big, b"\r\n"), count(&small, b"\r\n") + 1);
        let fielded = request("GET", "/items", None, false, 0, 8);
        assert_eq!(count(&fielded, b"\r\n"), count(&small, b"\r\n") + 8);
    }

    fn count(hay: &[u8], needle: &[u8]) -> usize {
        hay.windows(needle.len()).filter(|w| *w == needle).count()
    }
}
