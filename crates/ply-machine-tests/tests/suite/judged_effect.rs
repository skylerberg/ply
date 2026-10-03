//! `prover.judged` through the effect: a program configures a run, collects, prepares, and judges
//! points of one obligation it chose itself, reading what each came to. The fixture is the module
//! the payload's constructors are named after, so it declares them where the package does:
//! `the_fixture_declares_the_payload_where_the_machine_names_it` is what says so.

use crate::fixture::project;
use ply_eval::host::HostRegistry;
use ply_eval::{Analysis, Machine, Provider, Span, Value};
use std::sync::Arc;

/// The judging program. Every operation of the effect is declared, as the run that binds it
/// requires, and `Batch` and `Judged` are the shapes `claims.ply` sends and reads.
const JUDGING: &str = r#"
import std.value (Value, VInt, render)

nondet effect prover {
  write configure[claims](options: Options, front: LoadedAnalysis, world: World) -> Unit
  read collected[claims]() -> Result<Collection, Refusal>
  write compiled[claims](unit: Bytes) -> Result<Unit, List<Diag>>
  read schema[claims](name: String) -> Result<Value, List<Diag>>
  read prepared[claims](step_budget: Int, config: Configured) -> Result<Unit, Refusal>
  read cached[claims](keys: List<String>) -> List<Option<String>>
  read judged[claims](batches: List<Batch>) -> List<List<Judged>>
  read interleaved[claims](claim: Int, point: List<Value>, seed: Seed, steps: Int) -> LawRun
  read ended[claims]() -> List<Diag>
  write record[claims](entries: List<{ key: String, evidence: String }>) -> List<Diag>
  read baselines[claims](names: List<String>) -> List<Baseline>
  write accepted[claims](records: List<Baseline>) -> Accepted
}

type Configured = {
  values: List<{ key: String, value: String, secret: Bool }>,
  schema: Option<{ function: String, keys: List<{ name: String, shape: String }> }>,
  opened: Bool,
}

// The world's vocabulary is a shape this fixture only carries: it never reads one, so it names
// each type itself rather than borrowing the name of the package's.
type Shape =
  | Var(Int)
  | Fn(List<Shape>, Shape, Bool)
  | Record(List<{ name: String, ty: Shape }>)
  | Con(String, List<Shape>)
type Variant = { name: String, fields: List<Shape> }
type Decl = { name: String, params: Int, variants: List<Variant> }
type Signature = { name: String, ty: Shape, pure: Bool }
type Kind = | Ensures(Int) | Law(Option<String>)
type Frame = | Pure | Writes(List<String>)
type Points = | Every({ shapes: List<Unit>, points: Int }, String) | Drawn
type Unsettled = | Unhandled(String) | Run(Points)
type Strategy = | Interleave(Points) | Hosted | Static(Unsettled) | Fitting
type Bound = { name: String, ty: Shape, text: String }
type Claimed = {
  key: String,
  owner: String,
  kind: Kind,
  at: { module: Int, start: Int, end: Int },
  binders: List<Bound>,
  result: Option<Bound>,
  variables: List<String>,
  guards: List<{ module: Int, start: Int, end: Int }>,
  literals: List<Bytes>,
  host: Bool,
  footprint: Option<String>,
  frame: Frame,
  strategy: Strategy,
}
type World = { decls: List<Decl>, signatures: List<Signature>, obligations: List<Claimed> }
type Tls = Unit
type Named = Unit
type Db = { url: Option<String>, pool: Option<Int>, acquire_ms: Option<Int>, statement_ms: Option<Int>, idle_txn_ms: Option<Int>, connect_ms: Option<Int>, statement_cache: Option<Int>, schema: Option<String> }
type Config = { set: List<String>, files: List<String>, schema: Option<String> }
type Trace = { sink: String, level: String }
type ProveOpts = { cases: Option<Int>, roots: Option<Int>, budget: Option<Int>, shrink_budget: Option<Int>, steps: Option<Int> }
type SimOpts = { seed: Option<String>, mode: String, roots: Option<{ from: Int, to: Int }>, budget: Option<Int>, steps: Option<Int>, measure_reduction: Bool }
type Options = {
  path: String,
  no_incremental: Bool,
  no_cache: Bool,
  std: Bool,
  jobs: Option<Int>,
  host: Bool,
  tls: List<Tls>,
  trust: List<String>,
  fs: List<Named>,
  db: Db,
  config: Config,
  trace: Trace,
  prove: ProveOpts,
  sim: SimOpts,
}
type Refusal = Unit
type Collection = Unit
type Diag = Unit
type Evidence = Unit
type Seed = Unit
type LawRun = Unit
type Baseline = Unit
type Accepted = Unit
type LoadedAnalysis = {
  dump: Bytes,
  files: List<{ path: String, name: String, text: Bytes }>,
  read_ms: Int,
  front_ms: Int,
  file_ms: Int,
  cached: Bool,
}
type Mode = | MWhole | MWitness | MDomain | MCost(Int)
type Batch = { claim: Int, points: List<List<Value>>, mode: Mode }
type Judged =
  | JHeld
  | JFailed
  | JRejected
  | JRaised({ message: String, values: List<Value> })
  | JFaulted({ code: String, message: String, notes: List<String>, values: List<Value> })
  | JMeasured({ steps: Int, bound: Int })
  | JSpent(Int)

type Answer = { failed: Int, held: Int, rejected: Int, first: String }

/// The points judged: `n` from zero, one batch each, so a failing point ends only its own batch.
fn cases() -> Int = 64

fn judged_at(index: Int) -> Answer / {prover.judged[claims], abort.raise} = {
  let points = map(range(0, cases()), |n: Int| [VInt(n)]);
  let answers = prover.judged[claims](map(points, |p: List<Value>| { claim: index, points: [p], mode: MWhole }));
  fold(range(0, len(answers)), { failed: 0, held: 0, rejected: 0, first: "" }, |seen: Answer, i: Int|
    match (list_at(answers, i), list_at(points, i)) {
      (Some([JHeld]), _) -> { ..seen, held: seen.held + 1 },
      (Some([JFailed]), Some(p)) -> {
        ..seen,
        failed: seen.failed + 1,
        first: if seen.first == "" { fold(p, "", |acc: String, v: Value| acc ++ render(v)) } else { seen.first },
      },
      (Some([JRejected]), _) -> { ..seen, rejected: seen.rejected + 1 },
      _ -> seen,
    })
}

fn main(root: String, index: Int, front: LoadedAnalysis, unit: Bytes, world: World) -> Answer / {prover.configure[claims], prover.collected[claims], prover.compiled[claims], prover.prepared[claims], prover.judged[claims], abort.raise} = {
  prover.configure[claims]({
    path: root,
    no_incremental: false,
    no_cache: false,
    std: false,
    jobs: None,
    host: false,
    tls: [],
    trust: [],
    fs: [],
    db: {
      url: None,
      pool: None,
      acquire_ms: None,
      statement_ms: None,
      idle_txn_ms: None,
      connect_ms: None,
      statement_cache: None,
      schema: None,
    },
    config: { set: [], files: [], schema: None },
    trace: { sink: "off", level: "info" },
    prove: { cases: None, roots: None, budget: None, shrink_budget: None, steps: None },
    sim: {
      seed: None,
      mode: "dpor",
      roots: None,
      budget: None,
      steps: None,
      measure_reduction: false,
    },
  }, front, world);
  match (prover.collected[claims](), prover.compiled[claims](unit), prover.prepared[claims](1000000000, { values: [], schema: None, opened: false })) {
    (Ok(_), Ok(_), Ok(_)) -> judged_at(index),
    _ -> { failed: 0 - 1, held: 0, rejected: 0, first: "" },
  }
}
"#;

/// The program under test: one false law, so its single obligation is claim `0`.
const A_FALSE_LAW: &str = r#"
law "doubling is tripling"
  forall (n: Int) {
    n + n == n * 3
  }
"#;

/// That law as `proof.world` would hand it over.
const THE_LAW: (&str, &[&str]) = ("m.doubling is tripling", &["n"]);

fn front_of(source: &str) -> Analysis {
    ply_codegen::c::producer::ensure_default();
    ply_codegen::c::producer::checked_analysis_with_std(&[(
        "proof.obligation".to_string(),
        source.to_string(),
    )])
    .expect("the judging program checks")
    .front
}

/// The program checked with the standard library it imports, and compiled.
fn built(source: &str) -> (Analysis, &'static ply_codegen::Unit) {
    ply_codegen::c::producer::ensure_default();
    let answered = ply_codegen::c::producer::checked_analysis_with_std(&[(
        "proof.obligation".to_string(),
        source.to_string(),
    )])
    .expect("the judging program checks");
    let unit =
        ply_codegen::Unit::over_front(&answered.front, answered.modules.into_iter().collect())
            .expect("this host has a C toolchain");
    (answered.front, unit)
}

/// The fixture's answer, from one entered call.
fn one_run(source: &str, index: i64) -> Result<Value, ply_eval::Diagnostic> {
    let project = project(source);
    let (front, unit) = built(JUDGING);
    let mut machine =
        Machine::new(&front, unit.attach()).expect("the unit was compiled from this program");
    let mut registry = HostRegistry::new();
    for (op, handler) in ply_machine::claims::lent("proof.obligation") {
        registry.register(op, handler);
    }
    let binding = registry.bind(&front.check).expect("the prover ops bind");
    machine.set_host_binding(Arc::new(binding));
    machine
        .call(
            "proof.obligation.main",
            vec![
                Value::str(project.path().display().to_string()),
                Value::Int(index),
                crate::fixture::handed(project.path()),
                crate::fixture::unit(project.path()),
                crate::fixture::int_laws(&[THE_LAW]),
            ],
            Span::DUMMY,
        )
        .into_parts()
        .0
}

fn field(answer: &Value, name: &str) -> Value {
    match answer {
        Value::Record(fields) => fields
            .iter()
            .find(|(key, _)| key.as_str() == name)
            .map(|(_, value)| value.clone())
            .unwrap_or_else(|| panic!("the answer carries `{name}`: {answer:?}")),
        other => panic!("the answer is a record, not {other:?}"),
    }
}

fn int(answer: &Value, name: &str) -> i64 {
    match field(answer, name) {
        Value::Int(n) => n,
        other => panic!("`{name}` is an int, not {other:?}"),
    }
}

#[test]
fn a_judged_point_comes_back_as_the_value_that_falsifies_the_claim() {
    let answer = one_run(A_FALSE_LAW, 0).expect("the run finished");
    assert!(
        int(&answer, "failed") > 0 && int(&answer, "held") > 0,
        "a law that holds only at zero was judged as {answer:?}"
    );
    let first = field(&answer, "first");
    let drawn = match &first {
        Value::Str(text) => text.to_string(),
        other => panic!("the first falsifying point is a string, not {other:?}"),
    };
    // Independently of the prover: the point really does break the law.
    let n: i64 = drawn.parse().unwrap_or_else(|e| {
        panic!("the point was `{drawn}`, which is not an integer: {e}");
    });
    assert!(
        n + n != n * 3,
        "the point `{n}` was judged failed, and the law holds there"
    );
}

#[test]
fn a_claim_index_the_collection_does_not_hold_is_refused_rather_than_answered() {
    let why = one_run(A_FALSE_LAW, 99).expect_err("there is no claim 99");
    assert!(
        why.message.contains("no claim 99"),
        "the run said `{}` rather than naming the claim",
        why.message
    );
}

#[test]
fn the_fixture_declares_the_payload_where_the_machine_names_it() {
    let front = front_of(JUDGING);
    let mut checked = 0;
    for (home, ty, _) in ply_machine::claims::MARSHALLED {
        let declared: Vec<&str> = front
            .types
            .values()
            .filter(|t| t.simple_name.as_str() == *ty)
            .map(|t| t.module.as_str())
            .collect();
        if declared.is_empty() {
            continue;
        }
        assert!(
            declared.iter().all(|module| module == home),
            "the fixture declares `{ty}` in {declared:?}, and the machine builds its constructors \
             in `{home}`: a tag that names the wrong module is one no arm matches"
        );
        checked += 1;
    }
    assert!(
        checked > 0,
        "the fixture mirrors no type the machine marshals"
    );
}
