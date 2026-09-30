//! Region isolation, held to real programs: the atoms the front end leaves for `suite.schedule` to
//! colour, and the runtime running a class the program coloured without its tests observing each
//! other. How atoms become classes is the program's, and its tests pin it.

use crate::fixture::Compiled;
use ply_eval::{Footprint, SourceId, TaskRegions, Value};
use ply_store::Store;
use ply_test::{GroupRegion, Selection};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new() -> TempRoot {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ply-isolation-audit-{}-{}",
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

impl Compiled {
    fn rejected(src: &str) -> Vec<ply_eval::Diagnostic> {
        crate::fixture::port_diagnostics(&[(String::new(), src.to_string())], &[SourceId(0)])
    }
}

/// `suite.schedule`'s region-scoped and ambient effects: what it trusts an atom's effect name for.
const SCHEDULER_NAMES: [&str; 2] = ["cell", "sim"];

fn names_a_region(f: &Footprint) -> bool {
    f.atoms().any(|a| a.effect.as_str() == "cell")
}

/// Every test allocates its cell at the same id, writes it, and checks every value it wrote.
fn contending_source(tests: usize, label: impl Fn(usize) -> String) -> String {
    let mut out = String::new();
    let mut declared: Vec<String> = Vec::new();
    for i in 0..tests {
        let label = label(i);
        if !declared.contains(&label) {
            out.push_str(&format!(
                "\nfn touches_{label}(n: Int) -> Int / {{cell.read[{label}], \
                 cell.write[{label}]}} = n\n"
            ));
            declared.push(label.clone());
        }
        out.push_str(&format!(
            r#"
test "contender {label} {i}" {{
  let seen = with_cell[{label}]({i}) {{ c -> {{
    assert_eq(cell_get(c), {i});
    cell_set(c, cell_get(c) * 7);
    assert_eq(cell_get(c), {seven});
    cell_set(c, cell_get(c) + {i});
    cell_get(c)
  }} }};
  assert_eq(touches_{label}(seen), {eight})
}}
"#,
            i = i,
            label = label,
            seven = i * 7,
            eight = i * 8,
        ));
    }
    out
}

fn one_label(_: usize) -> String {
    "table".to_string()
}

fn a_label_each(i: usize) -> String {
    format!("table{i}")
}

#[test]
fn a_program_cannot_declare_either_effect_the_scheduler_names() {
    for name in SCHEDULER_NAMES {
        let diags = Compiled::rejected(&format!(
            r#"
effect {name} {{
  write put[users](v: Int) -> Unit
}}

test "claim the name" {{
  {name}.put[users](1)
}}
"#
        ));
        assert!(
            !diags.is_empty(),
            "`effect {name}` must be refused, or the scheduler's classification is claimable"
        );
        let said = diags
            .iter()
            .flat_map(|d| {
                std::iter::once(d.message.clone())
                    .chain(d.labels.iter().map(|l| l.message.clone()))
                    .chain(d.notes.iter().cloned())
            })
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(
            said.contains("builtin") || said.contains("declared by the language"),
            "the refusal must say the name belongs to the language: {said}"
        );
    }
}

#[test]
fn an_effect_whose_name_merely_resembles_the_builtin_names_no_region() {
    let compiled = Compiled::anonymous(
        r#"
effect cells {
  write put[rows](v: Int) -> Unit
}

test "one" { cells.put[rows](1) }

test "two" { cells.put[rows](2) }
"#,
    );
    let footprints = compiled.footprints();
    for f in &footprints {
        assert!(!f.is_empty(), "`cells` names a real resource: {f:?}");
        assert!(!names_a_region(f), "{f:?}");
    }
    assert!(
        footprints[0].conflicts_with(&footprints[1]),
        "two writers of one resource must never share a class"
    );
}

#[test]
fn tests_naming_one_region_label_all_conflict() {
    let compiled = Compiled::anonymous(&contending_source(6, one_label));
    let footprints = compiled.footprints();
    assert!(
        footprints.iter().all(names_a_region),
        "the corpus must retain cell atoms, or it is not exercising anything"
    );
    for (i, a) in footprints.iter().enumerate() {
        for b in &footprints[i + 1..] {
            assert!(a.conflicts_with(b), "{a:?} vs {b:?}");
        }
    }
}

#[test]
fn tests_on_distinct_region_labels_never_conflict() {
    let compiled = Compiled::anonymous(&contending_source(16, a_label_each));
    let footprints = compiled.footprints();
    assert!(footprints.iter().all(names_a_region));
    for (i, a) in footprints.iter().enumerate() {
        for b in &footprints[i + 1..] {
            assert!(!a.conflicts_with(b), "{a:?} vs {b:?}");
        }
    }
}

#[test]
fn a_cell_atom_beside_a_real_one_does_not_launder_the_real_one() {
    let compiled = Compiled::anonymous(
        r#"
effect db {
  read  get[users]() -> Int
  write put[users](v: Int) -> Unit
}

fn touches(n: Int) -> Int / {cell.read[table]} = n

test "cell only" {
  let seen = with_cell[table](1) { c -> cell_get(c) };
  assert_eq(touches(seen), 1)
}

test "cell and a real write" {
  let seen = with_cell[table](1) { c -> cell_get(c) };
  db.put[users](touches(seen))
}

test "a real read" {
  assert_eq(db.get[users](), 0)
}
"#,
    );

    let footprints = compiled.footprints();
    let atoms: Vec<String> = footprints[1].atoms().map(|a| a.to_string()).collect();
    assert_eq!(
        atoms,
        vec!["cell.read[table]".to_string(), "db.put[users]".to_string()]
    );
    assert!(
        footprints[1].conflicts_with(&footprints[2]),
        "a writer and a reader of `users`"
    );
    assert!(
        !footprints[0].conflicts_with(&footprints[1]),
        "a region label is readers-writers like any resource: two readers of `cell[table]` \
         do not conflict"
    );
}

#[test]
fn a_class_of_isolated_tests_running_at_once_never_observe_each_other() {
    const TESTS: usize = 32;
    let compiled = Compiled::new(&contending_source(TESTS, a_label_each));

    assert!(
        compiled.footprints().iter().all(names_a_region),
        "the corpus must retain cell atoms by inference, not by injection"
    );

    let unit = compiled.tier();
    for round in 0..3 {
        let root = TempRoot::new();
        let mut store = root.store();
        let selection = compiled.every();
        let executor = ply_test::InterpExecutor::new(&compiled.port)
            .with_backend(unit)
            .with_search(ply_test::Search::of(&selection))
            .with_hosts(ply_test::Hosting::hermetic());
        let report = ply_test::run_with(
            &selection,
            &compiled.check,
            &compiled.hashes,
            &mut store,
            &executor,
        );
        assert_eq!(
            (report.passed, report.failed),
            (TESTS, 0),
            "round {round}: {:#?}",
            report.failures
        );
        assert!(report.results.iter().all(|r| r.group == 0));
    }
}

#[test]
fn the_group_fixture_is_built_once_and_carries_each_tests_write_to_the_next() {
    const TESTS: usize = 12;
    let compiled = Compiled::anonymous(&contending_source(TESTS, a_label_each));
    let root = TempRoot::new();
    let mut store = root.store();
    let selection = compiled.every();

    let executor = FixtureProbe::default();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .expect("a one-worker pool");
    let report = pool.install(|| {
        ply_test::run_with(
            &selection,
            &compiled.check,
            &compiled.hashes,
            &mut store,
            &executor,
        )
    });
    assert_eq!((report.passed, report.failed), (TESTS, 0));

    assert_eq!(
        executor.built.load(Ordering::Relaxed),
        1,
        "one group and one worker is one fixture build"
    );

    let mut seen = executor.seen.into_inner().expect("no worker panicked");
    seen.sort_by_key(|o| o.index);
    for (n, o) in seen.iter().enumerate() {
        assert_eq!(o.mark, 1, "the region's mark moved: {o:?}");
        assert_eq!(o.fixture_len, 1, "the region grew by a test's own cells");
        let expected = if n == 0 { -1 } else { seen[n - 1].index as i64 };
        assert_eq!(
            o.observed_at_open, expected,
            "test {} did not open on the previous test's write",
            o.index
        );
    }
}

#[derive(Debug)]
struct Observation {
    index: usize,
    observed_at_open: i64,
    mark: usize,
    fixture_len: usize,
}

/// Reads the fixture, writes it, and allocates its own cell: everything a real test does to a region.
#[derive(Default)]
struct FixtureProbe {
    built: AtomicUsize,
    seen: Mutex<Vec<Observation>>,
}

impl ply_test::Executor for FixtureProbe {
    type Worker = GroupRegion;

    fn worker(&self) -> GroupRegion {
        self.built.fetch_add(1, Ordering::Relaxed);
        GroupRegion::build(|regions: &mut TaskRegions| {
            Value::Cell(regions.alloc_cell(Value::Int(-1)))
        })
    }

    fn execute(&self, region: &mut GroupRegion, index: usize) -> Result<(), ply_eval::Diagnostic> {
        let (mut stack, handle) = region.open();
        let seed = match handle {
            Value::Cell(slot) => slot,
            other => panic!("expected the fixture's handle, found {other:?}"),
        };
        let observed_at_open = match stack.get(seed) {
            Some(Value::Int(i)) => *i,
            other => panic!("the fixture cell is gone: {other:?}"),
        };
        for i in 0..4 {
            stack.alloc_cell(Value::Int(i));
        }
        assert!(stack.set(seed, Value::Int(index as i64)));
        assert!(region.close(&stack), "the stack came from this region");
        self.seen
            .lock()
            .expect("no worker panicked")
            .push(Observation {
                index,
                observed_at_open,
                mark: region.mark(),
                fixture_len: region.fixture().len(),
            });
        Ok(())
    }
}

#[test]
fn a_group_spread_over_eight_workers_gets_one_fixture_each() {
    const TESTS: usize = 24;
    const JOBS: usize = 8;
    let compiled = Compiled::anonymous(&contending_source(TESTS, a_label_each));
    let root = TempRoot::new();
    let mut store = root.store();
    let selection = compiled.every();

    let executor = FixtureProbe::default();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(JOBS)
        .build()
        .expect("an eight-worker pool");
    let report = pool.install(|| {
        ply_test::run_with(
            &selection,
            &compiled.check,
            &compiled.hashes,
            &mut store,
            &executor,
        )
    });
    assert_eq!((report.passed, report.failed), (TESTS, 0));

    let builds = executor.built.load(Ordering::Relaxed);
    assert!(
        (1..=JOBS).contains(&builds),
        "a group is served by at most one region per worker: {builds} builds at {JOBS} jobs"
    );

    let seen = executor.seen.into_inner().expect("no worker panicked");
    assert_eq!(seen.len(), TESTS);
    for o in &seen {
        assert_eq!(o.mark, 1, "the region's mark moved: {o:?}");
        assert_eq!(o.fixture_len, 1, "the region grew by a test's own cells");
        assert!(
            o.observed_at_open == -1 || (0..TESTS as i64).contains(&o.observed_at_open),
            "a test opened on a value no test and no seed ever wrote: {o:?}"
        );
    }
    assert_eq!(
        seen.iter().filter(|o| o.observed_at_open == -1).count(),
        builds,
        "exactly one test per worker opens on the seed, and the rest open on a \
         previous test's write to that worker's own region"
    );
}

#[test]
fn verdicts_do_not_move_between_one_worker_and_eight() {
    const SHARING: usize = 6;
    let source = format!(
        "{}{}{}",
        contending_source(SHARING, one_label),
        contending_source(10, |i| format!("own{i}")),
        (0..8)
            .map(|i| format!("\ntest \"pure {i}\" {{ assert_eq({i} + 1, {}) }}\n", i + 1))
            .collect::<String>()
    );
    let compiled = Compiled::new(&source);
    let unit = compiled.tier();
    // As a program colours it: each test sharing `table` in a class of its own, the rest together.
    let classes = {
        let mut first = vec![0];
        first.extend(SHARING..compiled.check.tests.len());
        let mut classes = vec![first];
        classes.extend((1..SHARING).map(|i| vec![i]));
        classes
    };

    let run_at = |jobs: usize| {
        let root = TempRoot::new();
        let mut store = root.store();
        let selection = Selection {
            groups: classes.clone(),
            ..compiled.every()
        };
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(jobs)
            .build()
            .expect("the worker pool");
        let executor = ply_test::InterpExecutor::new(&compiled.port)
            .with_backend(unit)
            .with_search(ply_test::Search::of(&selection))
            .with_hosts(ply_test::Hosting::hermetic());
        let report = pool.install(|| {
            ply_test::run_with(
                &selection,
                &compiled.check,
                &compiled.hashes,
                &mut store,
                &executor,
            )
        });
        let mut verdicts: Vec<(usize, ply_test::Status, usize)> = report
            .results
            .iter()
            .map(|r| (r.index, r.status, r.group))
            .collect();
        verdicts.sort_by_key(|v| v.0);
        (report.passed, report.failed, verdicts)
    };

    let one = run_at(1);
    let eight = run_at(8);

    assert_eq!(
        one.1, 0,
        "the corpus must be green before it proves anything"
    );
    assert_eq!((one.0, one.1), (eight.0, eight.1));
    assert_eq!(
        one.2, eight.2,
        "a verdict moved between one worker and eight"
    );
}
