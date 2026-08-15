//! What a database costs, on both sides of the effect boundary.
//!
//! `w3` priced a service whose store was an in-memory value. W4 put a real
//! postgres behind the same `db` atoms, and this module measures the three
//! numbers that decision is worth judging on:
//!
//! | section | question |
//! | --- | --- |
//! | [`ops`] | one statement through the boundary, against the same statement with no Ply in the path |
//! | [`pool`] | throughput against pool size, and what exhaustion does |
//! | [`crud`] | a route that hits the database against one that does not, under real load |
//!
//! **The interesting number is the delta, not the total.** A query against
//! loopback postgres costs tens of microseconds of server and protocol time
//! whatever language issued it, so a table that printed only "Ply did a select
//! in N microseconds" would be a table about postgres. Every section below is
//! therefore a *substitution*: the same statement text, the same server, the
//! same connection count, with one layer swapped underneath. A difference
//! between two rows is that layer and nothing else.
//!
//! The rungs, in the order [`ops`] prints them:
//!
//! | rung | what issues the statement | what it adds |
//! | --- | --- | --- |
//! | `rust-floor` | `tokio-postgres`, prepared once, on this harness's own runtime | the denominator: the server, the wire, and the client protocol |
//! | `ply-postgres` | a Ply `perform` through `ply_host::db` | the scan, the footprint check, the reactor hop, the pending token and the `Value` conversion |
//! | `ply-twin` | the same Ply, over `std.db`'s in-memory engine | no server at all: what a hermetic test pays instead |
//!
//! The program under measurement is `crates/ply-corpus/ply/w4.ply`, which is
//! one table and four workloads written once and reached by both handlers. It
//! is Ply source checked by the real front end on every run, so a workload that
//! stopped typechecking is a failed benchmark rather than a wrong number.

use anyhow::{Context, Result, bail};
use ply_core::CheckOutput;
use ply_core::ty::Footprint;
use ply_eval::host::HostRegistry;
use ply_eval::{Machine, Value};
use ply_host::db::{PoolConfig, value as dbvalue};
use ply_span::{Diagnostic, Span};
use ply_syntax::ast::ModuleName;
use serde::Serialize;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::serve::{Server, reserve_port};
use crate::w3;

/// The program the `ops` and `pool` sections run.
const BENCH: &str = include_str!("../ply/w4.ply");

/// Where `examples/desk.ply` stops being the service. Shared with [`w3`], which
/// splits it at the same line.
fn desk_service(repo: &Path) -> Result<String> {
    w3::Service::open(repo)?.source(w3::Variant::Sequential)
}

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

/// The checked bench program, and the pieces a run needs off it.
pub struct Program {
    program: ply_syntax::ast::Program,
    resolved: ply_syntax::resolve::Resolved,
    check: CheckOutput,
}

impl Program {
    pub fn parse() -> Result<Program> {
        Program::parse_source("bench.ply", BENCH)
    }

    fn parse_source(path: &str, source: &str) -> Result<Program> {
        let mut sources = ply_span::SourceMap::new();
        let id = sources.add(Path::new(path), source.to_string());
        let name = ModuleName::from_relative_path(Path::new(path))
            .map_err(|d| anyhow::anyhow!("{}", d.message))?;
        let mut inputs = vec![(id, name, source)];
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
        let resolved = ply_syntax::resolve::resolve(&program)
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

    fn footprint(&self, simple: &str) -> Option<Footprint> {
        self.check
            .defs
            .values()
            .find(|d| d.simple_name.as_str() == simple && d.module.to_string() == "bench")
            .map(|d| d.footprint.clone())
    }

    /// The DDL the fixture is created with, taken from the program's own
    /// `schema()` rather than restated here. A second copy of the schema is a
    /// second thing to drift.
    pub fn ddl(&self) -> Result<Vec<String>> {
        let name = self.full("ddl")?;
        let mut machine = Machine::new(&self.program, &self.resolved, &self.check);
        let value = machine
            .call(&name, vec![], Span::DUMMY)
            .map_err(|d| anyhow::anyhow!("`ddl` raised: {}", d.message))?;
        let Value::List(stmts) = &value else {
            bail!("`ddl` answered {value}, which is not a list of statements");
        };
        stmts
            .iter()
            .map(|s| {
                dbvalue::statement(s, Span::DUMMY)
                    .map_err(|d| anyhow::anyhow!("a `Stmt` would not decode: {}", d.message))
            })
            .collect()
    }

    /// One call of one entry point over a hermetic machine — no host at all.
    fn call_pure(&self, simple: &str, args: Vec<Value>) -> Result<(Duration, Value)> {
        let name = self.full(simple)?;
        let mut machine = Machine::new(&self.program, &self.resolved, &self.check);
        let started = Instant::now();
        let value = machine
            .call(&name, args, Span::DUMMY)
            .map_err(|d| anyhow::anyhow!("`{simple}` raised [{}]: {}", d.code, d.message))?;
        Ok((started.elapsed(), value))
    }

    /// One call of one entry point over a real database.
    ///
    /// The `Host` is built per call rather than shared, because the pool, the
    /// reactor thread and the scope table are what a run's configuration is,
    /// and a section that swept pool size over one shared pool would be
    /// sweeping nothing.
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
        let mut machine = Machine::new(&self.program, &self.resolved, &self.check);
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

    /// The same, keeping the diagnostic rather than the value: the exhaustion
    /// row of [`pool`] is a refusal, and a harness that turned it into an error
    /// string would have thrown away the code it is asserting.
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
        let mut machine = Machine::new(&self.program, &self.resolved, &self.check);
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

// --- The fixture ------------------------------------------------------------

/// The one table both handlers use, created from the program's own schema.
///
/// Dropped and recreated per run: a benchmark that inserted into a table left
/// over from the last one would be measuring an index that grew between two
/// invocations.
pub struct Fixture {
    url: String,
}

impl Fixture {
    pub fn create(url: &str, program: &Program) -> Result<Fixture> {
        let ddl = program.ddl()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("a runtime for the fixture")?;
        runtime.block_on(async {
            let (client, connection) = tokio_postgres::connect(url, tokio_postgres::NoTls)
                .await
                .with_context(|| format!("connecting to `{url}`"))?;
            let handle = tokio::spawn(connection);
            client
                .batch_execute("drop table if exists part cascade")
                .await
                .context("dropping the fixture table")?;
            for statement in &ddl {
                client
                    .batch_execute(statement)
                    .await
                    .with_context(|| format!("creating the fixture: `{statement}`"))?;
            }
            handle.abort();
            Ok::<(), anyhow::Error>(())
        })?;
        Ok(Fixture {
            url: url.to_string(),
        })
    }

    /// The table as a read workload needs it: the keys the twin's own fixture
    /// holds, and nothing a previous write workload left behind.
    pub fn reset(&self) -> Result<()> {
        self.fill(64)
    }

    /// The same, at a chosen row count.
    pub fn fill(&self, rows: u32) -> Result<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(async {
            let (client, connection) = tokio_postgres::connect(&self.url, tokio_postgres::NoTls)
                .await
                .context("connecting to reset the fixture")?;
            let handle = tokio::spawn(connection);
            client.batch_execute("truncate part").await?;
            // The same rows `bench.ply`'s `seeded_to` puts in the twin, by the
            // same keys, so a parameterised select finds a row on both sides.
            client
                .batch_execute(&format!(
                    "insert into part (sku, name, price, n) \
                     select 'sku-' || g, 'a part', 1.2500, g \
                     from generate_series(0, {}) g",
                    rows.saturating_sub(1)
                ))
                .await?;
            handle.abort();
            Ok::<(), anyhow::Error>(())
        })
    }
}

// --- The workloads ----------------------------------------------------------

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

    /// Whether a run of it leaves rows behind, and therefore needs the fixture
    /// reset and a fresh key range per repeat.
    fn writes(self) -> bool {
        matches!(self, Workload::Insert | Workload::Transaction)
    }

    fn sequential(self) -> &'static str {
        match self {
            Workload::Select => "selects",
            Workload::SelectParam => "selects_by",
            Workload::Insert => "inserts",
            Workload::Transaction => "transactions",
        }
    }

    fn concurrent(self) -> &'static str {
        match self {
            Workload::Select => "selects_at",
            Workload::SelectParam => "selects_by_at",
            Workload::Insert => "inserts_at",
            Workload::Transaction => "transactions_at",
        }
    }

    fn twin(self) -> &'static str {
        match self {
            Workload::Select => "twin_selects",
            Workload::SelectParam => "twin_selects_by",
            Workload::Insert => "twin_inserts",
            Workload::Transaction => "twin_transactions",
        }
    }

    /// The arguments the sequential entry point takes: the write workloads need
    /// a key base so a repeat does not collide with the last one.
    fn args(self, base: i64, count: u32) -> Vec<Value> {
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

// --- Section 1: one statement through the boundary --------------------------

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
///
/// Concurrency on the Ply rungs is `task.spawn` on the production scheduler
/// rather than a thread per client: that is what a service does, it is what
/// makes a transaction scope belong to a task, and it is the only shape in
/// which `E0437` can be reached at all.
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
    // A key base that never repeats across the whole table, so no write
    // workload ever collides with a row an earlier point inserted.
    let mut base: i64 = 1;
    // The twin's fixture is built through the twin's own scanner inside every
    // `twin_*` call. It is a setup cost and not an operation, so it is measured
    // once, subtracted, and printed as its own row.
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

            // The floor.
            let mut floor = Duration::MAX;
            for _ in 0..repeats {
                fixture.reset()?;
                let taken = floor_run(url, workload, concurrency, per, base)?;
                base += i64::from(total) + 1;
                floor = floor.min(taken);
            }
            out.push(point(workload, "rust-floor", concurrency, total, floor));

            // Ply, over the same server.
            let mut live = Duration::MAX;
            for _ in 0..repeats {
                fixture.reset()?;
                let host = Arc::new(
                    ply_host::Host::with_database(
                        ply_host::Credentials::empty(),
                        config(url, pool),
                    )
                    .map_err(|d| anyhow::anyhow!("[{}] {}", d.code, d.message))?,
                );
                let (taken, answered) = if concurrency == 1 {
                    program.call_on(&host, workload.sequential(), workload.args(base, per))?
                } else {
                    program.call_on(
                        &host,
                        workload.concurrent(),
                        workload.args_at(base, concurrency, per),
                    )?
                };
                expect(answered, total, workload, "ply-postgres")?;
                base += i64::from(total) + 1;
                live = live.min(taken);
            }
            out.push(point(workload, "ply-postgres", concurrency, total, live));

            // The twin has no server to be concurrent against — it is a value
            // threaded through a cell — so it is measured at the first
            // concurrency and not swept.
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

/// A workload that answered fewer operations than it was asked for did not run
/// the workload, and reporting its time would be reporting a different one.
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

fn config(url: &str, pool: usize) -> PoolConfig {
    PoolConfig {
        size: pool,
        ..PoolConfig::new(url)
    }
}

// --- The floor --------------------------------------------------------------

/// The same statements, prepared once per connection, with no Ply anywhere.
///
/// One current-thread runtime and `concurrency` connections, which is the shape
/// the driver's reactor has: a floor taken on a work-stealing runtime would be
/// a floor for a different program.
fn floor_run(
    url: &str,
    workload: Workload,
    concurrency: u32,
    per: u32,
    base: i64,
) -> Result<Duration> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("a runtime for the floor")?;
    runtime.block_on(async move {
        let mut clients = Vec::new();
        let mut connections = Vec::new();
        for _ in 0..concurrency {
            let (client, connection) = tokio_postgres::connect(url, tokio_postgres::NoTls)
                .await
                .context("connecting for the floor")?;
            connections.push(tokio::spawn(connection));
            clients.push(client);
        }
        let started = Instant::now();
        // Spawned rather than awaited in turn: the point of the row is that
        // `concurrency` statements are in flight at once, which is what the Ply
        // rung above it does with `task.spawn`.
        let mut running = tokio::task::JoinSet::new();
        for (slot, client) in clients.into_iter().enumerate() {
            let from = base + (slot as i64) * i64::from(per);
            running.spawn(async move { floor_task(client, workload, per, from).await });
        }
        while let Some(joined) = running.join_next().await {
            joined.context("a floor task panicked")??;
        }
        let taken = started.elapsed();
        for handle in connections {
            handle.abort();
        }
        Ok(taken)
    })
}

async fn floor_task(
    client: tokio_postgres::Client,
    workload: Workload,
    per: u32,
    base: i64,
) -> Result<()> {
    let select_all = client
        .prepare("select sku, name, price, n from part order by sku limit 1")
        .await?;
    let select_by = client
        .prepare("select sku, name, price, n from part where sku = $1")
        .await?;
    let insert = client
        .prepare("insert into part (sku, name, price, n) values ($1, $2, $3, $4)")
        .await?;
    let price = rust_decimal::Decimal::new(12500, 4);
    for i in 0..per {
        match workload {
            Workload::Select => {
                client.query(&select_all, &[]).await?;
            }
            Workload::SelectParam => {
                let sku = format!("sku-{}", i % 64);
                client.query(&select_by, &[&sku]).await?;
            }
            Workload::Insert => {
                let n = base + i64::from(i);
                let sku = format!("sku-{n}");
                client
                    .execute(&insert, &[&sku, &"a part", &price, &n])
                    .await?;
            }
            Workload::Transaction => {
                let n = base + i64::from(i);
                let sku = format!("sku-{n}");
                client.batch_execute("begin").await?;
                client
                    .execute(&insert, &[&sku, &"a part", &price, &n])
                    .await?;
                client.batch_execute("commit").await?;
            }
        }
    }
    Ok(())
}

// --- Section 2: what the twin costs as its table grows -----------------------

#[derive(Clone, Debug, Serialize)]
pub struct SizePoint {
    pub rows: u32,
    pub rung: &'static str,
    pub operations: u32,
    pub per_operation_micros: f64,
    pub per_second: f64,
}

/// One `select ... order by sku limit 1` against a table of `rows` rows, on the
/// twin and on postgres.
///
/// This is the axis a per-statement comparison hides. `std.db`'s `ORDER BY` is
/// an insertion sort over `Row`s that are `Map`s, so it is quadratic in the
/// table and logarithmic in nothing; postgres has an index. A test suite whose
/// fixtures are three rows never sees it, and one whose fixtures are a thousand
/// sees nothing else.
pub fn sizes(url: &str, rows: &[u32], operations: u32, repeats: usize) -> Result<Vec<SizePoint>> {
    let program = Program::parse()?;
    let fixture = Fixture::create(url, &program)?;
    let host = Arc::new(
        ply_host::Host::with_database(ply_host::Credentials::empty(), config(url, 4))
            .map_err(|d| anyhow::anyhow!("[{}] {}", d.code, d.message))?,
    );
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

        // Postgres, over the same statement and the same row count.
        fixture.fill(n)?;
        let mut live = Duration::MAX;
        for _ in 0..repeats {
            let (taken, _) =
                program.call_on(&host, "selects", vec![Value::Int(i64::from(operations))])?;
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

// --- Section 3: the pool ----------------------------------------------------

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
///
/// The read workload holds a connection for one statement and gives it back;
/// the transaction workload holds one for the whole scope. So the two columns
/// are the two shapes a pool sees, and only the second can exhaust it.
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
                let host = Arc::new(
                    ply_host::Host::with_database(
                        ply_host::Credentials::empty(),
                        config(url, size),
                    )
                    .map_err(|d| anyhow::anyhow!("[{}] {}", d.code, d.message))?,
                );
                let (taken, answered) = program.call_on(
                    &host,
                    workload.concurrent(),
                    workload.args_at(base, concurrency, per),
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
///
/// ADR 0014 §3.2 makes this `E0437` and not a `Failed`, and not a hang: the
/// program asked for a connection and the run said how many exist. This is the
/// row that checks it is a sentence rather than a deadlock.
pub fn exhaustion(url: &str, pool: usize, concurrency: u32, acquire_ms: u64) -> Result<Exhaustion> {
    let program = Program::parse()?;
    let fixture = Fixture::create(url, &program)?;
    fixture.reset()?;
    let mut settings = config(url, pool);
    settings.acquire = Duration::from_millis(acquire_ms);
    let host = Arc::new(
        ply_host::Host::with_database(ply_host::Credentials::empty(), settings)
            .map_err(|d| anyhow::anyhow!("[{}] {}", d.code, d.message))?,
    );
    let started = Instant::now();
    let answered = program.refusal_on(
        &host,
        Workload::Transaction.concurrent(),
        Workload::Transaction.args_at(9_000_000, concurrency, 8),
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

// --- Section 3: the service under load --------------------------------------

/// Which store the served `examples/desk.ply` runs over.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Store {
    /// `main` calls `run`, and the `db` atoms reach postgres.
    Postgres,
    /// `main` calls `run_memory`, and they reach the twin. The W3 shape.
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

/// `examples/desk.ply` as a project `ply run --host` can be pointed at, with its
/// store rewritten.
///
/// The port and the connection budget are **not** rewritten any more: W5 made
/// both configuration, so they arrive as `--set DESK_PORT=` and `--set
/// DESK_CONNECTIONS=` beside the run, which is one fewer thing this harness has
/// to keep matching the example's source. Which store a service uses is not
/// configuration, so that one is still a rewrite — and it asserts it found what
/// it replaced, because a silent miss would leave the harness measuring a
/// program it guessed at.
fn project(dir: &Path, service: &str, store: Store) -> Result<()> {
    let source = if store == Store::Twin {
        let narrowed = replace(
            service,
            "fn main() -> Int / {Serving, config.read[server], net.write[conn], net.write[listener]} = {",
            "fn main() -> Int / {config.read[server], config.read[credentials], net.write[conn], net.write[listener]} = {",
        )?;
        replace(
            &narrowed,
            "    run(port, count)",
            "    run_memory(port, key, count)",
        )?
    } else {
        service.to_string()
    };
    std::fs::write(dir.join("desk.ply"), source)?;
    Ok(())
}

/// The `--set` arguments a served desk needs, now that its port, its budget and
/// its credential are configuration rather than definitions.
fn settings(port: u16, connections: u32) -> Vec<String> {
    vec![
        format!("DESK_PORT={port}"),
        format!("DESK_CONNECTIONS={connections}"),
        // A benchmark's key is a fixture credential and is not a credential;
        // what it is here for is that `--config-schema desk.config` declares the
        // key `required`, so a run without one refuses to start.
        "DESK_API_KEY=bench-key".to_string(),
    ]
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

/// Throughput and tail latency for one route at one concurrency, over the real
/// binary and a real socket.
///
/// The routes are chosen so the delta is the database and nothing else:
/// `/health` reaches no `db` atom at all and is W3's number re-taken on this
/// machine, `/items` is one `select` behind the same framing, and `/orders/1`
/// is one parameterised `select`.
pub fn crud(
    repo: &Path,
    ply: &Path,
    url: &str,
    stores: &[Store],
    concurrencies: &[u32],
    per_conn: u32,
    requests_per_point: u32,
) -> Result<Vec<w3::LoadPoint>> {
    let service = desk_service(repo)?;
    let routes: [(&'static str, &'static str); 3] = [
        ("health (no db)", "/health"),
        ("items (1 select)", "/items"),
        ("order (1 select $1)", "/orders/1"),
    ];
    let mut out = Vec::new();
    for &store in stores {
        for &concurrency in concurrencies {
            let conns_per_thread = w3::share(concurrency, per_conn, requests_per_point);
            let budget = concurrency * conns_per_thread * routes.len() as u32 + 1;
            let dir = tempfile::tempdir().context("a temp dir for the served project")?;
            let port = reserve_port()?;
            project(dir.path(), &service, store)?;
            let sets = settings(port, budget);
            let mut args: Vec<&str> = vec!["--config-schema", "desk.config", "--trace", "off"];
            for set in &sets {
                args.push("--set");
                args.push(set);
            }
            if store == Store::Postgres {
                args.extend(["--db", url, "--db-schema", "desk.schema"]);
            }
            let mut server = Server::start(ply, dir.path(), &args)?;
            let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
            w3::wait_until_serving(&mut server, addr)?;
            for (name, path) in routes {
                out.push(w3::load_point(
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

// --- The report -------------------------------------------------------------

#[derive(Default, Serialize)]
pub struct Measurements {
    pub ops: Vec<OpPoint>,
    pub sizes: Vec<SizePoint>,
    pub pool: Vec<PoolPoint>,
    pub exhaustion: Vec<Exhaustion>,
    pub crud: Vec<w3::LoadPoint>,
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
                        && p.rung == "rust-floor"
                })
                .map(|p| p.per_operation_micros);
            let over = match floor {
                Some(f) if point.rung != "rust-floor" && f > 0.0 => {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The program the whole module measures has to be a program. Checked with
    /// the real front end, so a workload that stopped typechecking is a failing
    /// test rather than a benchmark that silently measures nothing.
    #[test]
    fn the_bench_program_checks() {
        let program = Program::parse().expect("the bench program checks");
        for simple in [
            "selects",
            "selects_by",
            "inserts",
            "transactions",
            "selects_at",
            "transactions_at",
            "twin_selects",
            "twin_transactions",
            "ddl",
        ] {
            program
                .full(simple)
                .unwrap_or_else(|e| panic!("`{simple}` is missing: {e}"));
        }
    }

    /// The rows a `ply hosts` listing would print for the workloads, which is
    /// the exit criterion W3 stated and W4 must not have widened: a read
    /// workload's row names one table and one mode.
    #[test]
    fn a_workload_publishes_the_table_it_names() {
        let program = Program::parse().unwrap();
        assert_eq!(
            program.footprint("selects").unwrap().to_string(),
            "{std.db.db.read[part]}"
        );
        assert_eq!(
            program.footprint("transactions").unwrap().to_string(),
            "{std.db.db.write[part], std.db.db.write}"
        );
    }

    /// The twin discharges every `db` atom, so a twin entry point's row is
    /// empty — which is what makes a twin-backed test `det`, cached and
    /// hermetic.
    #[test]
    fn a_twin_entry_point_reaches_nothing() {
        let program = Program::parse().unwrap();
        for simple in ["twin_selects", "twin_inserts", "twin_transactions"] {
            assert_eq!(
                program.footprint(simple).unwrap().to_string(),
                "{}",
                "`{simple}` publishes a row, so it is not hermetic"
            );
        }
    }

    /// The DDL comes from the program rather than from a copy of the schema in
    /// this file, and it has to be the one table the workloads name.
    #[test]
    fn the_fixture_ddl_is_the_programs_own() {
        let program = Program::parse().unwrap();
        let ddl = program.ddl().unwrap();
        assert_eq!(ddl.len(), 1, "the fixture is one table");
        assert!(ddl[0].starts_with("create table \"part\""), "{}", ddl[0]);
        assert!(ddl[0].contains("numeric(10,4)"), "{}", ddl[0]);
    }

    /// Every workload runs against the twin with no host at all, which is the
    /// property that makes the `ply-twin` rung a rung rather than a stub.
    #[test]
    fn every_workload_runs_against_the_twin() {
        let program = Program::parse().unwrap();
        for workload in Workload::ALL {
            let (_, answered) = program
                .call_pure(workload.twin(), workload.args(500, 4))
                .unwrap_or_else(|e| panic!("`{}` failed: {e}", workload.label()));
            assert_eq!(
                answered,
                Value::Int(4),
                "`{}` answered {answered}",
                workload.label()
            );
        }
    }
}
