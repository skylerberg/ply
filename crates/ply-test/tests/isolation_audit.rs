//! An attack on the scheduler's new claim: that a test whose atoms are all
//! world-backed conflicts with nothing, whatever else the corpus contains.
//!
//! The claim converts a correctness property into a scheduling decision, so if
//! it is wrong it is wrong in the worst available way — two tests run at once,
//! one of them fails on Tuesday, and the cache remembers the pass. Three things
//! have to hold for it, and each gets an attack here:
//!
//! 1. Only real region state can produce a `cell` atom, so the exemption cannot
//!    be spoofed by naming an effect `cell`.
//! 2. An atom that is *not* world-backed keeps its edge, even when it sits in
//!    the same footprint as one that is.
//! 3. Tests the exemption puts in one group really cannot see each other — run
//!    concurrently, on real threads, with every one of them writing the cell
//!    that every other one also allocated at the same id.
//!
//! Everything here goes through real inference: a footprint that was injected by
//! the test proves the colouring and nothing about whether that footprint was
//! reachable.

use ply_core::{CheckOutput, Footprint};
use ply_eval::{EngineChoice, Plan, Value, World};
use ply_hash::HashOutput;
use ply_span::SourceId;
use ply_store::Store;
use ply_syntax::resolve::Resolved;
use ply_test::{Isolation, group_by_conflict, is_world_backed, shared_footprint, world_isolated};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

// ------------------------------------------------------------------ harness

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
        let program = ply_syntax::ast::Program::single(module);
        let resolved = ply_syntax::resolve(&program)
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
        let program = ply_syntax::ast::Program::single(module);
        let Ok(resolved) = ply_syntax::resolve(&program) else {
            return Vec::new();
        };
        ply_core::check_program(&program, &resolved)
            .err()
            .unwrap_or_default()
    }

    fn footprints(&self) -> Vec<&Footprint> {
        self.check.tests.iter().map(|t| &t.footprint).collect()
    }
}

/// Each test allocates its cell at the same id as every other one, smuggles a
/// reader and a writer out of the region, and then checks every value it wrote.
/// If two of them shared a world, `assert_eq` inside the test is what would say
/// so — the audit is written in Ply, not in Rust assertions about Ply.
fn contending_source(tests: usize) -> String {
    let mut out = String::new();
    for i in 0..tests {
        out.push_str(&format!(
            r#"
test "contender {i}" {{
  let ops = with_cell[table]({i}) {{ c -> {{get: || cell_get(c), put: |v| cell_set(c, v)}} }};
  let get = ops.get;
  let put = ops.put;
  assert_eq(get(), {i});
  put(get() * 7);
  assert_eq(get(), {seven});
  put(get() + {i});
  assert_eq(get(), {eight})
}}
"#,
            i = i,
            seven = i * 7,
            eight = i * 8,
        ));
    }
    out
}

// ------------------------------------------- 1. the exemption cannot be spoofed

/// `is_world_backed` trusts one effect name. If a program could declare that
/// name, it could claim the exemption for state the world knows nothing about —
/// so the reservation is the load-bearing half of the rule, not a nicety.
#[test]
fn a_program_cannot_declare_the_effect_the_exemption_names() {
    for name in ply_test::WORLD_BACKED {
        let diags = Compiled::rejected(&format!(
            r#"
effect {name} {{
  write put[users](v: Int) -> Unit
}}

test "claim the exemption" {{
  {name}.put[users](1)
}}
"#
        ));
        assert!(
            !diags.is_empty(),
            "`effect {name}` must be refused, or the world-backed exemption is claimable"
        );
        assert!(
            diags.iter().any(|d| d.message.contains("builtin")),
            "the refusal must say the name is reserved: {diags:#?}"
        );
    }
}

/// The exemption is by exact effect name, so a neighbouring name gains nothing.
/// Two tests that both write `cells.write[rows]` are two writers of one resource
/// and must stay in separate groups.
#[test]
fn an_effect_whose_name_merely_resembles_the_builtin_is_not_exempt() {
    let compiled = Compiled::new(
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
        assert!(
            !world_isolated(f),
            "`cells` must not be world-backed: {f:?}"
        );
        assert!(f.atoms().all(|a| !is_world_backed(a)));
    }

    let scheduled: Vec<(usize, Footprint)> = footprints
        .iter()
        .enumerate()
        .map(|(i, f)| (i, (*f).clone()))
        .collect();
    assert_eq!(
        group_by_conflict(&scheduled).len(),
        2,
        "two writers of one non-world resource must not share a group"
    );
}

// ------------------------------------ 2. a real atom keeps its edge regardless

/// The exemption subtracts atoms; it never subtracts tests. A footprint that
/// mixes a cell atom with a real one still conflicts through the real one, and
/// the mixed test is `Shared` rather than `World`.
#[test]
fn a_cell_atom_beside_a_real_one_does_not_launder_the_real_one() {
    let compiled = Compiled::new(
        r#"
effect db {
  read  get[users]() -> Int
  write put[users](v: Int) -> Unit
}

test "cell only" {
  let read = with_cell[table](1) { c -> || cell_get(c) };
  assert_eq(read(), 1)
}

test "cell and a real write" {
  let read = with_cell[table](1) { c -> || cell_get(c) };
  db.put[users](read())
}

test "a real read" {
  assert_eq(db.get[users](), 0)
}
"#,
    );

    let footprints = compiled.footprints();
    let isolation: Vec<Isolation> = footprints.iter().map(|f| Isolation::of(f)).collect();
    assert_eq!(
        isolation,
        vec![Isolation::World, Isolation::Shared, Isolation::Shared]
    );

    let shared = shared_footprint(footprints[1]);
    let atoms: Vec<String> = shared.atoms().map(|a| a.to_string()).collect();
    assert_eq!(atoms, vec!["db.write[users]".to_string()]);
    assert!(shared.conflicts_with(&shared_footprint(footprints[2])));

    let scheduled: Vec<(usize, Footprint)> = footprints
        .iter()
        .enumerate()
        .map(|(i, f)| (i, (*f).clone()))
        .collect();
    let groups = group_by_conflict(&scheduled);
    assert_eq!(groups.len(), 2, "{groups:?}");
    let group_of = |t: usize| groups.iter().position(|g| g.contains(&t)).unwrap();
    assert_ne!(group_of(1), group_of(2), "a writer and a reader of `users`");
    assert_eq!(group_of(0), 0, "the isolated test is free in either group");
}

/// The colouring's own invariant, checked against the corpus rather than
/// asserted about it: any two tests sharing a group either do not conflict at
/// all, or conflict *only* through atoms the world backs. Anything else is the
/// exemption having been widened by accident.
#[test]
fn every_pair_in_a_group_conflicts_at_most_through_world_backed_atoms() {
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
        contending_source(6)
    );
    let compiled = Compiled::new(&source);
    let scheduled: Vec<(usize, Footprint)> = compiled
        .footprints()
        .iter()
        .enumerate()
        .map(|(i, f)| (i, (*f).clone()))
        .collect();

    for group in group_by_conflict(&scheduled) {
        for (n, &a) in group.iter().enumerate() {
            for &b in &group[n + 1..] {
                let (fa, fb) = (&scheduled[a].1, &scheduled[b].1);
                if !fa.conflicts_with(fb) {
                    continue;
                }
                assert!(
                    !shared_footprint(fa).conflicts_with(&shared_footprint(fb)),
                    "tests {a} and {b} share a group and conflict outside the world: \
                     {fa:?} vs {fb:?}"
                );
            }
        }
    }
}

// ----------------------------------- 3. the tests really cannot see each other

/// The claim, executed: one group, real threads, every test writing the cell
/// every other test also allocated at `#0`.
///
/// What this catches on its own is a shared *value* — two tests reaching one
/// entry — because each test's first assertion is that its cell still holds the
/// initial value only it wrote. It does not catch a world that merely grew,
/// since the next test would then allocate a fresh id and pass anyway; that half
/// is [`the_world_each_test_ends_with_holds_its_own_writes_and_nothing_else`],
/// which inspects the world instead of trusting the program.
///
/// Run more than once because interference is a race, and once is a sample.
#[test]
fn a_group_of_world_isolated_tests_running_at_once_never_observe_each_other() {
    const TESTS: usize = 32;
    let compiled = Compiled::new(&contending_source(TESTS));

    assert!(
        compiled.footprints().iter().all(|f| world_isolated(f)),
        "the corpus must be world-isolated by inference, not by injection"
    );
    assert!(
        compiled
            .footprints()
            .iter()
            .any(|f| f.atoms().next().is_some()),
        "and it must retain cell atoms, or it is not exercising the exemption"
    );

    for round in 0..3 {
        let root = TempRoot::new();
        let mut store = root.store();
        let selection =
            ply_test::select(&compiled.check, &compiled.hashes, &store, &Plan::default());

        assert_eq!(
            selection.groups.len(),
            1,
            "round {round}: {TESTS} contending tests must be one group"
        );
        assert_eq!(selection.parallelism.isolated, TESTS);
        assert_eq!(selection.parallelism.shared_groups, 0);
        assert!(selection.parallelism.holds());

        let report = ply_test::run(
            &selection,
            &compiled.program,
            &compiled.resolved,
            &compiled.check,
            &compiled.hashes,
            &mut store,
            EngineChoice::Both,
            ply_test::Search::of(&selection),
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

/// The white-box half, and the strongest statement the audit can make about the
/// exemption: after every test in the group, the world its worker holds contains
/// exactly one cell — `#0`, holding that test's own last write. A world carried
/// from a previous test would hold two, and a world shared with a concurrent one
/// would hold somebody else's number.
#[test]
fn the_world_each_test_ends_with_holds_its_own_writes_and_nothing_else() {
    const TESTS: usize = 24;
    let compiled = Compiled::new(&contending_source(TESTS));
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
    seen.sort_by_key(|(index, _)| *index);
    let expected: Vec<(usize, Vec<String>)> = (0..TESTS)
        .map(|i| (i, vec![format!("#0={}", i * 8)]))
        .collect();
    assert_eq!(seen, expected);
}

/// Wraps the real executor to look at each worker's world the moment its test
/// finishes — the only place from which one test's leftovers would be visible.
struct Recording<'a> {
    inner: ply_test::InterpExecutor<'a>,
    seen: Mutex<Vec<(usize, Vec<String>)>>,
}

impl<'a> ply_test::Executor for Recording<'a> {
    type Worker = ply_test::Worker<'a>;

    fn worker(&self) -> Self::Worker {
        self.inner.worker()
    }

    fn execute(&self, worker: &mut Self::Worker, index: usize) -> Result<(), ply_span::Diagnostic> {
        let outcome = self.inner.execute(worker, index);
        let cells = worker
            .world()
            .cells()
            .map(|(id, value)| format!("{id}={}", value.render()))
            .collect();
        self.seen
            .lock()
            .expect("no worker panicked")
            .push((index, cells));
        outcome
    }
}

/// The same corpus against a *seeded* base world, which is the fixture story:
/// one world built once, forked per test. A test that could write through to the
/// fixture would be the interference channel the fork exists to close, and every
/// test asserting its own seed is what would catch it.
#[test]
fn a_shared_fixture_is_forked_per_test_rather_than_shared_between_them() {
    const TESTS: usize = 16;
    let compiled = Compiled::new(&contending_source(TESTS));
    let root = TempRoot::new();
    let mut store = root.store();
    let selection = ply_test::select(&compiled.check, &compiled.hashes, &store, &Plan::default());

    let built = AtomicUsize::new(0);
    let fixture: &(dyn Fn() -> World + Sync) = &|| {
        built.fetch_add(1, Ordering::Relaxed);
        let mut world = World::new();
        world.alloc(Value::list(vec![Value::str("seed")]));
        world
    };
    let executor =
        ply_test::InterpExecutor::new(&compiled.program, &compiled.resolved, &compiled.check)
            .with_fixture(fixture);

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
    assert!(
        built.load(Ordering::Relaxed) >= 1,
        "the fixture factory runs per worker, on the worker's own thread"
    );
}

/// A world-isolated test may not create a group, for any corpus size. This is
/// the number a project watches: adding isolated tests is free, and the group
/// count is decided by the shared tests alone.
#[test]
fn adding_contending_isolated_tests_never_adds_a_group() {
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
        let compiled = Compiled::new(&format!("{shared}{}", contending_source(extra)));
        let root = TempRoot::new();
        let store = root.store();
        let selection =
            ply_test::select(&compiled.check, &compiled.hashes, &store, &Plan::default());
        assert_eq!(selection.parallelism.isolated, extra);
        assert!(selection.parallelism.holds(), "{:?}", selection.parallelism);
        counts.push(selection.groups.len());
    }
    assert_eq!(counts, vec![2, 2, 2, 2]);
}
