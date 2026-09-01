//! Taking the W6 ladder, so that `w6.rs` only has to assemble and judge it.

use anyhow::{Context, Result, bail};
use ply_eval::Value;
use ply_eval::host::HostRuntime;
use ply_span::Span;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::serve::reserve_port;
use crate::{w3, w5, w6};

fn micros(d: Duration) -> f64 {
    d.as_secs_f64() * 1e6
}

/// The route the ladder is built around.
const DB_ROUTE: &str = "/items";

/// Requests one connection carries, on every rung that has a connection.
const PER_CONN: u32 = 32;

/// A script of connections each carrying [`PER_CONN`] requests.
fn keep_alive_script(request: &[u8], requests: u32) -> Vec<Vec<Vec<u8>>> {
    (0..connections_for(requests))
        .map(|_| (0..PER_CONN).map(|_| request.to_vec()).collect())
        .collect()
}

/// The route rungs 1–6 are taken on.
const ROUTE: &str = "/health";

/// The service with the ladder's driver appended, exactly as the rungs measure it.
pub fn program(repo: &Path) -> Result<w3::Loaded> {
    let service = w3::Service::open(repo)?;
    let source = format!("{}{DRIVER}", service.source(w3::Variant::Sequential)?);
    w3::Loaded::parse(&source)
}

/// The head every in-process rung answers.
pub fn head() -> Vec<u8> {
    w3::request("GET", ROUTE, None, false, 0, 0)
}

/// The four in-Ply loops rungs 2, 3 and 4 are read off, and the empty one that says what the loop
/// itself costs.
const DRIVER: &str = r#"

// --- W6: the in-process ladder's driver -------------------------------------

fn w6_const() -> Int = 0

fn w6_head() -> Bytes = b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"

fn w6_empty(n: Int, acc: Int) -> Int =
  if n <= 0 { acc } else { w6_empty(n - 1, acc + 1) }

fn w6_endpoint(n: Int, acc: Int) -> Int =
  if n <= 0 {
    acc
  } else {
    let r = health();
    w6_endpoint(n - 1, acc + r.status)
  }

fn w6_framed(n: Int, acc: Int, raw: Bytes, l: http::Limits) -> Int =
  if n <= 0 {
    acc
  } else {
    let m = match http::parse_head(raw, l) {
      http::Parsed(h) -> h.request.method,
      _ -> http::Get,
    };
    let r = health();
    let out = http::encode(m, http::Http11, false, r);
    w6_framed(n - 1, acc + bytes_len(out), raw, l)
  }

fn w6_routed(n: Int, acc: Int, raw: Bytes, l: http::Limits) -> Int =
  if n <= 0 {
    acc
  } else {
    let p = match http::parse_head(raw, l) {
      http::Parsed(h) -> {method: h.request.method, path: h.request.path},
      _ -> {method: http::Get, path: "/"},
    };
    let hit = match route_of(p.method, p.path) {
      router::Found(_) -> 1,
      _ -> 0,
    };
    let r = health();
    let out = http::encode(p.method, http::Http11, false, r);
    w6_routed(n - 1, acc + bytes_len(out) + hit, raw, l)
  }

// The routing rung with the route table built once instead of once per
// request. The difference between this and `w6_routed` is what `table()` costs
// a request, which is the "caching derived work" lever priced on the request
// path.
fn w6_cached(n: Int, acc: Int, raw: Bytes, l: http::Limits,
             t: List<router::Route<Endpoint>>) -> Int =
  if n <= 0 {
    acc
  } else {
    let p = match http::parse_head(raw, l) {
      http::Parsed(h) -> {method: h.request.method, path: h.request.path},
      _ -> {method: http::Get, path: "/"},
    };
    let hit = match router::route(t, p.method, p.path) {
      router::Found(_) -> 1,
      _ -> 0,
    };
    let r = health();
    let out = http::encode(p.method, http::Http11, false, r);
    w6_cached(n - 1, acc + bytes_len(out) + hit, raw, l, t)
  }

// `table()` on its own: ten `Route` records built from their pattern strings.
fn w6_table(n: Int, acc: Int) -> Int =
  if n <= 0 { acc } else { w6_table(n - 1, acc + len(table())) }

fn w6_bench(mode: Int, n: Int) -> Int =
  if mode == 0 {
    w6_empty(n, 0)
  } else if mode == 1 {
    w6_endpoint(n, 0)
  } else if mode == 2 {
    w6_framed(n, 0, w6_head(), limits())
  } else if mode == 4 {
    w6_cached(n, 0, w6_head(), limits(), table())
  } else if mode == 5 {
    w6_table(n, 0)
  } else {
    w6_routed(n, 0, w6_head(), limits())
  }

// The twin's store, priced apart from the ladder.
//
// `/items` cannot carry the ladder's lower rungs: its handler performs, so a
// pure call needs a store, and the store the twin supplies is `std.db`'s memory
// engine — which parses its SQL in Ply on every call. That cost is the twin's
// and not the served stack's, so it is measured here and reported beside the
// ladder rather than inside a layer.
fn w6_items_loop(n: Int, acc: Int) -> Int / {db.read[items]} =
  if n <= 0 {
    acc
  } else {
    let r = list_items();
    w6_items_loop(n - 1, acc + r.status)
  }

fn w6_scan_loop(n: Int, acc: Int) -> Int / {db.read[items]} =
  if n <= 0 {
    acc
  } else {
    let a = db.query[items](items_all(), []);
    let got = match a {
      db::Rows(rs) -> len(rs),
      db::Count(k) -> k,
      db::Failed(_) -> 0,
    };
    w6_scan_loop(n - 1, acc + got)
  }

fn w6_items(mode: Int, n: Int) -> Int =
  with_cell[store](stocked(seed_shelf(), seed_orders())) { c -> {
    let step = |q: db::Stmt, ps: List<db::Param>| {
      let o = db::step(cell_get(c), q, ps);
      cell_set(c, o.db);
      o.out
    };
    handle {
      if mode == 0 { w6_items_loop(n, 0) } else { w6_scan_loop(n, 0) }
    } with {
      db.query[items](q, ps) -> step(q, ps),
    }
  } }
"#;

/// What the in-process half measured, in the units the ladder wants.
#[derive(Clone, Debug, serde::Serialize)]
pub struct InProcess {
    /// One `Machine::call` on a function returning a constant.
    pub call_micros: f64,
    pub call_worst_micros: f64,
    pub endpoint_worst_micros: f64,
    pub framed_worst_micros: f64,
    pub routed_worst_micros: f64,
    pub sim_worst_micros: f64,
    pub socket_worst_micros: f64,
    /// The floor answering the response `/items` returns, which is what the measured total serves.
    pub items_floor_micros: f64,
    pub items_response_bytes: usize,
    /// The in-Ply loop's own per-iteration cost, and the twin fixture the loop is wrapped in.
    pub loop_micros: f64,
    pub fixture_micros: f64,
    pub endpoint_micros: f64,
    pub framed_micros: f64,
    pub routed_micros: f64,
    /// The whole service over `SimNet`, per request.
    pub sim_micros: f64,
    /// The same over a real listener, in this process.
    pub socket_micros: f64,
    /// The same accept/recv/send/close in Rust, answering the same bytes.
    pub floor_micros: f64,
    /// The routing rung with the route table hoisted out of the loop, and `table()` on its own.
    pub cached_micros: f64,
    pub table_micros: f64,
    /// `/items` over the twin: the handler, and the twin's SQL scan under it.
    pub items_endpoint_micros: f64,
    pub items_scan_micros: f64,
    pub requests: u32,
    pub iterations: u32,
    pub repeats: usize,
    pub response_bytes: usize,
    pub head_bytes: usize,
}

/// The best and the worst of one measurement's repeats.
#[derive(Clone, Copy, Debug)]
struct Repeated {
    best: f64,
    worst: f64,
}

impl Repeated {
    fn new() -> Repeated {
        Repeated {
            best: f64::MAX,
            worst: 0.0,
        }
    }

    fn saw(&mut self, micros: f64) {
        self.best = self.best.min(micros);
        self.worst = self.worst.max(micros);
    }

    fn per(self, n: f64) -> Repeated {
        Repeated {
            best: self.best / n,
            worst: self.worst / n,
        }
    }
}

/// Rungs 1–6 and the floor.
pub fn in_process(
    repo: &Path,
    requests: u32,
    iterations: u32,
    repeats: usize,
) -> Result<InProcess> {
    let loaded = program(repo)?;
    let request = head();
    let response = loaded.response_over_sim(&request)?;
    let items_request = w3::request("GET", DB_ROUTE, None, false, 0, 0);
    let items_response = loaded.response_over_sim(&items_request)?;
    let per_run = (connections_for(requests) * PER_CONN) as f64;

    let constant = loaded.full("w6_const")?;
    let bench = loaded.full("w6_bench")?;

    let mut call = Repeated::new();
    for _ in 0..repeats.max(1) {
        let (taken, _) = loaded.pure_call(&constant, Vec::new(), requests)?;
        call.saw(micros(taken) / requests as f64);
    }

    let mode = |m: i64, n: u32| -> Result<Repeated> {
        let mut seen = Repeated::new();
        for _ in 0..repeats.max(1) {
            let (taken, _) =
                loaded.pure_call(&bench, vec![Value::Int(m), Value::Int(n as i64)], 1)?;
            seen.saw(micros(taken));
        }
        Ok(seen)
    };

    // Two counts of the empty loop separate the fixture from the scaffold: the slope is what one
    // iteration costs and the intercept is what building the `MemDb` cost, and neither has to be
    // assumed.
    let empty_one = mode(0, iterations)?.best;
    let empty_half = mode(0, iterations / 2)?.best;
    let per_iteration = (empty_one - empty_half) / (iterations - iterations / 2) as f64;
    let fixture = empty_one - per_iteration * iterations as f64;

    let endpoint = mode(1, iterations)?.per(iterations as f64);
    let framed = mode(2, iterations)?.per(iterations as f64);
    let routed = mode(3, iterations)?.per(iterations as f64);

    let cached = mode(4, iterations)?.per(iterations as f64);
    let table = mode(5, iterations)?.per(iterations as f64);

    let items = loaded.full("w6_items")?;
    let twin = |m: i64, n: u32| -> Result<f64> {
        let mut seen = Repeated::new();
        for _ in 0..repeats.max(1) {
            let (taken, _) =
                loaded.pure_call(&items, vec![Value::Int(m), Value::Int(n as i64)], 1)?;
            seen.saw(micros(taken));
        }
        Ok(seen.best / n as f64)
    };
    let items_endpoint = twin(0, iterations)?;
    let items_scan = twin(1, iterations)?;

    let mut sim = Repeated::new();
    for _ in 0..repeats.max(1) {
        let (taken, _) = loaded.over_sim(keep_alive_script(&request, requests))?;
        sim.saw(micros(taken) / (connections_for(requests) * PER_CONN) as f64);
    }

    let mut socket = Repeated::new();
    for _ in 0..repeats.max(1) {
        let taken = over_socket(&loaded, &request, requests)?;
        socket.saw(micros(taken) / per_run);
    }

    let mut floor = Repeated::new();
    let mut items_floor = Repeated::new();
    for _ in 0..repeats.max(1) {
        floor.saw(micros(rust_floor(&request, &response, requests)?) / per_run);
        items_floor.saw(micros(rust_floor(&items_request, &items_response, requests)?) / per_run);
    }

    Ok(InProcess {
        call_micros: call.best,
        call_worst_micros: call.worst,
        loop_micros: per_iteration,
        fixture_micros: fixture,
        endpoint_micros: endpoint.best,
        endpoint_worst_micros: endpoint.worst,
        framed_micros: framed.best,
        framed_worst_micros: framed.worst,
        routed_micros: routed.best,
        routed_worst_micros: routed.worst,
        sim_micros: sim.best,
        sim_worst_micros: sim.worst,
        socket_micros: socket.best,
        socket_worst_micros: socket.worst,
        floor_micros: floor.best,
        items_floor_micros: items_floor.best,
        items_response_bytes: items_response.len(),
        cached_micros: cached.best,
        table_micros: table.best,
        items_endpoint_micros: items_endpoint,
        items_scan_micros: items_scan,
        requests,
        iterations,
        repeats,
        response_bytes: response.len(),
        head_bytes: request.len(),
    })
}

/// The whole service over a real listener, in this process: the same `run_memory` the `SimNet` rung
/// calls, with `ply_host`'s TCP handler under it instead of the twin.
fn over_socket(loaded: &w3::Loaded, request: &[u8], requests: u32) -> Result<Duration> {
    let port = reserve_port()?;
    let host = Arc::new(ply_host::Host::new());
    let binding = host
        .registry()
        .bind(&loaded.check)
        .map_err(|d| anyhow::anyhow!("binding the network failed: {}", d[0].message))?;
    let name = loaded.full("run_memory")?;
    let mut machine = loaded.machine();
    machine.set_host_binding(Arc::new(binding));
    let runtime: Rc<dyn HostRuntime> = host.runtime();
    machine.set_host_runtime(runtime);
    if let Some(declared) = loaded.footprint("run_memory") {
        machine.set_declared_footprint(declared);
    }

    let connections = connections_for(requests);
    let client = spawn_client(port, request.to_vec(), connections);
    let started = Instant::now();
    let served = machine.call(
        &name,
        vec![
            Value::Int(port as i64),
            Value::ctor("None", Vec::new()),
            Value::Int(connections as i64),
        ],
        Span::DUMMY,
    );
    let taken = started.elapsed();
    let answered = client
        .join()
        .map_err(|_| anyhow::anyhow!("the client thread panicked"))?;
    served.map_err(|d| anyhow::anyhow!("the socket rung raised: {}", d.message))?;
    let answered = answered?;
    if answered != connections * PER_CONN {
        bail!(
            "the client got {answered} of {} responses on the socket rung",
            connections * PER_CONN
        );
    }
    Ok(taken)
}

/// The denominator: the same accept, read, write and close in Rust, answering the bytes the service
/// answers, driven by the same client over connections carrying the same number of requests.
fn rust_floor(request: &[u8], response: &[u8], requests: u32) -> Result<Duration> {
    let listener = TcpListener::bind("127.0.0.1:0").context("binding the floor's listener")?;
    let port = listener.local_addr()?.port();
    let connections = connections_for(requests);
    let client = spawn_client(port, request.to_vec(), connections);
    let mut buf = vec![0u8; 16384];
    let started = Instant::now();
    for _ in 0..connections {
        let (mut socket, _) = listener.accept()?;
        socket.set_nodelay(true)?;
        let mut at = 0usize;
        let mut answered = 0u32;
        while answered < PER_CONN {
            let read = socket.read(&mut buf[at..])?;
            if read == 0 {
                break;
            }
            at += read;
            // One head per `\r\n\r\n`, which is all the floor has to find: the requests carry no
            // body.
            while let Some(end) = find(&buf[..at], b"\r\n\r\n") {
                socket.write_all(response)?;
                answered += 1;
                buf.copy_within(end + 4..at, 0);
                at -= end + 4;
            }
            socket.flush()?;
        }
        let _ = socket.shutdown(Shutdown::Both);
    }
    let taken = started.elapsed();
    let answered = client
        .join()
        .map_err(|_| anyhow::anyhow!("the client thread panicked"))?;
    if answered? != connections * PER_CONN {
        bail!("the floor's client did not get every response");
    }
    Ok(taken)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Connections a rung opens to make `requests` of them, rounded up so the two sides of a pair
/// answer the same count.
fn connections_for(requests: u32) -> u32 {
    requests.div_ceil(PER_CONN).max(1)
}

/// [`PER_CONN`] requests down one connection, then the next connection.
fn spawn_client(
    port: u16,
    request: Vec<u8>,
    connections: u32,
) -> std::thread::JoinHandle<Result<u32>> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    std::thread::spawn(move || {
        let mut answered = 0u32;
        let deadline = Instant::now() + Duration::from_secs(60);
        for _ in 0..connections {
            let mut socket = loop {
                match TcpStream::connect_timeout(&addr, Duration::from_secs(5)) {
                    Ok(socket) => break socket,
                    Err(e) => {
                        if Instant::now() >= deadline {
                            return Err(anyhow::anyhow!("connecting to the rung's server: {e}"));
                        }
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
            };
            socket.set_nodelay(true)?;
            socket.set_read_timeout(Some(Duration::from_secs(20)))?;
            let mut buf: Vec<u8> = Vec::with_capacity(16384);
            let mut chunk = [0u8; 8192];
            for _ in 0..PER_CONN {
                socket.write_all(&request)?;
                socket.flush()?;
                let want = loop {
                    if let Some(end) = find(&buf, b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..end]).to_string();
                        let length: usize = head
                            .lines()
                            .filter_map(|l| l.split_once(':'))
                            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                            .and_then(|(_, value)| value.trim().parse().ok())
                            .unwrap_or(0);
                        break end + 4 + length;
                    }
                    let read = socket.read(&mut chunk)?;
                    if read == 0 {
                        bail!("the server closed inside a response head");
                    }
                    buf.extend_from_slice(&chunk[..read]);
                };
                while buf.len() < want {
                    let read = socket.read(&mut chunk)?;
                    if read == 0 {
                        bail!("the server closed inside a response body");
                    }
                    buf.extend_from_slice(&chunk[..read]);
                }
                buf.drain(..want);
                answered += 1;
            }
            let _ = socket.shutdown(Shutdown::Both);
        }
        Ok(answered)
    })
}

/// One phase, run on its own for as long as it is asked to, so a sampling profiler sees that phase
/// and nothing else.
pub fn only(repo: &Path, phase: &str, requests: u32, rounds: usize) -> Result<f64> {
    let loaded = program(repo)?;
    let request = head();
    let bench = loaded.full("w6_bench")?;
    let items = loaded.full("w6_items")?;
    let mut best = f64::MAX;
    for _ in 0..rounds.max(1) {
        let taken = match phase {
            "sim" => loaded.over_sim(keep_alive_script(&request, requests))?.0,
            "socket" => over_socket(&loaded, &request, requests)?,
            "routed" => {
                loaded
                    .pure_call(&bench, vec![Value::Int(3), Value::Int(requests as i64)], 1)?
                    .0
            }
            "endpoint" => {
                loaded
                    .pure_call(&bench, vec![Value::Int(1), Value::Int(requests as i64)], 1)?
                    .0
            }
            "items" => {
                loaded
                    .pure_call(&items, vec![Value::Int(0), Value::Int(requests as i64)], 1)?
                    .0
            }
            other => bail!(
                "`{other}` is not a phase; the phases are sim, socket, routed, endpoint and items"
            ),
        };
        best = best.min(micros(taken) / requests as f64);
    }
    Ok(best)
}

/// One served configuration, as a rate and a per-request cost.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Served {
    pub accept: String,
    pub stack: String,
    pub sink: String,
    pub route: String,
    pub concurrency: u32,
    pub requests: u32,
    pub per_second: f64,
    pub per_request_micros: f64,
    /// The same configuration's slowest repeat.
    pub per_request_worst_micros: f64,
    pub repeats: usize,
    pub p50_micros: f64,
    pub p95_micros: f64,
    pub p99_micros: f64,
}

/// The same sweep taken `repeats` times, merged into one row per configuration carrying its best
/// and its worst.
fn merged(rounds: Vec<Vec<Served>>) -> Vec<Served> {
    let mut out: Vec<Served> = Vec::new();
    for round in rounds {
        for row in round {
            match out.iter_mut().find(|kept| {
                kept.accept == row.accept
                    && kept.stack == row.stack
                    && kept.sink == row.sink
                    && kept.route == row.route
                    && kept.concurrency == row.concurrency
            }) {
                None => out.push(Served { repeats: 1, ..row }),
                Some(kept) => {
                    kept.repeats += 1;
                    kept.per_request_worst_micros =
                        kept.per_request_worst_micros.max(row.per_request_micros);
                    if row.per_request_micros < kept.per_request_micros {
                        kept.per_request_micros = row.per_request_micros;
                        kept.per_second = row.per_second;
                        kept.p50_micros = row.p50_micros;
                        kept.p95_micros = row.p95_micros;
                        kept.p99_micros = row.p99_micros;
                        kept.requests = row.requests;
                    }
                }
            }
        }
    }
    out
}

/// Rungs 7–9, the offering rows and the measured total, all from one sweep.
#[allow(clippy::too_many_arguments)]
pub fn served(
    repo: &Path,
    ply: &Path,
    url: &str,
    variant: w3::Variant,
    concurrencies: &[u32],
    per_conn: u32,
    requests_per_point: u32,
    api_key: &str,
    repeats: usize,
) -> Result<Vec<Served>> {
    let routes: [(&'static str, &'static str); 2] =
        [("health (no db)", ROUTE), ("items (1 select)", DB_ROUTE)];
    let mut rounds = Vec::new();
    for _ in 0..repeats.max(1) {
        let points = w5::tracing(
            repo,
            ply,
            url,
            &[w5::Stack::Twin, w5::Stack::Postgres, w5::Stack::PostgresTls],
            variant,
            &[w5::Sinking::Off, w5::Sinking::JsonNull],
            &routes,
            concurrencies,
            per_conn,
            requests_per_point,
            api_key,
        )?;
        rounds.push(
            points
                .into_iter()
                .map(|p| Served {
                    accept: p.accept.to_string(),
                    stack: p.stack.to_string(),
                    sink: p.sink.to_string(),
                    route: p.route,
                    concurrency: p.concurrency,
                    requests: p.requests,
                    per_second: p.per_second,
                    per_request_micros: 1e6 / p.per_second,
                    per_request_worst_micros: 1e6 / p.per_second,
                    repeats: 1,
                    p50_micros: p.p50_micros,
                    p95_micros: p.p95_micros,
                    p99_micros: p.p99_micros,
                })
                .collect(),
        );
    }
    Ok(merged(rounds))
}

/// Rows within this much of the best throughput are the same measurement.
const FLAT: f64 = 0.05;

/// The row of a served sweep a rung is read off: the concurrency that maximizes throughput (the ladder's own rule), with a flat curve resolved to its lowest concurrency rather than to its luckiest.
pub fn best(points: &[Served], stack: &str, sink: &str, route: &str) -> Option<Served> {
    let matching: Vec<&Served> = points
        .iter()
        .filter(|p| p.stack == stack && p.sink == sink && p.route == route)
        .collect();
    let fastest = matching
        .iter()
        .map(|p| p.per_second)
        .fold(f64::MIN, f64::max);
    matching
        .into_iter()
        .filter(|p| p.per_second >= fastest * (1.0 - FLAT))
        .min_by_key(|p| p.concurrency)
        .cloned()
}

/// One served row at a named concurrency.
pub fn at(
    points: &[Served],
    stack: &str,
    sink: &str,
    route: &str,
    concurrency: u32,
) -> Option<Served> {
    points
        .iter()
        .find(|p| {
            p.stack == stack && p.sink == sink && p.route == route && p.concurrency == concurrency
        })
        .cloned()
}

/// What one `/health` request allocates, from the binary that can count it.
pub fn allocations(repo: &Path, given: Option<PathBuf>) -> Result<Option<(f64, f64)>> {
    let counter = match given {
        Some(path) => path
            .parent()
            .map(|dir| dir.join("w6-alloc"))
            .unwrap_or_else(|| PathBuf::from("w6-alloc")),
        None => std::env::current_exe()
            .context("this binary's own path")?
            .parent()
            .context("this binary has no directory")?
            .join("w6-alloc"),
    };
    if !counter.exists() {
        return Ok(None);
    }
    let out = std::process::Command::new(&counter)
        .arg("--repo")
        .arg(repo)
        .output()
        .with_context(|| format!("running `{}`", counter.display()))?;
    if !out.status.success() {
        bail!(
            "`{}` failed: {}",
            counter.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let counted: serde_json::Value = serde_json::from_slice(&out.stdout)
        .with_context(|| format!("`{}` did not print a count", counter.display()))?;
    Ok(Some((
        counted["allocations_per_request"].as_f64().unwrap_or(0.0),
        counted["bytes_per_request"].as_f64().unwrap_or(0.0),
    )))
}

/// Definitions the control may not disable.
const NOT_DISABLED: [&str; 3] = ["main", "config", "schema"];

/// The same source with every nullary definition of its own given a dead parameter, which is the
/// narrowest edit that puts a definition outside the constant memo's rule without changing what it
/// computes.
pub fn without_constants(source: &str) -> String {
    let mut nullary: Vec<String> = Vec::new();
    for line in source.lines() {
        let rest = match line.strip_prefix("pub fn ") {
            Some(rest) => rest,
            None => match line.strip_prefix("fn ") {
                Some(rest) => rest,
                None => continue,
            },
        };
        if let Some(name) = rest.strip_suffix("()").or_else(|| {
            rest.split_once("()")
                .filter(|(_, after)| after.starts_with(' ') || after.starts_with(')'))
                .map(|(name, _)| name)
        }) && !name.is_empty()
            && name.chars().all(|c| c.is_alphanumeric() || c == '_')
            && !NOT_DISABLED.contains(&name)
        {
            nullary.push(name.to_string());
        }
    }
    let mut out = source.to_string();
    // Declarations first: a call-site rewrite cannot tell `fn table()` from `table()` by the
    // character in front of it.
    for name in &nullary {
        out = out.replace(
            &format!("fn {name}() "),
            &format!("fn {name}(w6_control: Int) "),
        );
    }
    for name in &nullary {
        out = replace_calls(&out, name);
    }
    out
}

/// `name()` becomes `name(0)` wherever the character before it can start an identifier reference
/// and is not part of a longer name or a module path.
fn replace_calls(source: &str, name: &str) -> String {
    let needle = format!("{name}()");
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(at) = rest.find(&needle) {
        let before = rest[..at].chars().next_back();
        let qualified = matches!(before, Some('.') | Some(':'));
        let inner = before.is_some_and(|c| c.is_alphanumeric() || c == '_');
        out.push_str(&rest[..at]);
        if qualified || inner {
            out.push_str(&needle);
        } else {
            out.push_str(&format!("{name}(0)"));
        }
        rest = &rest[at + needle.len()..];
    }
    out.push_str(rest);
    out
}

/// What the constant memo is worth **end to end on the served workload**, which is the only shape
/// the cheaper levers accepts as a price.
#[allow(clippy::too_many_arguments)]
pub fn memo_lever(
    repo: &Path,
    ply: &Path,
    url: &str,
    variant: w3::Variant,
    other: w3::Variant,
    concurrency: u32,
    per_conn: u32,
    requests_per_point: u32,
    api_key: &str,
    repeats: usize,
) -> Result<Levers> {
    let shipped = std::fs::read_to_string(repo.join("examples/desk.ply"))
        .context("reading `examples/desk.ply` to build the memo lever's control")?;
    let control = without_constants(&shipped);
    if control == shipped {
        bail!("the control rewrite found no nullary definition to disable");
    }
    let dir = tempfile::tempdir().context("a shadow repository for the control service")?;
    std::fs::create_dir_all(dir.path().join("examples"))?;
    std::fs::write(dir.path().join("examples/desk.ply"), &control)?;

    let routes: [(&'static str, &'static str); 2] =
        [("health (no db)", ROUTE), ("items (1 select)", DB_ROUTE)];
    // Both accept loops, because on this tree the answer depends on which one is under the service:
    // `task.spawn` opens a production region for the life of the server and `Machine::constant`
    // refuses the memo inside an open region, so a spawning service memoizes nothing.
    let mut priced: Vec<LoopPrice> = Vec::new();
    for loop_variant in [variant, other] {
        let mut with = Repeated::new();
        let mut without = Repeated::new();
        let mut with_health = Repeated::new();
        let mut without_health = Repeated::new();
        for _ in 0..repeats.max(1) {
            for (root, items, health) in [
                (repo, &mut with, &mut with_health),
                (dir.path(), &mut without, &mut without_health),
            ] {
                let points = w5::tracing(
                    root,
                    ply,
                    url,
                    &[w5::Stack::PostgresTls],
                    loop_variant,
                    &[w5::Sinking::JsonNull],
                    &routes,
                    &[concurrency],
                    per_conn,
                    requests_per_point,
                    api_key,
                )?;
                for point in &points {
                    if point.route == routes[1].0 {
                        items.saw(1e6 / point.per_second);
                    } else {
                        health.saw(1e6 / point.per_second);
                    }
                }
            }
        }
        priced.push(LoopPrice {
            accept: loop_variant.label(),
            items: without.best / with.best,
            health: without_health.best / with_health.best,
            items_with: with.best,
            items_without: without.best,
            health_with: with_health.best,
            health_without: without_health.best,
        });
    }
    let ladder = priced[0].clone();
    Ok(Levers {
        memo_items: Some(ladder.items),
        memo_health: Some(ladder.health),
        memo_evidence: format!(
            "The shipped service against the same source with every nullary definition of its own \
             given a dead parameter, served alternately by the same binary over postgres over TLS \
             with --trace json at c={concurrency}, best of {}. On the {} accept loop the ladder is \
             read off: /items {:.1}us with the memo against {:.1}us without it, and /health \
             {:.1}us against {:.1}us ({:.2}x). On the {} loop: /items {:.2}x and /health {:.2}x — \
             `task.spawn` opens a region for the life of the server and the memo is refused inside \
             one, so a spawning service memoizes nothing.",
            repeats.max(1),
            ladder.accept,
            ladder.items_with,
            ladder.items_without,
            ladder.health_with,
            ladder.health_without,
            ladder.health,
            priced[1].accept,
            priced[1].items,
            priced[1].health
        ),
        loops: priced,
        allocations: None,
    })
}

/// What the memo was worth on one accept loop.
#[derive(Clone, Debug, serde::Serialize)]
pub struct LoopPrice {
    pub accept: &'static str,
    pub items: f64,
    pub health: f64,
    pub items_with: f64,
    pub items_without: f64,
    pub health_with: f64,
    pub health_without: f64,
}

/// Everything the ladder half of the report carries: the whole file, not a fragment of one.
#[allow(clippy::too_many_arguments)]
pub fn report(
    machine: String,
    postgres: Option<String>,
    stack: &InProcess,
    served: &[Served],
    sequential: &[Served],
    levers: &Levers,
) -> Result<w6::Report> {
    let route = "items (1 select)";
    let health = "health (no db)";
    let off = w5::Sinking::Off.label();
    let json = w5::Sinking::JsonNull.label();
    let twin = w5::Stack::Twin.label();
    let pg = w5::Stack::Postgres.label();
    let tls = w5::Stack::PostgresTls.label();

    let total = best(served, tls, json, route).with_context(|| {
        format!("the served sweep has no `{tls}` x `{json}` x `{route}` row to read the total off")
    })?;
    // Every other served rung is taken at the total's concurrency, so a layer is one flag moved
    // rather than one flag moved and two rows selected.
    let c = total.concurrency;
    let need = |stack: &str, sink: &str, route: &str| -> Result<Served> {
        at(served, stack, sink, route, c).with_context(|| {
            format!("the served sweep has no `{stack}` x `{sink}` x `{route}` row at c={c}")
        })
    };

    let tls_with = need(tls, off, route)?;
    let tls_without = need(pg, off, route)?;
    let db_with = need(pg, off, route)?;
    let db_without = need(pg, off, health)?;
    let trace_with = need(tls, json, route)?;
    let trace_without = need(tls, off, route)?;

    let in_process =
        |layer: w6::Layer, with: (f64, f64), without: (f64, f64), requests: u32| -> w6::Point {
            w6::Point {
                layer,
                taken_on: ROUTE.to_string(),
                with_micros: with.0,
                without_micros: without.0,
                with_worst_micros: Some(with.1),
                without_worst_micros: Some(without.1),
                requests,
            }
        };
    let from_served =
        |layer: w6::Layer, taken_on: &str, with: &Served, without: &Served| -> w6::Point {
            w6::Point {
                layer,
                taken_on: taken_on.to_string(),
                with_micros: with.per_request_micros,
                without_micros: without.per_request_micros,
                with_worst_micros: Some(with.per_request_worst_micros),
                without_worst_micros: Some(without.per_request_worst_micros),
                requests: with.requests,
            }
        };

    let points = vec![
        w6::Point {
            layer: w6::Layer::Call,
            taken_on: "a constant-returning function".to_string(),
            with_micros: stack.call_micros,
            without_micros: 0.0,
            with_worst_micros: Some(stack.call_worst_micros),
            without_worst_micros: Some(0.0),
            requests: stack.requests,
        },
        in_process(
            w6::Layer::Endpoint,
            (stack.endpoint_micros, stack.endpoint_worst_micros),
            (stack.call_micros, stack.call_worst_micros),
            stack.iterations,
        ),
        in_process(
            w6::Layer::Framing,
            (stack.framed_micros, stack.framed_worst_micros),
            (stack.endpoint_micros, stack.endpoint_worst_micros),
            stack.iterations,
        ),
        in_process(
            w6::Layer::Routing,
            (stack.routed_micros, stack.routed_worst_micros),
            (stack.framed_micros, stack.framed_worst_micros),
            stack.iterations,
        ),
        in_process(
            w6::Layer::Machine,
            (stack.sim_micros, stack.sim_worst_micros),
            (stack.routed_micros, stack.routed_worst_micros),
            stack.requests,
        ),
        in_process(
            w6::Layer::Socket,
            (stack.socket_micros, stack.socket_worst_micros),
            (stack.sim_micros, stack.sim_worst_micros),
            stack.requests,
        ),
        from_served(w6::Layer::Tls, DB_ROUTE, &tls_with, &tls_without),
        // the workload ladder pins this rung's `without` as `run_memory`.
        from_served(
            w6::Layer::Database,
            "/items against /health",
            &db_with,
            &db_without,
        ),
        from_served(w6::Layer::Tracing, DB_ROUTE, &trace_with, &trace_without),
    ];

    // A row answering `/items` is read against the floor replaying `/items`' response and a row
    // answering `/health` against `/health`'s.
    let floor_for = |route_label: &str| {
        if route_label == health {
            1e6 / stack.floor_micros
        } else {
            1e6 / stack.items_floor_micros
        }
    };
    let mut offerings = Vec::new();
    for (rows, what, stack_label, sink_label, route_label) in [
        (
            served,
            "a route with no database, plaintext",
            twin,
            off,
            health,
        ),
        (served, "one select, plaintext", pg, off, route),
        (served, "one select, TLS", tls, off, route),
        (served, "one select, TLS, tracing to JSON", tls, json, route),
        (
            served,
            "no database, TLS, tracing to JSON",
            tls,
            json,
            health,
        ),
        (
            sequential,
            "a route with no database, plaintext",
            twin,
            off,
            health,
        ),
        (sequential, "one select, plaintext", pg, off, route),
        (
            sequential,
            "one select, TLS, tracing to JSON",
            tls,
            json,
            route,
        ),
    ] {
        if let Some(point) = best(rows, stack_label, sink_label, route_label) {
            offerings.push(w6::Offering {
                what: what.to_string(),
                stack: format!(
                    "{stack_label}, --trace {sink_label}, {} accept",
                    point.accept
                ),
                head_bytes: stack.head_bytes,
                concurrency: point.concurrency,
                per_second: point.per_second,
                p50_micros: point.p50_micros,
                p99_micros: point.p99_micros,
                floor_per_second: Some(floor_for(route_label)),
            });
        }
    }

    let health_plain = best(served, pg, off, health);
    Ok(w6::Report {
        provenance: w6::Provenance {
            machine,
            profile: "release".to_string(),
            taken: chrono_date(),
            repeats: stack.repeats,
            request_head_bytes: stack.head_bytes,
            postgres,
            not_measured: not_measured(stack, levers),
        },
        floor_micros: stack.items_floor_micros,
        total_micros: total.per_request_micros,
        denominators: w6::Denominators {
            floor_taken_on: format!(
                "a Rust accept/read/write/close replaying the same {}-byte /items response over plaintext, over \
  connections carrying {PER_CONN} requests, with no interpreter, no TLS and no database under it",
                stack.items_response_bytes
            ),
            total_taken_on: format!(
                "/items over postgres over TLS with --trace json, {} accept, c={}, best of {} sweeps",
                total.accept, total.concurrency, total.repeats
            ),
            total_worst_micros: Some(total.per_request_worst_micros),
        },
        points,
        spike: None,
        alternatives: alternatives(stack, levers),
        offerings,
        limits: limits(stack, levers, &total, health_plain.as_ref()),
    })
}

/// What the ladder run priced of the cheaper levers, as measured numbers rather than as prose.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Levers {
    /// The served end-to-end ratio of the shipped service against the same service with its nullary
    /// constants disabled, on the ladder's own workload, and the same on `/health`.
    pub memo_items: Option<f64>,
    pub memo_health: Option<f64>,
    /// How the two were measured, for the evidence column.
    pub memo_evidence: String,
    /// The same ratio on each accept loop, because on this tree they differ.
    pub loops: Vec<LoopPrice>,
    /// Allocations and bytes one `/health` request makes.
    pub allocations: Option<(f64, f64)>,
}

fn alternatives(stack: &InProcess, levers: &Levers) -> Vec<w6::Alternative> {
    LEVER_PROSE
        .iter()
        .map(|(name, what, cost)| {
            let mut alternative = w6::Alternative {
                name: (*name).to_string(),
                what: (*what).to_string(),
                priced: false,
                end_to_end: 0.0,
                evidence: String::new(),
                cost: (*cost).to_string(),
            };
            match *name {
                "caching derived work" => {
                    // Both routes, because the ratio C3 reads is `/items`' and the one the limits
                    // section reads is `/health`'s; a run that took only one of them priced
                    // neither.
                    if let (Some(items), Some(_health)) = (levers.memo_items, levers.memo_health) {
                        alternative.priced = true;
                        alternative.end_to_end = items;
                        alternative.what = format!(
 "`table()` builds eleven Route records from their pattern strings and costs {:.2}us per call; \
  `route_of` calls it on every request and `health` again through `len(table())`. The constant \
  memo evaluates a nullary pure definition once per process, and it has landed: \
  `ply-eval::memo`, CONTRACTS.md.",
                            stack.table_micros
                        );
                        alternative.evidence = levers.memo_evidence.clone();
                    }
                }
                "boxing on hot paths" => {
                    if let Some((allocs, bytes)) = levers.allocations {
                        alternative.what = format!(
                            "One /health request allocates {allocs:.0} times and {:.3} MB to \
                             produce a {}-byte response, counted with a counting global allocator \
                             in `w6-alloc`. The size of the lever, not a speedup: nothing was \
                             changed to move it.",
                            bytes / 1e6,
                            stack.response_bytes
                        );
                    }
                }
                _ => {}
            }
            alternative
        })
        .collect()
}

/// The roster's prose and cost column.
const LEVER_PROSE: [(&str, &str, &str); 7] = [
    (
        "more native builtins",
        "Fold `read_line`, `is_token`, `trim_ows` and `trim_end` into one native head scan. W2's lever \
  applied to W3's layer, and W2's was 4.8x.",
        "Each native builtin grows the trusted computing base `ply hosts` invites a reader to check.",
    ),
    (
        "the frame push",
        "the control-stack design's four heap allocations per frame push, paid per node the machine suspends inside.",
        "A cheaper frame representation touches capture, splice and every frame kind.",
    ),
    (
        "Env::lookup",
        "`crates/ply-eval/src/env.rs` walks an `Rc` chain linearly, so a variable reference costs \
  O(scope depth). No depth sweep was run.",
        "An indexed environment touches capture, splice and every frame kind.",
    ),
    (
        "boxing on hot paths",
        "Where a `Value::Int` per element survives, counted per request rather than guessed at.",
        "Unboxing or arena-allocating `Value` touches every builtin and every engine.",
    ),
    (
        "caching derived work",
        "`table()` rebuilds the route table from its pattern strings on every request, and a derived \
  codec dictionary is a record built per call.",
        "A memo in front of a pure function changes when a route table can be edited.",
    ),
    (
        "connection and statement reuse",
        "W4's pool and prepared-statement cache: hit rate, and what a miss costs. W6 did not re-take it.",
        "Not re-measured here.",
    ),
    (
        "response buffering",
        "Writes per response, and the copies `bytes_concat` and `bytes_slice` make. `Value::Bytes` is \
  `Arc<[u8]>` with no slicing, so a slice copies.",
        "Not measured.",
    ),
];

fn limits(
    stack: &InProcess,
    levers: &Levers,
    total: &Served,
    health_plain: Option<&Served>,
) -> Vec<w6::Limit> {
    let mut limits = vec![w6::Limit {
        what: "One machine is one core".to_string(),
        why: "A Value holds Rc and a continuation is Rc<Vec<Segment>>, so a Ply task cannot move \
       between OS threads. Throughput scales by processes, not by threads, and every runtime \
       this would be compared against scales by threads."
            .to_string(),
        evidence: None,
    }];
    limits.push(w6::Limit {
        what: format!(
            "A Ply request costs {:.1}x the same syscalls answering the same bytes",
            total.per_request_micros / stack.items_floor_micros
        ),
 why: "The floor is a Rust accept/read/write/close replaying the response the service answers. \
       It has no interpreter under it — and, on the /items row, no TLS and no database either, \
       so the multiple is stated with what each side did."
            .to_string(),
        evidence: Some(match health_plain {
            Some(health) => format!(
 "{:.1}us for /items over postgres over TLS with tracing against a {:.1}us floor replaying the \
  same {}-byte response; like for like, /health over plaintext is {:.1}us against a {:.1}us \
  floor replaying its 107-byte response, {:.1}x.",
                total.per_request_micros,
                stack.items_floor_micros,
                stack.items_response_bytes,
                health.per_request_micros,
                stack.floor_micros,
                health.per_request_micros / stack.floor_micros
            ),
            None => format!(
                "{:.1}us against a {:.1}us floor replaying the same {}-byte response.",
                total.per_request_micros, stack.items_floor_micros, stack.items_response_bytes
            ),
        }),
    });
    if levers.loops.len() == 2 {
        let (first, second) = (&levers.loops[0], &levers.loops[1]);
        let (sequential, spawning) = if first.accept == "sequential" {
            (first, second)
        } else {
            (second, first)
        };
        limits.push(w6::Limit {
            what: "A service whose accept loop spawns memoizes nothing".to_string(),
            why: "`task.spawn` opens a production region that stays open for the life of the \
                  server, and `Machine::constant` refuses the constant memo inside any open \
                  region — so every nullary pure definition is re-evaluated per call on the \
                  task-per-connection loop. The rule's stated reason is a `simulate` region's \
                  allocation trail, which a production region does not keep."
                .to_string(),
            evidence: Some(format!(
                "Disabling the memo by source substitution costs {:.2}x on /health and {:.2}x on \
                 /items on the {} loop, and {:.2}x and {:.2}x on the {} loop, where there is \
                 nothing to disable: /health is {:.1}us sequential against {:.1}us spawning, on \
                 the same service.",
                sequential.health,
                sequential.items,
                sequential.accept,
                spawning.health,
                spawning.items,
                spawning.accept,
                sequential.health_with,
                spawning.health_with
            )),
        });
    }
    limits.push(w6::Limit {
        what: "The in-memory twin is slower than the database it stands in for".to_string(),
 why: "std.db's memory engine parses its SQL in Ply on every call, and every twin handler clause \
       writes its whole state back through a persistent map."
            .to_string(),
        evidence: Some(format!(
 "In process the twin's /items handler is {:.1}us a call and its scan alone is {:.1}us of that.",
            stack.items_endpoint_micros, stack.items_scan_micros
        )),
    });
    limits.push(w6::Limit {
        what: "No cancellation, no backpressure, no load shedding".to_string(),
 why: "the host boundary contract left cancellation unresolved through W5, and W4's promise of backpressure was \
       broken explicitly by W5's `not in` list."
            .to_string(),
        evidence: None,
    });
    if let Some((allocs, bytes)) = levers.allocations {
        limits.push(w6::Limit {
            what: "A request allocates far more times than it writes bytes".to_string(),
 why: "Value is boxed, a frame push is four heap allocations, and every intermediate Bytes is an \
       Arc<[u8]>."
                .to_string(),
            evidence: Some(format!(
 "One /health request allocates {allocs:.0} times and {:.3} MB to produce a {}-byte response.",
                bytes / 1e6,
                stack.response_bytes
            )),
        });
    }
    limits
}

fn not_measured(stack: &InProcess, levers: &Levers) -> Vec<String> {
    let mut out = vec![
 "Anything the codegen spike's fragment does not reach. The spike compiles no perform, no \
  handler-stack walk, no host boundary, no continuation capture, no closure and no derived \
  codec, and it frees its values in an arena rather than reference-counting them. Projecting its \
  k onto the whole interpreter share assumes a coverage it did not demonstrate."
            .to_string(),
 "What a partially-covering backend is worth. The spike's `solo, trampolined` variant is one \
  point on that curve; no variant compiles a whole request path."
            .to_string(),
 "Rungs 1-6 are taken on /health, not /items: a pure call to the /items handler needs a store, \
  and the only one available in process is std.db's memory engine, whose SQL scanner is on no \
  served request path. /items' own decode and encode therefore sit inside the database rung."
            .to_string(),
 "The TLS handshake. The tls rung is steady state over connections carrying 32 requests, as the ladder \
  0016 section 1.2 requires."
            .to_string(),
 "Multi-core throughput, deliberately: one machine is one core and a process-per-core number \
  would measure an operating system."
            .to_string(),
        "Env::lookup's depth sweep, and the writes and copies a response makes.".to_string(),
        format!(
            "The ladder's rungs and total are taken on the {} accept loop rather than the \
             task-per-connection one the performance verdict section 1.6 pins, and the reason is measured rather \
             than preferred: a spawning service opens a production region for its whole life and \
             the constant memo is refused inside an open region, so the two arenas would be in \
             different regimes — an in-process numerator that memoizes over a served denominator \
             that does not. Both loops are measured; the other one's rows are the labelled \
             offerings, and what the difference costs is in the limits.",
            if levers.loops.is_empty() {
                "chosen"
            } else {
                levers.loops[0].accept
            }
        ),
        format!(
            "The in-Ply loop rungs 2-4 are read off, apart from the endpoint layer. It costs \
             {:.2}us an iteration, measured by running the empty loop at two counts, and it \
             cancels between rungs 3 and 4 because both sides of those are loops. It does not \
             cancel between rungs 1 and 2, whose `without` is a bare call, so the endpoint layer \
             carries it: that layer is {:.2}us against {:.2}us of scaffold, which is the \
             resolution this rung has and not a number to read to two decimals.",
            stack.loop_micros,
            stack.endpoint_micros - stack.call_micros,
            stack.loop_micros
        ),
    ];
    let unpriced: Vec<&str> = w6::LEVERS
        .iter()
        .map(|l| l.name)
        .filter(|name| !(*name == "caching derived work" && levers.memo_items.is_some()))
        .collect();
    if !unpriced.is_empty() {
        out.push(format!(
 "As an end-to-end speedup on the served workload: {}. the performance verdict section 2.5 makes an unpriced \
  lever sufficient on its own to keep deferring.",
            unpriced.join(", ")
        ));
    }
    out.push(format!(
 "What a route body costs when it is not a constant. `/health`'s whole body is a nullary pure \
  definition, so after the constant memo the endpoint rung is a memo hit; the /items handler \
  over the twin is {:.1}us a call against {:.1}us for the twin's scan alone, and that difference \
  is the closest thing here to a route body's own cost.",
        stack.items_endpoint_micros, stack.items_scan_micros
    ));
    out
}

/// The date, without a dependency for it.
fn chrono_date() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86_400;
    // Civil-from-days, Howard Hinnant's algorithm.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// The `ply` binary a served row drives, defaulting to this binary's sibling so a release
/// measurement never silently serves from a debug build.
pub fn ply_binary(given: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = given {
        return Ok(path);
    }
    let mine = std::env::current_exe().context("this binary's own path")?;
    Ok(mine
        .parent()
        .context("this binary has no directory")?
        .join("ply"))
}
