//! `tester.keys`, `tester.hashed` and `tester.searched`: what a selector reads before anything
//! runs. The selection and every key it is made with are the program's to compute, so the host
//! answers inputs only.

use ply_eval::host::HostRegistry;
use ply_eval::{Machine, Provider};
use ply_span::{SourceId, Span};
use ply_ty::Front;
use std::collections::HashMap;

/// A program that configures the tester over the directory it is handed, loads it, and reports
/// what the tree holds: one test and its hash, and the plan.
const OUTER: &str = r#"
nondet effect tester {
  write configure[r](options: Options, front: Front) -> Unit
  read loaded[r]() -> Result<Program, Refusal>
  read bound[r]() -> Result<Unit, Refusal>
  read ran[r]() -> Ran
  read trial[r](failure: Int, keys: List<{ name: String, ns: String }>, filed: Option<String>) -> Trial
  read stamped[r]() -> Option<List<String>>
  read keys[r]() -> List<Key>
  read hashed[r]() -> List<Hashed>
  read searched[r]() -> Plan
}

type Program = Unit
type Refusal = Unit
type Ran = Unit
type Key = {
  index: Int, label: String, name: String, module: String,
  hash: Option<String>, seeded: Bool, nondet: Bool,
}
type Hashed = { name: String, hash: String, test: Bool }
type Trial = { outcome: TrialOutcome, cached: Bool }
type TrialOutcome = | Fails | Passes | Unresolved(Unresolved)
type Unresolved = | DoesNotCheck | DifferentFailure | MissingBody | BudgetSpent
type Plan = { mode: String, seeds: Int, budget: String, steps: String }
type Named = { name: String, path: String }
type TlsCred = { name: String, cert: String, key: String }
type DbOpts = {
  url: Option<String>, pool: Option<Int>, acquire_ms: Option<Int>, statement_ms: Option<Int>,
  idle_txn_ms: Option<Int>, connect_ms: Option<Int>, statement_cache: Option<Int>, schema: Option<String>,
}
type ConfigOpts = { set: List<String>, files: List<String>, schema: Option<String> }
type TraceOpts = { sink: String, level: String }
type SimOpts = {
  seed: Option<String>, mode: String, roots: Option<{ from: Int, to: Int }>, budget: Option<Int>,
  steps: Option<Int>, measure_reduction: Bool,
}
type Front = {
  dump: Bytes,
  files: List<{ path: String, name: String, text: Bytes }>,
  read_ms: Int,
  front_ms: Int,
  file_ms: Int,
  cached: Bool,
}
type Options = {
  path: String, json: Bool, explain: Bool, no_cache: Bool, filter: Option<String>, jobs: Option<Int>,
  steps: Int, timeout: Int, bisect: String, bisect_budget: Int, coverage: Bool, mutate: Option<String>,
  mutate_budget: Int, profile: String, watch: Bool, std: Bool, host: Bool,
  tls: List<TlsCred>, trust: List<String>, fs: List<Named>, exec: List<Named>, allow: List<String>,
  db: DbOpts, config: ConfigOpts, sim: SimOpts,
}

fn options(root: String) -> Options =
  {
    path: root, json: false, explain: false, no_cache: false, filter: None, jobs: None,
    steps: 1000000000, timeout: 60000, bisect: "auto", bisect_budget: 500, coverage: false,
    mutate: None, mutate_budget: 32, profile: "development", watch: false,
    std: false, host: false, tls: [], trust: [], fs: [], exec: [], allow: [],
    db: { url: None, pool: None, acquire_ms: None, statement_ms: None, idle_txn_ms: None,
          connect_ms: None, statement_cache: None, schema: None },
    config: { set: [], files: [], schema: None },
    sim: { seed: None, mode: "exhaustive", roots: None, budget: None, steps: None, measure_reduction: false },
  }

fn main(root: String, front: Front) -> Bool / {
  tester.configure[r], tester.loaded[r], tester.keys[r], tester.hashed[r], tester.searched[r],
} = {
  tester.configure[r](options(root), front);
  match tester.loaded[r]() {
    Err(_) -> false,
    Ok(_) -> {
      let keys = tester.keys[r]();
      let hashed = tester.hashed[r]();
      let plan = tester.searched[r]();
      // The project has one test: it is named program-wide and hashed, and it reads no seed, so
      // its own hash is what its result is keyed on.
      let one = match list_at(keys, 0) {
        Some(k) -> match k.hash {
          Some(_) -> !k.seeded && !k.nondet && k.name == "p.doubles" && k.label == "doubles",
          None -> false,
        },
        None -> false,
      };
      let named = fold(hashed, false, |acc: Bool, h: Hashed| acc || (h.test && h.name == "p.doubles"));
      one && named && plan.mode != "" && plan.seeds >= 1
    },
  }
}
"#;

fn front_of(source: &str) -> Front {
    let named = vec![("m".to_string(), source.to_string())];
    let ids = vec![SourceId(0)];
    ply_codegen::c::producer::ensure_default();
    ply_codegen::c::producer::checked_front(&named, &ids).expect("the program checks")
}

#[test]
fn a_selector_reads_the_keys_the_hashes_and_the_plan_before_anything_runs() {
    // One project with one test, which is what the program's checks are written against.
    let dir = tempfile::tempdir().expect("a temp dir");
    std::fs::write(
        dir.path().join("p.ply"),
        "fn double(x: Int) -> Int = x * 2\n\ntest \"doubles\" { assert_eq(double(2), 4) }\n",
    )
    .unwrap();

    let front = front_of(OUTER);
    let texts: HashMap<String, String> =
        [("m".to_string(), OUTER.to_string())].into_iter().collect();
    let unit = ply_codegen::Unit::over_front(&front, texts).expect("this host has a C toolchain");
    let mut machine = Machine::new(&front);
    machine.set_compiled(Provider::attach(unit));

    let mut registry = HostRegistry::new();
    // Only the operations this program declares: the reads it makes, and the two that start it.
    for (op, handler) in
        ply_machine::policy::lent("tester", &|e: &str| e.to_string()).expect("the family")
    {
        if ["configure", "loaded", "keys", "hashed", "searched"].contains(&op.op.as_str()) {
            registry.register(op, handler);
        }
    }
    let binding = registry.bind(&front.check).expect("the tester ops bind");
    machine.set_host_binding(std::sync::Arc::new(binding));

    let answer = machine
        .call(
            "m.main",
            vec![
                ply_eval::Value::str(dir.path().display().to_string()),
                crate::fixture::handed(dir.path()),
            ],
            Span::DUMMY,
        )
        .expect("the outer main ran");
    assert_eq!(answer.to_string(), "true", "the answers read: {answer}");
}
