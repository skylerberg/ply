//! What `--engine both` actually compared, as a number the run reports.
//!
//! ADR 0017's evidence that meaning did not move is `--engine both` over the
//! corpora. That oracle is only as strong as its coverage, and its coverage is
//! not total: the tree-walker cannot express an explicit control stack, so a
//! clause binding a continuation is `E0504` and never runs on it (ADR 0005
//! required test 3), and a searched test is replayed per interleaving on a
//! machine built for the schedule, so the tree-walker never sees that either.
//!
//! Both exclusions are correct — comparing a refusal against an answer would
//! report a divergence between an engine that declined to start and one that
//! did the work. What is not correct is leaving them uncounted, because
//! `0 failed, N passed` under two engines reads as "two engines agreed about N
//! tests". ADR 0017 §6 states this trap for the isolation counts and requires
//! the reporting to change with the mechanism; the oracle's own coverage is the
//! same trap, and these are the tests that hold it closed.
//!
//! The excluded tests are not incidental. They are exactly the ones that bind a
//! continuation or schedule a task — the constructs a change to the memory
//! model moves — so a coverage number that silently included them would
//! over-claim in the one place it matters.

use ply_core::CheckOutput;
use ply_eval::{EngineChoice, Plan};
use ply_hash::HashOutput;
use ply_span::SourceId;
use ply_store::Store;
use ply_syntax::resolve::Resolved;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new() -> TempRoot {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ply-audit-coverage-{}-{}",
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
}

fn report(src: &str, engine: EngineChoice) -> ply_test::RunReport {
    let compiled = Compiled::new(src);
    let root = TempRoot::new();
    let mut store = root.store();
    let selection = ply_test::select(&compiled.check, &compiled.hashes, &store, &Plan::default());
    let report = ply_test::run(
        &selection,
        &compiled.program,
        &compiled.resolved,
        &compiled.check,
        &compiled.hashes,
        &mut store,
        engine,
        ply_test::Search::of(&selection),
        ply_test::Hosting::hermetic(),
    );
    assert_eq!(report.failed, 0, "{:#?}", report.failures);
    report
}

/// One test of each kind, so the counts below are read off a corpus that has
/// something in every bucket.
const MIXED: &str = r#"
effect amb {
  read flip[coin]() -> Bool
}

effect counter {
  write bump[shard]() -> Unit
}

// Both engines run this one.
test "plain arithmetic" {
  assert_eq(1 + 1, 2)
}

// `resume k` is E0504 on the tree-walker: it never runs.
test "a clause that binds its continuation" {
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

// A searched test: replayed per interleaving on a machine built for the
// schedule, which the tree-walker never sees.
test "two tasks over one shard" {
  with_cell[tally](0) { t ->
    handle {
      simulate {
        let a = task.spawn(|| counter.bump[shard]());
        let b = task.spawn(|| counter.bump[shard]());
        task.join(a);
        task.join(b);
        assert_eq(cell_get(t), 2)
      }
    } with {
      counter.bump[shard]() -> cell_set(t, cell_get(t) + 1),
    }
  }
}
"#;

#[test]
fn the_oracle_counts_the_tests_it_compared_and_the_tests_it_could_not() {
    let report = report(MIXED, EngineChoice::Both);
    let audit = report.audit.expect("`both` runs an oracle");
    assert_eq!(audit.total(), 3, "{audit:?}");
    assert_eq!(
        (audit.compared, audit.unaudited),
        (1, 2),
        "only the plain test is reachable by both engines: {audit:?}"
    );
}

/// Which tests, not only how many. A count that happened to be right while
/// naming the wrong tests would still send a reader to the wrong place.
#[test]
fn the_unaudited_tests_are_the_multi_shot_one_and_the_searched_one() {
    let report = report(MIXED, EngineChoice::Both);
    let audited: Vec<(&str, Option<bool>)> = report
        .results
        .iter()
        .map(|r| (r.name.as_str(), r.audited))
        .collect();
    assert!(
        audited.contains(&("plain arithmetic", Some(true))),
        "{audited:?}"
    );
    assert!(
        audited.contains(&("a clause that binds its continuation", Some(false))),
        "{audited:?}"
    );
    assert!(
        audited.contains(&("two tasks over one shard", Some(false))),
        "{audited:?}"
    );
}

/// A run with one engine reports **no** coverage rather than a coverage of
/// zero. "The oracle covered nothing" and "there was no oracle" are different
/// claims, and a consumer that cannot tell them apart will act on the wrong one
/// — the same rule `simulation` and `host_use` already follow.
#[test]
fn a_single_engine_run_has_no_coverage_to_report() {
    for engine in [EngineChoice::Machine, EngineChoice::Treewalk] {
        // The tree-walker refuses the multi-shot test outright, so a single
        // engine is asked about the one program both can run.
        let report = report(
            "test \"plain arithmetic\" { assert_eq(1 + 1, 2) }\n",
            engine,
        );
        assert!(report.audit.is_none(), "{engine:?}: {:?}", report.audit);
        assert!(
            report.results.iter().all(|r| r.audited.is_none()),
            "{engine:?}"
        );
    }
}

/// The line the run prints, and its silence when there is nothing to say.
#[test]
fn the_summary_line_appears_only_where_there_is_a_coverage() {
    let both = report(MIXED, EngineChoice::Both);
    let summary = both.summary().join("\n");
    assert!(summary.contains("audited 1 of 3"), "{summary}");
    assert!(summary.contains("2 ran on one engine only"), "{summary}");

    let one = report(
        "test \"plain arithmetic\" { assert_eq(1 + 1, 2) }\n",
        EngineChoice::Machine,
    );
    assert!(
        !one.summary().join("\n").contains("audited"),
        "{:?}",
        one.summary()
    );
}

/// The coverage is a fact about the oracle and not about the verdict: a corpus
/// every engine can run is fully audited, and the number says so rather than
/// being pinned at whatever the mixed corpus happens to produce.
#[test]
fn a_corpus_both_engines_can_run_is_fully_audited() {
    let report = report(
        "test \"a\" { assert_eq(1, 1) }\ntest \"b\" { assert_eq(2, 2) }\n",
        EngineChoice::Both,
    );
    let audit = report.audit.expect("`both` runs an oracle");
    assert_eq!((audit.compared, audit.unaudited), (2, 0), "{audit:?}");
    assert!(
        !report.summary().join("\n").contains("one engine only"),
        "nothing was skipped, so nothing is apologized for"
    );
}
