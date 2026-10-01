//! `tester.keys` and `tester.hashed`: what a selector reads before anything runs. The selection,
//! the search a seeded test is keyed on and every key it is made with are the program's to compute,
//! so the host answers inputs only.

use ply_eval::host::HostRegistry;
use ply_eval::{Front, Machine, Provider, SourceId, Span};
use std::collections::HashMap;

/// The tester as a program declares it, and the options it configures a run with.
const DECLARED: &str = r#"
nondet effect tester {
  write configure[r](options: Options) -> Unit
  read loaded[r](front: Front) -> Result<Program, Refusal>
  read bound[r]() -> Result<Unit, Refusal>
  read trial[r](failure: Int, keys: List<{ name: String, ns: String }>, filed: Option<String>) -> Trial
  read stamped[r]() -> Option<List<String>>
  read keys[r]() -> List<Key>
  read hashed[r]() -> List<Hashed>
}

type Program = { filtered_out: Int }
type Refusal = Unit
type Key = {
  index: Int, label: String, name: String, module: String,
  hash: Option<String>, seeded: Bool, nondet: Bool,
}
type Hashed = { name: String, hash: String, test: Bool }
type Trial = { outcome: TrialOutcome, cached: Bool }
type TrialOutcome = | Fails | Passes | Unresolved(Unresolved)
type Unresolved = | DoesNotCheck | DifferentFailure | MissingBody | BudgetSpent
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
  path: String, json: Bool, explain: Bool, no_cache: Bool, filters: List<String>, jobs: Option<Int>,
  steps: Int, timeout: Int, bisect: String, bisect_budget: Int, coverage: Bool, mutate: Option<String>,
  mutate_budget: Int, profile: String, watch: Bool, std: Bool, host: Bool,
  tls: List<TlsCred>, trust: List<String>, fs: List<Named>, exec: List<Named>, allow: List<String>,
  db: DbOpts, config: ConfigOpts, sim: SimOpts,
}

fn options(root: String) -> Options =
  {
    path: root, json: false, explain: false, no_cache: false, filters: [], jobs: None,
    steps: 1000000000, timeout: 60000, bisect: "auto", bisect_budget: 500, coverage: false,
    mutate: None, mutate_budget: 32, profile: "development", watch: false,
    std: false, host: false, tls: [], trust: [], fs: [], exec: [], allow: [],
    db: { url: None, pool: None, acquire_ms: None, statement_ms: None, idle_txn_ms: None,
          connect_ms: None, statement_cache: None, schema: None },
    config: { set: [], files: [], schema: None },
    sim: { seed: None, mode: "exhaustive", roots: None, budget: None, steps: None, measure_reduction: false },
  }
"#;

/// Configures the tester over the directory it is handed, loads it, and reports what the tree
/// holds: one test and its hash.
const KEYS_AND_HASHES: &str = r#"
fn main(root: String, front: Front) -> Bool / {
  tester.configure[r], tester.loaded[r], tester.keys[r], tester.hashed[r],
} = {
  tester.configure[r](options(root));
  match tester.loaded[r](front) {
    Err(_) -> false,
    Ok(_) -> {
      let keys = tester.keys[r]();
      let hashed = tester.hashed[r]();
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
      one && named
    },
  }
}
"#;

/// Two runs, each configured over a tree and filters of its own: the first test each one's load
/// holds, and how many tests its filter hid.
const TWO_RUNS: &str = r#"
fn first_test(root: String, front: Front, filters: List<String>) -> String / {
  tester.configure[r], tester.loaded[r], tester.keys[r],
} = {
  tester.configure[r]({ ..options(root), filters: filters });
  match tester.loaded[r](front) {
    Err(_) -> "refused",
    Ok(p) -> match list_at(tester.keys[r](), 0) {
      Some(k) -> k.name ++ " " ++ int_to_string(p.filtered_out),
      None -> "none",
    },
  }
}

fn main(root: String, front: Front, other: String, other_front: Front) -> List<String> / {
  tester.configure[r], tester.loaded[r], tester.keys[r],
} = [first_test(root, front, []), first_test(other, other_front, ["nothing"])]
"#;

fn front_of(source: &str) -> Front {
    let named = vec![("m".to_string(), source.to_string())];
    let ids = vec![SourceId(0)];
    ply_codegen::c::producer::ensure_default();
    ply_codegen::c::producer::checked_front(&named, &ids).expect("the program checks")
}

/// A machine over `DECLARED` and `main`, bound to the tester operations it performs.
fn driving(main: &str) -> Machine<'static> {
    let source = format!("{DECLARED}{main}");
    let front: &'static Front = Box::leak(Box::new(front_of(&source)));
    let texts: HashMap<String, String> = [("m".to_string(), source)].into_iter().collect();
    let unit = ply_codegen::Unit::over_front(front, texts).expect("this host has a C toolchain");
    let mut machine = Machine::new(front, Provider::attach(unit))
        .expect("the unit was compiled from this program");

    let mut registry = HostRegistry::new();
    // Only the operations this program declares: the reads it makes, and the two that start it.
    for (op, handler) in
        ply_machine::policy::lent("tester", &|e: &str| e.to_string()).expect("the family")
    {
        if ["configure", "loaded", "keys", "hashed"].contains(&op.op.as_str()) {
            registry.register(op, handler);
        }
    }
    let binding = registry.bind(&front.check).expect("the tester ops bind");
    machine.set_host_binding(std::sync::Arc::new(binding));
    machine
}

/// A project of one module, `name.ply`, holding `text`.
fn tree(name: &str, text: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a temp dir");
    std::fs::write(dir.path().join(format!("{name}.ply")), text).unwrap();
    dir
}

fn root_and_front(dir: &tempfile::TempDir) -> [ply_eval::Value; 2] {
    [
        ply_eval::Value::str(dir.path().display().to_string()),
        crate::fixture::handed(dir.path()),
    ]
}

#[test]
fn a_selector_reads_the_keys_and_the_hashes_before_anything_runs() {
    // One project with one test, which is what the program's checks are written against.
    let dir = tree(
        "p",
        "fn double(x: Int) -> Int = x * 2\n\ntest \"doubles\" { assert_eq(double(2), 4) }\n",
    );
    let mut machine = driving(KEYS_AND_HASHES);
    let answer = machine
        .call("m.main", root_and_front(&dir).to_vec(), Span::DUMMY)
        .into_parts()
        .0
        .expect("the outer main ran");
    assert_eq!(
        answer,
        ply_eval::Value::Bool(true),
        "the answers read: {answer:?}"
    );
}

/// A run is its configuration's: a second one loads the tree it is handed under its own options,
/// not under the first's.
#[test]
fn a_configuration_begins_a_run_over_the_tree_it_names() {
    let first = tree(
        "p",
        "fn double(x: Int) -> Int = x * 2\n\ntest \"doubles\" { assert_eq(double(2), 4) }\n",
    );
    let second = tree(
        "q",
        "fn triple(x: Int) -> Int = x * 3\n\ntest \"triples\" { assert_eq(triple(2), 6) }\n",
    );
    let mut machine = driving(TWO_RUNS);
    let mut args = root_and_front(&first).to_vec();
    args.extend(root_and_front(&second));
    let answer = machine
        .call("m.main", args, Span::DUMMY)
        .into_parts()
        .0
        .expect("the outer main ran");
    assert_eq!(
        answer,
        ply_eval::Value::list(vec![
            ply_eval::Value::str("p.doubles 0"),
            ply_eval::Value::str("q.triples 1"),
        ]),
        "each run's load answered: {answer:?}"
    );
}
