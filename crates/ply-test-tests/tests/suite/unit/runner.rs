//! The runtime carrying out a choice it was handed: running exactly the tests named, in the classes
//! given, filing each pass under the keys given and nothing else, and saying what it saw. Which
//! tests a run owes, how they are coloured and where a pass belongs are `suite`'s to decide, and its
//! own tests pin them; these state the choice and check what the runtime did with it.

use crate::fixture::{handed, plan_key, root_key};
use ply_eval::{Exploration, Naive, Plan, Race, RaceSite, Seed};
use ply_span::{Diagnostic, SourceId, Symbol};
use ply_store::{Outcome, Store};
use ply_test::{Executor, Hosting, InterpExecutor, Reason, Search, Selection, Status, run_with};
use ply_ty::{CheckOutput, DefHash, EffectAtom, Footprint, HashOutput, Mode, Resource};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new() -> TempRoot {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ply-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp root");
        TempRoot(dir)
    }

    fn store(&self) -> Store {
        Store::open(&self.0).expect("open store")
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Program {
    port: ply_ty::Front,
    check: CheckOutput,
    hashes: HashOutput,
    src: String,
}

impl Program {
    fn compile(src: &str) -> Program {
        let port = crate::fixture::port_front(&[(String::new(), src.to_string())], &[SourceId(0)]);
        Program {
            check: port.check.clone(),
            hashes: port.hashes.clone(),
            port,
            src: src.to_string(),
        }
    }

    fn texts(&self) -> std::collections::HashMap<String, String> {
        std::collections::HashMap::from([(String::new(), self.src.clone())])
    }

    fn index_of(&self, name: &str) -> usize {
        self.check
            .tests
            .iter()
            .position(|t| t.name == name)
            .unwrap_or_else(|| panic!("no test named {name:?}"))
    }

    /// Every test, as a cold cache has it.
    fn every(&self) -> Selection {
        crate::fixture::every(&self.check, &self.hashes, &Plan::default())
    }

    /// `runs` alone and every other test cached: what a program hands over once the store holds a
    /// pass for the rest.
    fn choose(&self, runs: &[usize]) -> Selection {
        crate::fixture::choose(&self.check, &self.hashes, runs, &Plan::default())
    }

    /// Runs on the compiled C tier; `Unit::over_front` leaks a `&'static Unit`.
    fn run(&self, selection: &Selection, store: &mut Store) -> ply_test::RunReport {
        let unit = ply_codegen::Unit::over_front(&self.port, self.texts())
            .expect("this host has a C compiler");
        let executor = InterpExecutor::new(&self.port)
            .with_backend(unit)
            .with_hosts(Hosting::hermetic())
            .with_search(Search::of(selection));
        run_with(selection, &self.check, &self.hashes, store, &executor)
    }

    fn def_hash(&self, name: &str) -> DefHash {
        self.hashes
            .defs
            .get(&Symbol::new(name))
            .copied()
            .expect("a definition by that name")
    }

    /// Whether the store holds a pass under this test's own hash: the fact a program's choice of
    /// what to run is made from.
    fn filed(&self, store: &Store, name: &str) -> bool {
        passed(store, self.hashes.tests[self.index_of(name)])
    }
}

fn passed(store: &Store, key: DefHash) -> bool {
    matches!(store.get(key), Some(Outcome::Pass))
}

fn suspects(failure: &ply_test::Failure) -> Vec<&str> {
    failure.suspects.iter().map(|s| s.as_str()).collect()
}

const ARITHMETIC: &str = r#"
fn add(a: Int, b: Int) -> Int = a + b
fn mul(a: Int, b: Int) -> Int = a * b
fn twice(x: Int) -> Int = mul(x, 2)

test "add is right" {
  assert_eq(add(1, 2), 3)
}

test "mul is right" {
  assert_eq(mul(2, 3), 6)
}

test "twice is right" {
  assert_eq(twice(5), 10)
}
"#;

const ARITHMETIC_TESTS: [&str; 3] = ["add is right", "mul is right", "twice is right"];

#[test]
fn a_pass_is_filed_under_its_key_and_a_choice_of_nothing_runs_nothing() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ARITHMETIC);

    let report = program.run(&program.every(), &mut store);
    assert_eq!(
        (report.passed, report.failed),
        (3, 0),
        "{:#?}",
        report.failures
    );
    for name in ARITHMETIC_TESTS {
        assert!(
            program.filed(&store, name),
            "`{name}` passed and was not filed"
        );
    }

    let warm = program.choose(&[]);
    assert!(warm.groups.is_empty());
    assert!(warm.reasons.iter().all(|r| *r == Reason::Cached));
    let report = program.run(&warm, &mut store);
    assert_eq!((report.passed, report.failed, report.cached), (0, 0, 3));
    assert!(report.results.is_empty());
}

#[test]
fn a_filed_pass_survives_reopening_the_store() {
    let root = TempRoot::new();
    let program = Program::compile(ARITHMETIC);
    {
        let mut store = root.store();
        program.run(&program.every(), &mut store);
    }
    let store = root.store();
    for name in ARITHMETIC_TESTS {
        assert!(
            program.filed(&store, name),
            "`{name}` was lost with the store"
        );
    }
}

#[test]
fn an_edit_moves_the_keys_of_exactly_the_tests_that_reach_it() {
    let root = TempRoot::new();
    let mut store = root.store();

    let before = Program::compile(ARITHMETIC);
    let report = before.run(&before.every(), &mut store);
    assert_eq!(report.failed, 0, "{:#?}", report.failures);

    // `mul` changes and `twice` calls it, so both hashes move.
    let after = Program::compile(&ARITHMETIC.replace(
        "fn mul(a: Int, b: Int) -> Int = a * b",
        "fn mul(a: Int, b: Int) -> Int = a * b * 1",
    ));
    assert!(after.filed(&store, "add is right"));
    assert!(!after.filed(&store, "mul is right"));
    assert!(!after.filed(&store, "twice is right"));
}

#[test]
fn renaming_a_definition_moves_no_key() {
    let root = TempRoot::new();
    let mut store = root.store();

    let before = Program::compile(ARITHMETIC);
    assert_eq!(before.run(&before.every(), &mut store).failed, 0);

    let after = Program::compile(&ARITHMETIC.replace("mul(", "product("));
    assert!(after.hashes.defs.contains_key(&Symbol::new("product")));
    for name in ARITHMETIC_TESTS {
        assert!(
            after.filed(&store, name),
            "a rename changes no behaviour, so `{name}`'s pass still answers for it"
        );
    }
}

const ONE_RED: &str = r#"
fn good() -> Int = 1
fn bad() -> Int = 2

test "good is one" {
  assert_eq(good(), 1)
}

test "bad is one" {
  assert_eq(bad(), 1)
}
"#;

#[test]
fn a_failure_never_reaches_the_store_and_a_pass_does() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ONE_RED);
    let bad = program.index_of("bad is one");

    for round in 0..3 {
        // Once `good` is filed, a program runs the red test alone.
        let selection = if round == 0 {
            program.every()
        } else {
            program.choose(&[bad])
        };
        let report = program.run(&selection, &mut store);
        assert_eq!(report.failed, 1, "round {round}");
        assert_eq!(report.failures[0].name, "bad is one");
        assert!(
            store.get(program.hashes.tests[bad]).is_none(),
            "round {round}: a failure must never reach the store"
        );
        assert!(
            program.filed(&store, "good is one"),
            "round {round}: a pass must reach the store"
        );
    }

    let fixed =
        Program::compile(&ONE_RED.replace("fn bad() -> Int = 2", "fn bad() -> Int = 3 - 2"));
    assert!(!fixed.filed(&store, "bad is one"));
    let report = fixed.run(&fixed.choose(&[fixed.index_of("bad is one")]), &mut store);
    assert_eq!((report.passed, report.failed), (1, 0));
    assert!(
        fixed.filed(&store, "bad is one"),
        "going green is what files the pass"
    );
}

#[test]
fn a_fix_that_reproduces_a_green_definition_is_already_on_file() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ONE_RED);
    assert_eq!(program.run(&program.every(), &mut store).failed, 1);

    // `bad` repaired into a copy of `good`, so it and its test hash identically.
    let fixed = Program::compile(&ONE_RED.replace("fn bad() -> Int = 2", "fn bad() -> Int = 1"));
    assert_eq!(fixed.def_hash("bad"), fixed.def_hash("good"));
    assert!(fixed.filed(&store, "bad is one"));
}

#[test]
fn a_red_test_does_not_vouch_for_the_definitions_it_exercised() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ONE_RED);
    program.run(&program.every(), &mut store);

    assert!(!store.knows_definition(program.def_hash("bad")));
    assert!(store.knows_definition(program.def_hash("good")));
}

#[test]
fn definitions_are_recorded_apart_from_test_results() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ARITHMETIC);
    assert_eq!(program.run(&program.every(), &mut store).failed, 0);

    for name in ["add", "mul", "twice"] {
        let hash = program.def_hash(name);
        assert!(store.knows_definition(hash), "`{name}` was never recorded");
        assert!(
            store.get(hash).is_none(),
            "`{name}` is a definition, not a test outcome"
        );
    }
    assert_eq!(store.len(), 3, "one result per test and nothing else");
}

const LEDGER: &str = r#"
fn debit(balance: Int, amount: Int) -> Int = balance - amount
fn credit(balance: Int, amount: Int) -> Int = balance + amount
fn settle(balance: Int) -> Int = debit(credit(balance, 10), 4)

test "credit adds" {
  assert_eq(credit(1, 2), 3)
}

test "settle nets out" {
  assert_eq(settle(0), 6)
}
"#;

#[test]
fn a_failure_names_only_the_changed_definitions_in_its_closure() {
    let root = TempRoot::new();
    let mut store = root.store();

    let green = Program::compile(LEDGER);
    let report = green.run(&green.every(), &mut store);
    assert_eq!(report.failed, 0, "{:#?}", report.failures);

    let red = Program::compile(&LEDGER.replace(
        "fn debit(balance: Int, amount: Int) -> Int = balance - amount",
        "fn debit(balance: Int, amount: Int) -> Int = balance - amount - 1",
    ));
    assert!(red.filed(&store, "credit adds"));
    let report = red.run(&red.choose(&[red.index_of("settle nets out")]), &mut store);
    assert_eq!(report.failed, 1);
    assert_eq!(
        suspects(&report.failures[0]),
        vec!["debit", "settle"],
        "only the edited definition and the one carrying it are suspect"
    );
}

#[test]
fn suspects_are_computed_against_the_cache_as_it_was_before_the_run() {
    let root = TempRoot::new();
    let mut store = root.store();

    let green = Program::compile(LEDGER);
    assert_eq!(green.run(&green.every(), &mut store).failed, 0);

    // `credit` is rewritten so that its own test stays green while its hash moves.
    let red = Program::compile(
        &LEDGER
            .replace(
                "fn credit(balance: Int, amount: Int) -> Int = balance + amount",
                "fn credit(balance: Int, amount: Int) -> Int = amount + balance",
            )
            .replace("assert_eq(settle(0), 6)", "assert_eq(settle(0), 99)"),
    );
    // Both in one class, so the green sibling can finish first.
    let report = red.run(&red.every(), &mut store);
    assert_eq!(report.failed, 1);
    let named = suspects(&report.failures[0]);
    assert!(
        named.contains(&"credit"),
        "a sibling test passing first must not clear a suspect: {named:?}"
    );
}

/// `base is right` covers `base`; `total is right` covers `base` and `total`.
fn shared(base: &str, total: &str) -> String {
    format!(
        "fn base(x: Int) -> Int = {base}\n\
         fn total(x: Int) -> Int = {total}\n\
         \n\
         test \"base is right\" {{ assert_eq(base(1), 2) }}\n\
         test \"total is right\" {{ assert_eq(total(1), 12) }}\n"
    )
}

#[test]
fn a_green_sibling_never_clears_a_suspect_on_a_later_run() {
    let root = TempRoot::new();
    let mut store = root.store();

    let green = Program::compile(&shared("x + 1", "base(x) + 10"));
    assert_eq!(green.run(&green.every(), &mut store).failed, 0);

    // `base` stays value-identical, so its hash moves and its test stays green; `total` breaks.
    let red = Program::compile(&shared("1 + x", "base(x) + 11"));
    let doomed = red.index_of("total is right");

    for round in 0..3 {
        let selection = if round == 0 {
            red.every()
        } else {
            assert!(red.filed(&store, "base is right"), "round {round}");
            red.choose(&[doomed])
        };
        let report = red.run(&selection, &mut store);
        assert_eq!(report.failed, 1, "round {round}: {:#?}", report.results);
        assert_eq!(
            suspects(&report.failures[0]),
            vec!["base", "total"],
            "round {round}: a passing sibling must not vouch for a red test's definitions"
        );
        assert!(
            !store.knows_definition(red.def_hash("base")),
            "round {round}: `base` is still under suspicion, so it is still unrecorded"
        );
    }
}

#[test]
fn a_run_that_skipped_a_test_does_not_vouch_for_what_it_would_have_covered() {
    let root = TempRoot::new();
    let mut store = root.store();

    let green = Program::compile(&shared("x + 1", "base(x) + 10"));
    assert_eq!(green.run(&green.every(), &mut store).failed, 0);

    let red = Program::compile(&shared("1 + x", "base(x) + 11"));
    let base = red.index_of("base is right");
    let doomed = red.index_of("total is right");

    // What `--filter base` narrows to: `total is right` keeps its reason but never reaches a group.
    let mut filtered = red.every();
    filtered.to_run.retain(|&i| i == base);
    filtered.groups = vec![vec![base]];
    let report = red.run(&filtered, &mut store);
    assert_eq!((report.passed, report.failed), (1, 0));
    assert!(
        !report.results.iter().any(|r| r.index == doomed),
        "the narrowed run must not have executed `total is right`"
    );

    let report = red.run(&red.choose(&[doomed]), &mut store);
    assert_eq!(report.failed, 1);
    assert_eq!(
        suspects(&report.failures[0]),
        vec!["base", "total"],
        "a test that never ran cannot have cleared its own closure"
    );
}

#[test]
fn going_green_ends_the_suspicion_a_failure_kept_alive() {
    let root = TempRoot::new();
    let mut store = root.store();

    let green = Program::compile(&shared("x + 1", "base(x) + 10"));
    assert_eq!(green.run(&green.every(), &mut store).failed, 0);

    let red = Program::compile(&shared("1 + x", "base(x) + 11"));
    assert_eq!(red.run(&red.every(), &mut store).failed, 1);

    let fixed = Program::compile(&shared("1 + x", "base(x) + 10"));
    assert!(fixed.filed(&store, "base is right"));
    let report = fixed.run(
        &fixed.choose(&[fixed.index_of("total is right")]),
        &mut store,
    );
    assert_eq!(report.failed, 0, "{:#?}", report.failures);
    assert!(store.knows_definition(fixed.def_hash("base")));
    assert!(store.knows_definition(fixed.def_hash("total")));

    // Only `total` moves now, so it is the only thing the next failure may name.
    let broken = Program::compile(&shared("1 + x", "base(x) + 12"));
    assert!(broken.filed(&store, "base is right"));
    let report = broken.run(
        &broken.choose(&[broken.index_of("total is right")]),
        &mut store,
    );
    assert_eq!(report.failed, 1);
    assert_eq!(suspects(&report.failures[0]), vec!["total"]);
}

#[test]
fn a_failure_whose_test_alone_moved_names_no_suspect() {
    let root = TempRoot::new();
    let mut store = root.store();

    let green = Program::compile(LEDGER);
    assert_eq!(green.run(&green.every(), &mut store).failed, 0);

    let red =
        Program::compile(&LEDGER.replace("assert_eq(settle(0), 6)", "assert_eq(settle(0), 7)"));
    let report = red.run(&red.choose(&[red.index_of("settle nets out")]), &mut store);
    assert_eq!((report.failed, report.passed, report.cached), (1, 0, 1));
    assert!(!report.is_success());

    let failure = &report.failures[0];
    assert_eq!(failure.name, "settle nets out");
    assert_eq!(failure.diagnostic.code, ply_span::codes::ASSERTION_FAILED);
    assert!(
        failure.suspects.is_empty(),
        "only the expectation inside the test moved, so the test is the change: {:?}",
        failure.suspects
    );
    assert_eq!(report.results[0].status, Status::Failed);
    assert!(report.results[0].hash.is_some());
}

#[test]
fn the_report_accounts_for_every_selected_test_exactly_once() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ONE_RED);

    let selection = program.every();
    let report = program.run(&selection, &mut store);
    assert_eq!(report.passed + report.failed, selection.to_run.len());
    let mut ran: Vec<usize> = report.results.iter().map(|r| r.index).collect();
    ran.sort_unstable();
    assert_eq!(ran, selection.to_run);
    assert!(report.warnings.is_empty(), "{:#?}", report.warnings);
    assert!(!report.is_success());
}

/// A selection whose every field is spelled out, for a run over a choice no program would make.
fn literal(to_run: Vec<usize>, groups: Vec<Vec<usize>>, reason: Reason) -> Selection {
    Selection {
        total: 3,
        cached: Vec::new(),
        to_run,
        groups,
        reasons: vec![reason; 3],
        plan: Plan::default(),
        narrowed: BTreeMap::new(),
        filed: BTreeMap::new(),
        out_of_scope: BTreeSet::new(),
    }
}

#[test]
fn an_empty_selection_runs_nothing() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ARITHMETIC);
    let report = program.run(&literal(Vec::new(), Vec::new(), Reason::Cached), &mut store);
    assert_eq!((report.passed, report.failed, report.cached), (0, 0, 0));
    assert!(report.warnings.is_empty());
}

#[test]
fn a_selected_test_left_out_of_every_group_is_still_run() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ARITHMETIC);
    let report = program.run(
        &literal(vec![0, 1, 2], vec![vec![0]], Reason::New),
        &mut store,
    );
    assert_eq!(report.passed, 3, "no selected test may be silently skipped");
    assert_eq!(report.warnings.len(), 1);
}

#[test]
fn a_selection_naming_a_test_that_does_not_exist_warns_instead_of_panicking() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ARITHMETIC);
    let report = program.run(
        &literal(vec![0, 99], vec![vec![0, 99]], Reason::New),
        &mut store,
    );
    assert_eq!((report.passed, report.failed), (1, 0));
    assert_eq!(report.warnings.len(), 1);
    assert!(report.warnings[0].message.contains("99"));
}

#[test]
fn every_group_is_run_in_sequence() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ARITHMETIC);

    let mut selection = program.every();
    selection.groups = vec![vec![0], vec![1, 2]];
    let report = program.run(&selection, &mut store);
    assert_eq!(report.passed, 3);
    assert_eq!(report.results.iter().filter(|r| r.group == 0).count(), 1);
    assert_eq!(report.results.iter().filter(|r| r.group == 1).count(), 2);
}

const DISJOINT_CELLS: &str = r#"
test "users cell" {
  with_cell[users](1) { c ->
    assert_eq(cell_get(c), 1)
  }
}

test "orders cell" {
  with_cell[orders](2) { c ->
    assert_eq(cell_get(c), 2)
  }
}

test "pure one" {
  assert_eq(1, 1)
}

test "pure two" {
  assert_eq(2, 2)
}
"#;

#[test]
fn tests_whose_regions_discharge_their_cells_run_as_one_class() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(DISJOINT_CELLS);

    // `with_cell` discharges its atoms at the region boundary, so nothing is left to contend over.
    assert!(program.check.tests.iter().all(|t| t.footprint.is_empty()));

    let report = program.run(&program.every(), &mut store);
    assert_eq!(
        (report.passed, report.failed),
        (4, 0),
        "{:#?}",
        report.failures
    );
    assert!(report.results.iter().all(|r| r.group == 0));
}

const PERFORMING: &str = r#"
effect disk {
  read peek[r](key: Int) -> Int
}

test "peeks three times" {
  let n = handle {
    disk.peek[log](1) + disk.peek[log](2) + disk.peek[log](3)
  } with {
    disk.peek[log](k) -> k,
  };
  assert_eq(n, 6)
}

test "peeks at nothing" { assert_eq(1, 1) }
"#;

#[test]
fn a_result_counts_the_operations_its_own_test_performed() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(PERFORMING);
    let report = program.run(&program.every(), &mut store);
    assert_eq!(report.failed, 0, "{:#?}", report.failures);
    let performs = |name: &str| {
        report
            .results
            .iter()
            .find(|r| r.name == name)
            .expect("reported")
            .performs
    };
    assert_eq!(
        performs("peeks three times"),
        3,
        "a handled operation is performed all the same"
    );
    assert_eq!(
        performs("peeks at nothing"),
        0,
        "a worker's count starts over with each test"
    );
}

struct PanickingExecutor {
    panic_on: usize,
}

impl Executor for PanickingExecutor {
    type Worker = ();

    fn worker(&self) {}

    fn execute(&self, _worker: &mut (), index: usize) -> Result<(), Diagnostic> {
        if index == self.panic_on {
            panic!("deliberate panic in test {index}");
        }
        Ok(())
    }
}

#[test]
fn a_panicking_test_is_contained_and_reported_as_a_failure() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ARITHMETIC);
    let selection = program.every();
    let doomed = program.index_of("mul is right");

    let executor = PanickingExecutor { panic_on: doomed };
    let report = run_with(
        &selection,
        &program.check,
        &program.hashes,
        &mut store,
        &executor,
    );

    assert_eq!(report.passed, 2, "the other tests must still have run");
    assert_eq!(report.failed, 1);

    let panicked = report
        .results
        .iter()
        .find(|r| r.index == doomed)
        .expect("reported");
    assert_eq!(panicked.status, Status::Panicked);
    let diagnostic = panicked
        .failure
        .as_ref()
        .expect("a panic carries a diagnostic");
    assert_eq!(diagnostic.code, ply_span::codes::INTERNAL_ERROR);
    assert!(
        diagnostic.message.contains("deliberate panic"),
        "{}",
        diagnostic.message
    );
    assert!(diagnostic.message.contains("mul is right"));
    assert!(
        diagnostic.primary_span().is_some_and(|s| !s.is_dummy()),
        "a panic must still point at the test's source"
    );
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].name, "mul is right");

    assert!(
        store.get(program.hashes.tests[doomed]).is_none(),
        "a panicking test must not be cached"
    );
    for other in selection.to_run.iter().filter(|&&i| i != doomed) {
        assert!(store.get(program.hashes.tests[*other]).is_some());
    }
}

#[test]
fn a_panic_does_not_stop_the_groups_that_follow() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ARITHMETIC);

    // One group per test forces the sequential path, so the unwound worker runs the next test.
    let selection = literal(vec![0, 1, 2], vec![vec![0], vec![1], vec![2]], Reason::New);
    let executor = PanickingExecutor { panic_on: 0 };
    let report = run_with(
        &selection,
        &program.check,
        &program.hashes,
        &mut store,
        &executor,
    );

    assert_eq!((report.passed, report.failed), (2, 1));
    assert!(
        report
            .results
            .iter()
            .skip(1)
            .all(|r| r.status == Status::Passed)
    );
    assert_eq!(
        report.results.iter().map(|r| r.group).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
}

/// An executor whose test reports that Ply broke one of its own invariants, without unwinding.
struct InternalErrorExecutor {
    fail_on: usize,
}

impl Executor for InternalErrorExecutor {
    type Worker = ();

    fn worker(&self) {}

    fn execute(&self, _worker: &mut (), index: usize) -> Result<(), Diagnostic> {
        if index == self.fail_on {
            return Err(Diagnostic::error(
                ply_span::codes::INTERNAL_ERROR,
                "internal error: a frame that is not a builtin step reached `advance`",
            ));
        }
        Ok(())
    }
}

#[test]
fn an_internal_error_is_a_defect_in_ply_rather_than_a_red_test() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ARITHMETIC);
    let doomed = program.index_of("mul is right");

    let executor = InternalErrorExecutor { fail_on: doomed };
    let report = run_with(
        &program.every(),
        &program.check,
        &program.hashes,
        &mut store,
        &executor,
    );

    let result = report
        .results
        .iter()
        .find(|r| r.index == doomed)
        .expect("reported");
    assert_eq!(result.status, Status::Panicked);
    assert!(report.failures[0].defect);
}

/// Reports a search without running one.
struct SimExecutor {
    explorations: BTreeMap<usize, Exploration>,
    failing: BTreeSet<usize>,
    /// What the last `execute` was asked to search, per test.
    plans: std::sync::Mutex<BTreeMap<usize, Plan>>,
    search: Search,
}

impl SimExecutor {
    fn new(selection: &Selection) -> SimExecutor {
        SimExecutor {
            explorations: BTreeMap::new(),
            failing: BTreeSet::new(),
            plans: std::sync::Mutex::new(BTreeMap::new()),
            search: Search::of(selection),
        }
    }

    fn exploring(mut self, index: usize, exploration: Exploration) -> SimExecutor {
        self.explorations.insert(index, exploration);
        self
    }

    fn failing(mut self, index: usize) -> SimExecutor {
        self.failing.insert(index);
        self
    }

    fn searched(&self, index: usize) -> Option<Plan> {
        self.plans
            .lock()
            .expect("not poisoned")
            .get(&index)
            .cloned()
    }
}

impl Executor for SimExecutor {
    type Worker = Option<Exploration>;

    fn worker(&self) -> Option<Exploration> {
        None
    }

    fn execute(&self, worker: &mut Option<Exploration>, index: usize) -> Result<(), Diagnostic> {
        self.plans
            .lock()
            .expect("not poisoned")
            .insert(index, self.search.plan_for(index).clone());
        *worker = self.explorations.get(&index).cloned();
        if self.failing.contains(&index) {
            return Err(Diagnostic::error(
                ply_span::codes::ASSERTION_FAILED,
                "balance went negative",
            ));
        }
        Ok(())
    }

    fn exploration(&self, worker: &Option<Exploration>) -> Option<Exploration> {
        worker.clone()
    }
}

fn exhaustive(explored: u32) -> Exploration {
    Exploration {
        explored,
        exhaustive: true,
        steps: u64::from(explored) * 4,
        ..Exploration::default()
    }
}

fn spent(explored: u32) -> Exploration {
    Exploration {
        explored,
        exhausted: true,
        ..Exploration::default()
    }
}

fn failed_at(seed: Seed, explored: u32) -> Exploration {
    Exploration {
        explored,
        failure: Some(seed),
        ..Exploration::default()
    }
}

/// Injects the seed atom; none of the rules below depend on the source that produced it.
fn make_seeded(program: &mut Program, name: &str) -> usize {
    let index = program.index_of(name);
    let seed = EffectAtom::new("sim", Resource::Singleton, Mode::Read);
    program.check.tests[index].footprint = program.check.tests[index]
        .footprint
        .union(&Footprint::from_atoms([seed]));
    index
}

fn seeded_program() -> (Program, usize) {
    let mut program = Program::compile(ARITHMETIC);
    let index = make_seeded(&mut program, "mul is right");
    (program, index)
}

/// Every test runs under `plan`, a seeded one's pass filed under the plan's key and, when the plan
/// is answered root by root, each root's too; the rest under their own hashes.
fn seeded_choice(program: &Program, seeded: &[usize], plan: &Plan, per_root: bool) -> Selection {
    let plan = plan.clone().normalized();
    let runs: Vec<usize> = (0..program.check.tests.len()).collect();
    let filed = runs
        .iter()
        .map(|&i| {
            let hash = program.hashes.tests[i];
            let keys = if !seeded.contains(&i) {
                vec![hash]
            } else if per_root {
                plan.roots
                    .iter()
                    .map(|&r| root_key(hash, r))
                    .chain([plan_key(hash, &plan)])
                    .collect()
            } else {
                vec![plan_key(hash, &plan)]
            };
            (i, keys)
        })
        .collect();
    handed(&program.check, &plan, &runs, BTreeMap::new(), filed)
}

#[test]
fn a_narrowed_choice_searches_only_the_roots_it_owes_and_files_the_widened_plan() {
    let root = TempRoot::new();
    let mut store = root.store();
    let (program, seeded) = seeded_program();
    let hash = program.hashes.tests[seeded];

    let four = Plan::random(4);
    let selection = seeded_choice(&program, &[seeded], &four, true);
    let executor = SimExecutor::new(&selection).exploring(seeded, exhaustive(4));
    run_with(
        &selection,
        &program.check,
        &program.hashes,
        &mut store,
        &executor,
    );
    assert_eq!(
        executor.searched(seeded).map(|p| p.roots),
        Some(vec![0, 1, 2, 3])
    );
    for r in 0..4 {
        assert!(passed(&store, root_key(hash, r)), "root {r}'s own key");
    }

    // The first four roots each hold a pass of their own, so widening to eight owes the rest.
    let eight = Plan::random(8);
    let owed: Vec<u64> = vec![4, 5, 6, 7];
    let keys: Vec<DefHash> = owed
        .iter()
        .map(|&r| root_key(hash, r))
        .chain([plan_key(hash, &eight)])
        .collect();
    let widened = handed(
        &program.check,
        &eight,
        &[seeded],
        BTreeMap::from([(seeded, owed.clone())]),
        BTreeMap::from([(seeded, keys)]),
    );
    assert_eq!(widened.plan_for(seeded).roots, owed);

    let executor = SimExecutor::new(&widened).exploring(seeded, exhaustive(4));
    run_with(
        &widened,
        &program.check,
        &program.hashes,
        &mut store,
        &executor,
    );
    assert_eq!(
        executor.searched(seeded).map(|p| p.roots),
        Some(owed),
        "the run must search only what it owes"
    );
    assert!(
        passed(&store, plan_key(hash, &eight)),
        "the widened plan's key is filed even though only half its roots ran"
    );
}

#[test]
fn an_exhausted_search_reports_green_and_writes_nothing() {
    let root = TempRoot::new();
    let mut store = root.store();
    let (program, seeded) = seeded_program();
    let plan = Plan::default();

    let selection = seeded_choice(&program, &[seeded], &plan, false);
    let executor = SimExecutor::new(&selection).exploring(seeded, spent(256));
    let report = run_with(
        &selection,
        &program.check,
        &program.hashes,
        &mut store,
        &executor,
    );

    assert_eq!(report.failed, 0);
    assert_eq!(report.passed, 3);
    let result = report
        .results
        .iter()
        .find(|r| r.index == seeded)
        .expect("reported");
    assert!(result.passed());
    assert!(result.green_but_uncached());
    assert_eq!(result.recorded, Some(ply_test::Record::Exhausted));

    let hash = program.hashes.tests[seeded];
    assert!(store.get(hash).is_none());
    assert!(store.get(plan_key(hash, &plan)).is_none());
    assert_eq!(report.simulation.exhausted, 1);
}

#[test]
fn a_simulated_failure_is_never_cached_under_any_key() {
    let root = TempRoot::new();
    let mut store = root.store();
    let (program, seeded) = seeded_program();
    let plan = Plan::random(2);
    let seed = Seed::at(0, vec![1, 0, 3]);

    let selection = seeded_choice(&program, &[seeded], &plan, true);
    let executor = SimExecutor::new(&selection)
        .exploring(seeded, failed_at(seed.clone(), 47))
        .failing(seeded);
    let report = run_with(
        &selection,
        &program.check,
        &program.hashes,
        &mut store,
        &executor,
    );

    assert_eq!(report.failed, 1);
    let hash = program.hashes.tests[seeded];
    assert!(store.get(hash).is_none());
    assert!(store.get(plan_key(hash, &plan)).is_none());
    for root in &plan.roots {
        assert!(store.get(root_key(hash, *root)).is_none());
    }
}

#[test]
fn a_failure_carries_the_seed_and_the_race_that_explain_it() {
    let root = TempRoot::new();
    let mut store = root.store();
    let (program, seeded) = seeded_program();
    let seed = Seed::at(0, vec![1, 0, 3]);
    let site = |task: u32| RaceSite {
        task: ply_eval::TaskId(task),
        definition: Some(Symbol::new("apply_debit")),
        access: "db.write[accounts]".into(),
        span: ply_span::Span::DUMMY,
    };

    let selection = seeded_choice(&program, &[seeded], &Plan::default(), false);
    let executor = SimExecutor::new(&selection)
        .exploring(
            seeded,
            Exploration {
                race: Some(Race {
                    left: site(1),
                    right: site(2),
                    at: 3,
                }),
                ..failed_at(seed.clone(), 47)
            },
        )
        .failing(seeded);
    let report = run_with(
        &selection,
        &program.check,
        &program.hashes,
        &mut store,
        &executor,
    );

    let failure = &report.failures[0];
    assert_eq!(failure.seed, Some(seed));
    assert_eq!(
        failure.race,
        Some(Race {
            left: site(1),
            right: site(2),
            at: 3,
        })
    );
}

#[test]
fn an_unsimulated_failure_carries_no_seed_and_no_race() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ONE_RED);
    let report = program.run(&program.every(), &mut store);

    assert_eq!(report.failed, 1);
    assert_eq!(report.failures[0].seed, None);
    assert_eq!(report.failures[0].race, None);
    assert_eq!(report.simulation.simulated, 0);
}

#[test]
fn a_seeded_test_with_no_observed_search_warns_and_is_not_cached() {
    let root = TempRoot::new();
    let mut store = root.store();
    let (program, seeded) = seeded_program();
    let plan = Plan::default();

    let selection = seeded_choice(&program, &[seeded], &plan, false);
    let executor = SimExecutor::new(&selection);
    let report = run_with(
        &selection,
        &program.check,
        &program.hashes,
        &mut store,
        &executor,
    );

    assert_eq!(report.failed, 0);
    assert!(
        store
            .get(plan_key(program.hashes.tests[seeded], &plan))
            .is_none()
    );
    assert_eq!(
        report
            .results
            .iter()
            .find(|r| r.index == seeded)
            .and_then(|r| r.recorded.clone()),
        Some(ply_test::Record::Unobserved)
    );
    assert_eq!(report.warnings.len(), 1);
    assert!(report.warnings[0].message.contains("reported no search"));
}

#[test]
fn the_summary_counts_the_seeds_the_interleavings_and_the_exhaustive_searches() {
    let root = TempRoot::new();
    let mut store = root.store();
    let mut program = Program::compile(ARITHMETIC);
    let one = make_seeded(&mut program, "mul is right");
    let two = make_seeded(&mut program, "twice is right");
    let plan = Plan::random(4);

    let selection = seeded_choice(&program, &[one, two], &plan, true);
    let executor = SimExecutor::new(&selection)
        .exploring(one, exhaustive(12))
        .exploring(two, spent(256));
    let report = run_with(
        &selection,
        &program.check,
        &program.hashes,
        &mut store,
        &executor,
    );

    let summary = report.simulation;
    assert_eq!(summary.simulated, 2);
    assert_eq!(summary.total, 3);
    assert_eq!(summary.seeds, 8, "four roots each, for two simulated tests");
    assert_eq!(summary.interleavings, 268);
    assert_eq!(summary.exhaustive, 1);
    assert_eq!(summary.exhausted, 1);

    let result = |index: usize| {
        report
            .results
            .iter()
            .find(|r| r.index == index)
            .expect("reported")
    };
    let searched = result(one).simulation.as_ref().expect("a search");
    assert_eq!((searched.explored, searched.exhaustive), (12, true));
    assert!(
        result(one)
            .recorded
            .as_ref()
            .is_some_and(|r| r.is_written())
    );
    // Absent, never zeroed: zero explored is not the same as never simulated.
    assert!(
        result(program.index_of("add is right"))
            .simulation
            .is_none()
    );
}

#[test]
fn a_measured_reduction_is_carried_on_the_result_and_a_spent_naive_budget_is_a_lower_bound() {
    let root = TempRoot::new();
    let mut store = root.store();
    let (program, seeded) = seeded_program();

    let selection = seeded_choice(&program, &[seeded], &Plan::default(), false);
    let naive = Naive {
        explored: 720,
        bounded: false,
    };
    let executor = SimExecutor::new(&selection).exploring(
        seeded,
        Exploration {
            naive: Some(naive),
            ..exhaustive(12)
        },
    );
    let report = run_with(
        &selection,
        &program.check,
        &program.hashes,
        &mut store,
        &executor,
    );

    let result = report
        .results
        .iter()
        .find(|r| r.index == seeded)
        .expect("reported");
    assert_eq!(
        result.simulation.as_ref().and_then(|e| e.naive),
        Some(naive)
    );

    let bounded = Naive {
        explored: 4096,
        bounded: true,
    };
    assert_eq!(bounded.to_string(), ">= 4096");
}
