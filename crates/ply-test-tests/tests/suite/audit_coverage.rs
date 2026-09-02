//! What `--audit-backend` actually compared, as a number the run reports.

use crate::fixture::Compiled;
use ply_eval::Plan;
use ply_store::Store;
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

fn report(src: &str, audit: bool) -> ply_test::RunReport {
    let compiled = Compiled::anonymous(src);
    let root = TempRoot::new();
    let mut store = root.store();
    let selection = ply_test::select(
        &compiled.check,
        &compiled.hashes,
        &store,
        &Plan::default(),
        &ply_test::Engine::Evaluator,
    );
    let fragment = ply_eval::Fragment::over(&compiled.program, &compiled.resolved, &compiled.check);
    let executor =
        ply_test::InterpExecutor::new(&compiled.program, &compiled.resolved, &compiled.check)
            .with_backend_audit(audit)
            .with_backend(fragment, ply_eval::BackendSpec::honest())
            .with_search(ply_test::Search::of(&selection))
            .with_hosts(ply_test::Hosting::hermetic());
    let report = ply_test::run_with(
        &selection,
        &compiled.check,
        &compiled.hashes,
        &mut store,
        &executor,
    );
    assert_eq!(report.failed, 0, "{:#?}", report.failures);
    report
}

/// One test of each kind, so the counts below are read off a corpus that has something in every
/// bucket.
const MIXED: &str = r#"
effect amb {
  read flip[coin]() -> Bool
}

effect counter {
  write bump[shard]() -> Unit
}

// The pair runs this one.
test "plain arithmetic" {
  assert_eq(1 + 1, 2)
}

// A multi-shot resumption: the pair runs it like any other.
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

// A searched test: replayed per interleaving on machines built for the
// schedule, which the pair never runs.
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
    let report = report(MIXED, true);
    let audit = report.audit.expect("`--audit-backend` runs an oracle");
    assert_eq!(audit.total(), 3, "{audit:?}");
    assert_eq!(
        (audit.compared, audit.unaudited),
        (2, 1),
        "the searched test is the one the pair cannot reach: {audit:?}"
    );
}

/// Which tests, not only how many.
#[test]
fn the_unaudited_test_is_the_searched_one() {
    let report = report(MIXED, true);
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
        audited.contains(&("a clause that binds its continuation", Some(true))),
        "{audited:?}"
    );
    assert!(
        audited.contains(&("two tasks over one shard", Some(false))),
        "{audited:?}"
    );
}

/// A run with no audit reports **no** coverage rather than a coverage of zero.
#[test]
fn an_unaudited_run_has_no_coverage_to_report() {
    let report = report("test \"plain arithmetic\" { assert_eq(1 + 1, 2) }\n", false);
    assert!(report.audit.is_none(), "{:?}", report.audit);
    assert!(report.results.iter().all(|r| r.audited.is_none()));
}

/// The line the run prints, and its silence when there is nothing to say.
#[test]
fn the_summary_line_appears_only_where_there_is_a_coverage() {
    let both = report(MIXED, true);
    let summary = both.summary().join("\n");
    assert!(summary.contains("audited 2 of 3"), "{summary}");
    assert!(summary.contains("1 ran unpaired"), "{summary}");

    let one = report("test \"plain arithmetic\" { assert_eq(1 + 1, 2) }\n", false);
    assert!(
        !one.summary().join("\n").contains("audited"),
        "{:?}",
        one.summary()
    );
}

/// The coverage is a fact about the oracle and not about the verdict: a corpus every engine can run
/// is fully audited, and the number says so rather than being pinned at whatever the mixed corpus
/// happens to produce.
#[test]
fn a_corpus_with_no_searched_test_is_fully_audited() {
    let report = report(
        "test \"a\" { assert_eq(1, 1) }\ntest \"b\" { assert_eq(2, 2) }\n",
        true,
    );
    let audit = report.audit.expect("`--audit-backend` runs an oracle");
    assert_eq!((audit.compared, audit.unaudited), (2, 0), "{audit:?}");
    assert!(
        !report.summary().join("\n").contains("unpaired"),
        "nothing was skipped, so nothing is apologized for"
    );
}
