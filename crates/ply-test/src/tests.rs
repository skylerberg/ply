use crate::{
    Executor, Isolation, Parallelism, Reason, Search, Selection, Status, group_by_conflict, run,
    run_with, select,
};
use ply_core::{CheckOutput, EffectAtom, Footprint, Resource};
use ply_eval::{Plan, TaskRegions, Value};
use ply_hash::HashOutput;
use ply_span::{Diagnostic, SourceId, Symbol};
use ply_store::{Outcome, Store};
use ply_syntax::ast::Mode;
use ply_syntax::resolve::Resolved;
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
    program: ply_syntax::ast::Program,
    resolved: Resolved,
    check: CheckOutput,
    hashes: HashOutput,
}

impl Program {
    fn compile(src: &str) -> Program {
        let module = ply_syntax::parse(SourceId(0), src).expect("the fixture must parse");
        let mut program = ply_syntax::ast::Program::single(module);
        let resolved = ply_syntax::resolve(&mut program)
            .unwrap_or_else(|d| panic!("the fixture must resolve: {d:#?}"));
        let check = ply_core::check_program(&program, &resolved)
            .unwrap_or_else(|d| panic!("the fixture must typecheck: {d:#?}"));
        let hashes = ply_hash::hash_program(&program, &resolved, &check)
            .unwrap_or_else(|d| panic!("hash: {d:#?}"));
        Program {
            program,
            resolved,
            check,
            hashes,
        }
    }

    fn index_of(&self, name: &str) -> usize {
        self.check
            .tests
            .iter()
            .position(|t| t.name == name)
            .unwrap_or_else(|| panic!("no test named {name:?}"))
    }

    fn select(&self, store: &Store) -> Selection {
        self.select_under(store, &Plan::default())
    }

    fn select_under(&self, store: &Store, plan: &Plan) -> Selection {
        select(&self.check, &self.hashes, store, plan)
    }

    /// Both engines, so that every scheduling and caching test in this file is also a differential
    /// test over a real Ply program.
    fn run(&self, selection: &Selection, store: &mut Store) -> crate::RunReport {
        run(
            selection,
            &self.program,
            &self.resolved,
            &self.check,
            &self.hashes,
            store,
            ply_eval::EngineChoice::Both,
            Search::of(selection),
            crate::Hosting::hermetic(),
        )
    }

    fn def_hash(&self, name: &str) -> ply_hash::DefHash {
        self.hashes
            .defs
            .get(&Symbol::new(name))
            .copied()
            .expect("a definition by that name")
    }
}

fn atom(effect: &str, resource: Option<&str>, mode: Mode) -> EffectAtom {
    EffectAtom::new(
        effect,
        resource
            .map(|r| Resource::Named(Symbol::new(r)))
            .unwrap_or(Resource::Singleton),
        mode,
    )
}

fn writes(resources: &[&str]) -> Footprint {
    Footprint::from_atoms(resources.iter().map(|r| atom("db", Some(r), Mode::Write)))
}

fn reads(resources: &[&str]) -> Footprint {
    Footprint::from_atoms(resources.iter().map(|r| atom("db", Some(r), Mode::Read)))
}

/// The colouring `group_by_conflict` replaces, so that "largest footprint first packs better" is
/// checked rather than asserted.
fn colours_in_source_order(tests: &[(usize, Footprint)]) -> usize {
    let mut classes: Vec<Vec<usize>> = Vec::new();
    for (p, (_, footprint)) in tests.iter().enumerate() {
        let slot = classes.iter().position(|class| {
            class
                .iter()
                .all(|&q| !footprint.conflicts_with(&tests[q].1))
        });
        match slot {
            Some(k) => classes[k].push(p),
            None => classes.push(vec![p]),
        }
    }
    classes.len()
}

/// Sharing a group is a claim about the atoms that contend: a seed is an input handed to one test
/// and never a resource two of them reach.
fn assert_groups_are_conflict_free(groups: &[Vec<usize>], tests: &[(usize, Footprint)]) {
    let footprint = |index: usize| {
        crate::shared_footprint(
            &tests
                .iter()
                .find(|(i, _)| *i == index)
                .expect("index came from the input")
                .1,
        )
    };
    for group in groups {
        for (a, &i) in group.iter().enumerate() {
            for &j in &group[a + 1..] {
                assert!(
                    !footprint(i).conflicts_with(&footprint(j)),
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

#[test]
fn pure_tests_all_land_in_the_first_group() {
    let tests: Vec<(usize, Footprint)> = (0..5).map(|i| (i, Footprint::empty())).collect();
    assert_eq!(group_by_conflict(&tests), vec![vec![0, 1, 2, 3, 4]]);
}

#[test]
fn a_pure_test_joins_the_group_of_an_effectful_one() {
    let tests = vec![
        (0, writes(&["users"])),
        (1, Footprint::empty()),
        (2, Footprint::empty()),
    ];
    assert_eq!(group_by_conflict(&tests), vec![vec![0, 1, 2]]);
}

#[test]
fn writes_to_disjoint_resources_group_together() {
    let tests = vec![
        (0, writes(&["users"])),
        (1, writes(&["orders"])),
        (2, writes(&["ledger"])),
    ];
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
    assert!(
        groups.contains(&vec![2]),
        "the writer runs alone: {groups:?}"
    );
    assert!(
        groups.contains(&vec![0, 1, 3]),
        "the readers share a group: {groups:?}"
    );
    assert_groups_are_conflict_free(&groups, &tests);
}

#[test]
fn a_write_to_one_resource_does_not_separate_readers_of_another() {
    let tests = vec![
        (0, reads(&["users"])),
        (1, writes(&["orders"])),
        (2, reads(&["users"])),
    ];
    assert_eq!(group_by_conflict(&tests), vec![vec![0, 1, 2]]);
}

#[test]
fn the_largest_footprint_claims_the_first_group() {
    let tests = vec![
        (0, writes(&["a"])),
        (1, writes(&["b"])),
        (2, writes(&["a", "b"])),
    ];
    // Source order would pair 0 and 1 and push 2 into a second group; colouring the two-atom test
    // first inverts which group each ends up in.
    assert_eq!(group_by_conflict(&tests), vec![vec![2], vec![0, 1]]);
}

#[test]
fn largest_footprint_first_uses_fewer_groups_than_source_order() {
    // A 2-colourable conflict graph — a 6-cycle, one shared resource per edge — laid out so that
    // colouring in index order needs three groups.
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
    assert_eq!(
        sorted(groups.iter().flatten().copied()),
        vec![3, 7, 8, 11, 12]
    );
    assert_groups_are_conflict_free(&groups, &tests);
}

#[test]
fn grouping_is_deterministic_and_handles_an_empty_input() {
    assert_eq!(group_by_conflict(&[]), Vec::<Vec<usize>>::new());
    let tests = vec![(0, writes(&["a"])), (1, reads(&["a"])), (2, writes(&["b"]))];
    assert_eq!(group_by_conflict(&tests), group_by_conflict(&tests));
}

fn cells(resources: &[&str]) -> Footprint {
    Footprint::from_atoms(resources.iter().map(|r| atom("cell", Some(r), Mode::Write)))
}

fn seeds() -> Footprint {
    Footprint::from_atoms([atom(crate::SIM_EFFECT, None, Mode::Read)])
}

#[test]
fn a_cell_atom_is_region_scoped_and_a_db_atom_is_not() {
    assert!(crate::is_region_scoped(&atom(
        "cell",
        Some("users"),
        Mode::Write
    )));
    assert!(!crate::is_region_scoped(&atom(
        "db",
        Some("users"),
        Mode::Write
    )));

    // A user effect is module-qualified and `cell` is a reserved name, so the one effect the report
    // names cannot be impersonated.
    assert!(!crate::is_region_scoped(&atom(
        "m.cell",
        Some("users"),
        Mode::Write
    )));

    assert!(crate::region_isolated(&Footprint::empty()));
    assert!(crate::region_isolated(&seeds()));
    assert!(
        !crate::region_isolated(&cells(&["users", "orders"])),
        "a region label names state a sibling test can write; only the fork hid that"
    );

    let mixed = cells(&["users"]).union(&writes(&["orders"]));
    assert_eq!(crate::shared_footprint(&mixed), mixed);
    assert!(!crate::contends_only_over_regions(&mixed));
    assert!(crate::contends_only_over_regions(&cells(&["users"])));
    assert!(!crate::contends_only_over_regions(&Footprint::empty()));
}

/// What ADR 0017 §6 costs, at the smallest size that has it: three tests over two labels colour
/// into two groups, and the two that share `users` are the pair that used to be free.
#[test]
fn tests_naming_one_region_label_are_coloured_apart() {
    let tests = vec![
        (0, cells(&["users"])),
        (1, cells(&["users"])),
        (2, cells(&["orders"])),
    ];
    let groups = group_by_conflict(&tests);
    assert_eq!(groups.len(), 2, "{groups:?}");
    assert_ne!(
        groups.iter().position(|g| g.contains(&0)),
        groups.iter().position(|g| g.contains(&1))
    );
    assert_groups_are_conflict_free(&groups, &tests);
}

/// The rest of a mixed footprint is unaffected: a label collides with a label, a resource with a
/// resource, and neither launders the other.
#[test]
fn a_region_label_and_a_real_resource_conflict_independently() {
    let tests = vec![
        (0, cells(&["users"]).union(&writes(&["accounts"]))),
        (1, cells(&["orders"]).union(&reads(&["accounts"]))),
        (2, cells(&["ledger"])),
    ];
    let groups = group_by_conflict(&tests);
    assert_eq!(groups.len(), 2, "{groups:?}");
    assert_ne!(
        groups.iter().position(|g| g.contains(&0)),
        groups.iter().position(|g| g.contains(&1)),
        "a real write and a real read of `accounts` may not share a group"
    );
    assert!(
        groups[0].contains(&2),
        "a label nobody else names is still free: {groups:?}"
    );
    assert_groups_are_conflict_free(&groups, &tests);
}

/// The half that must not regress: a real shared resource is still serialized.
#[test]
fn a_real_resource_still_separates_its_writers() {
    let tests = vec![
        (0, writes(&["users"])),
        (1, writes(&["users"])),
        (2, Footprint::empty()),
    ];
    let groups = group_by_conflict(&tests);
    assert_eq!(
        groups.len(),
        2,
        "the two real writers cannot share: {groups:?}"
    );
    assert!(
        groups[0].contains(&2),
        "the isolated test is free: {groups:?}"
    );
    assert_groups_are_conflict_free(&groups, &tests);
}

#[test]
fn every_region_isolated_test_lands_in_group_zero() {
    let mut tests: Vec<(usize, Footprint)> = vec![(0, writes(&["a"])), (1, writes(&["a"]))];
    for i in 2..40 {
        tests.push((
            i,
            if i % 2 == 0 {
                Footprint::empty()
            } else {
                seeds()
            },
        ));
    }
    let groups = group_by_conflict(&tests);
    assert_eq!(groups.len(), 2);
    for (i, footprint) in &tests {
        if crate::region_isolated(footprint) {
            assert!(groups[0].contains(i), "test {i} is free but not in group 0");
        }
    }
}

/// ADR 0005 §5's property, with the population ADR 0017 §6 leaves it: what is free to add is a test
/// that names nothing another test can reach.
#[test]
fn adding_region_isolated_tests_does_not_change_the_group_count() {
    let shared: Vec<(usize, Footprint)> = vec![
        (0, writes(&["users"])),
        (1, reads(&["users"])),
        (2, writes(&["orders"])),
    ];
    let baseline = group_by_conflict(&shared).len();
    assert_eq!(baseline, 2);

    for n in [0usize, 1, 100] {
        let mut tests = shared.clone();
        for i in 0..n {
            tests.push((3 + i, seeds()));
        }
        let groups = group_by_conflict(&tests);
        assert_eq!(
            groups.len(),
            baseline,
            "adding {n} region-isolated tests changed the group count"
        );
        assert_groups_are_conflict_free(&groups, &tests);
        assert_eq!(
            sorted(groups.iter().flatten().copied()).len(),
            tests.len(),
            "every test is still scheduled exactly once"
        );

        let p = crate::parallelism(tests.iter().map(|(_, f)| f), &tests, &groups);
        assert_eq!(p.isolated, n);
        assert_eq!(p.shared, 3);
        assert_eq!(p.shared_groups, baseline);
        assert!(p.holds(), "{p:?}");
    }
}

#[test]
fn a_selection_of_only_isolated_tests_needs_one_group_and_no_shared_ones() {
    let tests: Vec<(usize, Footprint)> = (0..4).map(|i| (i, seeds())).collect();
    let groups = group_by_conflict(&tests);
    let p = crate::parallelism(tests.iter().map(|(_, f)| f), &tests, &groups);
    assert_eq!((p.total, p.isolated, p.shared), (4, 4, 0));
    assert_eq!((p.groups, p.shared_groups), (1, 0));
    assert!(p.holds(), "{p:?}");

    let empty = crate::parallelism(std::iter::empty(), &[], &[]);
    assert_eq!((empty.groups, empty.shared_groups), (0, 0));
    assert!(empty.holds(), "{empty:?}");
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

const MULTI_SHOT: &str = r#"
effect amb {
  read flip[coin]() -> Bool
}

test "both branches" {
  with_cell[trace](0) { c -> {
    let total = handle {
      let b = amb.flip[coin]();
      cell_set(c, cell_get(c) + 1);
      if b { 10 } else { 20 }
    } with {
      amb.flip[coin]() resume k -> k(true) + k(false),
      return x -> x
    };
    assert_eq(total, 30);
    assert_eq(cell_get(c), 2)
  } }
}
"#;

/// ADR 0005 §6: under `both` a machine-only test runs once, on the machine.
#[test]
fn a_test_only_the_machine_can_run_is_not_reported_as_a_divergence() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(MULTI_SHOT);

    let selection = program.select(&store);
    let report = program.run(&selection, &mut store);
    assert_eq!(
        (report.passed, report.failed),
        (1, 0),
        "{:#?}",
        report.failures
    );
}

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
    assert_eq!(
        (report.passed, report.failed),
        (3, 0),
        "{:#?}",
        report.failures
    );

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

    // `mul` changes and `twice` calls it, so both hashes move.
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
nondet effect wall {
  read now() -> Int
}

fn tick() -> Int / {wall.read} = wall.now()

test "pure arithmetic" {
  assert_eq(1 + 1, 2)
}

test/nondet "the clock advances" {
  handle {
    assert(tick() > 0)
  } with {
    wall.now() -> 7,
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
            assert_eq!(
                selection.reason(pure),
                Some(Reason::Cached),
                "round {round}"
            );
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
        Outcome::Fail {
            message: "written by something else".into(),
            diagnostic: None,
        },
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

    let fixed =
        Program::compile(&ONE_RED.replace("fn bad() -> Int = 2", "fn bad() -> Int = 3 - 2"));
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

    // `bad` is repaired into something structurally identical to `good`, so it hashes identically
    // and so does its test.
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

    assert!(!store.knows_definition(program.def_hash("bad")));
    assert!(store.knows_definition(program.def_hash("good")));
}

#[test]
fn definitions_are_recorded_apart_from_test_results() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ARITHMETIC);
    let selection = program.select(&store);
    assert_eq!(program.run(&selection, &mut store).failed, 0);

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
    let suspects: Vec<&str> = report.failures[0]
        .suspects
        .iter()
        .map(|s| s.as_str())
        .collect();
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

    // `credit` is rewritten so that its own test stays green while its hash moves.
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
    assert_eq!(
        selection.groups,
        vec![vec![0, 1]],
        "both are pure, so they share a group"
    );

    let report = red.run(&selection, &mut store);
    assert_eq!(report.failed, 1);
    let suspects: Vec<&str> = report.failures[0]
        .suspects
        .iter()
        .map(|s| s.as_str())
        .collect();
    assert!(
        suspects.contains(&"credit"),
        "a sibling test passing first must not clear a suspect: {suspects:?}"
    );
}

/// Two tests over an overlapping closure: `base is right` covers `base`, and `total is right`
/// covers `base` and `total`.
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
    let selection = green.select(&store);
    assert_eq!(green.run(&selection, &mut store).failed, 0);

    // `base` is rewritten into something value-identical, so its hash moves while the test covering
    // it stays green; `total` is broken outright.
    let red = Program::compile(&shared("1 + x", "base(x) + 11"));
    let doomed = red.index_of("total is right");

    for round in 0..3 {
        let selection = red.select(&store);
        assert!(
            selection.to_run.contains(&doomed),
            "round {round}: a red test always re-runs"
        );

        let report = red.run(&selection, &mut store);
        assert_eq!(report.failed, 1, "round {round}: {:#?}", report.results);
        let suspects: Vec<&str> = report.failures[0]
            .suspects
            .iter()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(
            suspects,
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
    let selection = green.select(&store);
    assert_eq!(green.run(&selection, &mut store).failed, 0);

    let red = Program::compile(&shared("1 + x", "base(x) + 11"));
    let base = red.index_of("base is right");
    let doomed = red.index_of("total is right");

    // What `--filter base` narrows a selection to: `total is right` keeps its reason but is never
    // handed to a group.
    let mut filtered = red.select(&store);
    filtered.to_run.retain(|&i| i == base);
    filtered.groups = vec![vec![base]];
    let report = red.run(&filtered, &mut store);
    assert_eq!((report.passed, report.failed), (1, 0));
    assert!(
        !report.results.iter().any(|r| r.index == doomed),
        "the narrowed run must not have executed `total is right`"
    );

    let selection = red.select(&store);
    let report = red.run(&selection, &mut store);
    assert_eq!(report.failed, 1);
    let suspects: Vec<&str> = report.failures[0]
        .suspects
        .iter()
        .map(|s| s.as_str())
        .collect();
    assert_eq!(
        suspects,
        vec!["base", "total"],
        "a test that never ran cannot have cleared its own closure"
    );
}

#[test]
fn going_green_ends_the_suspicion_a_failure_kept_alive() {
    let root = TempRoot::new();
    let mut store = root.store();

    let green = Program::compile(&shared("x + 1", "base(x) + 10"));
    let selection = green.select(&store);
    assert_eq!(green.run(&selection, &mut store).failed, 0);

    let red = Program::compile(&shared("1 + x", "base(x) + 11"));
    let selection = red.select(&store);
    assert_eq!(red.run(&selection, &mut store).failed, 1);

    let fixed = Program::compile(&shared("1 + x", "base(x) + 10"));
    let selection = fixed.select(&store);
    let report = fixed.run(&selection, &mut store);
    assert_eq!(report.failed, 0, "{:#?}", report.failures);
    assert!(store.knows_definition(fixed.def_hash("base")));
    assert!(store.knows_definition(fixed.def_hash("total")));

    // Only `total` moves now, so it is the only thing the next failure may name.
    let broken = Program::compile(&shared("1 + x", "base(x) + 12"));
    let selection = broken.select(&store);
    assert_eq!(selection.to_run, vec![broken.index_of("total is right")]);

    let report = broken.run(&selection, &mut store);
    assert_eq!(report.failed, 1);
    let suspects: Vec<&str> = report.failures[0]
        .suspects
        .iter()
        .map(|s| s.as_str())
        .collect();
    assert_eq!(suspects, vec!["total"]);
}

#[test]
fn the_report_accounts_for_every_selected_test_exactly_once() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ONE_RED);

    let selection = program.select(&store);
    let report = program.run(&selection, &mut store);
    assert_eq!(report.passed + report.failed, selection.to_run.len());
    assert_eq!(
        sorted(report.results.iter().map(|r| r.index)),
        selection.to_run
    );
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
        isolation: vec![Isolation::Region; 3],
        parallelism: Parallelism::default(),
        plan: Plan::default(),
        narrowed: BTreeMap::new(),
        out_of_scope: BTreeSet::new(),
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
        isolation: vec![Isolation::Region; 3],
        parallelism: Parallelism::default(),
        plan: Plan::default(),
        narrowed: BTreeMap::new(),
        out_of_scope: BTreeSet::new(),
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
        isolation: vec![Isolation::Region; 3],
        parallelism: Parallelism::default(),
        plan: Plan::default(),
        narrowed: BTreeMap::new(),
        out_of_scope: BTreeSet::new(),
    };
    let report = program.run(&selection, &mut store);
    assert_eq!((report.passed, report.failed), (1, 0));
    assert_eq!(report.warnings.len(), 1);
    assert!(report.warnings[0].message.contains("99"));
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
    let selection = program.select(&store);
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

    // One group per test forces the sequential path, so the worker that unwound is the one asked to
    // run the next test.
    let selection = Selection {
        total: 3,
        cached: Vec::new(),
        to_run: vec![0, 1, 2],
        groups: vec![vec![0], vec![1], vec![2]],
        reasons: vec![Reason::New; 3],
        isolation: vec![Isolation::Region; 3],
        parallelism: Parallelism::default(),
        plan: Plan::default(),
        narrowed: BTreeMap::new(),
        out_of_scope: BTreeSet::new(),
    };
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

/// The reverse of the recursion-limit misclassification, and the more expensive direction: a defect
/// in Ply reported as an ordinary red test is a bug the user goes looking for in their own code,
/// and a suspect set invents a culprit for something no change in the program caused.
#[test]
fn an_internal_error_is_a_defect_in_ply_rather_than_a_red_test() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ARITHMETIC);
    let doomed = program.index_of("mul is right");
    let selection = program.select(&store);

    let executor = InternalErrorExecutor { fail_on: doomed };
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
        .find(|r| r.index == doomed)
        .expect("reported");
    assert_eq!(result.status, Status::Panicked);
    let failure = &report.failures[0];
    assert!(failure.defect);
    assert_eq!(
        failure.attribution.bisection.verdict,
        crate::Verdict::NotAttempted(crate::Skipped::Panicked)
    );
}

const RUNAWAY: &str = r#"
fn step(n: Int) -> Int = n + 1
fn spin(n: Int) -> Int = spin(step(n))

test "spins" { assert_eq(spin(0), 0) }
"#;

/// Exceeding a documented resource limit is the program's behaviour, not Ply falling over.
#[test]
fn a_runaway_recursion_is_a_red_test_and_not_a_defect_in_ply() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(RUNAWAY);
    let selection = program.select(&store);
    let report = program.run(&selection, &mut store);

    assert_eq!(report.failed, 1);
    assert_eq!(report.results[0].status, Status::Failed);

    let failure = &report.failures[0];
    assert!(!failure.defect, "{:#?}", failure.diagnostic);
    assert_eq!(failure.diagnostic.code, ply_span::codes::RUNTIME_ERROR);
    assert!(
        failure.diagnostic.message.contains("recursion limit"),
        "{}",
        failure.diagnostic.message
    );
    assert_ne!(
        failure.attribution.bisection.verdict,
        crate::Verdict::NotAttempted(crate::Skipped::Panicked),
        "bisection was suppressed for a perfectly bisectable failure"
    );
}

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
    assert_eq!(
        failure["diagnostic"]["code"],
        ply_span::codes::ASSERTION_FAILED
    );
    // Only the expectation inside the test moved, so no definition is under suspicion — the test is
    // the thing that changed.
    assert_eq!(failure["suspects"], serde_json::json!([]));

    assert_eq!(json["tests"][0]["status"], "failed");
    assert!(
        json["tests"][0]["hash"]
            .as_str()
            .is_some_and(|h| h.len() == 64)
    );

    let summary = report.summary();
    assert!(
        summary[0].starts_with("1 failed, 0 passed, 1 cached"),
        "{summary:#?}"
    );
    assert!(
        summary.iter().any(|l| l.contains("expected 7, found 6")),
        "{summary:#?}"
    );
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
    assert!(
        summary.iter().any(|l| l == "  suspects: debit, settle"),
        "{summary:#?}"
    );
}

#[test]
fn explain_covers_every_test_and_every_group() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ARITHMETIC);

    let selection = program.select(&store);
    let lines = selection.explain(&program.check, &program.hashes);
    let per_test = |lines: &[String]| {
        lines
            .iter()
            .filter(|l| l.starts_with("run ") || l.starts_with("skip"))
            .count()
    };
    assert_eq!(per_test(&lines), selection.total);
    assert_eq!(
        lines.iter().filter(|l| l.starts_with("group ")).count(),
        selection.groups.len()
    );
    assert!(
        lines.iter().take(3).all(|l| l.starts_with("run ")),
        "{lines:#?}"
    );
    assert!(
        lines.iter().any(|l| l.starts_with("isolated: 3 of 3")),
        "{lines:#?}"
    );

    program.run(&selection, &mut store);
    let selection = program.select(&store);
    let lines = selection.explain(&program.check, &program.hashes);
    assert_eq!(per_test(&lines), 3);
    assert!(
        lines.iter().take(3).all(|l| l.starts_with("skip")),
        "{lines:#?}"
    );

    let json = selection.to_json(&program.check, &program.hashes);
    assert_eq!(json["selected"], 0);
    assert_eq!(json["cached"], 3);
    assert_eq!(json["tests"][0]["reason"], "cached");
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
fn a_module_whose_tests_are_all_isolated_runs_as_a_single_group() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(DISJOINT_CELLS);

    // `with_cell` discharges its atoms at the region boundary, so each of these tests is observably
    // pure and they all share one group.
    assert!(program.check.tests.iter().all(|t| t.footprint.is_empty()));

    let selection = program.select(&store);
    assert_eq!(selection.groups, vec![vec![0, 1, 2, 3]]);

    let report = program.run(&selection, &mut store);
    assert_eq!(
        (report.passed, report.failed),
        (4, 0),
        "{:#?}",
        report.failures
    );
    assert!(report.results.iter().all(|r| r.group == 0));
}

/// A `cell` atom surviving into a test's footprint needs a continuation captured inside a
/// `with_cell` region, which no program can write until the machine lands — so the footprint is
/// injected rather than inferred.
fn with_footprint(program: &mut Program, name: &str, footprint: Footprint) {
    let index = program.index_of(name);
    program.check.tests[index].footprint = footprint;
}

/// ADR 0017 §6's lost case, end to end on the real runner: two tests whose only atoms name one
/// label used to share a group and are coloured apart now.
#[test]
fn two_tests_retaining_the_same_cell_resource_are_coloured_apart() {
    let root = TempRoot::new();
    let mut store = root.store();
    let mut program = Program::compile(DISJOINT_CELLS);
    with_footprint(&mut program, "users cell", cells(&["users"]));
    with_footprint(&mut program, "orders cell", cells(&["users"]));

    let selection = program.select(&store);
    assert_eq!(selection.groups, vec![vec![0, 2, 3], vec![1]]);
    assert_eq!(selection.parallelism.isolated, 2);
    assert_eq!(selection.parallelism.region_contended, 2);
    assert_eq!(selection.parallelism.shared_groups, 2);
    assert!(selection.parallelism.holds(), "{:?}", selection.parallelism);

    let report = program.run(&selection, &mut store);
    assert_eq!(
        (report.passed, report.failed),
        (4, 0),
        "{:#?}",
        report.failures
    );
}

/// Two labels nobody shares are still one group.
#[test]
fn two_tests_on_distinct_cell_resources_still_run_in_one_group() {
    let root = TempRoot::new();
    let mut store = root.store();
    let mut program = Program::compile(DISJOINT_CELLS);
    with_footprint(&mut program, "users cell", cells(&["users"]));
    with_footprint(&mut program, "orders cell", cells(&["orders"]));

    let selection = program.select(&store);
    assert_eq!(selection.groups, vec![vec![0, 1, 2, 3]]);
    assert_eq!(selection.parallelism.region_contended, 2);
    assert_eq!(selection.parallelism.shared_groups, 1);

    let report = program.run(&selection, &mut store);
    assert_eq!(
        (report.passed, report.failed),
        (4, 0),
        "{:#?}",
        report.failures
    );
    assert!(report.results.iter().all(|r| r.group == 0));
}

#[test]
fn a_test_that_reaches_a_real_resource_is_still_serialized_against_its_writer() {
    let root = TempRoot::new();
    let store = root.store();
    let mut program = Program::compile(DISJOINT_CELLS);
    with_footprint(
        &mut program,
        "users cell",
        cells(&["users"]).union(&writes(&["accounts"])),
    );
    with_footprint(&mut program, "orders cell", reads(&["accounts"]));
    with_footprint(&mut program, "pure one", cells(&["accounts"]));

    let selection = program.select(&store);
    assert_eq!(selection.groups.len(), 2);
    assert!(
        selection.group_of(0) != selection.group_of(1),
        "a real write and a real read of `accounts` must not share a group: {:?}",
        selection.groups
    );
    assert_eq!(selection.group_of(2), Some(0));
    assert_eq!(selection.group_of(3), Some(0));

    let p = &selection.parallelism;
    assert_eq!((p.total, p.isolated, p.shared), (4, 1, 3));
    assert_eq!(
        p.region_contended, 1,
        "`pure one` names a label and nothing else"
    );
    assert_eq!((p.groups, p.shared_groups), (2, 2));
    assert!(p.holds(), "{p:?}");
}

/// The artifact's numbers are the footprints' numbers, not a second opinion.
#[test]
fn the_artifact_reports_isolation_per_test_and_in_total() {
    let root = TempRoot::new();
    let store = root.store();
    let mut program = Program::compile(DISJOINT_CELLS);
    with_footprint(&mut program, "users cell", cells(&["users"]));
    with_footprint(
        &mut program,
        "orders cell",
        cells(&["orders"]).union(&writes(&["ledger"])),
    );

    let selection = program.select(&store);
    let json = selection.to_json(&program.check, &program.hashes);
    assert_eq!(json["isolated"], 2);
    assert_eq!(json["parallelism"]["total"], 4);
    assert_eq!(json["parallelism"]["shared"], 2);
    assert_eq!(json["parallelism"]["region_contended"], 1);
    assert_eq!(json["parallelism"]["shared_groups"], 1);
    assert_eq!(json["tests"][0]["isolation"], "shared");
    assert_eq!(json["tests"][0]["shared_atoms"][0], "cell.write[users]");
    assert_eq!(json["tests"][1]["isolation"], "shared");
    assert_eq!(json["tests"][1]["shared_atoms"][1], "db.write[ledger]");
    assert_eq!(json["tests"][2]["isolation"], "region");
    assert_eq!(
        json["tests"][2]["shared_atoms"].as_array().unwrap().len(),
        0
    );

    for (index, test) in program.check.tests.iter().enumerate() {
        let reported = json["tests"][index]["isolation"].as_str().unwrap();
        let expected = if crate::region_isolated(&test.footprint) {
            "region"
        } else {
            "shared"
        };
        assert_eq!(
            reported, expected,
            "test {index} disagrees with its footprint"
        );
    }

    let lines = selection.explain(&program.check, &program.hashes);
    assert!(
        lines.iter().any(|l| l.starts_with("isolated: 2 of 4")),
        "{lines:#?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("isolation: shared {cell.write[users]} (region labels)")),
        "a test that contends only over a label must say so: {lines:#?}"
    );
    assert!(
        lines.iter().any(|l| l
            .contains("isolation: shared {cell.write[orders], db.write[ledger]}")
            && !l.contains("(region labels)")),
        "a test that also reaches a real resource must not be blamed on its label: {lines:#?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("1 of the 2 contend only over a region label")),
        "the cost of losing the fork is reported per run: {lines:#?}"
    );

    let mut store = root.store();
    let summary = program.run(&selection, &mut store).summary();
    assert!(
        summary.iter().any(|l| l == "isolated: 2 of 4"),
        "{summary:#?}"
    );
}

/// The region ADR 0017 §6 asks `ply-test` for: a worker outlives a single test, so a test that
/// inherited the previous one's cells would be sharing state through the back door the whole design
/// exists to close.
#[test]
fn a_test_region_closes_and_the_group_fixture_does_not() {
    let program = Program::compile(DISJOINT_CELLS);
    let built = std::sync::atomic::AtomicUsize::new(0);
    let fixture: &(dyn Fn(&mut TaskRegions) -> Value + Sync) = &|regions: &mut TaskRegions| {
        built.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Value::Cell(regions.alloc_cell(Value::Int(7)))
    };
    let executor = crate::InterpExecutor::new(&program.program, &program.resolved, &program.check)
        .with_fixture(fixture);

    let mut worker = executor.worker();
    assert_eq!(built.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(worker.region().mark(), 1);
    assert_eq!(
        worker.cells().live(),
        1,
        "the worker starts inside the group's region"
    );

    executor
        .execute(&mut worker, 0)
        .expect("the first test passes");
    let allocated = worker.cells().stats().allocations;
    assert!(allocated > 1, "the test allocated a cell of its own");
    assert_eq!(
        worker.cells().live(),
        1,
        "and gave it back at its region's close, leaving the group's fixture"
    );
    assert_eq!(
        worker.region().fixture().len(),
        1,
        "closing the test's region left the group's own state alone"
    );
    assert_eq!(worker.region().mark(), 1);

    executor
        .execute(&mut worker, 1)
        .expect("the second test passes");
    assert_eq!(
        worker.cells().stats().allocations,
        allocated,
        "the second test bumped as many slots as the first, from the same mark"
    );
    assert_eq!(
        worker.cells().live(),
        1,
        "the second test opened the region, not the first test's leftovers"
    );
    let (seeded, handle) = worker.region().open();
    let seed = match handle {
        Value::Cell(slot) => seeded.get(slot).cloned(),
        other => panic!("expected the fixture's handle, found {other:?}"),
    };
    assert!(
        matches!(seed, Some(Value::Int(7))),
        "every test still sees the seeded state"
    );
    assert_eq!(
        built.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "the fixture is built once for the worker, not once per test"
    );
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

/// The rich suspect list and the flat one are two views of one set, so a consumer that reads either
/// gets the same answer about *what* is suspect.
#[test]
fn the_attribution_covers_exactly_the_suspect_set() {
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
    let report = red.run(&selection, &mut store);

    let failure = &report.failures[0];
    let mut rich: Vec<&str> = failure
        .attribution
        .suspects
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    rich.sort_unstable();
    let flat: Vec<&str> = failure.suspects.iter().map(|s| s.as_str()).collect();
    assert_eq!(rich, flat);
    assert!(
        failure
            .attribution
            .suspects
            .iter()
            .all(|s| s.hash.is_some())
    );
    // Nothing has been compared or traced yet, so every judgement is withheld rather than guessed
    // at.
    assert!(failure.attribution.suspects.iter().all(|s| s.ran.is_none()));
    assert!(
        failure
            .attribution
            .suspects
            .iter()
            .all(|s| s.before.is_none())
    );
    assert!(failure.attribution.slice.is_none());
}

#[test]
fn a_run_that_did_not_bisect_says_so_rather_than_naming_nobody() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program =
        Program::compile(&LEDGER.replace("assert_eq(settle(0), 6)", "assert_eq(settle(0), 7)"));
    let selection = program.select(&store);
    let report = program.run(&selection, &mut store);

    let bisection = &report.failures[0].attribution.bisection;
    assert_eq!(
        bisection.verdict,
        crate::Verdict::NotAttempted(crate::Skipped::NotRequested)
    );
    assert_eq!(bisection.confidence, crate::Confidence::None);
    assert!(bisection.culprits().is_empty());
    assert!(!bisection.is_conclusive());
}

/// A `test/nondet` outcome is not a function of the definition set, so the artifact has to say the
/// question was not asked rather than leave a consumer to infer it from an empty culprit list.
#[test]
fn a_nondet_failure_is_marked_unbisectable_at_the_point_it_fails() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(
        "nondet effect wall {\n  read now() -> Int\n}\n\
         test/nondet \"clock is negative\" { assert(wall.now() < 0) }\n",
    );
    let selection = program.select(&store);
    let report = program.run(&selection, &mut store);

    assert_eq!(report.failed, 1);
    assert_eq!(
        report.failures[0].attribution.bisection.verdict,
        crate::Verdict::NotAttempted(crate::Skipped::Nondet)
    );
}

#[test]
fn resolving_an_attribution_ranks_the_culprit_first_and_marks_what_ran() {
    use crate::bisect::{Bisection, Confidence, SearchStats, Verdict};
    use crate::slice::{CausalSlice, Entered, Frame};

    let hashes = HashOutput::default();
    let names = [
        Symbol::new("m.formats"),
        Symbol::new("m.debit"),
        Symbol::new("m.settle"),
    ];
    let mut attribution = crate::Attribution::from_suspects(&names, &hashes);

    let frame = |name: &str| Frame {
        name: Symbol::new(name),
        hash: None,
        call_site: ply_span::Span::new(SourceId(0), 0, 1),
    };
    let slice = CausalSlice {
        traced: true,
        reproduced: true,
        entered: ["m.settle", "m.debit"]
            .iter()
            .map(|n| Entered {
                name: Symbol::new(n),
                hash: None,
                calls: 1,
            })
            .collect(),
        stack: vec![frame("m.settle"), frame("m.debit")],
        observed: Footprint::empty(),
        truncated: false,
    };
    attribution.resolve(
        Bisection {
            verdict: Verdict::Bisected,
            confidence: Confidence::Minimal,
            groups: vec![vec![Symbol::new("m.debit")]],
            reason: "narrowed 3 changed definitions to m.debit".into(),
            search: SearchStats::default(),
        },
        Some(slice),
    );

    let order: Vec<&str> = attribution
        .suspects
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(order, ["m.debit", "m.settle", "m.formats"]);
    assert!(attribution.suspects[0].culprit);
    assert_eq!(attribution.suspects[0].depth, Some(0));
    assert_eq!(attribution.suspects[1].depth, Some(1));
    assert_eq!(attribution.suspects[2].ran, Some(false));
    assert_eq!(attribution.culprits(), vec![Symbol::new("m.debit")]);
}

/// A definition can be a cause without the store having noticed it change — dropping it because it
/// is not in the suspect set would discard the answer.
#[test]
fn a_culprit_outside_the_suspect_set_is_added_rather_than_dropped() {
    let mut attribution =
        crate::Attribution::from_suspects(&[Symbol::new("m.a")], &HashOutput::default());
    attribution.resolve(
        crate::Bisection {
            verdict: crate::Verdict::Bisected,
            confidence: crate::Confidence::Minimal,
            groups: vec![vec![Symbol::new("m.z")]],
            reason: String::new(),
            search: crate::SearchStats::default(),
        },
        None,
    );
    let names: Vec<&str> = attribution
        .suspects
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(names, ["m.z", "m.a"]);
    assert!(attribution.suspects[0].culprit);
}

#[test]
fn the_summary_leads_with_the_culprit_and_the_artifact_carries_the_verdict() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program =
        Program::compile(&LEDGER.replace("assert_eq(settle(0), 6)", "assert_eq(settle(0), 7)"));
    let selection = program.select(&store);
    let mut report = program.run(&selection, &mut store);

    report.failures[0].attribution.resolve(
        crate::Bisection {
            verdict: crate::Verdict::Bisected,
            confidence: crate::Confidence::Minimal,
            groups: vec![vec![Symbol::new("debit")]],
            reason: "narrowed 2 changed definitions to debit in 2 runs (1 answered from the cache)"
                .into(),
            search: crate::SearchStats {
                evaluated: 2,
                cached: 1,
                ..Default::default()
            },
        },
        None,
    );

    let summary = report.summary();
    let culprit = summary
        .iter()
        .position(|l| l.contains("culprit: debit"))
        .expect("a culprit line");
    let diff = summary
        .iter()
        .position(|l| l.contains("expected 7"))
        .expect("the assertion line");
    assert!(
        culprit < diff,
        "the culprit must come before the diff:\n{summary:#?}"
    );

    let json = report.to_json();
    assert_eq!(json["schema_version"], crate::report::SCHEMA_VERSION);
    let failure = &json["failures"][0];
    assert_eq!(failure["culprit"]["verdict"], "bisected");
    assert_eq!(failure["culprit"]["confidence"], "minimal");
    assert_eq!(
        failure["culprit"]["definitions"],
        serde_json::json!(["debit"])
    );
    assert_eq!(failure["culprit"]["search"]["evaluated"], 2);
    assert_eq!(failure["culprit"]["skipped"], serde_json::Value::Null);
    assert!(failure["causal_slice"].is_null());
    assert!(failure["assertion"].is_null());
}

use ply_eval::{Exploration, Naive, Race, RaceSite, Seed, SimMode};

/// An executor that reports a search without running one, so every cache rule in this section is
/// exercised against the outcomes a real scheduler produces without waiting for one.
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

fn passed(store: &Store, key: ply_hash::DefHash) -> bool {
    matches!(store.get(key), Some(Outcome::Pass))
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

/// `simulate` is not yet something a fixture can write, and none of the rules below are about the
/// source that produced the atom.
fn make_seeded(program: &mut Program, name: &str) -> usize {
    let index = program.index_of(name);
    program.check.tests[index].footprint =
        program.check.tests[index]
            .footprint
            .union(&Footprint::from_atoms([atom(
                crate::SIM_EFFECT,
                None,
                Mode::Read,
            )]));
    index
}

fn seeded_program() -> (Program, usize) {
    let mut program = Program::compile(ARITHMETIC);
    let index = make_seeded(&mut program, "mul is right");
    (program, index)
}

/// The rule whose absence is silent: a run under one plan reading a pass another plan earned.
#[test]
fn a_seeded_test_is_never_written_under_its_bare_hash() {
    let root = TempRoot::new();
    let mut store = root.store();
    let (program, seeded) = seeded_program();
    let plan = Plan::default();

    let selection = program.select_under(&store, &plan);
    let executor = SimExecutor::new(&selection).exploring(seeded, exhaustive(12));
    let report = run_with(
        &selection,
        &program.check,
        &program.hashes,
        &mut store,
        &executor,
    );
    assert_eq!((report.passed, report.failed), (3, 0));

    let hash = program.hashes.tests[seeded];
    assert!(store.get(hash).is_none(), "the bare hash must stay empty");
    assert!(
        passed(&store, crate::sim_key(hash, &plan)),
        "the plan key is where the claim lives"
    );
    // Every other test is unaffected: nothing about the existing cache changes for a test whose row
    // never mentions a seed.
    let plain = program.hashes.tests[program.index_of("add is right")];
    assert!(passed(&store, plain));
}

#[test]
fn widening_the_budget_re_runs_a_seeded_test_and_changing_nothing_does_not() {
    let root = TempRoot::new();
    let mut store = root.store();
    let (program, seeded) = seeded_program();
    let narrow = Plan::default();

    let selection = program.select_under(&store, &narrow);
    let executor = SimExecutor::new(&selection).exploring(seeded, exhaustive(12));
    run_with(
        &selection,
        &program.check,
        &program.hashes,
        &mut store,
        &executor,
    );

    let again = program.select_under(&store, &narrow);
    assert!(
        again.to_run.is_empty(),
        "the same plan proved the same thing: {again:?}"
    );

    let wider = Plan {
        budget: narrow.budget * 2,
        ..narrow
    };
    let widened = program.select_under(&store, &wider);
    assert_eq!(
        widened.to_run,
        vec![seeded],
        "a wider search is a different claim and has to be made"
    );
    assert_eq!(widened.reason(seeded), Some(Reason::New));
}

/// A `random` root is a standalone claim, so widening a root set costs only the roots that are new.
#[test]
fn widening_a_random_root_set_runs_only_the_roots_nothing_answered_for() {
    let root = TempRoot::new();
    let mut store = root.store();
    let (program, seeded) = seeded_program();

    let four = Plan::random(4);
    let selection = program.select_under(&store, &four);
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

    let eight = Plan::random(8);
    let widened = program.select_under(&store, &eight);
    assert_eq!(widened.to_run, vec![seeded]);
    assert_eq!(
        widened.plan_for(seeded).roots,
        vec![4, 5, 6, 7],
        "the first four roots each hold a pass of their own"
    );

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
        Some(vec![4, 5, 6, 7]),
        "the run must search only what it owes"
    );
    // The widened plan's own key is what a third run reads, and it is published even though only
    // half the roots ran.
    assert!(program.select_under(&store, &eight).to_run.is_empty());
}

/// A `dpor` root's exploration does not decompose, so nothing about it can be lifted out of its
/// search.
#[test]
fn a_dpor_search_never_narrows_and_writes_no_per_root_key() {
    let root = TempRoot::new();
    let mut store = root.store();
    let (program, seeded) = seeded_program();
    let plan = Plan {
        mode: SimMode::Dpor,
        roots: vec![0, 1, 2, 3],
        ..Plan::default()
    };

    let selection = program.select_under(&store, &plan);
    let executor = SimExecutor::new(&selection).exploring(seeded, exhaustive(30));
    run_with(
        &selection,
        &program.check,
        &program.hashes,
        &mut store,
        &executor,
    );

    let hash = program.hashes.tests[seeded];
    for root in &plan.roots {
        assert!(
            store
                .get(crate::seed_key(hash, &Seed::root(*root)))
                .is_none(),
            "root {root} is not a standalone claim under dpor"
        );
    }
    assert!(passed(&store, crate::sim_key(hash, &plan)));

    let wider = Plan {
        roots: vec![0, 1, 2, 3, 4],
        ..plan
    };
    let widened = program.select_under(&store, &wider);
    assert_eq!(widened.to_run, vec![seeded]);
    assert!(
        widened.narrowed.is_empty(),
        "a dpor plan has nothing to narrow: {:?}",
        widened.narrowed
    );
}

/// The first green `det` test in the language that is not cacheable, and it is correct that it is
/// not.
#[test]
fn an_exhausted_search_reports_green_writes_nothing_and_re_runs() {
    let root = TempRoot::new();
    let mut store = root.store();
    let (program, seeded) = seeded_program();
    let plan = Plan::default();

    let selection = program.select_under(&store, &plan);
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
    assert_eq!(result.recorded, Some(crate::Record::Exhausted));

    let hash = program.hashes.tests[seeded];
    assert!(store.get(hash).is_none());
    assert!(store.get(crate::sim_key(hash, &plan)).is_none());
    assert_eq!(program.select_under(&store, &plan).to_run, vec![seeded]);
    assert!(report.simulation.line().unwrap().contains("not cached"));
}

/// Unchanged, and it has to stay unchanged for a seeded test too.
#[test]
fn a_simulated_failure_is_never_cached_under_any_key() {
    let root = TempRoot::new();
    let mut store = root.store();
    let (program, seeded) = seeded_program();
    let plan = Plan::random(2);
    let seed = Seed::at(0, vec![1, 0, 3]);

    let selection = program.select_under(&store, &plan);
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
    assert!(store.get(crate::sim_key(hash, &plan)).is_none());
    for root in &plan.roots {
        assert!(
            store
                .get(crate::seed_key(hash, &Seed::root(*root)))
                .is_none()
        );
    }
    assert_eq!(program.select_under(&store, &plan).to_run, vec![seeded]);
}

#[test]
fn a_failure_carries_the_seed_that_replays_it() {
    let root = TempRoot::new();
    let mut store = root.store();
    let (program, seeded) = seeded_program();
    let seed = Seed::at(0, vec![1, 0, 3]);

    let selection = program.select_under(&store, &Plan::default());
    let executor = SimExecutor::new(&selection)
        .exploring(
            seeded,
            Exploration {
                race: Some(Race {
                    left: RaceSite {
                        task: ply_eval::TaskId(1),
                        definition: Some(Symbol::new("apply_debit")),
                        access: "db.write[accounts]".into(),
                        span: ply_span::Span::DUMMY,
                    },
                    right: RaceSite {
                        task: ply_eval::TaskId(2),
                        definition: Some(Symbol::new("apply_debit")),
                        access: "db.write[accounts]".into(),
                        span: ply_span::Span::DUMMY,
                    },
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
        failure.replay().unwrap(),
        "ply test --seed 0:1.0.3 --filter \"mul is right\""
    );

    let summary = report.summary();
    assert!(summary.iter().any(|l| l.contains("seed: 0:1.0.3")));
    assert!(summary.iter().any(|l| l.contains("race: @1")));
    assert!(summary.iter().any(|l| l.contains("@2")));
    let replay = summary
        .iter()
        .position(|l| l.contains("replay: ply test --seed 0:1.0.3"))
        .expect("the artifact prints the command rather than describing it");
    assert!(replay > 0);

    let json = report.to_json();
    assert_eq!(json["schema_version"], 4);
    assert_eq!(json["failures"][0]["seed"], "0:1.0.3");
    assert_eq!(json["failures"][0]["race"]["left"]["task"], "@1");
    assert_eq!(json["failures"][0]["race"]["at"], 3);
    assert!(
        json["failures"][0]["replay"]
            .as_str()
            .unwrap()
            .contains("--seed 0:1.0.3")
    );
}

/// A field the run did not observe is never reported as though it had been.
#[test]
fn an_unsimulated_failure_carries_no_seed_and_no_race() {
    let root = TempRoot::new();
    let mut store = root.store();
    let program = Program::compile(ONE_RED);
    let selection = program.select(&store);
    let report = program.run(&selection, &mut store);

    assert_eq!(report.failed, 1);
    assert_eq!(report.failures[0].seed, None);
    assert_eq!(report.failures[0].race, None);
    assert!(report.failures[0].replay().is_none());
    assert!(!report.summary().iter().any(|l| l.contains("replay:")));

    let json = report.to_json();
    assert!(json["failures"][0]["seed"].is_null());
    assert!(json["failures"][0]["race"].is_null());
    assert!(json["simulation"]["simulated"] == 0);
}

/// A test whose row says it simulated and whose evaluator reported no search is a run nobody
/// watched.
#[test]
fn a_seeded_test_with_no_observed_search_warns_and_is_not_cached() {
    let root = TempRoot::new();
    let mut store = root.store();
    let (program, seeded) = seeded_program();
    let plan = Plan::default();

    let selection = program.select_under(&store, &plan);
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
            .get(crate::sim_key(program.hashes.tests[seeded], &plan))
            .is_none()
    );
    assert_eq!(
        report
            .results
            .iter()
            .find(|r| r.index == seeded)
            .and_then(|r| r.recorded.clone()),
        Some(crate::Record::Unobserved)
    );
    assert_eq!(report.warnings.len(), 1);
    assert!(report.warnings[0].message.contains("reported no search"));
    assert_eq!(program.select_under(&store, &plan).to_run, vec![seeded]);
}

#[test]
fn the_summary_counts_the_seeds_the_interleavings_and_the_exhaustive_searches() {
    let root = TempRoot::new();
    let mut store = root.store();
    let mut program = Program::compile(ARITHMETIC);
    let one = make_seeded(&mut program, "mul is right");
    let two = make_seeded(&mut program, "twice is right");
    let plan = Plan::random(4);

    let selection = program.select_under(&store, &plan);
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

    let json = report.to_json();
    assert_eq!(json["simulation"]["seeds"], 8);
    assert_eq!(json["simulation"]["interleavings"], 268);
    let simulated = json["tests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["index"] == one)
        .expect("reported");
    assert_eq!(simulated["simulation"]["explored"], 12);
    assert_eq!(simulated["simulation"]["exhaustive"], true);
    assert_eq!(simulated["cached"], true);
    // Absent, never zeroed: a consumer cannot tell an explored count of zero from a test that never
    // simulated.
    let plain = json["tests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["index"] == program.index_of("add is right"))
        .expect("reported");
    assert!(plain["simulation"].is_null());
}

#[test]
fn a_measured_reduction_is_reported_and_a_spent_naive_budget_is_a_lower_bound() {
    let root = TempRoot::new();
    let mut store = root.store();
    let (program, seeded) = seeded_program();

    let selection = program.select_under(&store, &Plan::default());
    let executor = SimExecutor::new(&selection).exploring(
        seeded,
        Exploration {
            naive: Some(Naive {
                explored: 720,
                bounded: false,
            }),
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

    let json = report.to_json();
    let simulated = json["tests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["index"] == seeded)
        .expect("reported");
    assert_eq!(simulated["simulation"]["naive"]["explored"], 720);
    assert_eq!(simulated["simulation"]["naive"]["rendered"], "720");
    assert_eq!(simulated["simulation"]["reduction"], 60.0);

    let bounded = Naive {
        explored: 4096,
        bounded: true,
    };
    assert_eq!(bounded.to_string(), ">= 4096");
}
