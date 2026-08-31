//! What operating the service costs: a trace call, a drain, and a deploy.
//!
//! W3 priced HTTP and W4 priced the database. W5 adds the three things every
//! service acquires — a log, a configuration and a way to stop — and each is an
//! effect rather than an ambient global. That buys the rows in `ply check
//! --types`; what it costs is what this module measures.
//!
//! | section | question |
//! | --- | --- |
//! | [`events`] | what one trace operation costs, per sink, against the same loop performing nothing |
//! | [`tracing`] | what the *service* pays for tracing it turned off, and for tracing it turned on |
//! | [`drain`] | how long a stop takes with N requests in flight, and what the deadline does to them |
//! | [`transaction_at_deadline`] | whether a transaction open at the deadline commits, rolls back, or is lost — a correctness result reported as a measurement |
//! | [`deploy`] | the artifact's bytes, the binary's bytes, and what an incremental transfer would have saved |
//!
//! **The discipline is `w3`'s and `w4`'s: substitution, never instrumentation.**
//! Every row of [`events`] runs one Ply definition — `crates/ply-corpus/ply/w5.ply`'s
//! fold — and changes only which handler answers it. Every row of [`tracing`]
//! serves `examples/desk.ply` verbatim and changes only `--trace`. A difference
//! between two rows is that one substitution and nothing else.
//!
//! The one rung that is not a configuration a Ply service can be in is `bare`,
//! and it is the most important one. There is no disabled path — a row cannot be
//! conditional on a flag — so `bare` is the program with the perform *deleted*,
//! which is what every other language gets from a level check at the call site.
//! The gap between `bare` and `discard` is therefore precisely what ADR 0015
//! §1.4 owes a number for: what a service pays, on every request, forever, for
//! tracing it turned off.

use anyhow::{Context, Result, bail};
use ply_core::CheckOutput;
use ply_core::ty::Footprint;
use ply_eval::host::HostRegistry;
use ply_eval::{Machine, Value};
use ply_host::trace::{Level, Record, Trace, sink};
use ply_span::{Diagnostic, Span};
use ply_syntax::ast::ModuleName;
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

/// The program [`events`] runs. Ply source checked by the real front end on
/// every run, so a loop that stopped typechecking is a failed benchmark rather
/// than a wrong number.
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

// --- The program ------------------------------------------------------------

pub struct Program {
    program: ply_syntax::ast::Program,
    resolved: ply_syntax::resolve::Resolved,
    check: CheckOutput,
}

impl Program {
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
        let mut program = ply_syntax::parse_program(inputs)
            .map_err(|d| diagnostics("parsing the bench program", &d))?;
        let expanded = ply_derive::expand_program(&mut program);
        if !expanded.is_empty() {
            return Err(diagnostics("expanding a `derive`", &expanded));
        }
        let resolved = ply_syntax::resolve::resolve(&mut program)
            .map_err(|d| diagnostics("resolving the bench program", &d))?;
        let check = ply_core::check_program(&program, &resolved)
            .map_err(|d| diagnostics("checking the bench program", &d))?;
        Ok(Program {
            program,
            resolved,
            check,
        })
    }

    fn full(&self, simple: &str) -> Result<String> {
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

    /// One call over a hermetic machine: no host at all, which is what the
    /// `bare` and `twin` rungs run on.
    fn call_pure(&self, simple: &str, n: i64) -> Result<(Duration, Value)> {
        let name = self.full(simple)?;
        let mut machine = Machine::new(&self.program, &self.resolved, &self.check);
        let started = Instant::now();
        let value = machine
            .call(&name, vec![Value::Int(n)], Span::DUMMY)
            .map_err(|d| anyhow::anyhow!("`{simple}` raised [{}]: {}", d.code, d.message))?;
        Ok((started.elapsed(), value))
    }

    /// One call with a real `trace` binding, which is the whole of what the
    /// bound rungs add: a `perform` that leaves the program and a sink that
    /// answers it.
    fn call_traced(
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
        let mut machine = Machine::new(&self.program, &self.resolved, &self.check);
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

// --- A sink whose destination is a parameter --------------------------------

/// `ply_host::trace::json`'s formatting with somewhere other than this process's
/// stderr to put it.
///
/// The shipped `Json` writes to the real fd 2, which an in-process benchmark
/// cannot separate from its own output — so the format cost and the write cost
/// are priced here with the destination swapped, and the *shipped* sink is
/// measured where it belongs, under [`tracing`], with a whole `ply run` between
/// the harness and the file. `write_json` is the shipped one's own encoder,
/// called directly, so the bytes on both sides are the same bytes.
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

// --- Section 1: what one trace operation costs ------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct EventPoint {
    /// Which sink answered, or `bare` for the loop with the perform deleted.
    pub rung: &'static str,
    /// `none`, `event`, `span` (an `enter` and an `exit`) or `count`.
    pub operation: &'static str,
    pub operations: u32,
    pub per_operation_micros: f64,
    pub per_second: f64,
    /// Microseconds this rung adds over `bare` at the same operation, which is
    /// the number ADR 0015 §1.4 owes.
    pub over_bare_micros: f64,
}

/// Which sink a rung installs, and what it is called in the table.
#[derive(Clone, Copy)]
enum Rung {
    /// The same loop with no perform in it. Not a configuration a Ply service
    /// can be in, and the denominator for every row that is.
    Bare,
    /// `--trace off`: the shipped `ply_host::trace::discard`.
    Discard,
    /// `--trace json --trace-level warn` over `Debug` events: the shipped
    /// filter, refusing before a name is decoded or a field list is built.
    Filtered,
    /// `ply_host::trace::json`'s encoder, written to `/dev/null`.
    JsonNull,
    /// The same, written to a real file on this filesystem.
    JsonFile,
    /// `std.trace`'s collecting twin, in Ply, over a region-scoped cell.
    Twin,
}

impl Rung {
    fn label(self) -> &'static str {
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
///
/// The bound sinks are measured at `iterations` and the twin at
/// `twin_iterations`, which is smaller and is not a hedge: `std.trace`'s `Sink`
/// builds its record list with `push`, so collecting N records is O(N²) and a
/// twin row taken at twenty thousand would be a number about list append rather
/// than about the twin. A twin sink lives inside one test and holds tens of
/// records, which is the size the row is taken at. The `ops` column carries the
/// count so the two are never read as one measurement.
pub fn events(
    iterations: u32,
    twin_iterations: u32,
    repeats: usize,
    dir: &Path,
) -> Result<Vec<EventPoint>> {
    let program = Program::parse()?;
    let mut out: Vec<EventPoint> = Vec::new();

    // The denominator, taken once per size, because the fold, the list and the
    // `Fields` map are the program's cost at every rung and charging them to
    // tracing would overstate what a sink costs.
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
    // Answered by every rung, so a rung whose loop did not run is a failure
    // rather than a fast row.
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

fn sink_for(rung: Rung, dir: &Path) -> Result<Arc<Trace>> {
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

// --- The served project -----------------------------------------------------

/// Which store, transport and sink a served point runs under.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Sinking {
    /// `--trace off` — `ply_host::trace::discard`, a listed handler and not an
    /// absence.
    Off,
    /// `--trace json`, with stderr on `/dev/null`: the encoder's cost with the
    /// destination's taken out.
    JsonNull,
    /// `--trace json`, with stderr on a file the harness reads afterwards, so
    /// the records are counted rather than assumed.
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
    /// W3's shape, re-taken on this machine.
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
///
/// The service is read from the repository and its tests are cut; the only
/// rewrite is which entry point `main` calls, because which store a service uses
/// is not configuration. The port, the connection budget and the credential are
/// `--set`, which is the whole of what W5 bought here.
fn project(dir: &Path, service: &str, stack: Stack, variant: w3::Variant) -> Result<()> {
    // The twin discharges every `db`, `trace` and `signal` atom in Ply, so its
    // entry point's row is narrower than the real desk's and moves with the
    // call. `w3::Service::source` has already widened every row by the one atom
    // spawning adds, so the needle depends on which accept loop is in the file.
    let spawning = variant == w3::Variant::TaskPerConn;
    let task = if spawning { "task.write, " } else { "" };
    let from = format!(
        "fn main() -> Int / {{Serving, config.read[server], {task}net.write[conn], net.write[listener]}} = {{"
    );
    let to = format!(
        "fn main() -> Int / {{config.read[server], config.read[credentials], {task}net.write[conn], net.write[listener]}} = {{"
    );
    let source = match stack {
        Stack::Postgres => service.to_string(),
        Stack::PostgresTls => replace(
            service,
            "    run(port, count)",
            &format!("    run_tls(port, \"{CREDENTIAL}\", count)"),
        )?,
        Stack::Twin => {
            let narrowed = replace(service, &from, &to)?;
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

/// Everything a served point is configured with, in one place.
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
        project(dir.path(), &service, stack, variant)?;

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
            // Every record `desk.ply` writes is `Info` or above, so the sink
            // admits all of them. What a *filtered* record costs is priced in
            // `events`, where it can be separated from a request.
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

    /// Records the sink actually wrote, so a `json` row is a row about a sink
    /// that wrote something rather than one that was configured to.
    fn records_written(&self) -> usize {
        let Some(path) = &self.records else {
            return 0;
        };
        std::fs::read_to_string(path)
            .map(|s| s.lines().filter(|l| l.starts_with('{')).count())
            .unwrap_or(0)
    }
}

// --- Section 2: what the service pays for tracing ---------------------------

#[derive(Clone, Debug, Serialize)]
pub struct ServedPoint {
    pub stack: &'static str,
    /// Which accept loop served it: `sequential` is `examples/desk.ply` as
    /// written, `task-per-conn` is the same service with a spawn in its loop.
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
    /// Lines the sink wrote, counted from the file it wrote them to, and `0`
    /// on every row whose sink has no file to count — `off`, `/dev/null`, and
    /// every route but the last of a point, because one server serves the
    /// routes of a point and its file does not say which line came from which.
    /// So the last row of a `json → file` point carries the whole point, and
    /// divided by that point's requests it is records per request, which is what
    /// reconciles this table with [`events`].
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
                // Charged to the last route of the point, because the sink is
                // shared across the routes a server served and splitting it
                // would be inventing an attribution the file does not carry.
                if let Some(last) = out.last_mut() {
                    last.records = written;
                }
            }
        }
    }
    Ok(out)
}

// --- Section 3: the drain ---------------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct DrainPoint {
    /// How the point was set up, in one phrase.
    pub scenario: String,
    /// Connections holding a request the server had not answered when the signal
    /// was delivered.
    pub in_flight: u32,
    pub drain_ms: u64,
    pub lead_ms: u64,
    /// Milliseconds from the signal to the process exiting.
    pub stop_to_exit_ms: f64,
    pub exit_code: i32,
    /// Requests that got a response after the signal.
    pub answered: u32,
    /// Requests whose connection was closed with no response, which is what W5
    /// costs at the deadline for want of cancellation.
    pub abandoned: u32,
    /// Whether the run printed `W0608`.
    pub drain_incomplete: bool,
}

/// A stop with N requests in flight, under a drain that is long enough and under
/// one that is not.
///
/// "In flight" is a request whose head the client has begun and not finished, so
/// the server is inside `net.recv` with the connection accepted and nothing
/// written back. That is the shape a request has for most of its life and the
/// one the drain has to wait out.
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
    // The deadline case: the clients hold their requests open for longer than
    // the drain, so the run runs out of time with them still in flight.
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
    // And the lead: accept keeps running while `signal.stopping()` already
    // answers true, which is what lets a readiness route shed before the
    // listener closes.
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
    project(
        dir.path(),
        &service,
        Stack::Postgres,
        w3::Variant::TaskPerConn,
    )?;

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

    // N connections, each with a request head begun and not finished. The
    // server has accepted every one of them and is inside `net.recv`.
    let mut held = Vec::new();
    for _ in 0..in_flight {
        held.push(crate::w5::Partial::open(addr, hold_ms)?);
    }
    // Give the accept loop time to take every one of them, so the signal finds
    // them in flight rather than in the listen backlog.
    std::thread::sleep(Duration::from_millis(200));

    let pid = server.pid().context("the server has already been reaped")?;
    let signalled = Instant::now();
    signal(pid, "TERM")?;

    // The clients run on their own threads, because the time under measurement
    // is the signal to the process exiting and a harness that finished its
    // clients first would be timing its own sleep.
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

// --- Section 4: a transaction open at the deadline --------------------------

#[derive(Clone, Debug, Serialize)]
pub struct TxnOutcome {
    /// The order sequence before and after. A sequence is **not** transactional,
    /// so an advance across the run is proof the `INSERT` really executed —
    /// without it, a table whose row count did not move is equally consistent
    /// with a transaction that never got as far as writing anything.
    pub sequence_before: i64,
    pub sequence_after: i64,
    /// Orders in the table before the request was made.
    pub orders_before: i64,
    /// Orders after the process exited. Equal to `orders_before` is a rollback;
    /// one more is a commit, which is the outcome ADR 0015 §4.4 exists to make
    /// unreachable.
    pub orders_after: i64,
    /// What the run's own teardown reported.
    pub verdict: String,
    pub exit_code: i32,
    pub stop_to_exit_ms: f64,
    /// Backends left `idle in transaction` after the process exited. A
    /// connection closed with a `BEGIN` still open leaves postgres to abort it
    /// whenever it notices, which is the same outcome by luck rather than by
    /// construction — so this being zero is the interesting half.
    pub sessions_left: i64,
}

/// Whether a transaction open at the drain deadline commits, rolls back, or is
/// lost.
///
/// The transaction is held open by the database rather than by a sleep in the
/// program: a second session takes a row lock on the item the order draws down,
/// so `place_order` gets as far as inserting its order row and then blocks on
/// the `UPDATE`. The drain deadline expires with the `INSERT` done and the
/// `COMMIT` not issued, which is exactly the state the ordering in §4.4 is about.
///
/// Asserted against the table's contents and `pg_stat_activity`, never against
/// the driver's own bookkeeping — a driver that believed it had rolled back is
/// the failure this is written to catch.
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
    project(
        dir.path(),
        &service,
        Stack::Postgres,
        w3::Variant::TaskPerConn,
    )?;

    let sets = [
        format!("DESK_PORT={port}"),
        "DESK_CONNECTIONS=8".to_string(),
        format!("DESK_API_KEY={api_key}"),
    ];
    let drain = drain_ms.to_string();
    // Above the drain, so what stops the run is the deadline under measurement
    // rather than postgres losing patience first — and not far above it, because
    // the *teardown* is bounded by this and not by `--drain-ms`: a `ROLLBACK`
    // queues behind the statement the connection is still executing, and that
    // statement is blocked on a lock. That relationship is what makes
    // `stop→exit` longer than `--drain-ms`, and it is the driver's own deadline
    // doing the job ADR 0015 §4.4 says it does.
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
    // Wait until the desk's own connection is the one waiting on the lock, which
    // is the state the measurement is about.
    wait_until_blocked(url, Duration::from_secs(30))?;

    let pid = server.pid().context("the server has already been reaped")?;
    let signalled = Instant::now();
    signal(pid, "TERM")?;
    let (status, output) = server.wait(Duration::from_secs(180))?;
    let stop_to_exit = signalled.elapsed();
    drop(order);

    // The lock is released only now, so nothing the desk left behind could have
    // been resolved by this harness getting out of the way first.
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

// --- Section 5: the deploy --------------------------------------------------

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
    /// The number §5.1's refusal has to be judged against.
    pub changed_body_bytes: u64,
    /// Those bodies as a fraction of a whole artifact, and of an artifact plus
    /// the binary a deploy must also ship.
    pub of_artifact: f64,
    pub of_deploy: f64,
}

/// Artifact size, reproducibility, and what an incremental transfer would have
/// saved.
///
/// `edit` is `(needle, replacement)` — one definition's body, rewritten. It
/// asserts it found what it replaced, because a silent miss would report a
/// second build that changed nothing as evidence that a change costs nothing.
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
        // The record as the `BODIES` section holds it: the key, the length and
        // the bytes, because a transfer ships all three.
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

// --- The report -------------------------------------------------------------

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

#[cfg(test)]
mod tests {
    use super::*;

    /// The program every row of [`events`] runs has to be a program, and the
    /// rows it publishes have to be the ones the substitution rests on.
    #[test]
    fn the_bench_program_checks_and_publishes_one_channel() {
        let program = Program::parse().expect("the bench program checks");
        for simple in [
            "bare",
            "events",
            "debug_events",
            "spans",
            "counters",
            "twin_events",
            "twin_spans",
            "twin_counters",
        ] {
            program
                .full(simple)
                .unwrap_or_else(|e| panic!("`{simple}` is missing: {e}"));
        }
        assert_eq!(program.footprint("bare").unwrap().to_string(), "{}");
        assert_eq!(
            program.footprint("events").unwrap().to_string(),
            "{std.trace.trace.write[bench]}"
        );
    }

    /// The twin discharges every `trace` atom, which is what makes the `twin`
    /// rung a rung rather than a stub: it runs on a machine with no host at all.
    #[test]
    fn a_twin_entry_point_reaches_nothing() {
        let program = Program::parse().unwrap();
        for simple in ["twin_events", "twin_spans", "twin_counters"] {
            assert_eq!(
                program.footprint(simple).unwrap().to_string(),
                "{}",
                "`{simple}` publishes a row, so it is not hermetic"
            );
        }
    }

    /// Every rung answers the same count, which is the whole of what makes a
    /// difference between two rows the operation rather than the work.
    #[test]
    fn every_rung_runs_the_same_loop() {
        let program = Program::parse().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (_, bare) = program.call_pure("bare", 32).unwrap();
        assert_eq!(bare, Value::Int(64));
        for simple in ["twin_events", "twin_spans", "twin_counters"] {
            assert_eq!(program.call_pure(simple, 32).unwrap().1, Value::Int(64));
        }
        for (rung, entry) in [
            (Rung::Discard, "events"),
            (Rung::Discard, "spans"),
            (Rung::Discard, "counters"),
            (Rung::JsonNull, "events"),
        ] {
            let host = ply_host::Host::new().traced(sink_for(rung, dir.path()).unwrap());
            assert_eq!(
                program.call_traced(&host, entry, 32).unwrap().1,
                Value::Int(64),
                "`{entry}` under `{}`",
                rung.label()
            );
        }
    }
}
