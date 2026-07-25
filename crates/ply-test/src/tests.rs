use crate::{Executor, Reason, Selection, Status, group_by_conflict, run, run_with, select};
use ply_core::{CheckOutput, EffectAtom, Footprint, Resource};
use ply_hash::HashOutput;
use ply_span::{Diagnostic, SourceId, Symbol};
use ply_store::{Outcome, Store};
use ply_syntax::ast::{Mode, Module};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

// ---------------------------------------------------------------- fixtures

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
    module: Module,
    check: CheckOutput,
    hashes: HashOutput,
}

impl Program {
    fn compile(src: &str) -> Program {
        let module = ply_syntax::parse(SourceId(0), src).expect("the fixture must parse");
        let check = ply_core::check_module(&module)
            .unwrap_or_else(|d| panic!("the fixture must typecheck: {d:#?}"));
        let hashes =
            ply_hash::hash_module(&module, &check).unwrap_or_else(|d| panic!("hash: {d:#?}"));
        Program { module, check, hashes }
    }

    fn index_of(&self, name: &str) -> usize {
        self.check
            .tests
            .iter()
            .position(|t| t.name == name)
            .unwrap_or_else(|| panic!("no test named {name:?}"))
    }

    fn select(&self, store: &Store) -> Selection {
        select(&self.check, &self.hashes, store)
    }

    fn run(&self, selection: &Selection, store: &mut Store) -> crate::RunReport {
        run(selection, &self.module, &self.check, &self.hashes, store)
    }

    fn def_hash(&self, name: &str) -> ply_hash::DefHash {
        self.hashes.defs.get(&Symbol::new(name)).copied().expect("a definition by that name")
    }
}

fn atom(effect: &str, resource: Option<&str>, mode: Mode) -> EffectAtom {
    EffectAtom::new(
        effect,
        resource.map(|r| Resource::Named(Symbol::new(r))).unwrap_or(Resource::Singleton),
        mode,
    )
}

fn writes(resources: &[&str]) -> Footprint {
    Footprint::from_atoms(resources.iter().map(|r| atom("db", Some(r), Mode::Write)))
}

fn reads(resources: &[&str]) -> Footprint {
    Footprint::from_atoms(resources.iter().map(|r| atom("db", Some(r), Mode::Read)))
}

/// The colouring `group_by_conflict` replaces, so that "largest footprint first
/// packs better" is checked rather than asserted.
fn colours_in_source_order(tests: &[(usize, Footprint)]) -> usize {
    let mut classes: Vec<Vec<usize>> = Vec::new();
    for (p, (_, footprint)) in tests.iter().enumerate() {
        let slot = classes
            .iter()
            .position(|class| class.iter().all(|&q| !footprint.conflicts_with(&tests[q].1)));
        match slot {
            Some(k) => classes[k].push(p),
            None => classes.push(vec![p]),
        }
    }
    classes.len()
}

fn assert_groups_are_conflict_free(groups: &[Vec<usize>], tests: &[(usize, Footprint)]) {
    let footprint = |index: usize| {
        &tests.iter().find(|(i, _)| *i == index).expect("index came from the input").1
    };
    for group in groups {
        for (a, &i) in group.iter().enumerate() {
            for &j in &group[a + 1..] {
                assert!(
                    !footprint(i).conflicts_with(footprint(j)),
                    "tests {i} and {j} share a group but conflict: {} vs {}",
                    footprint(i),
                    footprint(j)
                );
            }
        }
    }
}

fn sorted(indices: impl IntoIterator<Item = usize>) -> Vec<usize> {
    let mut out: Vec<usize> = indices.into_iter().collect();
    out.sort_unstable();
    out
}

// ---------------------------------------------------------------- scheduling

#[test]
fn pure_tests_all_land_in_the_first_group() {
    let tests: Vec<(usize, Footprint)> = (0..5).map(|i| (i, Footprint::empty())).collect();
    assert_eq!(group_by_conflict(&tests), vec![vec![0, 1, 2, 3, 4]]);
}

#[test]
fn a_pure_test_joins_the_group_of_an_effectful_one() {
    let tests = vec![(0, writes(&["users"])), (1, Footprint::empty()), (2, Footprint::empty())];
    assert_eq!(group_by_conflict(&tests), vec![vec![0, 1, 2]]);
}

#[test]
fn writes_to_disjoint_resources_group_together() {
    let tests = vec![(0, writes(&["users"])), (1, writes(&["orders"])), (2, writes(&["ledger"]))];
    assert_eq!(group_by_conflict(&tests), vec![vec![0, 1, 2]]);
}

#[test]
fn readers_of_one_resource_group_and_the_writer_is_separated() {
    let tests = vec![
        (0, reads(&["users"])),
        (1, reads(&["users"])),
        (2, writes(&["users"])),
        (3, reads(&["users"])),
    ];
    let groups = group_by_conflict(&tests);
    assert_eq!(groups.len(), 2);
    assert!(groups.contains(&vec![2]), "the writer runs alone: {groups:?}");
    assert!(groups.contains(&vec![0, 1, 3]), "the readers share a group: {groups:?}");
    assert_groups_are_conflict_free(&groups, &tests);
}

#[test]
fn a_write_to_one_resource_does_not_separate_readers_of_another() {
    let tests = vec![(0, reads(&["users"])), (1, writes(&["orders"])), (2, reads(&["users"]))];
    assert_eq!(group_by_conflict(&tests), vec![vec![0, 1, 2]]);
}

#[test]
fn the_largest_footprint_claims_the_first_group() {
    let tests = vec![(0, writes(&["a"])), (1, writes(&["b"])), (2, writes(&["a", "b"]))];
    // Source order would pair 0 and 1 and push 2 into a second group; colouring
    // the two-atom test first inverts which group each ends up in.
    assert_eq!(group_by_conflict(&tests), vec![vec![2], vec![0, 1]]);
}

#[test]
fn largest_footprint_first_uses_fewer_groups_than_source_order() {
    // A 2-colourable conflict graph — a 6-cycle, one shared resource per edge —
    // laid out so that colouring in index order needs three groups. The private
    // `p*` reads add no edges; they exist only to make one side of the cycle
    // look larger than the other.
    let tests = vec![
        (
            0,
            Footprint::from_atoms([
                atom("db", Some("e03"), Mode::Write),
                atom("db", Some("e05"), Mode::Write),
                atom("db", Some("p0"), Mode::Read),
            ]),
        ),
        (1, writes(&["e12", "e14"])),
        (
            2,
            Footprint::from_atoms([
                atom("db", Some("e12"), Mode::Write),
                atom("db", Some("e25"), Mode::Write),
                atom("db", Some("p2"), Mode::Read),
            ]),
        ),
        (3, writes(&["e03", "e34"])),
        (
            4,
            Footprint::from_atoms([
                atom("db", Some("e14"), Mode::Write),
                atom("db", Some("e34"), Mode::Write),
                atom("db", Some("p4"), Mode::Read),
            ]),
        ),
        (5, writes(&["e05", "e25"])),
    ];

    let groups = group_by_conflict(&tests);
    assert_groups_are_conflict_free(&groups, &tests);
    assert_eq!(groups, vec![vec![0, 2, 4], vec![1, 3, 5]]);
    assert_eq!(colours_in_source_order(&tests), 3);
}

#[test]
fn grouping_partitions_every_selected_test_exactly_once() {
    let tests = vec![
        (3, writes(&["users"])),
        (7, reads(&["users"])),
        (8, Footprint::empty()),
        (11, writes(&["users", "orders"])),
        (12, reads(&["orders"])),
    ];
    let groups = group_by_conflict(&tests);
    assert_eq!(sorted(groups.iter().flatten().copied()), vec![3, 7, 8, 11, 12]);
    assert_groups_are_conflict_free(&groups, &tests);
}

#[test]
fn grouping_is_deterministic_and_handles_an_empty_input() {
    assert_eq!(group_by_conflict(&[]), Vec::<Vec<usize>>::new());
    let tests = vec![(0, writes(&["a"])), (1, reads(&["a"])), (2, writes(&["b"]))];
    assert_eq!(group_by_conflict(&tests), group_by_conflict(&tests));
}

// ---------------------------------------------------------------- selection

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

#[test]
fn a_cold_cache_selects_every_test() {
    let root = TempRoot::new();
    let store = root.store();
    let program = Program::compile(ARITHMETIC);

    let selection = program.select(&store);
    assert_eq!(selection.total, 3);
    assert_eq!(selection.to_run, vec![0, 1, 2]);
    assert!(selection.cached.is_empty());
    assert!(selection.reasons.iter().all(|r| *r == Reason::New));
    assert_eq!(selection.groups, vec![vec![0, 1, 2]]);
}

#[test]
fn a_warm_cache_selects_nothing() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ARITHMETIC);

    let first = program.select(&store);
    let report = program.run(&first, &mut store);
    assert_eq!((report.passed, report.failed), (3, 0), "{:#?}", report.failures);

    let second = program.select(&store);
    assert!(second.to_run.is_empty());
    assert_eq!(second.cached.len(), 3);
    assert!(second.groups.is_empty());
    assert!(second.reasons.iter().all(|r| *r == Reason::Cached));

    let report = program.run(&second, &mut store);
    assert_eq!((report.passed, report.failed, report.cached), (0, 0, 3));
    assert!(report.results.is_empty());
}

#[test]
fn a_warm_cache_survives_reopening_the_store() {
    let root = TempRoot::new();
    let program = Program::compile(ARITHMETIC);
    {
        let mut store = root.store();
        let selection = program.select(&store);
        program.run(&selection, &mut store);
    }
    let store = root.store();
    assert!(program.select(&store).to_run.is_empty());
}

#[test]
fn an_edit_selects_only_the_tests_that_reach_it() {
    let root = TempRoot::new();
    let mut store = root.store();

    let before = Program::compile(ARITHMETIC);
    let selection = before.select(&store);
    let report = before.run(&selection, &mut store);
    assert_eq!(report.failed, 0, "{:#?}", report.failures);

    // `mul` changes and `twice` calls it, so both hashes move. `add` is
    // untouched and its test must stay cached.
    let after = Program::compile(&ARITHMETIC.replace(
        "fn mul(a: Int, b: Int) -> Int = a * b",
        "fn mul(a: Int, b: Int) -> Int = a * b * 1",
    ));
    let selection = after.select(&store);

    let add = after.index_of("add is right");
    let mul = after.index_of("mul is right");
    let twice = after.index_of("twice is right");

    assert_eq!(selection.reason(add), Some(Reason::Cached));
    assert_eq!(selection.reason(mul), Some(Reason::New));
    assert_eq!(selection.reason(twice), Some(Reason::New));
    assert_eq!(selection.to_run, sorted([mul, twice]));
}

#[test]
fn renaming_a_definition_selects_nothing() {
    let root = TempRoot::new();
    let mut store = root.store();

    let before = Program::compile(ARITHMETIC);
    let selection = before.select(&store);
    assert_eq!(before.run(&selection, &mut store).failed, 0);

    let after = Program::compile(&ARITHMETIC.replace("mul(", "product("));
    assert!(after.hashes.defs.contains_key(&Symbol::new("product")));

    let selection = after.select(&store);
    assert!(
        selection.to_run.is_empty(),
        "a rename changes no behaviour, so nothing may re-run: {selection:?}"
    );
}

const NONDETERMINISTIC: &str = r#"
nondet effect clock {
  read now() -> Int
}

fn tick() -> Int / {clock.read} = clock.now()

test "pure arithmetic" {
  assert_eq(1 + 1, 2)
}

test/nondet "the clock advances" {
  handle {
    assert(tick() > 0)
  } with {
    clock.now() -> 7,
  }
}
"#;

#[test]
fn a_nondet_test_always_runs_and_is_never_cached() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(NONDETERMINISTIC);
    let nondet = program.index_of("the clock advances");
    let pure = program.index_of("pure arithmetic");

    for round in 0..3 {
        let selection = program.select(&store);
        assert_eq!(selection.reason(nondet), Some(Reason::Nondet));
        assert!(selection.to_run.contains(&nondet), "round {round}");
        if round > 0 {
            assert_eq!(selection.reason(pure), Some(Reason::Cached), "round {round}");
            assert_eq!(selection.to_run, vec![nondet], "round {round}");
        }
        let report = program.run(&selection, &mut store);
        assert_eq!(report.failed, 0, "round {round}: {:#?}", report.failures);
    }

    assert!(
        store.get(program.hashes.tests[nondet]).is_none(),
        "a nondet pass must never reach the store"
    );
}

#[test]
fn a_stored_failure_is_never_trusted() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ARITHMETIC);

    let selection = program.select(&store);
    assert_eq!(program.run(&selection, &mut store).failed, 0);
    assert!(program.select(&store).to_run.is_empty());

    let mul = program.index_of("mul is right");
    store.put(
        program.hashes.tests[mul],
        Outcome::Fail { message: "written by something else".into(), diagnostic: None },
    );

    let selection = program.select(&store);
    assert_eq!(selection.reason(mul), Some(Reason::PreviousFailure));
    assert_eq!(selection.to_run, vec![mul]);
}

#[test]
fn a_test_with_no_hash_runs_and_is_never_cached() {
    let root = TempRoot::new();
    let mut store = root.store();
    let mut program = Program::compile(ARITHMETIC);
    program.hashes.tests.truncate(1);

    for _ in 0..2 {
        let selection = program.select(&store);
        assert_eq!(selection.reason(1), Some(Reason::Unhashed));
        assert_eq!(selection.reason(2), Some(Reason::Unhashed));
        assert!(selection.to_run.contains(&1) && selection.to_run.contains(&2));
        let report = program.run(&selection, &mut store);
        assert_eq!(report.failed, 0, "{:#?}", report.failures);
        assert!(report.warnings.is_empty());
    }

    assert_eq!(program.select(&store).reason(0), Some(Reason::Cached));
}

// ---------------------------------------------------------------- running

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
fn a_failure_is_never_cached_and_re_runs_until_it_goes_green() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ONE_RED);
    let good = program.index_of("good is one");
    let bad = program.index_of("bad is one");

    for round in 0..3 {
        let selection = program.select(&store);
        assert!(
            selection.to_run.contains(&bad),
            "round {round}: a red test must be selected every single time"
        );
        if round > 0 {
            assert_eq!(selection.to_run, vec![bad], "round {round}");
            assert_eq!(selection.reason(good), Some(Reason::Cached));
        }

        let report = program.run(&selection, &mut store);
        assert_eq!(report.failed, 1, "round {round}");
        assert_eq!(report.failures[0].name, "bad is one");

        assert!(
            store.get(program.hashes.tests[bad]).is_none(),
            "round {round}: a failure must never reach the store"
        );
        assert!(
            store.get(program.hashes.tests[good]).is_some(),
            "round {round}: a pass must reach the store"
        );
    }

    let fixed = Program::compile(&ONE_RED.replace("fn bad() -> Int = 2", "fn bad() -> Int = 3 - 2"));
    let selection = fixed.select(&store);
    assert!(selection.to_run.contains(&bad));
    let report = fixed.run(&selection, &mut store);
    assert_eq!((report.passed, report.failed), (selection.to_run.len(), 0));
    assert!(
        fixed.select(&store).to_run.is_empty(),
        "only going green may stop a test from being selected"
    );
}

#[test]
fn a_fix_that_reproduces_a_green_definition_is_green_without_running() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ONE_RED);
    let selection = program.select(&store);
    assert_eq!(program.run(&selection, &mut store).failed, 1);

    // `bad` is repaired into something structurally identical to `good`, so it
    // hashes identically and so does its test. The cache already holds that
    // hash, green — selection is exact enough to notice.
    let fixed = Program::compile(&ONE_RED.replace("fn bad() -> Int = 2", "fn bad() -> Int = 1"));
    assert_eq!(fixed.def_hash("bad"), fixed.def_hash("good"));
    assert!(fixed.select(&store).to_run.is_empty());
}

#[test]
fn a_red_test_does_not_vouch_for_the_definitions_it_exercised() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ONE_RED);
    let selection = program.select(&store);
    program.run(&selection, &mut store);

    assert!(store.get(program.def_hash("bad")).is_none());
    assert!(store.get(program.def_hash("good")).is_some());
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
    let selection = green.select(&store);
    let report = green.run(&selection, &mut store);
    assert_eq!(report.failed, 0, "{:#?}", report.failures);

    let red = Program::compile(&LEDGER.replace(
        "fn debit(balance: Int, amount: Int) -> Int = balance - amount",
        "fn debit(balance: Int, amount: Int) -> Int = balance - amount - 1",
    ));
    let selection = red.select(&store);
    assert_eq!(selection.to_run, vec![red.index_of("settle nets out")]);

    let report = red.run(&selection, &mut store);
    assert_eq!(report.failed, 1);
    let suspects: Vec<&str> = report.failures[0].suspects.iter().map(|s| s.as_str()).collect();
    assert_eq!(
        suspects,
        vec!["debit", "settle"],
        "only the edited definition and the one carrying it are suspect"
    );
}

#[test]
fn suspects_are_computed_against_the_cache_as_it_was_before_the_run() {
    let root = TempRoot::new();
    let mut store = root.store();

    let green = Program::compile(LEDGER);
    let selection = green.select(&store);
    assert_eq!(green.run(&selection, &mut store).failed, 0);

    // `credit` is rewritten so that its own test stays green while its hash
    // moves. Both tests re-run in one batch, so if the passing test's writes
    // were visible to the failing one, `credit` would drop off the suspect list
    // — exactly the attribution bug worth guarding.
    let red = Program::compile(
        &LEDGER
            .replace(
                "fn credit(balance: Int, amount: Int) -> Int = balance + amount",
                "fn credit(balance: Int, amount: Int) -> Int = amount + balance",
            )
            .replace("assert_eq(settle(0), 6)", "assert_eq(settle(0), 99)"),
    );
    let selection = red.select(&store);
    assert_eq!(selection.to_run.len(), 2);
    assert_eq!(selection.groups, vec![vec![0, 1]], "both are pure, so they share a group");

    let report = red.run(&selection, &mut store);
    assert_eq!(report.failed, 1);
    let suspects: Vec<&str> = report.failures[0].suspects.iter().map(|s| s.as_str()).collect();
    assert!(
        suspects.contains(&"credit"),
        "a sibling test passing first must not clear a suspect: {suspects:?}"
    );
}

#[test]
fn the_report_accounts_for_every_selected_test_exactly_once() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ONE_RED);

    let selection = program.select(&store);
    let report = program.run(&selection, &mut store);
    assert_eq!(report.passed + report.failed, selection.to_run.len());
    assert_eq!(sorted(report.results.iter().map(|r| r.index)), selection.to_run);
    assert!(report.warnings.is_empty(), "{:#?}", report.warnings);
    assert!(!report.is_success());
}

#[test]
fn an_empty_selection_runs_nothing() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ARITHMETIC);
    let selection = Selection {
        total: 3,
        cached: Vec::new(),
        to_run: Vec::new(),
        groups: Vec::new(),
        reasons: vec![Reason::Cached; 3],
    };
    let report = program.run(&selection, &mut store);
    assert_eq!((report.passed, report.failed, report.cached), (0, 0, 0));
    assert!(report.warnings.is_empty());
}

#[test]
fn a_selected_test_left_out_of_every_group_is_still_run() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ARITHMETIC);
    let selection = Selection {
        total: 3,
        cached: Vec::new(),
        to_run: vec![0, 1, 2],
        groups: vec![vec![0]],
        reasons: vec![Reason::New; 3],
    };
    let report = program.run(&selection, &mut store);
    assert_eq!(report.passed, 3, "no selected test may be silently skipped");
    assert_eq!(report.warnings.len(), 1);
}

#[test]
fn a_selection_naming_a_test_that_does_not_exist_warns_instead_of_panicking() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ARITHMETIC);
    let selection = Selection {
        total: 3,
        cached: Vec::new(),
        to_run: vec![0, 99],
        groups: vec![vec![0, 99]],
        reasons: vec![Reason::New; 3],
    };
    let report = program.run(&selection, &mut store);
    assert_eq!((report.passed, report.failed), (1, 0));
    assert_eq!(report.warnings.len(), 1);
    assert!(report.warnings[0].message.contains("99"));
}

// ---------------------------------------------------------------- panics

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
    let selection = program.select(&store);
    let doomed = program.index_of("mul is right");

    let executor = PanickingExecutor { panic_on: doomed };
    let report = run_with(&selection, &program.check, &program.hashes, &mut store, &executor);

    assert_eq!(report.passed, 2, "the other tests must still have run");
    assert_eq!(report.failed, 1);

    let panicked = report.results.iter().find(|r| r.index == doomed).expect("reported");
    assert_eq!(panicked.status, Status::Panicked);
    let diagnostic = panicked.failure.as_ref().expect("a panic carries a diagnostic");
    assert_eq!(diagnostic.code, ply_span::codes::RUNTIME_ERROR);
    assert!(diagnostic.message.contains("deliberate panic"), "{}", diagnostic.message);
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

    // One group per test forces the sequential path, so the worker that unwound
    // is the one asked to run the next test.
    let selection = Selection {
        total: 3,
        cached: Vec::new(),
        to_run: vec![0, 1, 2],
        groups: vec![vec![0], vec![1], vec![2]],
        reasons: vec![Reason::New; 3],
    };
    let executor = PanickingExecutor { panic_on: 0 };
    let report = run_with(&selection, &program.check, &program.hashes, &mut store, &executor);

    assert_eq!((report.passed, report.failed), (2, 1));
    assert!(report.results.iter().skip(1).all(|r| r.status == Status::Passed));
    assert_eq!(report.results.iter().map(|r| r.group).collect::<Vec<_>>(), vec![0, 1, 2]);
}

// ---------------------------------------------------------------- reporting

#[test]
fn the_json_report_carries_the_diagnostic_and_the_suspects() {
    let root = TempRoot::new();
    let mut store = root.store();

    let green = Program::compile(LEDGER);
    let selection = green.select(&store);
    green.run(&selection, &mut store);

    let red =
        Program::compile(&LEDGER.replace("assert_eq(settle(0), 6)", "assert_eq(settle(0), 7)"));
    let selection = red.select(&store);
    let report = red.run(&selection, &mut store);

    let json = report.to_json();
    assert_eq!(json["failed"], 1);
    assert_eq!(json["success"], false);
    assert!(json["duration_ms"].is_number());

    let failure = &json["failures"][0];
    assert_eq!(failure["name"], "settle nets out");
    assert_eq!(failure["diagnostic"]["code"], ply_span::codes::ASSERTION_FAILED);
    // Only the expectation inside the test moved, so no definition is under
    // suspicion — the test is the thing that changed.
    assert_eq!(failure["suspects"], serde_json::json!([]));

    assert_eq!(json["tests"][0]["status"], "failed");
    assert!(json["tests"][0]["hash"].as_str().is_some_and(|h| h.len() == 64));

    let summary = report.summary();
    assert!(summary[0].starts_with("1 failed, 0 passed, 1 cached"), "{summary:#?}");
    assert!(summary.iter().any(|l| l.contains("expected 7, found 6")), "{summary:#?}");
    assert!(report.results[0].line().starts_with('✗'));
}

#[test]
fn the_summary_lists_the_suspects_for_a_failure() {
    let root = TempRoot::new();
    let mut store = root.store();

    let green = Program::compile(LEDGER);
    let selection = green.select(&store);
    assert_eq!(green.run(&selection, &mut store).failed, 0);

    let red = Program::compile(&LEDGER.replace(
        "fn debit(balance: Int, amount: Int) -> Int = balance - amount",
        "fn debit(balance: Int, amount: Int) -> Int = balance - amount - 1",
    ));
    let selection = red.select(&store);
    let summary = red.run(&selection, &mut store).summary();
    assert!(summary.iter().any(|l| l == "  suspects: debit, settle"), "{summary:#?}");
}

#[test]
fn explain_covers_every_test_and_every_group() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ARITHMETIC);

    let selection = program.select(&store);
    let lines = selection.explain(&program.check, &program.hashes);
    assert_eq!(lines.len(), selection.total + selection.groups.len());
    assert!(lines.iter().take(3).all(|l| l.starts_with("run ")), "{lines:#?}");

    program.run(&selection, &mut store);
    let selection = program.select(&store);
    let lines = selection.explain(&program.check, &program.hashes);
    assert!(lines.iter().all(|l| l.starts_with("skip")), "{lines:#?}");

    let json = selection.to_json(&program.check, &program.hashes);
    assert_eq!(json["selected"], 0);
    assert_eq!(json["cached"], 3);
    assert_eq!(json["tests"][0]["reason"], "cached");
}

// ---------------------------------------------------------------- concurrency

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
fn a_module_whose_tests_are_all_isolated_runs_as_a_single_group() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(DISJOINT_CELLS);

    // `with_cell` discharges its atoms at the region boundary, so each of these
    // tests is observably pure and they all share one group.
    assert!(program.check.tests.iter().all(|t| t.footprint.is_empty()));

    let selection = program.select(&store);
    assert_eq!(selection.groups, vec![vec![0, 1, 2, 3]]);

    let report = program.run(&selection, &mut store);
    assert_eq!((report.passed, report.failed), (4, 0), "{:#?}", report.failures);
    assert!(report.results.iter().all(|r| r.group == 0));
}

#[test]
fn every_group_is_run_in_sequence() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ARITHMETIC);

    let mut selection = program.select(&store);
    selection.groups = vec![vec![0], vec![1, 2]];
    let report = program.run(&selection, &mut store);
    assert_eq!(report.passed, 3);
    assert_eq!(report.results.iter().filter(|r| r.group == 0).count(), 1);
    assert_eq!(report.results.iter().filter(|r| r.group == 1).count(), 2);
}
