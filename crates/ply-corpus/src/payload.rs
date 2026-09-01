//! What W2's payload types cost, measured rather than assumed.

use anyhow::{Context, Result, bail};
use ply_cli::driver;
use ply_eval::{Machine, Value};
use ply_span::Span;
use ply_store::Store;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// Megabytes are 1e6 bytes here, and stated rather than assumed: a throughput quoted in MiB against
/// one quoted in MB differs by five percent, which is inside the range these numbers are argued
/// over.
const MEGABYTE: f64 = 1e6;

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

fn micros(d: Duration) -> f64 {
    d.as_secs_f64() * 1e6
}

fn millis(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// A checked project and a machine over it.
struct Checked {
    loaded: ply_cli::load::Loaded,
}

impl Checked {
    fn open(root: &Path) -> Result<Checked> {
        let loaded = driver::load_full(root).map_err(|e| {
            let shown: Vec<String> = e
                .diagnostics
                .iter()
                .take(5)
                .map(|d| format!("{}: {}", d.code, d.message))
                .collect();
            anyhow::anyhow!(
                "the measurement program does not compile:\n  {}",
                shown.join("\n  ")
            )
        })?;
        Ok(Checked { loaded })
    }

    fn machine(&self) -> Machine<'_> {
        Machine::new(
            &self.loaded.program,
            &self.loaded.resolved,
            &self.loaded.check,
        )
    }

    /// `Machine::call` takes a program-wide name, so a simple one is looked up rather than guessed
    /// at from the file it was written in.
    fn full(&self, simple: &str) -> Result<String> {
        self.loaded
            .check
            .defs
            .values()
            .find(|d| d.simple_name.as_str() == simple)
            .map(|d| d.name.to_string())
            .with_context(|| format!("the measurement program declares no `{simple}`"))
    }
}

fn call(machine: &mut Machine<'_>, name: &str, args: Vec<Value>) -> Result<Value> {
    machine
        .call(name, args, Span::DUMMY)
        .map_err(|d| anyhow::anyhow!("`{name}` raised {}: {}", d.code, d.message))
}

fn write_project(files: &[(&str, String)]) -> Result<tempfile::TempDir> {
    let dir = tempfile::tempdir().context("a temp dir for a measurement project")?;
    for (name, source) in files {
        std::fs::write(dir.path().join(name), source)?;
    }
    Ok(dir)
}

/// An order with `lines` line items, which is the shape a payload benchmark should have: a record
/// of scalars and a list of records, with a `String` needing escape analysis, an `Int`, a `Decimal`
/// and a `Bool` in every element.
const JSON_SRC: &str = r#"import std.json

pub type Line = { sku: String, qty: Int, unit_price: Decimal, note: String, active: Bool }

pub type Order = { id: Int, customer: String, tags: List<String>, lines: List<Line> }

derive json for Line
derive json for Order

fn line(i: Int) -> Line =
  { sku: "SKU-" ++ int_to_string(i),
    qty: i % 7 + 1,
    unit_price: decimal_of_int(i % 500 + 100) * 0.01m,
    note: "line " ++ int_to_string(i) ++ " of a synthetic order",
    active: i % 2 == 0 }

pub fn order(n: Int) -> Order =
  { id: 4242,
    customer: "ada lovelace",
    tags: ["priority", "gift", "eu"],
    lines: map(range(0, n), |i: Int| line(i)) }

pub fn payload(n: Int) -> Bytes = json::encode_bytes(order(n), order_json())

fn pad(width: Int) -> String = fold(range(0, width), "", |s: String, _i: Int| s ++ "x")

// The same order with one field widened. Line count fixes the number of fields
// the codec visits; `width` fixes how many bytes the scanner crosses to get
// between them. Varying them separately is the only way to say whether a
// decode is priced per field or per byte, and it is the same experiment the
// head sweep runs on the request line.
pub fn wide_order(n: Int, width: Int) -> Order {
  let note = pad(width);
  { id: 4242,
    customer: "ada lovelace",
    tags: ["priority", "gift", "eu"],
    lines: map(range(0, n), |i: Int|
      { sku: "SKU-" ++ int_to_string(i),
        qty: i % 7 + 1,
        unit_price: decimal_of_int(i % 500 + 100) * 0.01m,
        note: note,
        active: i % 2 == 0 }) }
}

pub fn wide_payload(n: Int, width: Int) -> Bytes =
  json::encode_bytes(wide_order(n, width), order_json())

pub fn encode_once(o: Order) -> Int = bytes_len(json::encode_bytes(o, order_json()))

// The first half of a decode on its own: bytes to a `Json`, before any
// dictionary runs.
pub fn parse_only(src: Bytes) -> Int =
  match json::parse(src) {
    Ok(_) -> 1,
    Err(_) -> -1,
  }

// The second half on its own, over an already-parsed document. Timed directly
// rather than as `decode_once` minus `parse_only`: a difference of two
// separately-timed runs can come out negative under load, and a clamped
// negative reads as "the codec was free" — a number about the machine's
// scheduler wearing the units of a measurement.
pub fn to_json(src: Bytes) -> json::Json =
  match json::parse(src) {
    Ok(j) -> j,
    Err(_) -> json::Null,
  }

pub fn codec_only(j: json::Json) -> Int =
  match (order_json().decode)(j) {
    Ok(o) -> len(o.lines),
    Err(_) -> -1,
  }

pub fn decode_once(src: Bytes) -> Int =
  match json::decode_bytes(src, order_json()) {
    Ok(o) -> len(o.lines),
    Err(_) -> -1,
  }

test "the derived codec round-trips the payload it is measured on" {
  let src = payload(4);
  assert_eq(decode_once(src), 4);
  assert_eq(encode_once(order(4)), bytes_len(src))
}
"#;

#[derive(Clone, Debug, Serialize)]
pub struct JsonPoint {
    pub lines: usize,
    pub payload_bytes: usize,
    pub encode_micros: f64,
    pub decode_micros: f64,
    pub encode_mb_per_second: f64,
    pub decode_mb_per_second: f64,
}

/// Encode and decode a derived codec, at several payload sizes.
pub fn json_throughput(sizes: &[usize], iterations: u32, repeats: usize) -> Result<Vec<JsonPoint>> {
    let dir = write_project(&[("payload.ply", JSON_SRC.to_string())])?;
    let checked = Checked::open(dir.path())?;
    let (order, payload) = (checked.full("order")?, checked.full("payload")?);
    let (encode, decode) = (checked.full("encode_once")?, checked.full("decode_once")?);

    let mut out = Vec::new();
    for &lines in sizes {
        let mut machine = checked.machine();
        let value = call(&mut machine, &order, vec![Value::Int(lines as i64)])?;
        let bytes = call(&mut machine, &payload, vec![Value::Int(lines as i64)])?;
        let Value::Bytes(raw) = &bytes else {
            bail!("`payload` answered a {}, not Bytes", bytes.type_name());
        };
        let payload_bytes = raw.len();

        // The machine lowers a body on first call and caches nothing across calls, but a first call
        // still pays for whatever the engine defers — and charged to a twenty-iteration batch that
        // is a fifth of the number.
        call(&mut machine, &encode, vec![value.clone()])?;
        call(&mut machine, &decode, vec![bytes.clone()])?;

        let encoded = best_of(repeats, || {
            let started = Instant::now();
            for _ in 0..iterations {
                call(&mut machine, &encode, vec![value.clone()])?;
            }
            Ok(started.elapsed())
        })?;
        let decoded = best_of(repeats, || {
            let started = Instant::now();
            for _ in 0..iterations {
                call(&mut machine, &decode, vec![bytes.clone()])?;
            }
            Ok(started.elapsed())
        })?;

        let per = |d: Duration| micros(d) / iterations as f64;
        let rate =
            |d: Duration| payload_bytes as f64 * iterations as f64 / d.as_secs_f64() / MEGABYTE;
        out.push(JsonPoint {
            lines,
            payload_bytes,
            encode_micros: per(encoded),
            decode_micros: per(decoded),
            encode_mb_per_second: rate(encoded),
            decode_mb_per_second: rate(decoded),
        });
    }
    Ok(out)
}

#[derive(Clone, Debug, Serialize)]
pub struct ShapePoint {
    pub lines: usize,
    /// Bytes of filler inside one string field.
    pub note_width: usize,
    pub payload_bytes: usize,
    /// Leaf values the codec visits: five per line, plus the order's own four.
    pub fields: usize,
    pub encode_micros: f64,
    pub decode_micros: f64,
    /// `json::parse` alone: bytes to a `Json`, before any codec runs.
    pub parse_micros: f64,
    /// The derived codec's own half — walking an already-parsed `Json` into the ADT, timed on its
    /// own rather than as `decode - parse`.
    pub codec_micros: f64,
    pub decode_micros_per_byte: f64,
    pub decode_micros_per_field: f64,
}

/// Whether a decode is priced by the fields it visits or by the bytes it crosses — the question ADR
/// 0012 §5 asks of the request head, asked of the payload.
pub fn json_shape(
    points: &[(usize, usize)],
    iterations: u32,
    repeats: usize,
) -> Result<Vec<ShapePoint>> {
    let dir = write_project(&[("payload.ply", JSON_SRC.to_string())])?;
    let checked = Checked::open(dir.path())?;
    let (order, payload) = (checked.full("wide_order")?, checked.full("wide_payload")?);
    let (encode, decode) = (checked.full("encode_once")?, checked.full("decode_once")?);
    let parse = checked.full("parse_only")?;
    let (to_json, codec) = (checked.full("to_json")?, checked.full("codec_only")?);

    let mut out = Vec::new();
    for &(lines, note_width) in points {
        let mut machine = checked.machine();
        let args = vec![Value::Int(lines as i64), Value::Int(note_width as i64)];
        let value = call(&mut machine, &order, args.clone())?;
        let bytes = call(&mut machine, &payload, args)?;
        let Value::Bytes(raw) = &bytes else {
            bail!("`wide_payload` answered a {}, not Bytes", bytes.type_name());
        };
        let payload_bytes = raw.len();

        let document = call(&mut machine, &to_json, vec![bytes.clone()])?;

        call(&mut machine, &encode, vec![value.clone()])?;
        call(&mut machine, &decode, vec![bytes.clone()])?;
        call(&mut machine, &parse, vec![bytes.clone()])?;
        call(&mut machine, &codec, vec![document.clone()])?;

        let encoded = best_of(repeats, || {
            let started = Instant::now();
            for _ in 0..iterations {
                call(&mut machine, &encode, vec![value.clone()])?;
            }
            Ok(started.elapsed())
        })?;
        let decoded = best_of(repeats, || {
            let started = Instant::now();
            for _ in 0..iterations {
                call(&mut machine, &decode, vec![bytes.clone()])?;
            }
            Ok(started.elapsed())
        })?;
        let parsed = best_of(repeats, || {
            let started = Instant::now();
            for _ in 0..iterations {
                call(&mut machine, &parse, vec![bytes.clone()])?;
            }
            Ok(started.elapsed())
        })?;
        let codeced = best_of(repeats, || {
            let started = Instant::now();
            for _ in 0..iterations {
                call(&mut machine, &codec, vec![document.clone()])?;
            }
            Ok(started.elapsed())
        })?;

        let per = |d: Duration| micros(d) / iterations as f64;
        let fields = lines * 5 + 4;
        out.push(ShapePoint {
            lines,
            note_width,
            payload_bytes,
            fields,
            encode_micros: per(encoded),
            decode_micros: per(decoded),
            parse_micros: per(parsed),
            codec_micros: per(codeced),
            decode_micros_per_byte: per(decoded) / payload_bytes as f64,
            decode_micros_per_field: per(decoded) / fields as f64,
        });
    }
    Ok(out)
}

/// `loop_only` is the subtrahend, and it is why these rows are about `Map` and not about `fold`.
const MAP_SRC: &str = r#"
fn key(i: Int) -> Int = (i * 2654435761) % 1000003

pub fn loop_only(n: Int) -> Int = fold(range(0, n), 0, |a: Int, i: Int| a + key(i))

pub fn build(n: Int) -> Map<Int, Int> =
  fold(range(0, n), map_new(), |m: Map<Int, Int>, i: Int| map_insert(m, key(i), i))

pub fn build_descending(n: Int) -> Map<Int, Int> =
  fold(range(0, n), map_new(), |m: Map<Int, Int>, i: Int|
    map_insert(m, key(n - 1 - i), n - 1 - i))

pub fn lookups(m: Map<Int, Int>, n: Int) -> Int =
  fold(range(0, n), 0, |acc: Int, i: Int|
    match map_get(m, key(i)) { Some(v) -> acc + v, None -> acc })

pub fn keys_sum(m: Map<Int, Int>) -> Int = fold(map_keys(m), 0, |a: Int, k: Int| a + k)

pub fn folded(m: Map<Int, Int>) -> Int = map_fold(m, 0, |a: Int, k: Int, v: Int| a + k + v)

test "insertion order changes neither the map nor the order it iterates in" {
  assert_eq(build(256), build_descending(256));
  assert_eq(map_keys(build(256)), map_keys(build_descending(256)))
}
"#;

#[derive(Clone, Debug, Serialize)]
pub struct MapPoint {
    pub entries: usize,
    /// `map_insert`, with the enclosing `fold` and the key computation subtracted — measured
    /// alternately with that scaffold rather than against a separate run of it.
    pub insert_nanos: f64,
    /// `map_get` on a key that is present, same subtraction.
    pub get_nanos: f64,
    /// `map_keys`, per entry of the list it materializes.
    pub keys_nanos_per_entry: f64,
    /// `map_fold`, per entry, which is the iteration that allocates no list.
    pub fold_nanos_per_entry: f64,
    /// What the `fold`/`range`/`key` scaffold cost per iteration on its own.
    pub loop_nanos: f64,
}

/// Times two calls that differ by one operation, alternating them call by call so that whatever the
/// machine was doing landed on both.
fn paired(
    machine: &mut Machine<'_>,
    target: (&str, Vec<Value>),
    subtrahend: (&str, Vec<Value>),
    batch: u32,
    repeats: usize,
) -> Result<(Duration, Duration)> {
    let (target_name, target_args) = target;
    let (other_name, other_args) = subtrahend;
    call(machine, target_name, target_args.clone())?;
    call(machine, other_name, other_args.clone())?;

    let mut best: Option<(Duration, Duration)> = None;
    for _ in 0..repeats.max(1) {
        let (mut a, mut b) = (Duration::ZERO, Duration::ZERO);
        for _ in 0..batch {
            let started = Instant::now();
            call(machine, target_name, target_args.clone())?;
            a += started.elapsed();
            let started = Instant::now();
            call(machine, other_name, other_args.clone())?;
            b += started.elapsed();
        }
        if best.is_none_or(|(previous, _)| a < previous) {
            best = Some((a, b));
        }
    }
    let (a, b) = best.expect("at least one repeat");
    Ok((a / batch, b / batch))
}

pub fn map_ops(sizes: &[usize], repeats: usize) -> Result<Vec<MapPoint>> {
    let dir = write_project(&[("maps.ply", MAP_SRC.to_string())])?;
    let checked = Checked::open(dir.path())?;
    let (loop_only, build) = (checked.full("loop_only")?, checked.full("build")?);
    let (lookups, keys_sum) = (checked.full("lookups")?, checked.full("keys_sum")?);
    let folded = checked.full("folded")?;

    let mut out = Vec::new();
    for &entries in sizes {
        let mut machine = checked.machine();
        let n = Value::Int(entries as i64);
        let map = call(&mut machine, &build, vec![n.clone()])?;

        // Enough calls that one call's fixed cost is spread over a batch, and few enough that the
        // largest size still finishes.
        let batch = (500_000 / entries.max(1)).clamp(1, 200) as u32;
        let time = |machine: &mut Machine<'_>, name: &str, args: Vec<Value>| -> Result<Duration> {
            call(machine, name, args.clone())?;
            let taken = best_of(repeats, || {
                let started = Instant::now();
                for _ in 0..batch {
                    call(machine, name, args.clone())?;
                }
                Ok(started.elapsed())
            })?;
            Ok(taken / batch)
        };

        // `map_insert` and `map_get` cannot be called without a `fold` around them, so these two
        // rows are subtractions and there is no way to make them anything else.
        let (built, scaffold) = paired(
            &mut machine,
            (&build, vec![n.clone()]),
            (&loop_only, vec![n.clone()]),
            batch,
            repeats,
        )?;
        let (got, lookup_scaffold) = paired(
            &mut machine,
            (&lookups, vec![map.clone(), n.clone()]),
            (&loop_only, vec![n.clone()]),
            batch,
            repeats,
        )?;
        let keys = time(&mut machine, &keys_sum, vec![map.clone()])?;
        let folds = time(&mut machine, &folded, vec![map.clone()])?;

        let per = |d: Duration| d.as_secs_f64() * 1e9 / entries as f64;
        // Not clamped.
        out.push(MapPoint {
            entries,
            insert_nanos: per(built) - per(scaffold),
            get_nanos: per(got) - per(lookup_scaffold),
            keys_nanos_per_entry: per(keys),
            fold_nanos_per_entry: per(folds),
            loop_nanos: per(scaffold),
        });
    }
    Ok(out)
}

/// A program whose whole output is what `map_keys` answered, under three insertion orders that
/// build one key set.
const ORDER_SRC: &str = r#"
fn key(i: Int) -> Int = (i * 2654435761) % 100003

fn insert_all(order: List<Int>) -> Map<Int, Int> =
  fold(order, map_new(), |m: Map<Int, Int>, i: Int| map_insert(m, key(i), i))

fn ascending(n: Int) -> List<Int> = range(0, n)

fn descending(n: Int) -> List<Int> = map(range(0, n), |i: Int| n - 1 - i)

fn shuffled(n: Int) -> List<Int> = map(range(0, n), |i: Int| (i * 7919) % n)

fn render(m: Map<Int, Int>) -> String =
  fold(map_keys(m), "", |acc: String, k: Int| acc ++ "," ++ int_to_string(k))

fn n() -> Int = 997

// A leading `|` so that the three sequences are recovered by splitting rather
// than by trusting where `ply run`'s own framing stops. Without it the first
// sequence carries the framing and compares unequal to two that do not, and the
// harness reports an ordering defect that is its own.
fn main() -> String =
  "|" ++ render(insert_all(ascending(n())))
    ++ "|" ++ render(insert_all(descending(n())))
    ++ "|" ++ render(insert_all(shuffled(n())))
"#;

#[derive(Clone, Debug, Serialize)]
pub struct OrderCheck {
    pub processes: usize,
    pub entries: usize,
    /// Every process printed the same bytes.
    pub identical_across_processes: bool,
    /// All three insertion orders produced one key sequence.
    pub identical_across_insertion_orders: bool,
    /// That sequence is strictly ascending, which is the contract rather than merely a stable
    /// order.
    pub ascending: bool,
}

/// Runs `ply run` in `processes` separate processes and compares what each printed.
pub fn map_order(ply: &Path, processes: usize) -> Result<OrderCheck> {
    let dir = write_project(&[("order.ply", ORDER_SRC.to_string())])?;

    let mut outputs = Vec::new();
    for _ in 0..processes.max(2) {
        let out = Command::new(ply)
            .args(["run", "--color", "never"])
            .current_dir(dir.path())
            .output()
            .with_context(|| format!("running `{} run`", ply.display()))?;
        if !out.status.success() {
            bail!(
                "`ply run` exited {}:\n{}{}",
                out.status,
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        outputs.push(String::from_utf8_lossy(&out.stdout).trim().to_string());
    }

    let first = outputs[0].clone();
    let identical_across_processes = outputs.iter().all(|o| *o == first);

    // `ply run` renders the returned `String` with its own framing — an indent and the quotes a
    // `String` is printed inside — so the three renderings are recovered from the separator the
    // program wrote after that framing is stripped, rather than from the whole line.
    let body = first.trim().trim_matches('"');
    let runs: Vec<&str> = body.split('|').skip(1).collect();
    if runs.len() != 3 {
        bail!("`main` should have printed three key sequences, and printed {first}");
    }
    let keys: Vec<i64> = runs[0]
        .split(',')
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.trim().parse().ok())
        .collect();

    Ok(OrderCheck {
        processes: outputs.len(),
        entries: keys.len(),
        identical_across_processes,
        identical_across_insertion_orders: runs[1] == runs[0] && runs[2] == runs[0],
        ascending: !keys.is_empty() && keys.windows(2).all(|w| w[0] < w[1]),
    })
}

/// One module of `types` record types, with or without a `derive` for each.
fn derived_module(index: usize, types: usize, derived: bool) -> String {
    let mut s = String::new();
    if derived {
        s.push_str("import std.json\n\n");
    }
    for i in 0..types {
        let name = format!("T{index}x{i}");
        let lower = format!("t{index}x{i}");
        s.push_str(&format!(
            "pub type {name} = {{ name: String, count: Int, price: Decimal, tags: List<String>, live: Bool }}\n\n"
        ));
        if derived {
            s.push_str(&format!("derive json for {name}\n\n"));
        }
        s.push_str(&format!(
            "fn {lower}_value() -> {name} =\n  \
             {{name: \"{lower}\", count: {i}, price: 1.25m, tags: [\"a\", \"b\"], live: true}}\n\n"
        ));
        if derived {
            s.push_str(&format!(
                "test \"{lower} round-trips\" {{\n  \
                 let src = json::encode_bytes({lower}_value(), {lower}_json());\n  \
                 assert_eq(json::decode_bytes(src, {lower}_json()), Ok({lower}_value()))\n}}\n\n"
            ));
        } else {
            s.push_str(&format!(
                "test \"{lower} round-trips\" {{\n  \
                 assert_eq({lower}_value().count, {i})\n}}\n\n"
            ));
        }
    }
    s
}

#[derive(Clone, Debug, Serialize)]
pub struct DerivePoint {
    pub variant: &'static str,
    pub types: usize,
    pub modules: usize,
    /// Everything the front end checked, the stdlib included.
    pub definitions: usize,
    /// The project's own.
    pub project_definitions: usize,
    /// Tests the project declares.
    pub tests: usize,
    /// A check with no cache at all.
    pub cold_check_millis: f64,
    /// A check against a cache a previous identical run filled — both gates hit, which is what a
    /// project's second `ply check` costs.
    pub warm_check_millis: f64,
    /// Selecting and running every test, from an empty result cache.
    pub cold_test_millis: f64,
    /// The same run when every test is a cache hit.
    pub warm_test_millis: f64,
    pub cache_bytes: u64,
}

pub fn derivation_cost(
    type_counts: &[usize],
    types_per_module: usize,
    repeats: usize,
) -> Result<Vec<DerivePoint>> {
    let mut out = Vec::new();
    for &types in type_counts {
        for derived in [false, true] {
            out.push(one_derivation_point(
                types,
                types_per_module,
                derived,
                repeats,
            )?);
        }
    }
    Ok(out)
}

fn one_derivation_point(
    types: usize,
    per_module: usize,
    derived: bool,
    repeats: usize,
) -> Result<DerivePoint> {
    let per_module = per_module.max(1);
    let modules = types.div_ceil(per_module);
    let files: Vec<(String, String)> = (0..modules)
        .map(|m| {
            let here = per_module.min(types - m * per_module);
            (format!("m{m}.ply"), derived_module(m, here, derived))
        })
        .collect();

    // A fresh copy per timing, so a cold number is cold: a store the previous measurement filled
    // would make the second project's cold check a warm one.
    let cold = best_of(repeats, || {
        let dir = write_files(&files)?;
        let mut store = Store::open(dir.path())?;
        let started = Instant::now();
        let loaded =
            driver::load_incremental(dir.path(), &mut store).map_err(|e| compile_error(&e))?;
        let taken = started.elapsed();
        drop(loaded);
        store.flush()?;
        Ok(taken)
    })?;

    let dir = write_files(&files)?;
    let mut store = Store::open(dir.path())?;
    let loaded = driver::load_incremental(dir.path(), &mut store).map_err(|e| compile_error(&e))?;
    let definitions = loaded.check.defs.len();
    let project_definitions = loaded
        .check
        .defs
        .values()
        .filter(|d| !ply_std::is_std(&d.module))
        .count();
    let tests = loaded
        .check
        .tests
        .iter()
        .filter(|t| !ply_std::is_std(&t.module))
        .count();
    let cold_test = run_tests(&loaded, &mut store)?;
    let warm_test = run_tests(&loaded, &mut store)?;
    drop(loaded);
    store.flush()?;
    drop(store);

    let warm = best_of(repeats, || {
        let mut store = Store::open(dir.path())?;
        let started = Instant::now();
        let loaded =
            driver::load_incremental(dir.path(), &mut store).map_err(|e| compile_error(&e))?;
        let taken = started.elapsed();
        drop(loaded);
        store.flush()?;
        Ok(taken)
    })?;

    Ok(DerivePoint {
        variant: if derived { "derived" } else { "plain" },
        types,
        modules,
        definitions,
        project_definitions,
        tests,
        cold_check_millis: millis(cold),
        warm_check_millis: millis(warm),
        cold_test_millis: millis(cold_test),
        warm_test_millis: millis(warm_test),
        cache_bytes: directory_bytes(&dir.path().join(ply_store::CACHE_DIR_NAME))?,
    })
}

/// Every test the *project* declares, which is what `ply test` runs.
fn run_tests(loaded: &ply_cli::load::Loaded, store: &mut Store) -> Result<Duration> {
    let started = Instant::now();
    let selection = ply_test::select(
        &loaded.check,
        &loaded.hashes,
        store,
        &ply_eval::Plan::default(),
    );
    let plan = ply_cli::commands::test::Plan::new(selection, &loaded.check, None, false);
    let selection = plan.selection;
    let report = ply_test::run(
        &selection,
        &loaded.program,
        &loaded.resolved,
        &loaded.check,
        &loaded.hashes,
        store,
        false,
        ply_test::Search::of(&selection),
        ply_test::Hosting::hermetic(),
    );
    if report.failed > 0 {
        bail!(
            "{} of the measurement project's tests failed; the number would be about failure \
             formatting rather than about derivation",
            report.failed
        );
    }
    Ok(started.elapsed())
}

fn write_files(files: &[(String, String)]) -> Result<tempfile::TempDir> {
    let dir = tempfile::tempdir().context("a temp dir for a derivation project")?;
    for (name, source) in files {
        std::fs::write(dir.path().join(name), source)?;
    }
    Ok(dir)
}

fn compile_error(e: &ply_cli::load::LoadError) -> anyhow::Error {
    let shown: Vec<String> = e
        .diagnostics
        .iter()
        .take(5)
        .map(|d| format!("{}: {}", d.code, d.message))
        .collect();
    anyhow::anyhow!(
        "the derivation project does not compile:\n  {}",
        shown.join("\n  ")
    )
}

fn directory_bytes(dir: &Path) -> Result<u64> {
    if !dir.exists() {
        return Ok(0);
    }
    let mut total = 0;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        total += if meta.is_dir() {
            directory_bytes(&entry.path())?
        } else {
            meta.len()
        };
    }
    Ok(total)
}

/// Where the `ply` binary is, given this one.
pub fn ply_binary() -> Result<PathBuf> {
    crate::serve::ply_binary()
}

#[derive(Clone, Debug, Serialize)]
pub struct Measurements {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub json: Vec<JsonPoint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shape: Vec<ShapePoint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub maps: Vec<MapPoint>,
    pub order: Option<OrderCheck>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derivation: Vec<DerivePoint>,
}

pub fn render(m: &Measurements) -> String {
    let mut s = String::new();

    if !m.json.is_empty() {
        s.push_str("JSON through a derived codec — one thread, machine engine\n");
        s.push_str(&format!(
            "  {:>7} {:>10} {:>12} {:>12} {:>12} {:>12}\n",
            "lines", "bytes", "encode µs", "decode µs", "encode MB/s", "decode MB/s"
        ));
        for p in &m.json {
            s.push_str(&format!(
                "  {:>7} {:>10} {:>12.1} {:>12.1} {:>12.2} {:>12.2}\n",
                p.lines,
                p.payload_bytes,
                p.encode_micros,
                p.decode_micros,
                p.encode_mb_per_second,
                p.decode_mb_per_second
            ));
        }
        s.push('\n');
    }

    if !m.shape.is_empty() {
        s.push_str(
            "what a decode is priced by — fields fixed down a column, bytes grown beside them\n",
        );
        s.push_str(&format!(
            "  {:>7} {:>7} {:>9} {:>8} {:>10} {:>10} {:>10} {:>10} {:>10} {:>10}\n",
            "lines",
            "pad",
            "bytes",
            "fields",
            "encode µs",
            "decode µs",
            "parse µs",
            "codec µs",
            "µs/byte",
            "µs/field"
        ));
        for p in &m.shape {
            s.push_str(&format!(
                "  {:>7} {:>7} {:>9} {:>8} {:>10.1} {:>10.1} {:>10.1} {:>10.1} {:>10.4} {:>10.2}\n",
                p.lines,
                p.note_width,
                p.payload_bytes,
                p.fields,
                p.encode_micros,
                p.decode_micros,
                p.parse_micros,
                p.codec_micros,
                p.decode_micros_per_byte,
                p.decode_micros_per_field
            ));
        }
        let widened: Vec<&ShapePoint> = m
            .shape
            .iter()
            .filter(|p| p.lines == m.shape[0].lines)
            .collect();
        if let (Some(first), Some(last)) = (widened.first(), widened.last())
            && first.payload_bytes < last.payload_bytes
        {
            s.push_str(&format!(
                "  at {} lines: {:.0}x the bytes cost {:.2}x the decode; per field would be 1.00x\n",
                first.lines,
                last.payload_bytes as f64 / first.payload_bytes as f64,
                last.decode_micros / first.decode_micros
            ));
        }
        s.push('\n');
    }

    if !m.maps.is_empty() {
        s.push_str("Map — the `fold`/`range` scaffold subtracted, and printed\n");
        s.push_str(&format!(
            "  {:>9} {:>12} {:>12} {:>12} {:>12} {:>12}\n",
            "entries", "insert ns", "get ns", "keys ns/e", "fold ns/e", "loop ns"
        ));
        for p in &m.maps {
            s.push_str(&format!(
                "  {:>9} {:>12.0} {:>12.0} {:>12.0} {:>12.0} {:>12.0}\n",
                p.entries,
                p.insert_nanos,
                p.get_nanos,
                p.keys_nanos_per_entry,
                p.fold_nanos_per_entry,
                p.loop_nanos
            ));
        }
        s.push('\n');
    }

    if let Some(o) = &m.order {
        s.push_str(&format!(
            "map_keys over {} entries — {} processes: identical {}, order-independent {}, ascending {}\n\n",
            o.entries,
            o.processes,
            o.identical_across_processes,
            o.identical_across_insertion_orders,
            o.ascending
        ));
    }

    if !m.derivation.is_empty() {
        s.push_str("derivation's cost — the same types, with and without a `derive`\n");
        s.push_str(&format!(
            "  {:<9} {:>6} {:>7} {:>8} {:>7} {:>11} {:>11} {:>10} {:>10} {:>10}\n",
            "variant",
            "types",
            "defs",
            "own defs",
            "tests",
            "cold check",
            "warm check",
            "cold test",
            "warm test",
            "cache KiB"
        ));
        for p in &m.derivation {
            s.push_str(&format!(
                "  {:<9} {:>6} {:>7} {:>8} {:>7} {:>10.1}m {:>10.1}m {:>9.1}m {:>9.1}m {:>10}\n",
                p.variant,
                p.types,
                p.definitions,
                p.project_definitions,
                p.tests,
                p.cold_check_millis,
                p.warm_check_millis,
                p.cold_test_millis,
                p.warm_test_millis,
                p.cache_bytes / 1024
            ));
        }
        for types in m
            .derivation
            .iter()
            .map(|p| p.types)
            .collect::<std::collections::BTreeSet<_>>()
        {
            let at = |variant: &str| {
                m.derivation
                    .iter()
                    .find(|p| p.types == types && p.variant == variant)
            };
            if let (Some(plain), Some(derived)) = (at("plain"), at("derived")) {
                let codecs = derived.project_definitions as i64 - plain.project_definitions as i64;
                let shipped = derived.definitions as i64 - derived.project_definitions as i64;
                s.push_str(&format!(
                    "  {types} types: {codecs} project definitions ({:.2} per type) plus {shipped} in `std`, \
                     cache {:.1}x, cold check {:.2}x, warm check {:.2}x\n",
                    codecs as f64 / types as f64,
                    derived.cache_bytes as f64 / plain.cache_bytes.max(1) as f64,
                    derived.cold_check_millis / plain.cold_check_millis,
                    derived.warm_check_millis / plain.warm_check_millis,
                ));
            }
        }
    }

    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three programs are the measurement, so a change that stops one of them compiling has to
    /// fail here rather than at the moment somebody wants a number.
    #[test]
    fn every_measurement_program_compiles_and_its_own_test_passes() {
        for (name, source) in [
            ("payload.ply", JSON_SRC),
            ("maps.ply", MAP_SRC),
            ("order.ply", ORDER_SRC),
        ] {
            let dir = write_project(&[(name, source.to_string())]).unwrap();
            let checked = Checked::open(dir.path())
                .unwrap_or_else(|e| panic!("`{name}` does not compile: {e:#}"));
            let mut store = Store::open(dir.path()).unwrap();
            run_tests(&checked.loaded, &mut store)
                .unwrap_or_else(|e| panic!("`{name}`'s own test failed: {e:#}"));
        }
    }

    /// The subtraction has to leave something.
    #[test]
    fn the_map_rows_survive_subtracting_the_fold_around_them() {
        for p in map_ops(&[256, 4_096], 2).unwrap() {
            assert!(
                p.insert_nanos > 0.0 && p.get_nanos > 0.0,
                "at {} entries insert was {} ns and get {} ns above a {} ns scaffold",
                p.entries,
                p.insert_nanos,
                p.get_nanos,
                p.loop_nanos
            );
            assert!(p.keys_nanos_per_entry > 0.0 && p.fold_nanos_per_entry > 0.0);
        }
    }

    /// The two axes have to move independently, or the table cannot separate a per-field cost from
    /// a per-byte one — which is the only question it is there to answer.
    #[test]
    fn widening_a_field_grows_the_bytes_and_not_the_field_count() {
        let points = json_shape(&[(2, 0), (2, 200), (8, 0)], 2, 1).unwrap();
        let [narrow, wide, longer] = &points[..] else {
            panic!("three points were asked for and {} came back", points.len());
        };
        assert_eq!(narrow.fields, wide.fields);
        assert!(wide.payload_bytes > narrow.payload_bytes + 200);
        assert!(longer.fields > narrow.fields && longer.payload_bytes > narrow.payload_bytes);
        // Both halves are timed on their own, so each is a duration rather than a difference: a
        // zero here means a half that did not run.
        assert!(
            narrow.parse_micros > 0.0 && narrow.codec_micros > 0.0,
            "parse {} µs, codec {} µs of a {} µs decode",
            narrow.parse_micros,
            narrow.codec_micros,
            narrow.decode_micros
        );
    }

    /// Both variants have to be the same program in everything but the derivation, or the
    /// comparison prices two projects rather than one feature.
    #[test]
    fn the_two_derivation_variants_declare_the_same_types_and_the_same_tests() {
        let plain = derived_module(0, 3, false);
        let derived = derived_module(0, 3, true);
        for i in 0..3 {
            let decl = format!("pub type T0x{i} = ");
            assert!(plain.contains(&decl) && derived.contains(&decl));
            assert!(plain.contains(&format!("t0x{i} round-trips")));
            assert!(derived.contains(&format!("t0x{i} round-trips")));
        }
        assert!(!plain.contains("derive json"));
        assert_eq!(derived.matches("derive json for").count(), 3);
    }

    /// Small, but it exercises the whole path — two projects, four timings, a cache measured —
    /// which is what a table nobody can reproduce would hide.
    #[test]
    fn a_derivation_point_is_produced_for_both_variants() {
        let points = derivation_cost(&[4], 4, 1).unwrap();
        assert_eq!(points.len(), 2);
        let derived = points.iter().find(|p| p.variant == "derived").unwrap();
        let plain = points.iter().find(|p| p.variant == "plain").unwrap();
        assert_eq!(derived.tests, plain.tests);
        assert!(
            derived.definitions > plain.definitions,
            "a derivation that added no definition is not a derivation"
        );
    }
}
