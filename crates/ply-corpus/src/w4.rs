//! What a database costs, on both sides of the effect boundary.

use anyhow::{Context, Result, bail};
use ply_eval::host::HostRegistry;
use ply_eval::{CheckOutput, Diagnostic, Footprint, Machine, ModuleName, Span, Symbol, Value};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The `ops` and `pool` sections' programs.
const BENCH: &str = include_str!("../ply/w4.ply");

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

/// The checked bench program, and the pieces a run needs off it.
pub struct Program {
    check: CheckOutput,
    /// The tier is built from this rather than from a second front end.
    port: ply_eval::Front,
    sources: ply_eval::SourceMap,
}

impl Program {
    fn machine(&self) -> Machine<'_> {
        crate::tier_machine(&self.port, &self.sources)
    }

    pub fn parse() -> Result<Program> {
        Program::parse_source("bench.ply", BENCH)
    }

    fn parse_source(path: &str, source: &str) -> Result<Program> {
        let name = ModuleName::from_relative_path(Path::new(path))
            .map_err(|d| anyhow::anyhow!("{}", d.message))?;
        let (port, sources) = crate::checked_front_with_std(Path::new(path), name.as_str(), source)
            .map_err(|e| anyhow::anyhow!("checking the bench program: {e:#}"))?;
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

    /// The DDL the fixture is created with, from the program's own `schema()`.
    pub fn ddl(&self) -> Result<Vec<String>> {
        let name = self.full("ddl")?;
        let mut machine = self.machine();
        let value = machine
            .call(&name, vec![], Span::DUMMY)
            .map_err(|d| anyhow::anyhow!("`ddl` raised: {}", d.message))?;
        let Value::List(stmts) = &value else {
            bail!("`ddl` answered {value}, which is not a list of statements");
        };
        stmts.iter().map(statement_of).collect()
    }

    /// One call of one entry point with no host.
    pub fn call_pure(&self, simple: &str, args: Vec<Value>) -> Result<(Duration, Value)> {
        let name = self.full(simple)?;
        let mut machine = self.machine();
        let started = Instant::now();
        let value = machine
            .call(&name, args, Span::DUMMY)
            .map_err(|d| anyhow::anyhow!("`{simple}` raised [{}]: {}", d.code, d.message))?;
        Ok((started.elapsed(), value))
    }

    /// One call of one entry point over a real database.
    fn call_on(
        &self,
        host: &Arc<ply_host::Host>,
        simple: &str,
        args: Vec<Value>,
    ) -> Result<(Duration, Value)> {
        let name = self.full(simple)?;
        let binding = self
            .binding(host)
            .map_err(|d| diagnostics("binding the database", &d))?;
        let mut machine = self.machine();
        machine.set_host_binding(Arc::new(binding));
        machine.set_host_runtime(host.runtime());
        if let Some(declared) = self.footprint(simple) {
            machine.set_declared_footprint(declared);
        }
        let started = Instant::now();
        let value = machine
            .call(&name, args, Span::DUMMY)
            .map_err(|d| anyhow::anyhow!("`{simple}` raised [{}]: {}", d.code, d.message))?;
        Ok((started.elapsed(), value))
    }

    /// One call of one entry point over a real database, for the harness's own questions: W5's
    /// verification reads the desk's schema through the same program every measurement runs.
    pub fn call_served(
        &self,
        host: &Arc<ply_host::Host>,
        simple: &str,
        args: Vec<Value>,
    ) -> Result<(Duration, Value)> {
        self.call_on(host, simple, args)
    }

    /// The same, keeping the diagnostic, whose code the exhaustion row asserts.
    fn refusal_on(
        &self,
        host: &Arc<ply_host::Host>,
        simple: &str,
        args: Vec<Value>,
    ) -> Result<Result<Value, Diagnostic>> {
        let name = self.full(simple)?;
        let binding = self
            .binding(host)
            .map_err(|d| diagnostics("binding the database", &d))?;
        let mut machine = self.machine();
        machine.set_host_binding(Arc::new(binding));
        machine.set_host_runtime(host.runtime());
        if let Some(declared) = self.footprint(simple) {
            machine.set_declared_footprint(declared);
        }
        Ok(machine.call(&name, args, Span::DUMMY))
    }

    fn binding(
        &self,
        host: &Arc<ply_host::Host>,
    ) -> Result<ply_eval::host::HostBinding, Vec<Diagnostic>> {
        let registry: HostRegistry = host.registry();
        registry.bind(&self.check)
    }
}

/// The SQL a `db::Stmt` holds: `{ sql: String }`, and nothing else.
fn statement_of(value: &Value) -> Result<String> {
    let Value::Record(fields) = value else {
        bail!("`ddl` answered {value}, which is not a `Stmt`");
    };
    let Some(sql) = fields.get(&Symbol::new("sql")) else {
        bail!("a `Stmt` with no `sql` field");
    };
    Ok(sql
        .as_str(Span::DUMMY, "a statement's `sql`")
        .map_err(|d| anyhow::anyhow!("a `Stmt` would not decode: {}", d.message))?
        .to_string())
}

/// The one table both handlers use, installed by the program that reads it.
///
/// The harness holds no SQL of its own: the statements are the program's `ddl` and its seed, run
/// through `std.db` over `net` exactly as a served rung runs them. A second client in another
/// language would be a second implementation of the thing under measurement.
pub struct Fixture<'a> {
    program: &'a Program,
    host: Arc<ply_host::Host>,
    url: String,
    /// The pool the harness's own statements use, which is one connection: they are not measured.
    size: usize,
}

impl<'a> Fixture<'a> {
    pub fn create(url: &str, program: &'a Program) -> Result<Fixture<'a>> {
        let fixture = Fixture {
            program,
            host: Arc::new(ply_host::Host::new()),
            url: url.to_string(),
            size: 1,
        };
        fixture.install()?;
        Ok(fixture)
    }

    /// The fixture's schema, applied: the program's own `ddl`, over the one client that may run
    /// `drop` and `create`.
    fn install(&self) -> Result<()> {
        let mut statements = vec!["drop table if exists part cascade".to_string()];
        statements.extend(self.program.ddl()?);
        crate::pg::sql(&self.url, &statements.join("; "))
    }

    /// Refills the table with the twin fixture's keys, dropping earlier writes.
    pub fn reset(&self) -> Result<()> {
        self.fill(64)
    }

    /// The same, at a chosen row count.
    pub fn fill(&self, rows: u32) -> Result<()> {
        self.harness(
            "refill",
            vec![Value::Int(self.size as i64), Value::Int(i64::from(rows))],
        )
    }

    /// One of the harness's own entry points, with the url it reaches the database at.
    fn harness(&self, entry: &str, rest: Vec<Value>) -> Result<()> {
        let mut args = vec![Value::str(&self.url)];
        args.extend(rest);
        self.program.call_on(&self.host, entry, args)?;
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Workload {
    /// `select ... from part order by sku limit 1`, no parameters.
    Select,
    /// The same with a `where sku = $1`, so a `Bind` carries a value.
    SelectParam,
    /// One `insert`, one row, four parameters.
    Insert,
    /// `begin`, one insert, `commit`.
    Transaction,
}

impl Workload {
    pub const ALL: [Workload; 4] = [
        Workload::Select,
        Workload::SelectParam,
        Workload::Insert,
        Workload::Transaction,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Workload::Select => "select",
            Workload::SelectParam => "select $1",
            Workload::Insert => "insert",
            Workload::Transaction => "transaction",
        }
    }

    /// Whether it leaves rows behind, needing a reset and a fresh key range per repeat.
    fn writes(self) -> bool {
        matches!(self, Workload::Insert | Workload::Transaction)
    }

    fn sequential(self) -> &'static str {
        match self {
            Workload::Select => "selects_served",
            Workload::SelectParam => "selects_by_served",
            Workload::Insert => "inserts_served",
            Workload::Transaction => "transactions_served",
        }
    }

    fn concurrent(self) -> &'static str {
        match self {
            Workload::Select => "selects_at_served",
            Workload::SelectParam => "selects_by_at_served",
            Workload::Insert => "inserts_at_served",
            Workload::Transaction => "transactions_at_served",
        }
    }

    /// A served entry point takes where to reach the database and how big a pool to keep, then
    /// whatever the twin of that call takes: the program answers its own `db`.
    fn served_args(self, url: &str, size: usize, base: i64, count: u32) -> Vec<Value> {
        let mut args = vec![Value::str(url), Value::Int(size as i64)];
        args.extend(self.args(base, count));
        args
    }

    fn served_args_at(self, url: &str, size: usize, base: i64, tasks: u32, per: u32) -> Vec<Value> {
        let mut args = vec![Value::str(url), Value::Int(size as i64)];
        args.extend(self.args_at(base, tasks, per));
        args
    }

    pub fn twin(self) -> &'static str {
        match self {
            Workload::Select => "twin_selects",
            Workload::SelectParam => "twin_selects_by",
            Workload::Insert => "twin_inserts",
            Workload::Transaction => "twin_transactions",
        }
    }

    /// Write workloads also take a key base so a repeat does not collide with the last.
    pub fn args(self, base: i64, count: u32) -> Vec<Value> {
        if self.writes() {
            vec![Value::Int(base), Value::Int(count as i64)]
        } else {
            vec![Value::Int(count as i64)]
        }
    }

    fn args_at(self, base: i64, tasks: u32, per: u32) -> Vec<Value> {
        if self.writes() {
            vec![
                Value::Int(base),
                Value::Int(tasks as i64),
                Value::Int(per as i64),
            ]
        } else {
            vec![Value::Int(tasks as i64), Value::Int(per as i64)]
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct OpPoint {
    pub workload: &'static str,
    pub rung: &'static str,
    pub concurrency: u32,
    pub operations: u32,
    pub seconds: f64,
    pub per_second: f64,
    pub per_operation_micros: f64,
}

/// The three rungs, over four workloads, at every concurrency.
pub fn ops(
    url: &str,
    concurrencies: &[u32],
    operations: u32,
    pool: usize,
    repeats: usize,
) -> Result<Vec<OpPoint>> {
    let program = Program::parse()?;
    let fixture = Fixture::create(url, &program)?;
    let mut out = Vec::new();
    // A key base that never repeats *and* never meets the fixture's own keys (`sku-0` … `sku-63`),
    // so no write collides with an earlier point's row or with the rows the reset puts there.
    let mut base: i64 = 1_000_000;
    // The twin's fixture is built through the twin's own scanner inside every `twin_*` call.
    let mut seed = Duration::MAX;
    for _ in 0..repeats.max(2) {
        let (taken, _) = program.call_pure("twin_seed", vec![])?;
        seed = seed.min(taken);
    }
    out.push(OpPoint {
        workload: "twin fixture",
        rung: "ply-twin",
        concurrency: 0,
        operations: 1,
        seconds: seed.as_secs_f64(),
        per_second: 0.0,
        per_operation_micros: micros(seed),
    });

    for workload in Workload::ALL {
        for &concurrency in concurrencies {
            let per = (operations / concurrency).max(1);
            let total = per * concurrency;

            let mut floor = Duration::MAX;
            for _ in 0..repeats {
                fixture.reset()?;
                let taken = floor_run(url, workload, concurrency, per, base)?;
                base += i64::from(total) + 1;
                floor = floor.min(taken);
            }
            out.push(point(workload, "libpq-floor", concurrency, total, floor));

            let mut live = Duration::MAX;
            for _ in 0..repeats {
                fixture.reset()?;
                let host = Arc::new(ply_host::Host::new());
                let (taken, answered) = if concurrency == 1 {
                    program.call_on(
                        &host,
                        workload.sequential(),
                        workload.served_args(url, pool, base, per),
                    )?
                } else {
                    program.call_on(
                        &host,
                        workload.concurrent(),
                        workload.served_args_at(url, pool, base, concurrency, per),
                    )?
                };
                expect(answered, total, workload, "ply-postgres")?;
                base += i64::from(total) + 1;
                live = live.min(taken);
            }
            out.push(point(workload, "ply-postgres", concurrency, total, live));

            // The twin has no server to be concurrent against, so it is not swept.
            if concurrency != concurrencies[0] {
                continue;
            }
            let mut twin = Duration::MAX;
            for _ in 0..repeats {
                let (taken, answered) =
                    program.call_pure(workload.twin(), workload.args(base, total))?;
                expect(answered, total, workload, "ply-twin")?;
                base += i64::from(total) + 1;
                twin = twin.min(taken);
            }
            out.push(point(
                workload,
                "ply-twin",
                concurrency,
                total,
                twin.saturating_sub(seed),
            ));
        }
    }
    Ok(out)
}

fn point(
    workload: Workload,
    rung: &'static str,
    concurrency: u32,
    operations: u32,
    taken: Duration,
) -> OpPoint {
    let seconds = taken.as_secs_f64();
    OpPoint {
        workload: workload.label(),
        rung,
        concurrency,
        operations,
        seconds,
        per_second: operations as f64 / seconds,
        per_operation_micros: micros(taken) / operations as f64,
    }
}

/// A short count means a statement failed, so its time would describe a different workload.
fn expect(answered: Value, want: u32, workload: Workload, rung: &str) -> Result<()> {
    match answered {
        Value::Int(n) if n == i64::from(want) => Ok(()),
        other => bail!(
            "`{}` on `{rung}` answered {other} rows for {want} operations; a statement failed \
             rather than ran",
            workload.label()
        ),
    }
}

/// The same statements, prepared once per connection, with no Ply anywhere.
///
/// The baseline is a C program over libpq rather than a client in this harness: a floor written in
/// Rust would be the harness comparing itself with itself, which is the one thing a floor is for.
fn floor_run(
    url: &str,
    workload: Workload,
    concurrency: u32,
    per: u32,
    base: i64,
) -> Result<Duration> {
    crate::pg::floor(url, workload.label(), concurrency, per, base)
}

#[derive(Clone, Debug, Serialize)]
pub struct SizePoint {
    pub rows: u32,
    pub rung: &'static str,
    pub operations: u32,
    pub per_operation_micros: f64,
    pub per_second: f64,
}

/// One `select ... order by sku limit 1` against `rows` rows, on the twin and on postgres.
pub fn sizes(url: &str, rows: &[u32], operations: u32, repeats: usize) -> Result<Vec<SizePoint>> {
    let program = Program::parse()?;
    let fixture = Fixture::create(url, &program)?;
    let host = Arc::new(ply_host::Host::new());
    let mut out = Vec::new();
    for &n in rows {
        // The twin, with its own fixture build subtracted.
        let mut seed = Duration::MAX;
        let mut whole = Duration::MAX;
        for _ in 0..repeats {
            let (taken, _) = program.call_pure("twin_scan_seed", vec![Value::Int(i64::from(n))])?;
            seed = seed.min(taken);
            let (taken, _) = program.call_pure(
                "twin_scan",
                vec![Value::Int(i64::from(n)), Value::Int(i64::from(operations))],
            )?;
            whole = whole.min(taken);
        }
        let twin = whole.saturating_sub(seed);
        out.push(SizePoint {
            rows: n,
            rung: "ply-twin",
            operations,
            per_operation_micros: micros(twin) / f64::from(operations),
            per_second: f64::from(operations) / twin.as_secs_f64(),
        });

        fixture.fill(n)?;
        let mut live = Duration::MAX;
        for _ in 0..repeats {
            let (taken, _) = program.call_on(
                &host,
                Workload::Select.sequential(),
                Workload::Select.served_args(url, 4, 0, operations),
            )?;
            live = live.min(taken);
        }
        out.push(SizePoint {
            rows: n,
            rung: "ply-postgres",
            operations,
            per_operation_micros: micros(live) / f64::from(operations),
            per_second: f64::from(operations) / live.as_secs_f64(),
        });
    }
    Ok(out)
}

#[derive(Clone, Debug, Serialize)]
pub struct PoolPoint {
    pub workload: &'static str,
    pub pool: usize,
    pub concurrency: u32,
    pub operations: u32,
    pub seconds: f64,
    pub per_second: f64,
}

/// Throughput against pool size, at a fixed number of concurrent tasks.
pub fn pool(
    url: &str,
    sizes: &[usize],
    concurrency: u32,
    operations: u32,
    repeats: usize,
) -> Result<Vec<PoolPoint>> {
    let program = Program::parse()?;
    let fixture = Fixture::create(url, &program)?;
    let mut out = Vec::new();
    let mut base: i64 = 1_000_000;

    for workload in [Workload::Select, Workload::Transaction] {
        for &size in sizes {
            let per = (operations / concurrency).max(1);
            let total = per * concurrency;
            let mut best = Duration::MAX;
            for _ in 0..repeats {
                fixture.reset()?;
                let host = Arc::new(ply_host::Host::new());
                let (taken, answered) = program.call_on(
                    &host,
                    workload.concurrent(),
                    workload.served_args_at(url, size, base, concurrency, per),
                )?;
                expect(answered, total, workload, "pool")?;
                base += i64::from(total) + 1;
                best = best.min(taken);
            }
            let seconds = best.as_secs_f64();
            out.push(PoolPoint {
                workload: workload.label(),
                pool: size,
                concurrency,
                operations: total,
                seconds,
                per_second: total as f64 / seconds,
            });
        }
    }
    Ok(out)
}

#[derive(Clone, Debug, Serialize)]
pub struct Exhaustion {
    pub pool: usize,
    pub concurrency: u32,
    pub acquire_ms: u64,
    /// The diagnostic code the run stopped with, or `"none"` if it completed.
    pub code: String,
    pub message: String,
    pub seconds: f64,
}

/// What a pool smaller than the number of open scopes does.
pub fn exhaustion(url: &str, pool: usize, concurrency: u32, acquire_ms: u64) -> Result<Exhaustion> {
    let program = Program::parse()?;
    let fixture = Fixture::create(url, &program)?;
    fixture.reset()?;
    // The pool size is the run's argument and the acquire bound is the driver's own: `std.db`
    // refuses a checkout past its size rather than waiting, so a run below takes the refusal.
    let host = Arc::new(ply_host::Host::new());
    let started = Instant::now();
    let answered = program.refusal_on(
        &host,
        Workload::Transaction.concurrent(),
        Workload::Transaction.served_args_at(url, pool, 9_000_000, concurrency, 8),
    )?;
    let seconds = started.elapsed().as_secs_f64();
    Ok(match answered {
        Ok(_) => Exhaustion {
            pool,
            concurrency,
            acquire_ms,
            code: "none".to_string(),
            message: "every task got a connection".to_string(),
            seconds,
        },
        Err(d) => Exhaustion {
            pool,
            concurrency,
            acquire_ms,
            code: d.code.to_string(),
            message: d.message.clone(),
            seconds,
        },
    })
}

/// Which store the served `examples/desk.ply` runs over.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Store {
    /// `DESK_STORE=postgres`: the `db` atoms reach postgres.
    Postgres,
    /// `DESK_STORE=memory`: they reach the twin.
    Twin,
}

impl Store {
    pub fn label(self) -> &'static str {
        match self {
            Store::Postgres => "postgres",
            Store::Twin => "twin",
        }
    }
}

/// Which accept loop a served desk runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Variant {
    /// One connection at a time.
    Sequential,
    /// A task per connection on the production scheduler.
    TaskPerConn,
}

impl Variant {
    pub fn label(self) -> &'static str {
        match self {
            Variant::Sequential => "sequential",
            Variant::TaskPerConn => "task-per-conn",
        }
    }

    /// What `DESK_ACCEPT` calls it.
    fn setting(self) -> &'static str {
        match self {
            Variant::Sequential => "sequential",
            Variant::TaskPerConn => "task-per-connection",
        }
    }
}

/// `examples/desk.ply`, copied into a project `ply run --host` can be pointed at; which store and
/// accept loop it serves are settings `served_args` carries.
pub fn project(dir: &Path, repo: &Path) -> Result<()> {
    std::fs::copy(repo.join("examples/desk.ply"), dir.join("desk.ply"))
        .context("copying `examples/desk.ply`")?;
    Ok(())
}

/// The command line a served desk runs on: which config schema, its own settings, and where its
/// database is when it has one, which is also what chooses postgres over the twin.
///
/// One function rather than four, so a flag the CLI does not declare is one edit from being caught:
/// `a_served_run_takes_the_flags_the_sections_pass` drives the real CLI with this.
pub fn served_args(
    port: u16,
    connections: u32,
    api_key: &str,
    database: Option<&str>,
    variant: Variant,
) -> Vec<String> {
    let mut args = vec![
        "--config-schema".to_string(),
        "desk.config".to_string(),
        "--set".to_string(),
        format!("DESK_PORT={port}"),
        "--set".to_string(),
        format!("DESK_CONNECTIONS={connections}"),
        // A fixture value: `desk.config` declares the key `required`, so a run needs one.
        "--set".to_string(),
        format!("DESK_API_KEY={api_key}"),
        "--set".to_string(),
        format!(
            "DESK_STORE={}",
            if database.is_some() {
                "postgres"
            } else {
                "memory"
            }
        ),
        "--set".to_string(),
        format!("DESK_ACCEPT={}", variant.setting()),
    ];
    if let Some(url) = database {
        args.push("--set".to_string());
        args.push(format!("DESK_DATABASE={url}"));
    }
    args
}

/// Throughput and tail latency per route and concurrency, over the real binary and socket.
pub fn crud(
    repo: &Path,
    ply: &Path,
    url: &str,
    stores: &[Store],
    concurrencies: &[u32],
    per_conn: u32,
    requests_per_point: u32,
) -> Result<Vec<LoadPoint>> {
    let routes: [(&'static str, &'static str); 3] = [
        ("health (no db)", "/health"),
        ("items (1 select)", "/items"),
        ("order (1 select $1)", "/orders/1"),
    ];
    let mut out = Vec::new();
    for &store in stores {
        for &concurrency in concurrencies {
            let conns_per_thread = share(concurrency, per_conn, requests_per_point);
            let budget = concurrency * conns_per_thread * routes.len() as u32 + 1;
            let dir = tempfile::tempdir().context("a temp dir for the served project")?;
            let port = reserve_port()?;
            project(dir.path(), repo)?;
            let mut args = served_args(
                port,
                budget,
                "bench-key",
                (store == Store::Postgres).then_some(url),
                Variant::Sequential,
            );
            args.extend(["--trace".to_string(), "off".to_string()]);
            let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
            let mut server = Server::start(ply, dir.path(), &borrowed)?;
            let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
            wait_until_serving(&mut server, addr)?;
            for (name, path) in routes {
                out.push(load_point(
                    &mut server,
                    addr,
                    store.label(),
                    name,
                    path,
                    concurrency,
                    per_conn,
                    conns_per_thread,
                )?);
            }
            server.finish()?;
        }
    }
    Ok(out)
}

// --- A served desk and the Ply load client, for `crud` and `w5`'s served sections ------------

/// A port nothing is listening on.
pub fn reserve_port() -> Result<u16> {
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").context("reserving an ephemeral port")?;
    Ok(listener.local_addr()?.port())
}

/// `ply run --host`, killed however the harness leaves.
pub struct Server {
    child: Option<Child>,
    /// The binary this was started with, which is also the one a load client runs.
    ply: PathBuf,
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
        Ok(Server {
            child: Some(child),
            ply: ply.to_path_buf(),
        })
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

    /// The output if the server has died; never blocks on a live process's pipe.
    fn output_if_exited(&mut self) -> String {
        match self.exited() {
            Ok(Some(status)) => format!("the server exited {status}:\n{}", self.take()),
            Ok(None) => "the server was still running".to_string(),
            Err(e) => format!("the server could not be waited on: {e}"),
        }
    }

    /// The server was given its fixed connection count, so it must exit on its own.
    pub fn finish(mut self) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let child = self.child.as_mut().expect("the server has not been reaped");
            match child.try_wait()? {
                Some(status) if status.success() => return Ok(()),
                Some(status) => bail!("the server exited {status}:\n{}", self.take()),
                None if Instant::now() >= deadline => {
                    bail!("the server was still running a minute after every connection")
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

/// The load client's own source, so the file the harness runs and the file the corpus checks are
/// one file.
const LOAD_CLIENT: &str = include_str!("../fixtures/load.ply");

/// What one run of the load client found, as the program wrote it down.
#[derive(Clone, Debug, Deserialize)]
struct Report {
    answered: u32,
    wall_micros: i64,
    latencies: Vec<i64>,
    statuses: Vec<Status>,
    failures: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
struct Status {
    status: u16,
    count: u32,
}

/// The load client: the program on disk and a root its report is written under.
struct LoadClient {
    ply: PathBuf,
    dir: tempfile::TempDir,
    root: tempfile::TempDir,
}

impl LoadClient {
    fn new(ply: &Path) -> Result<LoadClient> {
        let dir = tempfile::tempdir().context("a temp dir for the load client")?;
        std::fs::write(dir.path().join("load.ply"), LOAD_CLIENT)
            .context("writing the load client")?;
        let root = tempfile::tempdir().context("a temp dir for the load client's report")?;
        Ok(LoadClient {
            ply: ply.to_path_buf(),
            dir,
            root,
        })
    }

    /// One run: `connections` connections carrying `per_conn` requests each over `paths`, over TLS
    /// when `trust` names the certificate to accept.
    fn measure(
        &self,
        addr: std::net::SocketAddr,
        paths: &str,
        connections: u32,
        per_conn: u32,
        trust: Option<&Path>,
    ) -> Result<Report> {
        let report = "load.json";
        // A stale report would be read as this run's.
        let _ = std::fs::remove_file(self.root.path().join(report));
        let mut command = Command::new(&self.ply);
        command
            .args(["run", "--host", "--color", "never"])
            .arg("--fs")
            .arg(format!("report={}", self.root.path().display()))
            // The name `certgen` issues a certificate for, which is what TLS verifies.
            .args(["--set", "LOAD_HOST=localhost"])
            .args(["--set", &format!("LOAD_PORT={}", addr.port())])
            .args(["--set", &format!("LOAD_ROUTES={paths}")])
            .args(["--set", &format!("LOAD_CONNECTIONS={connections}")])
            .args(["--set", &format!("LOAD_PER_CONN={per_conn}")])
            .args(["--set", &format!("LOAD_TLS={}", trust.is_some())])
            .args(["--set", &format!("LOAD_REPORT={report}")])
            .current_dir(self.dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(certificate) = trust {
            command.arg("--trust").arg(certificate);
        }
        let out = command.output().with_context(|| {
            format!(
                "running `{} run --host` as a load client",
                self.ply.display()
            )
        })?;
        if !out.status.success() {
            bail!(
                "the load client exited {} at {connections}x{per_conn} on {paths}:\n{}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let path = self.root.path().join(report);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("the load client wrote no report to `{}`", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("reading `{}`", path.display()))
    }
}

/// Connections per client for a point, so each carries about `requests_per_point` requests.
pub fn share(concurrency: u32, per_conn: u32, requests_per_point: u32) -> u32 {
    let wanted = requests_per_point.div_ceil(concurrency * per_conn).max(1);
    wanted.min((1000 / concurrency).max(1))
}

#[derive(Clone, Debug, Serialize)]
pub struct LoadPoint {
    pub variant: &'static str,
    pub transport: &'static str,
    pub label: String,
    pub concurrency: u32,
    pub per_conn: u32,
    pub connections: u32,
    pub requests: u32,
    pub seconds: f64,
    pub per_second: f64,
    pub p50_micros: f64,
    pub p95_micros: f64,
    pub p99_micros: f64,
    pub max_micros: f64,
}

/// Nearest-rank.
fn percentile(of: &[i64], p: f64) -> f64 {
    if of.is_empty() {
        return 0.0;
    }
    let mut sorted = of.to_vec();
    sorted.sort_unstable();
    let rank = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted[rank - 1] as f64
}

/// A partial answer is a different measurement, not a slower server.
fn require(report: &Report, requests: u32) -> Result<()> {
    if !report.failures.is_empty() || report.answered != requests {
        bail!(
            "{} of {requests} requests were answered ({} failures); first: {}",
            report.answered,
            report.failures.len(),
            report
                .failures
                .first()
                .map(String::as_str)
                .unwrap_or("none recorded")
        );
    }
    let bad: Vec<String> = report
        .statuses
        .iter()
        .filter(|s| s.status != 200)
        .map(|s| format!("{}x {}", s.count, s.status))
        .collect();
    if !bad.is_empty() {
        bail!("the server answered {}", bad.join(", "));
    }
    Ok(())
}

/// Block until the server answers a real request, so timing does not race its typecheck.
pub fn wait_until_serving(server: &mut Server, addr: std::net::SocketAddr) -> Result<()> {
    wait_until_serving_over(server, addr, None)
}

/// The same over whichever transport the server was started with; `trust` is a certificate a TLS
/// client accepts beside the built-in roots, which is what makes it speak TLS at all.
pub fn wait_until_serving_over(
    server: &mut Server,
    addr: std::net::SocketAddr,
    trust: Option<&Path>,
) -> Result<()> {
    let client = LoadClient::new(&server.ply)?;
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if let Some(status) = server.exited()? {
            bail!(
                "the server exited {status} before listening:\n{}",
                server.take()
            );
        }
        if let Ok(report) = client.measure(addr, "/health", 1, 1, trust)
            && report.answered == 1
            && report.failures.is_empty()
        {
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
    addr: std::net::SocketAddr,
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
    addr: std::net::SocketAddr,
    trust: Option<&Path>,
    variant: &'static str,
    label: &'static str,
    path: &str,
    concurrency: u32,
    per_conn: u32,
    conns_per_thread: u32,
) -> Result<LoadPoint> {
    let connections = concurrency * conns_per_thread;
    let requests = connections * per_conn;
    let client = LoadClient::new(&server.ply)?;
    let measured = client.measure(addr, path, connections, per_conn, trust);
    let report = measured.with_context(|| {
        format!(
            "{label} at concurrency {concurrency}\n{}",
            server.output_if_exited()
        )
    })?;
    require(&report, requests).with_context(|| {
        format!(
            "{label} at concurrency {concurrency}\n{}",
            server.output_if_exited()
        )
    })?;
    let seconds = report.wall_micros as f64 / 1e6;
    Ok(LoadPoint {
        variant,
        transport: if trust.is_some() { "https" } else { "http" },
        label: label.to_string(),
        concurrency,
        per_conn,
        connections,
        requests,
        seconds,
        per_second: if seconds > 0.0 {
            requests as f64 / seconds
        } else {
            0.0
        },
        p50_micros: percentile(&report.latencies, 0.50),
        p95_micros: percentile(&report.latencies, 0.95),
        p99_micros: percentile(&report.latencies, 0.99),
        max_micros: percentile(&report.latencies, 1.0),
    })
}

#[derive(Default, Serialize)]
pub struct Measurements {
    pub ops: Vec<OpPoint>,
    pub sizes: Vec<SizePoint>,
    pub pool: Vec<PoolPoint>,
    pub exhaustion: Vec<Exhaustion>,
    pub crud: Vec<LoadPoint>,
}

pub fn render(m: &Measurements) -> String {
    let mut out = String::new();
    if !m.ops.is_empty() {
        out.push_str("\nops — one statement through the effect boundary\n\n");
        out.push_str(
            "  workload      rung           conc     ops      us/op       ops/s   over floor\n",
        );
        for point in &m.ops {
            let floor = m
                .ops
                .iter()
                .find(|p| {
                    p.workload == point.workload
                        && p.concurrency == point.concurrency
                        && p.rung == "libpq-floor"
                })
                .map(|p| p.per_operation_micros);
            let over = match floor {
                Some(f) if point.rung != "libpq-floor" && f > 0.0 => {
                    format!(
                        "{:+.1}us {:.2}x",
                        point.per_operation_micros - f,
                        point.per_operation_micros / f
                    )
                }
                _ => "—".to_string(),
            };
            let _ = writeln!(
                out,
                "  {:<13} {:<13} {:>4} {:>7} {:>10.1} {:>11.0}   {}",
                point.workload,
                point.rung,
                point.concurrency,
                point.operations,
                point.per_operation_micros,
                point.per_second,
                over
            );
        }
    }
    if !m.sizes.is_empty() {
        out.push_str("\nsizes — one `order by … limit 1` against the rows it sorts\n\n");
        out.push_str("   rows  rung             ops       us/op        ops/s\n");
        for point in &m.sizes {
            let _ = writeln!(
                out,
                "  {:>5}  {:<13} {:>7} {:>11.1} {:>12.0}",
                point.rows,
                point.rung,
                point.operations,
                point.per_operation_micros,
                point.per_second
            );
        }
    }
    if !m.pool.is_empty() {
        out.push_str("\npool — throughput against pool size\n\n");
        out.push_str("  workload      pool  conc     ops    seconds       ops/s\n");
        for point in &m.pool {
            let _ = writeln!(
                out,
                "  {:<13} {:>4} {:>5} {:>7} {:>10.3} {:>11.0}",
                point.workload,
                point.pool,
                point.concurrency,
                point.operations,
                point.seconds,
                point.per_second
            );
        }
    }
    if !m.exhaustion.is_empty() {
        out.push_str("\nexhaustion — a pool smaller than the open scopes\n\n");
        out.push_str("  pool  conc  acquire     after   code    what the run said\n");
        for point in &m.exhaustion {
            let _ = writeln!(
                out,
                "  {:>4} {:>5} {:>8}ms {:>8.3}s  {:<6}  {}",
                point.pool,
                point.concurrency,
                point.acquire_ms,
                point.seconds,
                point.code,
                point.message
            );
        }
    }
    if !m.crud.is_empty() {
        out.push_str("\ncrud — a route that hits the database against one that does not\n\n");
        out.push_str(
            "  store      route                  conc    reqs     req/s     p50     p95     p99\n",
        );
        for point in &m.crud {
            let _ = writeln!(
                out,
                "  {:<10} {:<21} {:>5} {:>7} {:>9.0} {:>7.0} {:>7.0} {:>7.0}",
                point.variant,
                point.label,
                point.concurrency,
                point.requests,
                point.per_second,
                point.p50_micros,
                point.p95_micros,
                point.p99_micros
            );
        }
    }
    out
}
