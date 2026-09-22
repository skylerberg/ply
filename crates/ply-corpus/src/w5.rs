//! What operating the service costs: a trace call, a drain, and a deploy.

use anyhow::{Context, Result, bail};
use ply_eval::host::HostRegistry;
use ply_eval::{Machine, Value};
use ply_host::trace::{Level, Record, Trace, sink};
use ply_span::{Diagnostic, Span};
use ply_ty::CheckOutput;
use ply_ty::ModuleName;
use ply_ty::ty::Footprint;
use serde::Serialize;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::serve::{Server, reserve_port};
use crate::w3;

/// The program [`events`] runs.
const BENCH: &str = include_str!("../ply/w5.ply");

/// The TLS credential name the served project uses, matching `w3`'s.
const CREDENTIAL: &str = "desk";

fn micros(d: Duration) -> f64 {
    d.as_secs_f64() * 1e6
}

fn diagnostics(what: &str, diagnostics: &[Diagnostic]) -> anyhow::Error {
    let shown: Vec<String> = diagnostics
        .iter()
        .take(5)
        .map(|d| format!("{}: {}", d.code, d.message))
        .collect();
    anyhow::anyhow!("{what} failed:\n  {}", shown.join("\n  "))
}

pub struct Program {
    check: CheckOutput,
    /// The port's whole answer, which the tier is built from.
    port: ply_ty::Front,
    sources: ply_span::SourceMap,
}

impl Program {
    fn machine(&self) -> Machine<'_> {
        crate::tier_machine(&self.port, &self.sources)
    }

    pub fn parse() -> Result<Program> {
        let path = "bench.ply";
        let mut sources = ply_span::SourceMap::new();
        let id = sources.add(Path::new(path), BENCH.to_string());
        let name = ModuleName::from_relative_path(Path::new(path))
            .map_err(|d| anyhow::anyhow!("{}", d.message))?;
        let mut inputs = vec![(id, name, BENCH)];
        let shipped: Vec<(ModuleName, &'static str)> = ply_std::sources()
            .map(|(module, source)| (ModuleName::from_dotted(module), source))
            .collect();
        for (module, source) in &shipped {
            let id = sources.add(ply_std::pseudo_path(module), source.to_string());
            inputs.push((id, module.clone(), source));
        }
        // In `inputs` order: a span names its module by position.
        let ordered: Vec<(String, String)> = inputs
            .iter()
            .map(|(_, m, s)| (m.to_string(), s.to_string()))
            .collect();
        let ids: Vec<ply_span::SourceId> = inputs.iter().map(|(id, _, _)| *id).collect();
        let port = ply_codegen::c::producer::checked_front(&ordered, &ids)
            .map_err(|e| anyhow::anyhow!("checking the bench program: {e}"))?;
        Ok(Program {
            check: port.check.clone(),
            port,
            sources,
        })
    }

    pub fn full(&self, simple: &str) -> Result<String> {
        self.check
            .defs
            .values()
            .find(|d| d.simple_name.as_str() == simple && d.module.to_string() == "bench")
            .map(|d| d.name.to_string())
            .with_context(|| format!("the bench program declares no `{simple}`"))
    }

    pub fn footprint(&self, simple: &str) -> Option<Footprint> {
        self.check
            .defs
            .values()
            .find(|d| d.simple_name.as_str() == simple && d.module.to_string() == "bench")
            .map(|d| d.footprint.clone())
    }

    /// One call over a hermetic machine with no host, as the `bare` and `twin` rungs run.
    pub fn call_pure(&self, simple: &str, n: i64) -> Result<(Duration, Value)> {
        let name = self.full(simple)?;
        let mut machine = self.machine();
        let started = Instant::now();
        let value = machine
            .call(&name, vec![Value::Int(n)], Span::DUMMY)
            .map_err(|d| anyhow::anyhow!("`{simple}` raised [{}]: {}", d.code, d.message))?;
        Ok((started.elapsed(), value))
    }

    /// One call with a real `trace` binding: a `perform` that leaves the program, and a sink.
    pub fn call_traced(
        &self,
        host: &ply_host::Host,
        simple: &str,
        n: i64,
    ) -> Result<(Duration, Value)> {
        let name = self.full(simple)?;
        let registry: HostRegistry = host.registry();
        let binding = registry
            .bind(&self.check)
            .map_err(|d| diagnostics("binding the trace sink", &d))?;
        let mut machine = self.machine();
        machine.set_host_binding(Arc::new(binding));
        machine.set_host_runtime(host.runtime());
        if let Some(declared) = self.footprint(simple) {
            machine.set_declared_footprint(declared);
        }
        let started = Instant::now();
        let value = machine
            .call(&name, vec![Value::Int(n)], Span::DUMMY)
            .map_err(|d| anyhow::anyhow!("`{simple}` raised [{}]: {}", d.code, d.message))?;
        Ok((started.elapsed(), value))
    }
}

/// `ply_host::trace::json`'s formatting, written somewhere other than stderr.
struct FileJson {
    level: Level,
    out: Mutex<std::io::BufWriter<std::fs::File>>,
    path: &'static str,
}

impl FileJson {
    fn to(path: &Path, label: &'static str, level: Level) -> Result<FileJson> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("opening `{}` for the sink", path.display()))?;
        Ok(FileJson {
            level,
            out: Mutex::new(std::io::BufWriter::new(file)),
            path: label,
        })
    }
}

impl sink::Sink for FileJson {
    fn path(&self) -> &'static str {
        self.path
    }

    fn destination(&self) -> &'static str {
        "a file"
    }

    fn wants(&self, level: Level) -> bool {
        level >= self.level
    }

    fn write(&self, record: &Record<'_>) {
        let mut line = String::with_capacity(160);
        sink::write_json(&mut line, record);
        line.push('\n');
        let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        let _ = out.write_all(line.as_bytes());
    }

    fn flush(&self) {
        let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        let _ = out.flush();
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct EventPoint {
    /// Which sink answered, or `bare` for the loop with the perform deleted.
    pub rung: &'static str,
    /// `none`, `event`, `span` (an `enter` and an `exit`) or `count`.
    pub operation: &'static str,
    pub operations: u32,
    pub per_operation_micros: f64,
    pub per_second: f64,
    /// Microseconds this rung adds over `bare` at the same operation.
    pub over_bare_micros: f64,
}

/// Which sink a rung installs, and what it is called in the table.
#[derive(Clone, Copy)]
pub enum Rung {
    /// The same loop with no perform in it.
    Bare,
    /// `--trace off`: the shipped `ply_host::trace::discard`.
    Discard,
    /// `--trace json --trace-level warn` over `Debug` events: the shipped filter refuses early.
    Filtered,
    /// `ply_host::trace::json`'s encoder, written to `/dev/null`.
    JsonNull,
    /// The same, written to a real file on this filesystem.
    JsonFile,
    /// `std.trace`'s collecting twin, in Ply, over a region-scoped cell.
    Twin,
}

impl Rung {
    pub fn label(self) -> &'static str {
        match self {
            Rung::Bare => "bare (no perform)",
            Rung::Discard => "discard",
            Rung::Filtered => "json, level-filtered",
            Rung::JsonNull => "json → /dev/null",
            Rung::JsonFile => "json → file",
            Rung::Twin => "twin (Ply)",
        }
    }
}

/// Every operation, under every sink, against the same loop.
pub fn events(
    iterations: u32,
    twin_iterations: u32,
    repeats: usize,
    dir: &Path,
) -> Result<Vec<EventPoint>> {
    let program = Program::parse()?;
    let mut out: Vec<EventPoint> = Vec::new();

    // Taken once per size: the fold, list and `Fields` map cost the same at every rung.
    let bare_big = time(&program, Rung::Bare, "bare", iterations, repeats, dir)?;
    let bare_small = time(&program, Rung::Bare, "bare", twin_iterations, repeats, dir)?;
    out.push(point(Rung::Bare, "none", iterations, bare_big, bare_big));

    for (rung, operation, entry) in [
        (Rung::Discard, "event", "events"),
        (Rung::Discard, "span", "spans"),
        (Rung::Discard, "count", "counters"),
        (Rung::Filtered, "event", "debug_events"),
        (Rung::JsonNull, "event", "events"),
        (Rung::JsonNull, "span", "spans"),
        (Rung::JsonFile, "event", "events"),
        (Rung::JsonFile, "span", "spans"),
    ] {
        let per = time(&program, rung, entry, iterations, repeats, dir)?;
        out.push(point(rung, operation, iterations, per, bare_big));
    }

    out.push(point(
        Rung::Bare,
        "none",
        twin_iterations,
        bare_small,
        bare_small,
    ));
    for (operation, entry) in [
        ("event", "twin_events"),
        ("span", "twin_spans"),
        ("count", "twin_counters"),
    ] {
        let per = time(&program, Rung::Twin, entry, twin_iterations, repeats, dir)?;
        out.push(point(
            Rung::Twin,
            operation,
            twin_iterations,
            per,
            bare_small,
        ));
    }
    Ok(out)
}

/// Microseconds per operation, over the fastest of `repeats` runs.
fn time(
    program: &Program,
    rung: Rung,
    entry: &str,
    iterations: u32,
    repeats: usize,
    dir: &Path,
) -> Result<f64> {
    // Every rung answers this, so a loop that did not run fails rather than looking fast.
    let n = iterations as i64;
    let expect = Value::Int(2 * n);
    let mut best = Duration::MAX;
    for _ in 0..repeats.max(1) {
        let (taken, answered) = match rung {
            Rung::Bare | Rung::Twin => program.call_pure(entry, n)?,
            _ => {
                let host = ply_host::Host::new().traced(sink_for(rung, dir)?);
                program.call_traced(&host, entry, n)?
            }
        };
        if answered != expect {
            bail!("`{entry}` answered {answered} rather than {expect}: the loop did not run");
        }
        best = best.min(taken);
    }
    Ok(micros(best) / iterations as f64)
}

fn point(rung: Rung, operation: &'static str, operations: u32, per: f64, floor: f64) -> EventPoint {
    EventPoint {
        rung: rung.label(),
        operation,
        operations,
        per_operation_micros: per,
        per_second: if per > 0.0 { 1e6 / per } else { 0.0 },
        over_bare_micros: per - floor,
    }
}

pub fn sink_for(rung: Rung, dir: &Path) -> Result<Arc<Trace>> {
    let s: Arc<dyn sink::Sink> = match rung {
        Rung::Discard => Arc::new(sink::Discard),
        Rung::Filtered => Arc::new(sink::Json::new(Level::Warn)),
        Rung::JsonNull => Arc::new(FileJson::to(
            Path::new("/dev/null"),
            "ply_host::trace::json → /dev/null",
            Level::Info,
        )?),
        Rung::JsonFile => Arc::new(FileJson::to(
            &dir.join("trace.jsonl"),
            "ply_host::trace::json → file",
            Level::Info,
        )?),
        Rung::Bare | Rung::Twin => bail!("`{}` installs no sink", rung.label()),
    };
    Ok(Arc::new(Trace::new(s)))
}

/// Which store, transport and sink a served point runs under.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Sinking {
    /// `--trace off` — `ply_host::trace::discard`, a listed handler and not an absence.
    Off,
    /// `--trace json`, with stderr on `/dev/null`: the encoder without the destination.
    JsonNull,
    /// `--trace json`, with stderr on a file the harness reads, so records are counted.
    JsonFile,
}

impl Sinking {
    pub fn label(self) -> &'static str {
        match self {
            Sinking::Off => "off (discard)",
            Sinking::JsonNull => "json → /dev/null",
            Sinking::JsonFile => "json → file",
        }
    }

    fn flag(self) -> &'static str {
        match self {
            Sinking::Off => "off",
            Sinking::JsonNull | Sinking::JsonFile => "json",
        }
    }
}

/// How much of the operable stack a point runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stack {
    /// `run_memory`: the twin behind the routes, no host database, no host sink.
    Twin,
    /// `run`: postgres behind the routes, over plaintext.
    Postgres,
    /// `run_tls`: the same, with the transport terminated by `ply_host::tls`.
    PostgresTls,
}

impl Stack {
    pub fn label(self) -> &'static str {
        match self {
            Stack::Twin => "twin, http",
            Stack::Postgres => "postgres, http",
            Stack::PostgresTls => "postgres, https",
        }
    }
}

/// `examples/desk.ply` as a project `ply run --host` can be pointed at.
fn project(dir: &Path, service: &str, stack: Stack) -> Result<()> {
    let source = match stack {
        Stack::Postgres => service.to_string(),
        Stack::PostgresTls => replace(
            service,
            "    run(port, count)",
            &format!("    run_tls(port, \"{CREDENTIAL}\", count)"),
        )?,
        Stack::Twin => {
            let from = w3::main_header(service)?;
            let narrowed = replace(service, from, &w3::twin_entry_row(from))?;
            replace(
                &narrowed,
                "    run(port, count)",
                "    run_memory(port, key, count)",
            )?
        }
    };
    std::fs::write(dir.join("desk.ply"), source)?;
    Ok(())
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

struct Serving {
    _dir: tempfile::TempDir,
    server: Server,
    addr: std::net::SocketAddr,
    /// Where the sink wrote, for the run that reads its records back.
    records: Option<PathBuf>,
    tls: Option<Arc<rustls::ClientConfig>>,
}

impl Serving {
    #[allow(clippy::too_many_arguments)]
    fn start(
        repo: &Path,
        ply: &Path,
        url: &str,
        stack: Stack,
        variant: w3::Variant,
        sinking: Sinking,
        connections: u32,
        api_key: &str,
    ) -> Result<Serving> {
        let service = w3::Service::open(repo)?.source(variant)?;
        let dir = tempfile::tempdir().context("a temp dir for the served project")?;
        let port = reserve_port()?;
        project(dir.path(), &service, stack)?;

        let port_set = format!("DESK_PORT={port}");
        let conns_set = format!("DESK_CONNECTIONS={connections}");
        let key_set = format!("DESK_API_KEY={api_key}");
        let mut args: Vec<String> = vec![
            "--config-schema".into(),
            "desk.config".into(),
            "--set".into(),
            port_set,
            "--set".into(),
            conns_set,
            "--set".into(),
            key_set,
            "--trace".into(),
            sinking.flag().into(),
            // Every record `desk.ply` writes is `Info` or above, so the sink admits all of them.
            "--trace-level".into(),
            "info".into(),
        ];
        let mut tls = None;
        if stack != Stack::Twin {
            args.push("--db".into());
            args.push(url.to_string());
            args.push("--db-schema".into());
            args.push("desk.schema".into());
        }
        if stack == Stack::PostgresTls {
            let material = w3::credential(dir.path())?;
            args.push("--tls".into());
            args.push(format!(
                "{CREDENTIAL}={},{}",
                material.certificate.display(),
                material.key.display()
            ));
            tls = Some(Arc::new(w3::client_config(&material.der)?));
        }

        let (stderr, records) = match sinking {
            Sinking::JsonNull => (Stdio::from(std::fs::File::create("/dev/null")?), None),
            Sinking::JsonFile => {
                let path = dir.path().join("trace.jsonl");
                (Stdio::from(std::fs::File::create(&path)?), Some(path))
            }
            // A discarding sink writes nothing, so a pipe cannot fill.
            Sinking::Off => (Stdio::piped(), None),
        };
        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut server = Server::start_with(ply, dir.path(), &borrowed, stderr)?;
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        w3::wait_until_serving_over(&mut server, addr, tls.clone())?;
        Ok(Serving {
            _dir: dir,
            server,
            addr,
            records,
            tls,
        })
    }

    /// Records the sink actually wrote, so a `json` row shows a sink that wrote something.
    fn records_written(&self) -> usize {
        let Some(path) = &self.records else {
            return 0;
        };
        std::fs::read_to_string(path)
            .map(|s| s.lines().filter(|l| l.starts_with('{')).count())
            .unwrap_or(0)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ServedPoint {
    pub stack: &'static str,
    /// Which accept loop served it: `sequential` or `task-per-conn`.
    pub accept: &'static str,
    pub sink: &'static str,
    pub route: String,
    pub concurrency: u32,
    pub requests: u32,
    pub per_second: f64,
    pub p50_micros: f64,
    pub p95_micros: f64,
    pub p99_micros: f64,
    pub max_micros: f64,
    /// Lines the sink wrote; `0` for sinks with no file and for all but a point's last route.
    pub records: usize,
}

/// The same routes under the same load with only `--trace` moved.
#[allow(clippy::too_many_arguments)]
pub fn tracing(
    repo: &Path,
    ply: &Path,
    url: &str,
    stacks: &[Stack],
    variant: w3::Variant,
    sinks: &[Sinking],
    routes: &[(&'static str, &'static str)],
    concurrencies: &[u32],
    per_conn: u32,
    requests_per_point: u32,
    api_key: &str,
) -> Result<Vec<ServedPoint>> {
    let mut out = Vec::new();
    for &stack in stacks {
        for &sinking in sinks {
            for &concurrency in concurrencies {
                let conns = w3::share(concurrency, per_conn, requests_per_point);
                // One spare for the probe that proves the server answers.
                let budget = concurrency * conns * routes.len() as u32 + 1;
                let mut serving =
                    Serving::start(repo, ply, url, stack, variant, sinking, budget, api_key)?;
                let before = serving.records_written();
                for (label, path) in routes {
                    let point = w3::load_point_over(
                        &mut serving.server,
                        serving.addr,
                        serving.tls.clone(),
                        stack.label(),
                        label,
                        path,
                        concurrency,
                        per_conn,
                        conns,
                    )?;
                    out.push(ServedPoint {
                        stack: stack.label(),
                        accept: variant.label(),
                        sink: sinking.label(),
                        route: label.to_string(),
                        concurrency,
                        requests: point.requests,
                        per_second: point.per_second,
                        p50_micros: point.p50_micros,
                        p95_micros: point.p95_micros,
                        p99_micros: point.p99_micros,
                        max_micros: point.max_micros,
                        records: 0,
                    });
                }
                let written = serving.records_written().saturating_sub(before);
                serving.server.finish()?;
                // Charged to the last route: the point's routes share one server and one file.
                if let Some(last) = out.last_mut() {
                    last.records = written;
                }
            }
        }
    }
    Ok(out)
}

#[derive(Clone, Debug, Serialize)]
pub struct DrainPoint {
    pub scenario: String,
    /// Connections holding a request the server had not answered when the signal was delivered.
    pub in_flight: u32,
    pub drain_ms: u64,
    pub lead_ms: u64,
    pub stop_to_exit_ms: f64,
    pub exit_code: i32,
    /// Requests that got a response after the signal.
    pub answered: u32,
    /// Requests whose connection closed with no response, for want of cancellation.
    pub abandoned: u32,
    /// Whether the run printed `W0608`.
    pub drain_incomplete: bool,
}

/// A stop with N requests in flight, under a drain that is long enough and under one that is not.
pub fn drain(
    repo: &Path,
    ply: &Path,
    url: &str,
    in_flight: &[u32],
    drain_ms: u64,
    hold_ms: u64,
    api_key: &str,
) -> Result<Vec<DrainPoint>> {
    let mut out = Vec::new();
    for &n in in_flight {
        out.push(one_drain(
            repo,
            ply,
            url,
            n,
            drain_ms,
            0,
            hold_ms,
            api_key,
            "completes",
        )?);
    }
    // The clients hold their requests longer than the drain, so it runs out with them in flight.
    let &widest = in_flight.last().unwrap_or(&1);
    out.push(one_drain(
        repo,
        ply,
        url,
        widest,
        drain_ms,
        0,
        drain_ms + 3_000,
        api_key,
        "expires",
    )?);
    // Accept keeps running while `signal.stopping()` answers true, so a readiness route can shed.
    out.push(one_drain(
        repo,
        ply,
        url,
        1,
        drain_ms,
        1_500,
        hold_ms,
        api_key,
        "lead 1500ms",
    )?);
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn one_drain(
    repo: &Path,
    ply: &Path,
    url: &str,
    in_flight: u32,
    drain_ms: u64,
    lead_ms: u64,
    hold_ms: u64,
    api_key: &str,
    scenario: &str,
) -> Result<DrainPoint> {
    let service = w3::Service::open(repo)?.source(w3::Variant::TaskPerConn)?;
    let dir = tempfile::tempdir().context("a temp dir for the served project")?;
    let port = reserve_port()?;
    project(dir.path(), &service, Stack::Postgres)?;

    let sets = [
        format!("DESK_PORT={port}"),
        format!("DESK_CONNECTIONS={}", in_flight + 8),
        format!("DESK_API_KEY={api_key}"),
    ];
    let drain = drain_ms.to_string();
    let lead = lead_ms.to_string();
    let mut args: Vec<&str> = vec![
        "--config-schema",
        "desk.config",
        "--db",
        url,
        "--db-schema",
        "desk.schema",
        "--trace",
        "off",
        "--drain-ms",
        &drain,
        "--drain-lead-ms",
        &lead,
    ];
    for s in &sets {
        args.push("--set");
        args.push(s);
    }
    let mut server = Server::start_with(ply, dir.path(), &args, Stdio::piped())?;
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    w3::wait_until_serving(&mut server, addr)?;

    let mut held = Vec::new();
    for _ in 0..in_flight {
        held.push(crate::w5::Partial::open(addr, hold_ms)?);
    }
    // Let the accept loop take them all, so the signal finds them in flight, not in the backlog.
    std::thread::sleep(Duration::from_millis(200));

    let pid = server.pid().context("the server has already been reaped")?;
    let signalled = Instant::now();
    signal(pid, "TERM")?;

    // Clients run on their own threads, so the clock measures signal-to-exit, not this harness.
    let clients: Vec<_> = held
        .into_iter()
        .map(|conn| std::thread::spawn(move || conn.finish().unwrap_or(false)))
        .collect();
    let deadline = Instant::now() + Duration::from_secs(180);
    let stop_to_exit = loop {
        if server.exited()?.is_some() {
            break signalled.elapsed();
        }
        if Instant::now() >= deadline {
            bail!("the server was still running three minutes after the signal");
        }
        std::thread::sleep(Duration::from_millis(1));
    };

    let mut answered = 0;
    let mut abandoned = 0;
    for client in clients {
        match client.join() {
            Ok(true) => answered += 1,
            _ => abandoned += 1,
        }
    }
    let (status, output) = server.wait(Duration::from_secs(120))?;

    Ok(DrainPoint {
        scenario: scenario.to_string(),
        in_flight,
        drain_ms,
        lead_ms,
        stop_to_exit_ms: stop_to_exit.as_secs_f64() * 1e3,
        exit_code: status.code().unwrap_or(-1),
        answered,
        abandoned,
        drain_incomplete: output.contains("W0608"),
    })
}

/// A connection carrying a request head the client has not finished sending.
pub struct Partial {
    socket: std::net::TcpStream,
    hold: Duration,
    opened: Instant,
}

impl Partial {
    fn open(addr: std::net::SocketAddr, hold_ms: u64) -> Result<Partial> {
        let socket = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(10))
            .with_context(|| format!("connecting to {addr}"))?;
        socket.set_nodelay(true)?;
        socket.set_read_timeout(Some(Duration::from_secs(120)))?;
        let mut socket = socket;
        socket.write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\n")?;
        socket.flush()?;
        Ok(Partial {
            socket,
            hold: Duration::from_millis(hold_ms),
            opened: Instant::now(),
        })
    }

    /// Finish the head after the hold, and answer whether a response arrived.
    fn finish(mut self) -> Result<bool> {
        let left = self.hold.saturating_sub(self.opened.elapsed());
        if !left.is_zero() {
            std::thread::sleep(left);
        }
        if self.socket.write_all(b"\r\n").is_err() {
            return Ok(false);
        }
        let _ = self.socket.flush();
        let mut buf = [0u8; 1024];
        match std::io::Read::read(&mut self.socket, &mut buf) {
            Ok(0) | Err(_) => Ok(false),
            Ok(n) => Ok(buf[..n].starts_with(b"HTTP/1.1 200")),
        }
    }
}

fn signal(pid: u32, name: &str) -> Result<()> {
    let status = Command::new("kill")
        .args([&format!("-{name}"), &pid.to_string()])
        .status()
        .with_context(|| format!("delivering SIG{name} to {pid}"))?;
    if !status.success() {
        bail!("`kill -{name} {pid}` exited {status}");
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize)]
pub struct TxnOutcome {
    /// The order sequence before and after.
    pub sequence_before: i64,
    pub sequence_after: i64,
    /// Orders in the table before the request was made.
    pub orders_before: i64,
    /// Orders after the process exited.
    pub orders_after: i64,
    /// What the run's own teardown reported.
    pub verdict: String,
    pub exit_code: i32,
    pub stop_to_exit_ms: f64,
    /// Backends left `idle in transaction` after the process exited.
    pub sessions_left: i64,
}

/// Whether a transaction open at the drain deadline commits, rolls back, or is lost.
pub fn transaction_at_deadline(
    repo: &Path,
    ply: &Path,
    url: &str,
    drain_ms: u64,
    api_key: &str,
) -> Result<TxnOutcome> {
    let service = w3::Service::open(repo)?.source(w3::Variant::TaskPerConn)?;
    let dir = tempfile::tempdir().context("a temp dir for the served project")?;
    let port = reserve_port()?;
    project(dir.path(), &service, Stack::Postgres)?;

    let sets = [
        format!("DESK_PORT={port}"),
        "DESK_CONNECTIONS=8".to_string(),
        format!("DESK_API_KEY={api_key}"),
    ];
    let drain = drain_ms.to_string();
    // Above the drain so the deadline stops the run; not far, since the `ROLLBACK` waits on it.
    let statement = (drain_ms + 5_000).to_string();
    let mut args: Vec<&str> = vec![
        "--config-schema",
        "desk.config",
        "--db",
        url,
        "--db-schema",
        "desk.schema",
        "--trace",
        "off",
        "--drain-ms",
        &drain,
        "--db-statement-ms",
        &statement,
        "--db-idle-txn-ms",
        &statement,
    ];
    for s in &sets {
        args.push("--set");
        args.push(s);
    }

    let mut blocker = Blocker::open(url)?;
    let orders_before = count_orders(url)?;
    let sequence_before = last_order_id(url)?;

    let mut server = Server::start_with(ply, dir.path(), &args, Stdio::piped())?;
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    w3::wait_until_serving(&mut server, addr)?;

    blocker.lock_bolt()?;
    let order = post_order(addr, api_key)?;
    wait_until_blocked(url, Duration::from_secs(30))?;

    let pid = server.pid().context("the server has already been reaped")?;
    let signalled = Instant::now();
    signal(pid, "TERM")?;
    let (status, output) = server.wait(Duration::from_secs(180))?;
    let stop_to_exit = signalled.elapsed();
    drop(order);

    // Released only now, so nothing the desk left behind is resolved by this harness.
    blocker.release()?;

    let orders_after = count_orders(url)?;
    let sequence_after = last_order_id(url)?;
    let sessions_left = idle_in_transaction(url)?;
    Ok(TxnOutcome {
        sequence_before,
        sequence_after,
        orders_before,
        orders_after,
        verdict: verdict_of(&output),
        exit_code: status.code().unwrap_or(-1),
        stop_to_exit_ms: stop_to_exit.as_secs_f64() * 1e3,
        sessions_left,
    })
}

fn verdict_of(output: &str) -> String {
    let mut lines: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|l| {
            l.contains("W0608")
                || l.contains("rolled back")
                || l.contains("transactions")
                || l.starts_with("desk.main")
                || l.contains("exit ")
        })
        .collect();
    lines.dedup();
    if lines.is_empty() {
        "the run printed nothing about its teardown".to_string()
    } else {
        lines.join(" · ")
    }
}

/// A second session holding a row lock, so the desk's `UPDATE` blocks.
struct Blocker {
    runtime: tokio::runtime::Runtime,
    client: Option<tokio_postgres::Client>,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl Blocker {
    fn open(url: &str) -> Result<Blocker> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("a runtime for the blocking session")?;
        let (client, handle) = runtime.block_on(async {
            let (client, connection) = tokio_postgres::connect(url, tokio_postgres::NoTls)
                .await
                .with_context(|| format!("connecting to `{url}`"))?;
            let handle = tokio::spawn(async move {
                let _ = connection.await;
            });
            Ok::<_, anyhow::Error>((client, handle))
        })?;
        Ok(Blocker {
            runtime,
            client: Some(client),
            handle: Some(handle),
        })
    }

    fn lock_bolt(&mut self) -> Result<()> {
        let client = self.client.as_ref().context("the session is closed")?;
        self.runtime.block_on(async {
            client.batch_execute("begin").await?;
            client
                .batch_execute("select 1 from items where sku = 'bolt' for update")
                .await?;
            Ok::<(), anyhow::Error>(())
        })
    }

    fn release(&mut self) -> Result<()> {
        if let Some(client) = &self.client {
            self.runtime
                .block_on(async { client.batch_execute("rollback").await })?;
        }
        self.client = None;
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
        Ok(())
    }
}

/// The request, issued on a thread of its own because it will not answer.
fn post_order(addr: std::net::SocketAddr, api_key: &str) -> Result<std::thread::JoinHandle<()>> {
    let key = api_key.to_string();
    Ok(std::thread::spawn(move || {
        let Ok(mut socket) = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(10))
        else {
            return;
        };
        let body = br#"{"customer":"drain","lines":[{"sku":"bolt","qty":1}]}"#;
        let head = format!(
            "POST /orders HTTP/1.1\r\nHost: 127.0.0.1\r\nx-api-key: {key}\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let _ = socket.write_all(head.as_bytes());
        let _ = socket.write_all(body);
        let _ = socket.flush();
        let _ = socket.set_read_timeout(Some(Duration::from_secs(120)));
        let mut buf = [0u8; 1024];
        let _ = std::io::Read::read(&mut socket, &mut buf);
    }))
}

fn count_orders(url: &str) -> Result<i64> {
    query_one_i64(url, "select count(*) from orders")
}

fn last_order_id(url: &str) -> Result<i64> {
    query_one_i64(url, "select last_value from orders_id_seq")
}

fn idle_in_transaction(url: &str) -> Result<i64> {
    query_one_i64(
        url,
        "select count(*) from pg_stat_activity \
         where state = 'idle in transaction' and application_name <> 'ply-corpus'",
    )
}

fn query_one_i64(url: &str, sql: &'static str) -> Result<i64> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let (client, connection) = tokio_postgres::connect(url, tokio_postgres::NoTls)
            .await
            .with_context(|| format!("connecting to `{url}`"))?;
        let handle = tokio::spawn(async move {
            let _ = connection.await;
        });
        let row = client.query_one(sql, &[]).await?;
        handle.abort();
        Ok(row.get::<_, i64>(0))
    })
}

/// Wait until some backend is waiting on a lock, which is the desk's `UPDATE`.
fn wait_until_blocked(url: &str, within: Duration) -> Result<()> {
    let deadline = Instant::now() + within;
    loop {
        let waiting = query_one_i64(
            url,
            "select count(*) from pg_stat_activity \
             where wait_event_type = 'Lock' and state = 'active'",
        )?;
        if waiting > 0 {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("no backend was waiting on a lock after {within:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct DeployReport {
    pub definitions: usize,
    pub artifact_bytes: u64,
    pub binary_bytes: u64,
    pub digest: String,
    /// The same tree built again from a different absolute root.
    pub reproducible: bool,
    /// After a one-definition edit.
    pub second_artifact_bytes: u64,
    pub second_digest: String,
    pub changed_definitions: usize,
    pub unchanged_definitions: usize,
    /// The bodies a transfer of only the changed definitions would have carried.
    pub changed_body_bytes: u64,
    /// Those bodies as a fraction of a whole artifact, and of the artifact plus the binary.
    pub of_artifact: f64,
    pub of_deploy: f64,
}

/// Artifact size, reproducibility, and what an incremental transfer would have saved.
pub fn deploy(repo: &Path, ply: &Path, edit: (&str, &str)) -> Result<DeployReport> {
    let service = std::fs::read_to_string(repo.join("examples/desk.ply"))
        .context("reading `examples/desk.ply`")?;
    let dir = tempfile::tempdir().context("a temp dir for the build")?;
    let first = dir.path().join("a");
    let second = dir.path().join("b");
    let elsewhere = dir.path().join("c");
    for root in [&first, &second, &elsewhere] {
        std::fs::create_dir_all(root)?;
    }
    std::fs::write(first.join("desk.ply"), &service)?;
    std::fs::write(elsewhere.join("desk.ply"), &service)?;
    std::fs::write(second.join("desk.ply"), replace(&service, edit.0, edit.1)?)?;

    let one = build(ply, &first, &dir.path().join("one.plyx"))?;
    let again = build(ply, &elsewhere, &dir.path().join("again.plyx"))?;
    let two = build(ply, &second, &dir.path().join("two.plyx"))?;

    let reproducible = std::fs::read(&one.path)? == std::fs::read(&again.path)?;

    let (old, _) = ply_cli::artifact::read(&one.path)
        .map_err(|d| anyhow::anyhow!("reading the first artifact: {}", d.message))?;
    let (new, _) = ply_cli::artifact::read(&two.path)
        .map_err(|d| anyhow::anyhow!("reading the second artifact: {}", d.message))?;

    let changed: Vec<_> = new
        .bodies
        .iter()
        .filter(|(hash, _)| !old.bodies.contains_key(*hash))
        .collect();
    let changed_body_bytes: u64 = changed
        .iter()
        // A `BODIES` record: the key, the length and the bytes, since a transfer ships all three.
        .map(|(_, body)| body.len() as u64 + 32 + 4)
        .sum();
    let unchanged = new.bodies.len() - changed.len();
    let deploy_bytes = two.artifact_bytes + two.binary_bytes;

    Ok(DeployReport {
        definitions: new.bodies.len(),
        artifact_bytes: one.artifact_bytes,
        binary_bytes: one.binary_bytes,
        digest: one.digest,
        reproducible,
        second_artifact_bytes: two.artifact_bytes,
        second_digest: two.digest,
        changed_definitions: changed.len(),
        unchanged_definitions: unchanged,
        changed_body_bytes,
        of_artifact: changed_body_bytes as f64 / two.artifact_bytes as f64,
        of_deploy: changed_body_bytes as f64 / deploy_bytes as f64,
    })
}

struct BuiltArtifact {
    path: PathBuf,
    artifact_bytes: u64,
    binary_bytes: u64,
    digest: String,
}

fn build(ply: &Path, root: &Path, to: &Path) -> Result<BuiltArtifact> {
    let out = Command::new(ply)
        .args(["build", "--json", "-o"])
        .arg(to)
        .arg(root)
        .output()
        .with_context(|| format!("running `{} build`", ply.display()))?;
    if !out.status.success() {
        bail!(
            "`ply build` exited {}:\n{}{}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).context("`ply build --json` did not emit JSON")?;
    Ok(BuiltArtifact {
        path: to.to_path_buf(),
        artifact_bytes: json["artifact_bytes"].as_u64().unwrap_or(0),
        binary_bytes: json["binary_bytes"].as_u64().unwrap_or(0),
        digest: json["digest"].as_str().unwrap_or("").to_string(),
    })
}

#[derive(Default, Serialize)]
pub struct Measurements {
    pub events: Vec<EventPoint>,
    pub served: Vec<ServedPoint>,
    pub drain: Vec<DrainPoint>,
    pub transaction: Option<TxnOutcome>,
    pub deploy: Option<DeployReport>,
}

pub fn render(m: &Measurements) -> String {
    let mut out = String::new();
    if !m.events.is_empty() {
        out.push_str("\nevents — one trace operation, against the same loop performing none\n\n");
        out.push_str(
            "  rung                   operation      ops      us/op        ops/s   over bare\n",
        );
        for p in &m.events {
            let _ = writeln!(
                out,
                "  {:<22} {:<9} {:>8} {:>10.3} {:>12.0}   {:+.3}us",
                p.rung,
                p.operation,
                p.operations,
                p.per_operation_micros,
                p.per_second,
                p.over_bare_micros
            );
        }
    }
    if !m.served.is_empty() {
        out.push_str(
            "\nserved — the same routes under the same load, with only `--trace` moved\n\n",
        );
        out.push_str(
            "  stack             accept          sink               route            conc    reqs     req/s     p50     p95     p99   records\n",
        );
        for p in &m.served {
            let _ = writeln!(
                out,
                "  {:<17} {:<15} {:<18} {:<16} {:>5} {:>7} {:>9.0} {:>7.0} {:>7.0} {:>7.0} {:>9}",
                p.stack,
                p.accept,
                p.sink,
                p.route,
                p.concurrency,
                p.requests,
                p.per_second,
                p.p50_micros,
                p.p95_micros,
                p.p99_micros,
                p.records
            );
        }
    }
    if !m.drain.is_empty() {
        out.push_str("\ndrain — a stop with N requests in flight\n\n");
        out.push_str(
            "  scenario        in flight  drain ms  lead ms   stop→exit  exit  answered  abandoned  W0608\n",
        );
        for p in &m.drain {
            let _ = writeln!(
                out,
                "  {:<15} {:>9} {:>9} {:>8} {:>10.0}ms {:>5} {:>9} {:>10}  {}",
                p.scenario,
                p.in_flight,
                p.drain_ms,
                p.lead_ms,
                p.stop_to_exit_ms,
                p.exit_code,
                p.answered,
                p.abandoned,
                if p.drain_incomplete { "yes" } else { "no" }
            );
        }
    }
    if let Some(t) = &m.transaction {
        out.push_str("\ntransaction — a transaction open at the drain deadline\n\n");
        let _ = writeln!(
            out,
            "  orders before {} · orders after {} · {}",
            t.orders_before,
            t.orders_after,
            if t.orders_after == t.orders_before {
                "ROLLED BACK — nothing was committed"
            } else {
                "COMMITTED — a half-finished body reached the table"
            }
        );
        let _ = writeln!(
            out,
            "  order sequence {} → {} · {}",
            t.sequence_before,
            t.sequence_after,
            if t.sequence_after > t.sequence_before {
                "the INSERT ran, so there was a transaction to lose"
            } else {
                "the sequence did not move: the INSERT never ran and this measures nothing"
            }
        );
        let _ = writeln!(
            out,
            "  exit {} · stop→exit {:.0}ms · backends left idle in transaction {}",
            t.exit_code, t.stop_to_exit_ms, t.sessions_left
        );
        let _ = writeln!(out, "  the run said: {}", t.verdict);
    }
    if let Some(d) = &m.deploy {
        out.push_str(
            "\ndeploy — what goes out, and what an incremental transfer would have saved\n\n",
        );
        let _ = writeln!(
            out,
            "  artifact    {:>10} bytes · {} definitions · digest {} · reproducible {}",
            d.artifact_bytes,
            d.definitions,
            d.digest,
            if d.reproducible { "yes" } else { "NO" }
        );
        let _ = writeln!(out, "  binary      {:>10} bytes", d.binary_bytes);
        let _ = writeln!(
            out,
            "  second      {:>10} bytes · digest {} · {} changed, {} unchanged",
            d.second_artifact_bytes,
            d.second_digest,
            d.changed_definitions,
            d.unchanged_definitions
        );
        let _ = writeln!(
            out,
            "  changed     {:>10} bytes of bodies — {:.2}% of the artifact, {:.4}% of artifact+binary",
            d.changed_body_bytes,
            d.of_artifact * 100.0,
            d.of_deploy * 100.0
        );
    }
    out
}
