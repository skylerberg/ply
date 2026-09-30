//! `prover.replay` through the effect: a program configures a run, collects, and re-runs one
//! case of one obligation's guard at a root it chooses, reading what the draw said. The fixture is
//! the module the payload's constructors are named after, so it declares them where the package
//! does: `the_fixture_declares_the_payload_where_the_machine_names_it` is what says so.

use crate::fixture::project;
use ply_eval::host::HostRegistry;
use ply_eval::{Machine, Provider, Value};
use ply_span::{SourceId, Span};
use ply_ty::Front;
use std::collections::HashMap;
use std::sync::Arc;

/// The re-run program. Every operation of the effect is declared, as the run that binds it
/// requires, and `Point` is the shape `claims.ply` reads.
const REPLAY: &str = r#"
nondet effect prover {
  write configure[claims](options: Options, front: Front) -> Unit
  read collected[claims]() -> Result<Collection, Refusal>
  read typed[claims]() -> Result<Typed, Refusal>
  read shrink[claims](claim: Int) -> Result<Option<Int>, Refusal>
  read offers[claims](i: Int) -> Result<Option<Offer>, Refusal>
  read would[claims](i: Int, position: Int) -> Result<Bool, Refusal>
  write accept[claims](i: Int, position: Int) -> Result<Unit, Refusal>
  read settled[claims]() -> Result<Option<Settled>, Refusal>
  read outcomes[claims](keys: List<String>) -> List<Option<String>>
  read discharged[claims](choice: Choice) -> Result<Verdicts, Refusal>
  write record[claims](entries: List<{ at: Int, key: String }>) -> List<Unit>
  read replay[claims](index: Int, root: Int, case: Int) -> Result<Point, Refusal>
  read baselines[claims]() -> List<Baseline>
  write accepted[claims](records: List<Baseline>) -> Accepted
}

type Binder = { name: String, text: String, ty: Shape }
type Offer = { here: Int, candidates: List<{ position: Int, size: Int }> }
type Settled = { bindings: List<Binding>, original: List<Binding> }
// The domain vocabulary is a shape this fixture only carries: it never reads one, so it names
// the type itself rather than borrowing the name of the package's.
type Shape = | Var(Int) | Fn | Record(List<{ name: String, ty: Shape }>) | Con(String, List<Shape>)
type Variant = { name: String, fields: List<Shape> }
type Decl = { name: String, variants: List<Variant> }
type Typed = { decls: List<Decl>, claims: List<{ claim: Int, binders: List<Binder> }> }
type Measured = { claim: Int, sizes: List<Int>, name: String }
type Tls = Unit
type Named = Unit
type Db = { url: Option<String>, pool: Option<Int>, acquire_ms: Option<Int>, statement_ms: Option<Int>, idle_txn_ms: Option<Int>, connect_ms: Option<Int>, statement_cache: Option<Int>, schema: Option<String> }
type Config = { set: List<String>, files: List<String>, schema: Option<String> }
type Trace = { sink: String, level: String }
type ProveOpts = { cases: Option<Int>, roots: Option<Int>, budget: Option<Int>, shrink_budget: Option<Int>, steps: Option<Int> }
type SimOpts = { seed: Option<String>, mode: String, seeds: Option<Int>, budget: Option<Int>, steps: Option<Int>, measure_reduction: Bool }
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
type Verdicts = Unit
type Baseline = Unit
type Accepted = Unit
type Binding = { name: String, ty: String, rendered: String }
type Front = {
  dump: Bytes,
  files: List<{ path: String, name: String, text: Bytes }>,
  packages: List<{ root: String, digest: String }>,
  read_ms: Int,
  front_ms: Int,
}
type Gap = Unit
type Point = | Kept(List<Binding>) | Falsified(List<Binding>) | Rejected | Undrawn(Gap)

type Answer = { falsified: Int, kept: Int, rejected: Int, first: String }

/// Cases scanned from one root: enough to find the draw that falsifies.
fn cases() -> Int = 64

fn nothing() -> Answer = { falsified: 0, kept: 0, rejected: 0, first: "" }

fn drawn(bs: List<Binding>) -> String =
  fold(bs, "", |acc: String, b: Binding|
    if acc == "" { b.rendered } else { acc ++ ", " ++ b.rendered })

fn scan(index: Int, case: Int, seen: Answer) -> Answer / {prover.replay[claims]} =
  if case >= cases() { seen } else {
    match prover.replay[claims](index, 0, case) {
      Err(_) -> seen,
      Ok(point) -> match point {
        Kept(_) -> scan(index, case + 1, { ..seen, kept: seen.kept + 1 }),
        Falsified(bs) ->
          if seen.falsified == 0 {
            scan(index, case + 1, {
              ..seen,
              falsified: seen.falsified + 1,
              first: drawn(bs),
            })
          } else { scan(index, case + 1, { ..seen, falsified: seen.falsified + 1 }) },
        Rejected -> scan(index, case + 1, { ..seen, rejected: seen.rejected + 1 }),
        Undrawn(_) -> scan(index, case + 1, seen),
      },
    }
  }

fn main(root: String, index: Int, front: Front) -> Answer / {prover.configure[claims], prover.collected[claims], prover.replay[claims]} = {
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
      seeds: None,
      budget: None,
      steps: None,
      measure_reduction: false,
    },
  }, front);
  match prover.collected[claims]() {
    Err(_) -> { falsified: 0 - 1, kept: 0, rejected: 0, first: "" },
    Ok(_) -> scan(index, 0, nothing()),
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

fn front_of(source: &str) -> Front {
    let named = vec![("proof.obligation".to_string(), source.to_string())];
    let ids = vec![SourceId(0)];
    ply_codegen::c::producer::ensure_default();
    ply_codegen::c::producer::checked_front(&named, &ids).expect("the re-run program checks")
}

/// The fixture's answer, from one entered call.
fn one_run(source: &str, index: i64) -> Result<Value, ply_span::Diagnostic> {
    let project = project(source);
    let front = front_of(REPLAY);
    let texts: HashMap<String, String> = [("proof.obligation".to_string(), REPLAY.to_string())]
        .into_iter()
        .collect();
    let unit = ply_codegen::Unit::over_front(&front, texts).expect("this host has a C toolchain");
    let mut machine = Machine::new(&front);
    machine.set_compiled(unit.attach());
    let mut registry = HostRegistry::new();
    for (op, handler) in ply_machine::claims::lent() {
        registry.register(op, handler);
    }
    let binding = registry.bind(&front.check).expect("the prover ops bind");
    machine.set_host_binding(Arc::new(binding));
    machine.call(
        "proof.obligation.main",
        vec![
            Value::str(project.path().display().to_string()),
            Value::Int(index),
            crate::fixture::handed(project.path()),
        ],
        Span::DUMMY,
    )
}

fn field(answer: &Value, name: &str) -> Value {
    match answer {
        Value::Record(fields) => fields
            .iter()
            .find(|(key, _)| key.as_str() == name)
            .map(|(_, value)| value.clone())
            .unwrap_or_else(|| panic!("the answer carries `{name}`: {answer}")),
        other => panic!("the answer is a record, not {other}"),
    }
}

fn int(answer: &Value, name: &str) -> i64 {
    match field(answer, name) {
        Value::Int(n) => n,
        other => panic!("`{name}` is an int, not {other}"),
    }
}

#[test]
fn a_replayed_case_comes_back_as_the_value_that_falsifies_the_claim() {
    let answer = one_run(A_FALSE_LAW, 0).expect("the run finished");
    assert!(
        int(&answer, "falsified") > 0,
        "no draw falsified a law that does not hold: {answer}"
    );
    let first = field(&answer, "first");
    let drawn = match &first {
        Value::Str(text) => text.to_string(),
        other => panic!("the first falsifying point is a string, not {other}"),
    };
    // Independently of the prover: the value the point drew really does break the law.
    let n: i64 = drawn.parse().unwrap_or_else(|e| {
        panic!("the point drew `{drawn}`, which is not an integer: {e}");
    });
    assert!(
        n + n != n * 3,
        "the replayed point drew `{n}`, at which the law holds"
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
    let front = front_of(REPLAY);
    let mut checked = 0;
    for (home, ty) in ply_machine::claims::MARSHALLED {
        let declared: Vec<&str> = front
            .check
            .ctors
            .values()
            .filter(|c| c.type_name.as_str().rsplit('.').next() == Some(*ty))
            .map(|c| c.module.as_str())
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
