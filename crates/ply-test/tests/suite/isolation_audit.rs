//! An attack on the scheduler's claim under region isolation: that a test's allocations live in a region
//! closed when the test ends, so tests still cannot observe each other's allocations — while a
//! region *label* two tests both write is one piece of state and colours them apart.

use ply_core::{CheckOutput, Footprint};
use ply_eval::{Plan, TaskRegions, Value};
use ply_hash::HashOutput;
use ply_span::SourceId;
use ply_store::Store;
use ply_syntax::resolve::Resolved;
use ply_test::{
    GroupRegion, Isolation, contends_only_over_regions, group_by_conflict, is_region_scoped,
    region_isolated, shared_footprint,
};
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

struct Compiled {
    program: ply_syntax::ast::Program,
    resolved: Resolved,
    check: CheckOutput,
    hashes: HashOutput,
}

impl Compiled {
    fn new(src: &str) -> Compiled {
        let module = ply_syntax::parse(SourceId(0), src).expect("the fixture must parse");
        let mut program = ply_syntax::ast::Program::single(module);
        let resolved = ply_syntax::resolve(&mut program)
            .unwrap_or_else(|d| panic!("the fixture must resolve: {d:#?}"));
        let check = ply_core::check_program(&program, &resolved)
            .unwrap_or_else(|d| panic!("the fixture must typecheck: {d:#?}"));
        let hashes = ply_hash::hash_program(&program, &resolved, &check)
            .unwrap_or_else(|d| panic!("the fixture must hash: {d:#?}"));
        Compiled {
            program,
            resolved,
            check,
            hashes,
        }
    }

    fn rejected(src: &str) -> Vec<ply_span::Diagnostic> {
        let module = ply_syntax::parse(SourceId(0), src).expect("the fixture must parse");
        let mut program = ply_syntax::ast::Program::single(module);
        let Ok(resolved) = ply_syntax::resolve(&mut program) else {
            return Vec::new();
        };
        ply_core::check_program(&program, &resolved)
            .err()
            .unwrap_or_default()
    }

    fn footprints(&self) -> Vec<&Footprint> {
        self.check.tests.iter().map(|t| &t.footprint).collect()
    }

    fn scheduled(&self) -> Vec<(usize, Footprint)> {
        self.footprints()
            .iter()
            .enumerate()
            .map(|(i, f)| (i, (*f).clone()))
            .collect()
    }
}

/// Each test allocates its cell at the same id as every other one, reads and writes it, and checks
/// every value it wrote.
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

/// `is_region_scoped` and `is_ambient` both trust one effect name.
#[test]
fn a_program_cannot_declare_either_effect_the_scheduler_names() {
    for name in ply_test::REGION_SCOPED.iter().chain(ply_test::AMBIENT) {
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

/// The classification is by exact effect name, so a neighbouring name gains nothing.
#[test]
fn an_effect_whose_name_merely_resembles_the_builtin_is_not_region_scoped() {
    let compiled = Compiled::new(
        r#"
effect cells {
  write put[rows](v: Int) -> Unit
}

test "one" { cells.put[rows](1) }

test "two" { cells.put[rows](2) }
"#,
    );
    for f in &compiled.footprints() {
        assert!(!region_isolated(f), "`cells` names a real resource: {f:?}");
        assert!(f.atoms().all(|a| !is_region_scoped(a)));
        assert!(!contends_only_over_regions(f));
    }
    assert_eq!(
        group_by_conflict(&compiled.scheduled()).len(),
        2,
        "two writers of one resource must not share a group"
    );
}

/// region isolation's lost case, reached by inference rather than by injection: six tests whose only
/// atoms name one label are six colours wide, and every one of them is `shared` on the line
/// `--explain` prints.
#[test]
fn tests_naming_one_region_label_are_coloured_apart_and_reported_as_shared() {
    let compiled = Compiled::new(&contending_source(6, one_label));
    let footprints = compiled.footprints();
    assert!(
        footprints.iter().all(|f| f.atoms().any(is_region_scoped)),
        "the corpus must retain cell atoms, or it is not exercising anything"
    );
    assert!(footprints.iter().all(|f| !region_isolated(f)));
    assert!(footprints.iter().all(|f| contends_only_over_regions(f)));
    assert!(
        footprints
            .iter()
            .all(|f| Isolation::of(f) == Isolation::Shared)
    );

    assert_eq!(group_by_conflict(&compiled.scheduled()).len(), 6);
}

/// A label nobody else names conflicts with nothing, so losing the fork bought it nothing to lose.
#[test]
fn tests_on_distinct_region_labels_still_share_one_group() {
    let compiled = Compiled::new(&contending_source(16, a_label_each));
    assert!(
        compiled
            .footprints()
            .iter()
            .all(|f| contends_only_over_regions(f))
    );
    assert_eq!(group_by_conflict(&compiled.scheduled()).len(), 1);
}

/// The classification subtracts atoms; it never subtracts tests.
#[test]
fn a_cell_atom_beside_a_real_one_does_not_launder_the_real_one() {
    let compiled = Compiled::new(
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
    let isolation: Vec<Isolation> = footprints.iter().map(|f| Isolation::of(f)).collect();
    assert_eq!(isolation, vec![Isolation::Shared; 3]);
    assert!(contends_only_over_regions(footprints[0]));
    assert!(
        !contends_only_over_regions(footprints[1]),
        "a test that also reaches `users` must not be blamed on its label"
    );

    let shared = shared_footprint(footprints[1]);
    let atoms: Vec<String> = shared.atoms().map(|a| a.to_string()).collect();
    assert_eq!(
        atoms,
        vec![
            "cell.read[table]".to_string(),
            "db.write[users]".to_string()
        ]
    );
    assert!(shared.conflicts_with(&shared_footprint(footprints[2])));

    let groups = group_by_conflict(&compiled.scheduled());
    assert_eq!(groups.len(), 2, "{groups:?}");
    let group_of = |t: usize| groups.iter().position(|g| g.contains(&t)).unwrap();
    assert_ne!(group_of(1), group_of(2), "a writer and a reader of `users`");
    assert_eq!(
        group_of(0),
        group_of(1),
        "a region label is readers-writers like any resource: two readers of \
         `cell[table]` still share a group"
    );
}

/// The colouring's own invariant, checked against the corpus rather than asserted about it: no two
/// tests sharing a group conflict at all.
#[test]
fn no_pair_in_a_group_conflicts_at_all() {
    let source = format!(
        r#"
effect db {{
  read  get[users]() -> Int
  write put[users](v: Int) -> Unit
  write log[audit](v: Int) -> Unit
}}

test "real reader" {{ assert_eq(db.get[users](), 0) }}

test "real writer" {{ db.put[users](1) }}

test "other writer" {{ db.log[audit](1) }}
{}
"#,
        contending_source(6, |i| format!("table{}", i % 3))
    );
    let compiled = Compiled::new(&source);
    let scheduled = compiled.scheduled();

    for group in group_by_conflict(&scheduled) {
        for (n, &a) in group.iter().enumerate() {
            for &b in &group[n + 1..] {
                let (fa, fb) = (&scheduled[a].1, &scheduled[b].1);
                assert!(
                    !shared_footprint(fa).conflicts_with(&shared_footprint(fb)),
                    "tests {a} and {b} share a group and conflict: {fa:?} vs {fb:?}"
                );
            }
        }
    }
}

/// The claim, executed: one group, real threads, every test writing the cell every other test also
/// allocated at `#0`.
#[test]
fn a_group_of_isolated_tests_running_at_once_never_observe_each_other() {
    const TESTS: usize = 32;
    let compiled = Compiled::new(&contending_source(TESTS, a_label_each));

    assert!(
        compiled
            .footprints()
            .iter()
            .all(|f| f.atoms().any(is_region_scoped)),
        "the corpus must retain cell atoms by inference, not by injection"
    );

    for round in 0..3 {
        let root = TempRoot::new();
        let mut store = root.store();
        let selection =
            ply_test::select(&compiled.check, &compiled.hashes, &store, &Plan::default());

        assert_eq!(
            selection.groups.len(),
            1,
            "round {round}: {TESTS} tests on {TESTS} labels must be one group"
        );
        assert_eq!(selection.parallelism.region_contended, TESTS);
        assert!(selection.parallelism.holds());

        let report = ply_test::run(
            &selection,
            &compiled.program,
            &compiled.resolved,
            &compiled.check,
            &compiled.hashes,
            &mut store,
            true,
            ply_test::Search::of(&selection),
            ply_test::Hosting::hermetic(),
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

/// The white-box half, and the strongest statement the audit can make about the region: after every
/// test in the group, the arena its worker holds contains exactly one cell — slot index 0, holding
/// that test's own last write.
#[test]
fn the_arena_each_test_ends_with_holds_its_own_writes_and_nothing_else() {
    const TESTS: usize = 24;
    let compiled = Compiled::new(&contending_source(TESTS, a_label_each));
    let root = TempRoot::new();
    let mut store = root.store();
    let selection = ply_test::select(&compiled.check, &compiled.hashes, &store, &Plan::default());
    assert_eq!(selection.groups.len(), 1);

    let executor = Recording {
        inner: ply_test::InterpExecutor::new(
            &compiled.program,
            &compiled.resolved,
            &compiled.check,
        ),
        seen: Mutex::new(Vec::new()),
        generations: Mutex::new(Vec::new()),
    };
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
        "{:#?}",
        report.failures
    );

    let mut seen = executor.seen.into_inner().expect("no worker panicked");
    seen.sort_by_key(|(index, _, _)| *index);
    let expected: Vec<(usize, Vec<String>, usize)> = (0..TESTS)
        .map(|i| (i, vec![format!("@0={}", i * 8)], 0))
        .collect();
    assert_eq!(
        seen, expected,
        "a test whose region did not close would leave the mark above zero"
    );
}

/// Wraps the real executor to look at each worker's arena the moment its test finishes — the only
/// place from which one test's leftovers would be visible — and at the group's region, which is
/// what must not have grown.
struct Recording<'a> {
    inner: ply_test::InterpExecutor<'a>,
    seen: Mutex<Vec<(usize, Vec<String>, usize)>>,
    /// Slot 0's generation as each test left it, in completion order.
    generations: Mutex<Vec<u32>>,
}

impl<'a> ply_test::Executor for Recording<'a> {
    type Worker = ply_test::Worker<'a>;

    fn worker(&self) -> Self::Worker {
        self.inner.worker()
    }

    fn execute(&self, worker: &mut Self::Worker, index: usize) -> Result<(), ply_span::Diagnostic> {
        // What the test's region reclaimed at its close, because that is where its cells are: a
        // region hands its slots back at its lexical end and the arena afterwards holds only the
        // group's fixture.
        worker.cells_mut().journal();
        let outcome = self.inner.execute(worker, index);
        let cells = worker
            .cells()
            .journalled()
            .iter()
            .map(|(slot, value)| format!("@{}={}", slot.index(), value.render()))
            .collect();
        if let Some((slot, _)) = worker.cells().journalled().first() {
            self.generations
                .lock()
                .expect("no worker panicked")
                .push(slot.generation());
        }
        self.seen
            .lock()
            .expect("no worker panicked")
            .push((index, cells, worker.region().mark()));
        outcome
    }
}

/// The mechanism that makes a closed region unreadable rather than merely forgotten, on the real
/// runner: one worker, so every test reuses slot index 0, and the generation at that index must
/// rise every time.
#[test]
fn a_slot_is_never_handed_to_two_tests_under_one_identity() {
    const TESTS: usize = 16;
    let compiled = Compiled::new(&contending_source(TESTS, a_label_each));
    let root = TempRoot::new();
    let mut store = root.store();
    let selection = ply_test::select(&compiled.check, &compiled.hashes, &store, &Plan::default());
    assert_eq!(selection.groups.len(), 1);

    let executor = Recording {
        inner: ply_test::InterpExecutor::new(
            &compiled.program,
            &compiled.resolved,
            &compiled.check,
        ),
        seen: Mutex::new(Vec::new()),
        generations: Mutex::new(Vec::new()),
    };
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

    let generations = executor
        .generations
        .into_inner()
        .expect("no worker panicked");
    assert_eq!(generations.len(), TESTS);
    assert!(
        generations.windows(2).all(|w| w[1] > w[0]),
        "slot 0's generation must rise at every entry point: {generations:?}"
    );
}

/// region isolation's fixture, at the runner: built once for the group, mutated in place by every test
/// in it, and the test's own allocations closed on top.
#[test]
fn the_group_fixture_is_built_once_and_carries_each_tests_write_to_the_next() {
    const TESTS: usize = 12;
    let compiled = Compiled::new(&contending_source(TESTS, a_label_each));
    let root = TempRoot::new();
    let mut store = root.store();
    let selection = ply_test::select(&compiled.check, &compiled.hashes, &store, &Plan::default());
    assert_eq!(selection.groups.len(), 1);

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
    /// What the fixture cell held when this test opened the region.
    observed_at_open: i64,
    mark: usize,
    fixture_len: usize,
}

/// A worker whose "test" reads the fixture, writes it, and allocates a cell of its own — the three
/// things a real test does to a region, with nothing else in the way.
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

    fn execute(&self, region: &mut GroupRegion, index: usize) -> Result<(), ply_span::Diagnostic> {
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

/// The same fixture at eight workers, which is where "built once per group" is not what the runner
/// does and saying so would over-claim.
#[test]
fn a_group_spread_over_eight_workers_gets_one_fixture_each() {
    const TESTS: usize = 24;
    const JOBS: usize = 8;
    let compiled = Compiled::new(&contending_source(TESTS, a_label_each));
    let root = TempRoot::new();
    let mut store = root.store();
    let selection = ply_test::select(&compiled.check, &compiled.hashes, &store, &Plan::default());
    assert_eq!(selection.groups.len(), 1);

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

/// W4's and W5's shared-state defects surfaced as a verdict that moved with the job count, so that
/// is what the region model is checked against: one worker and eight, over a corpus that has
/// colliding labels, disjoint labels and pure tests all at once.
#[test]
fn verdicts_do_not_move_between_one_worker_and_eight() {
    let source = format!(
        "{}{}{}",
        contending_source(6, one_label),
        contending_source(10, |i| format!("own{i}")),
        (0..8)
            .map(|i| format!("\ntest \"pure {i}\" {{ assert_eq({i} + 1, {}) }}\n", i + 1))
            .collect::<String>()
    );
    let compiled = Compiled::new(&source);

    let run_at = |jobs: usize| {
        let root = TempRoot::new();
        let mut store = root.store();
        let selection =
            ply_test::select(&compiled.check, &compiled.hashes, &store, &Plan::default());
        let groups = selection.groups.clone();
        let parallelism = selection.parallelism;
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(jobs)
            .build()
            .expect("the worker pool");
        let report = pool.install(|| {
            ply_test::run(
                &selection,
                &compiled.program,
                &compiled.resolved,
                &compiled.check,
                &compiled.hashes,
                &mut store,
                true,
                ply_test::Search::of(&selection),
                ply_test::Hosting::hermetic(),
            )
        });
        let mut verdicts: Vec<(usize, ply_test::Status, usize)> = report
            .results
            .iter()
            .map(|r| (r.index, r.status, r.group))
            .collect();
        verdicts.sort_by_key(|v| v.0);
        (groups, parallelism, report.passed, report.failed, verdicts)
    };

    let one = run_at(1);
    let eight = run_at(8);

    assert_eq!(
        one.3, 0,
        "the corpus must be green before it proves anything"
    );
    assert_eq!(
        one.0, eight.0,
        "the colouring is a function of the footprints and may not move with --jobs"
    );
    assert_eq!(one.1, eight.1);
    assert_eq!((one.2, one.3), (eight.2, eight.3));
    assert_eq!(
        one.4, eight.4,
        "a verdict moved between one worker and eight"
    );
    assert_eq!(
        one.0.len(),
        6,
        "six tests share `table`, so the corpus must need six rounds: {:?}",
        one.0
    );
}

/// A region-isolated test may not create a group, for any corpus size.
#[test]
fn adding_isolated_tests_never_adds_a_group() {
    let shared = r#"
effect db {
  read  get[users]() -> Int
  write put[users](v: Int) -> Unit
}

test "real reader" { assert_eq(db.get[users](), 0) }

test "real writer" { db.put[users](1) }
"#;

    let mut counts = Vec::new();
    for extra in [0usize, 1, 8, 64] {
        let pure: String = (0..extra)
            .map(|i| format!("\ntest \"pure {i}\" {{ assert_eq({i} + 1, {}) }}\n", i + 1))
            .collect();
        let compiled = Compiled::new(&format!("{shared}{pure}"));
        let root = TempRoot::new();
        let store = root.store();
        let selection =
            ply_test::select(&compiled.check, &compiled.hashes, &store, &Plan::default());
        assert_eq!(selection.parallelism.isolated, extra);
        assert_eq!(selection.parallelism.region_contended, 0);
        assert!(selection.parallelism.holds(), "{:?}", selection.parallelism);
        counts.push(selection.groups.len());
    }
    assert_eq!(counts, vec![2, 2, 2, 2]);
}
