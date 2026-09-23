use ply_cli::cli::{TestArgs, When};
use ply_cli::commands::common::exit_code;
use ply_cli::hosts::Hosts;
use ply_cli::load::{Loaded, load};
use ply_cli::test::*;
use ply_eval::Plan as SimPlan;
use ply_machine::support::run_on_tier;
use ply_span::codes;
use ply_store::{Outcome, Store};
use ply_test::{Reason, Record, RunReport, Selection, Skipped, Status, TestResult, Verdict};
use ply_ty::HashOutput;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A footprint is what the handlers did not discharge, so a residual atom comes from granting less than the code may use.
const SOURCE: &str = "\
effect db {
  read  all[t]() -> List<Int>
  write save[t](rows: List<Int>) -> Unit
}

fn peek(table: String) -> Int / {db.read[users], db.read[orders]} =
  if table == \"users\" { len(db.all[users]()) } else { len(db.all[orders]()) }

fn wipe(table: String) -> Unit / {db.write[users], db.read[orders]} =
  if table == \"users\" { db.save[users]([]) } else { assert_eq(len(db.all[orders]()), 0) }

test \"reads orders only\" {
  handle { assert_eq(peek(\"orders\"), 0) } with { db.all[orders]() -> [] }
}

test \"reads orders only again\" {
  handle { assert_eq(peek(\"orders\"), 0) } with { db.all[orders]() -> [] }
}

test \"writes users when asked\" {
  handle { wipe(\"orders\") } with { db.all[orders]() -> [] }
}

test \"pure arithmetic\" { assert_eq(1 + 1, 2) }
";

fn write(dir: &Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, text).unwrap();
}

fn project(files: &[(&str, &str)]) -> (tempfile::TempDir, Loaded, HashOutput) {
    let dir = tempfile::tempdir().unwrap();
    for (rel, text) in files {
        write(dir.path(), rel, text);
    }
    let loaded = load(dir.path()).unwrap();
    let hashes = loaded.hashes().unwrap();
    (dir, loaded, hashes)
}

fn fixture() -> (tempfile::TempDir, Loaded, HashOutput) {
    project(&[("m.ply", SOURCE)])
}

fn run(loaded: &Loaded, selection: &Selection, store: &mut Store) -> RunReport {
    run_on_tier(loaded, selection, ply_test::Hosting::hermetic(), store)
}

fn plan_for(filter: Option<&str>) -> (tempfile::TempDir, Loaded, HashOutput, Plan) {
    let (dir, loaded, hashes) = fixture();
    let store = Store::open(dir.path()).unwrap();
    let selected = ply_test::select(
        &loaded.check,
        &hashes,
        &store,
        &SimPlan::default(),
        &ply_test::Engine::Evaluator,
    );
    let plan = Plan::new(selected, &loaded.check, filter, false);
    (dir, loaded, hashes, plan)
}

fn args_for(filter: Option<&str>) -> TestArgs {
    TestArgs {
        steps: ply_eval::DEFAULT_STEP_BUDGET,
        timeout: 60_000,
        profile: "development".to_string(),
        watch: false,
        path: PathBuf::from("."),
        json: true,
        explain: false,
        no_cache: false,
        filter: filter.map(str::to_string),
        jobs: None,
        bisect: When::Auto,
        bisect_budget: 64,
        coverage: false,
        mutate: None,
        mutate_budget: 64,
        trace: When::Auto,
        backend: None,
        host: false,
        tls: ply_cli::cli::TlsOptions::default(),
        fs: ply_cli::cli::FsOptions::default(),
        db: ply_cli::cli::DbOptions::default(),
        config: ply_cli::cli::ConfigOptions::default(),
        std: false,
        simulation: ply_cli::cli::SimOptions {
            seed: None,
            sim: ply_cli::cli::SimArg::default(),
            seeds: None,
            sim_budget: None,
            sim_steps: None,
            measure_reduction: false,
        },
    }
}

#[test]
fn a_cold_cache_selects_everything() {
    let (_dir, loaded, _h, plan) = plan_for(None);
    assert_eq!(plan.selection.total, 4);
    assert_eq!(plan.selection.to_run.len(), 4);
    assert!(plan.selection.cached.is_empty());
    assert_eq!(plan.visible, vec![0, 1, 2, 3]);
    assert_eq!(loaded.check.tests.len(), 4);
}

#[test]
fn a_write_is_scheduled_apart_from_the_reads_of_the_same_resource() {
    let (_dir, loaded, _h, plan) = plan_for(None);
    let index_of = |name: &str| {
        loaded
            .check
            .tests
            .iter()
            .position(|t| t.name == name)
            .unwrap()
    };
    let group_of = |name: &str| plan.selection.group_of(index_of(name)).unwrap();

    assert_eq!(
        loaded.check.tests[index_of("reads orders only")]
            .footprint
            .to_string(),
        "{m.db.read[users]}"
    );
    assert_eq!(
        loaded.check.tests[index_of("writes users when asked")]
            .footprint
            .to_string(),
        "{m.db.write[users]}"
    );
    assert_eq!(
        group_of("reads orders only"),
        group_of("reads orders only again")
    );
    assert_ne!(
        group_of("reads orders only"),
        group_of("writes users when asked")
    );
    // A test that touches nothing conflicts with nothing, so it shares a group rather than forcing a third.
    assert_eq!(plan.selection.groups.len(), 2);
}

#[test]
fn the_filter_renarrows_the_denominator_and_regroups() {
    let (_dir, _l, _h, plan) = plan_for(Some("reads orders"));
    assert_eq!(plan.selection.total, 2);
    assert_eq!(plan.filtered_out, 2);
    assert_eq!(plan.visible.len(), 2);
    // Both survivors only read, so the writer's group is gone entirely.
    assert_eq!(plan.selection.groups.len(), 1);
    assert_eq!(plan.selection.groups[0].len(), 2);
    // `isolated n of m` has to answer for the same m as `selected n of m`.
    let parallelism = &plan.selection.parallelism;
    assert_eq!(parallelism.total, 2);
    assert_eq!(parallelism.isolated, 0);
    assert_eq!(parallelism.shared_groups, 1);
    assert!(parallelism.holds(), "{parallelism:?}");
}

#[test]
fn a_filter_matching_nothing_selects_nothing_and_says_so() {
    let (_dir, _loaded, _h, plan) = plan_for(Some("no such test"));
    assert_eq!(plan.selection.total, 0);
    assert!(plan.selection.to_run.is_empty());
    assert!(plan.selection.groups.is_empty());
}

#[test]
fn the_filter_matches_the_module_qualified_key() {
    let (dir, loaded, hashes) = project(&[
        ("alpha.ply", "test \"shared label\" { assert_eq(1, 1) }\n"),
        ("beta.ply", "test \"shared label\" { assert_eq(2, 2) }\n"),
    ]);
    let store = Store::open(dir.path()).unwrap();
    let keys: Vec<&str> = loaded.check.tests.iter().map(|t| t.key.as_str()).collect();
    assert_eq!(keys, ["alpha.shared label", "beta.shared label"]);

    let select = |needle| {
        Plan::new(
            ply_test::select(
                &loaded.check,
                &hashes,
                &store,
                &SimPlan::default(),
                &ply_test::Engine::Evaluator,
            ),
            &loaded.check,
            Some(needle),
            false,
        )
    };
    assert_eq!(select("beta.").visible, vec![1]);
    assert_eq!(select("shared label").visible, vec![0, 1]);
}

#[test]
fn a_warm_cache_selects_nothing_and_a_second_run_stays_empty() {
    let (dir, loaded, hashes) = fixture();
    let mut store = Store::open(dir.path()).unwrap();
    let selected = ply_test::select(
        &loaded.check,
        &hashes,
        &store,
        &SimPlan::default(),
        &ply_test::Engine::Evaluator,
    );
    let report = run(&loaded, &selected, &mut store);
    assert_eq!(report.passed, 4);
    assert_eq!(report.failed, 0);

    let store = Store::open(dir.path()).unwrap();
    let again = ply_test::select(
        &loaded.check,
        &hashes,
        &store,
        &SimPlan::default(),
        &ply_test::Engine::Evaluator,
    );
    assert!(again.to_run.is_empty());
    assert_eq!(again.cached.len(), 4);
    assert_eq!(
        Plan::new(again, &loaded.check, None, false).selection.total,
        4
    );
}

#[test]
fn tests_from_every_module_are_selected_and_run_together() {
    let (dir, loaded, hashes) = project(&[
        (
            "lib.ply",
            "pub fn one() -> Int = 1\ntest \"one is one\" { assert_eq(one(), 1) }\n",
        ),
        (
            "app.ply",
            "import lib\n\
             fn two() -> Int = lib::one() + lib::one()\n\
             test \"two is two\" { assert_eq(two(), 2) }\n",
        ),
    ]);
    assert_eq!(loaded.module_count(), 2);

    let mut store = Store::open(dir.path()).unwrap();
    let selected = ply_test::select(
        &loaded.check,
        &hashes,
        &store,
        &SimPlan::default(),
        &ply_test::Engine::Evaluator,
    );
    assert_eq!(selected.total, 2);
    let report = run(&loaded, &selected, &mut store);
    assert_eq!(report.passed, 2, "failures: {:?}", report.failures);
    assert!(report.is_success());
}

#[test]
fn a_failing_test_names_its_suspects_by_program_wide_name() {
    let (dir, loaded, hashes) = project(&[
        ("lib.ply", "pub fn one() -> Int = 2\n"),
        (
            "app.ply",
            "import lib\ntest \"one is one\" { assert_eq(lib::one(), 1) }\n",
        ),
    ]);
    let mut store = Store::open(dir.path()).unwrap();
    let selected = ply_test::select(
        &loaded.check,
        &hashes,
        &store,
        &SimPlan::default(),
        &ply_test::Engine::Evaluator,
    );
    let report = run(&loaded, &selected, &mut store);

    assert_eq!(report.failed, 1);
    assert_eq!(
        report.failures[0].suspects,
        vec![ply_span::Symbol::new("lib.one")]
    );
}

#[test]
fn no_cache_never_touches_the_real_store() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "m.ply", SOURCE);
    let loaded = load(dir.path()).unwrap();
    let hashes = loaded.hashes().unwrap();

    let scratch_dir = {
        let mut cache = Cache::open(dir.path(), true).unwrap();
        let scratch = cache
            .scratch
            .clone()
            .expect("bypass must use a scratch store");
        let selected = ply_test::select(
            &loaded.check,
            &hashes,
            &cache.store,
            &SimPlan::default(),
            &ply_test::Engine::Evaluator,
        );
        assert_eq!(selected.to_run.len(), 4);
        run(&loaded, &selected, &mut cache.store);
        assert!(!cache.store.is_empty());
        scratch
    };

    assert!(!scratch_dir.exists(), "the scratch cache outlived the run");
    assert_eq!(Store::open(dir.path()).unwrap().len(), 0);
}

#[test]
fn an_unopenable_cache_still_runs_but_says_it_gave_up_on_caching() {
    let dir = tempfile::tempdir().unwrap();
    // A file where the cache directory must go, so `Store::open` has to fail.
    std::fs::write(dir.path().join(ply_store::CACHE_DIR_NAME), "in the way").unwrap();

    let cache = Cache::open(dir.path(), false).unwrap();
    assert!(
        cache.scratch.is_some(),
        "the run must fall back rather than abort"
    );
    assert_eq!(cache.warnings.len(), 1);
    assert!(
        cache.warnings[0]
            .message
            .contains("could not open the cache")
    );
    assert!(
        cache.warnings[0]
            .notes
            .iter()
            .any(|n| n.contains("nothing this run proved"))
    );
}

#[test]
fn a_failure_is_never_cached_so_it_re_runs() {
    let (dir, loaded, hashes) = project(&[(
        "m.ply",
        "fn f() -> Int = 1\ntest \"wrong\" { assert_eq(f(), 2) }\n",
    )]);
    let mut store = Store::open(dir.path()).unwrap();

    let selected = ply_test::select(
        &loaded.check,
        &hashes,
        &store,
        &SimPlan::default(),
        &ply_test::Engine::Evaluator,
    );
    let report = run(&loaded, &selected, &mut store);
    assert_eq!(report.failed, 1);
    assert_eq!(exit_code(report.is_success()), ply_cli::EXIT_FAILED);
    assert_eq!(report.failures[0].diagnostic.code, codes::ASSERTION_FAILED);

    let store = Store::open(dir.path()).unwrap();
    let again = ply_test::select(
        &loaded.check,
        &hashes,
        &store,
        &SimPlan::default(),
        &ply_test::Engine::Evaluator,
    );
    assert_eq!(again.to_run.len(), 1, "a red test must re-run");
}

fn failing(source: &str) -> (tempfile::TempDir, Loaded, HashOutput, RunReport) {
    let (dir, loaded, hashes) = project(&[("m.ply", source)]);
    let mut store = Store::open(dir.path()).unwrap();
    let selected = ply_test::select(
        &loaded.check,
        &hashes,
        &store,
        &SimPlan::default(),
        &ply_test::Engine::Evaluator,
    );
    let report = run(&loaded, &selected, &mut store);
    (dir, loaded, hashes, report)
}

const ONE_FAILURE: &str = "fn f() -> Int = 1\ntest \"wrong\" { assert_eq(f(), 2) }\n";

#[test]
fn bisect_never_reports_that_nothing_was_attempted_and_evaluates_nothing() {
    let (_dir, loaded, _hashes, mut report) = failing(ONE_FAILURE);
    let mut args = args_for(None);
    args.bisect = When::Never;
    ply_test::diagnose_failures(
        &mut report,
        &loaded.texts(),
        &loaded.front,
        &mut Store::open(_dir.path()).unwrap(),
        &ply_machine::tester::diagnosis_options(&ply_machine::tester::TestOptions {
            steps: args.steps,
            timeout: args.timeout,
            bisect: match args.bisect {
                When::Auto => ply_machine::options::When::Auto,
                When::Always => ply_machine::options::When::Always,
                When::Never => ply_machine::options::When::Never,
            },
            bisect_budget: args.bisect_budget,
            mutate: args.mutate.clone(),
            mutate_budget: args.mutate_budget,
            simulation: (&args.simulation).into(),
            path: args.path.clone(),
            json: args.json,
            explain: args.explain,
            no_cache: args.no_cache,
            filter: args.filter.clone(),
            jobs: args.jobs,
            coverage: args.coverage,
            trace: ply_machine::options::When::Auto,
            backend: args.backend.clone(),
            profile: args.profile.clone(),
            watch: args.watch,
            host: args.host,
            tls: ply_machine::options::TlsOptions::default(),
            fs: Vec::new(),
            db: ply_machine::db::DbOptions::default(),
            config: ply_machine::config::ConfigOptions::default(),
            std: args.std,
        }),
    );

    let bisection = &report.failures[0].attribution.bisection;
    assert_eq!(
        bisection.verdict,
        Verdict::NotAttempted(Skipped::NotRequested)
    );
    assert_eq!(bisection.search.evaluated, 0);

    // The artifact the program places under `failures` says the same, for an agent that branches
    // on it rather than on the line.
    let v = ply_test::report::failure_json(&report.failures[0]);
    assert_eq!(v["culprit"]["verdict"], "not_attempted");
    assert_eq!(v["culprit"]["skipped"], "not_requested");
}

#[test]
fn a_nondet_test_always_runs_and_is_never_recorded() {
    let (dir, loaded, hashes) = project(&[(
        "m.ply",
        "nondet effect wall {\n  read now() -> Int\n}\n\
         test/nondet \"reads the clock\" { assert(wall.now() > 0) }\n",
    )]);
    let mut store = Store::open(dir.path()).unwrap();

    let selected = ply_test::select(
        &loaded.check,
        &hashes,
        &store,
        &SimPlan::default(),
        &ply_test::Engine::Evaluator,
    );
    assert_eq!(selected.reason(0), Some(Reason::Nondet));
    run(&loaded, &selected, &mut store);

    let store = Store::open(dir.path()).unwrap();
    let again = ply_test::select(
        &loaded.check,
        &hashes,
        &store,
        &SimPlan::default(),
        &ply_test::Engine::Evaluator,
    );
    assert_eq!(again.to_run, vec![0]);
}

#[test]
fn cached_results_do_not_appear_as_run_results() {
    let (dir, loaded, hashes) = fixture();
    let mut store = Store::open(dir.path()).unwrap();
    let first = ply_test::select(
        &loaded.check,
        &hashes,
        &store,
        &SimPlan::default(),
        &ply_test::Engine::Evaluator,
    );
    run(&loaded, &first, &mut store);

    let mut store = Store::open(dir.path()).unwrap();
    let second = ply_test::select(
        &loaded.check,
        &hashes,
        &store,
        &SimPlan::default(),
        &ply_test::Engine::Evaluator,
    );
    let report = run(&loaded, &second, &mut store);
    assert!(report.results.is_empty());
    assert_eq!(report.cached, 4);
    assert!(report.is_success());
}

#[test]
fn a_stored_failure_is_re_run_rather_than_believed() {
    let (dir, loaded, hashes) = fixture();
    let mut store = Store::open(dir.path()).unwrap();
    store.put(
        hashes.tests[0],
        Outcome::Fail {
            message: "from an older runtime".into(),
            diagnostic: None,
        },
    );
    store.flush().unwrap();

    let selected = ply_test::select(
        &loaded.check,
        &hashes,
        &store,
        &SimPlan::default(),
        &ply_test::Engine::Evaluator,
    );
    assert_eq!(selected.reason(0), Some(Reason::PreviousFailure));
    assert!(selected.to_run.contains(&0));
}

/// Binds the fixture's `db.all[users]`, the residual atom of the two reading tests and of neither of the others.
fn bound(loaded: &Loaded) -> Hosts {
    use crate::unit::hosts::fixture::{deterministic, named, op, registry};
    Hosts::bind(
        registry(vec![deterministic(op(
            "db",
            "all",
            named("users"),
            ply_eval::host::Linearity::AtMostOnce,
            false,
            "ply_host::postgres::read",
        ))]),
        &loaded.check,
        true,
    )
    .expect("the fixture binds")
}

fn recorded_result(recorded: Option<Record>) -> TestResult {
    TestResult {
        index: 0,
        name: "a".into(),
        hash: None,
        group: 0,
        duration: Duration::ZERO,
        status: Status::Passed,
        failure: None,
        simulation: None,
        recorded,
        backend: None,
    }
}

/// The command names the engine before building a provider and the run names it from the provider; nothing else compares them.
#[test]
fn recording_under_an_engine_the_run_did_not_select_against_is_an_escape() {
    let mut report = report_over(vec![recorded_result(Some(Record::Under(vec![])))]);
    report.engine = ply_test::Engine::backend("c:wide");
    let escapes = backend_escapes(&report, &ply_test::Engine::Evaluator);
    assert_eq!(escapes.len(), 1, "the disagreement was not reported");
    assert!(
        escapes[0].message.contains("evaluator"),
        "{}",
        escapes[0].message
    );
    assert!(
        escapes[0].message.contains("c:wide"),
        "{}",
        escapes[0].message
    );
}

/// A run that selected nothing builds no backend, so the runner names the evaluator.
#[test]
fn a_run_that_recorded_nothing_cannot_have_escaped() {
    let mut report = report_over(vec![recorded_result(None)]);
    report.engine = ply_test::Engine::Evaluator;
    assert!(
        backend_escapes(&report, &ply_test::Engine::backend("c:wide")).is_empty(),
        "a run that wrote nothing was reported as writing in the wrong namespace"
    );
}

fn report_over(results: Vec<TestResult>) -> RunReport {
    RunReport {
        engine: ply_test::Engine::Evaluator,
        passed: 0,
        failed: 0,
        abandoned: 0,
        cached: 0,
        failures: Vec::new(),
        duration: Duration::ZERO,
        parallelism: ply_test::Parallelism::default(),
        results,
        warnings: Vec::new(),
        simulation: ply_test::SimSummary::default(),
    }
}

fn index_of(loaded: &Loaded, name: &str) -> usize {
    loaded
        .check
        .tests
        .iter()
        .position(|t| t.name == name)
        .unwrap()
}

#[test]
fn a_cached_pass_over_the_host_fails_the_run_that_wrote_it() {
    let (_dir, loaded, hashes, _plan) = plan_for(None);
    let hosts = bound(&loaded);
    let index = index_of(&loaded, "reads orders only");

    let recorded = |index: usize| {
        report_over(vec![TestResult {
            index,
            name: loaded.check.tests[index].name.clone(),
            hash: hashes.tests.get(index).copied(),
            group: 0,
            duration: Duration::from_millis(1),
            status: Status::Passed,
            failure: None,
            simulation: None,
            recorded: Some(Record::Under(vec![hashes.tests[index]])),
            backend: None,
        }])
    };

    let escaped = recorded(index);
    let escapes = hosts_escapes(&escaped, &loaded.check, &hosts);
    assert_eq!(escapes.len(), 1, "{:?}", escapes);
    assert_eq!(escapes[0].code, codes::INTERNAL_ERROR);
    assert!(escapes[0].message.contains("reads orders only"));
    assert!(
        escapes[0]
            .notes
            .iter()
            .any(|n| n.contains("ply cache clear"))
    );

    // `--host` is not `--no-cache`: a test the binding cannot reach is cached as always.
    let ordinary = recorded(index_of(&loaded, "pure arithmetic"));
    let escapes = hosts_escapes(&ordinary, &loaded.check, &hosts);
    assert!(escapes.is_empty());

    // And hermetically the check costs nothing, because nothing is reachable.
    let hermetic = Hosts::open(
        &loaded.check,
        false,
        &ply_machine::options::TlsOptions::default(),
        &[],
        None,
        ply_cli::config::Configuration::default(),
        &ply_machine::trace::TraceOptions::silent(),
        None,
    )
    .unwrap();
    let escapes = hosts_escapes(&escaped, &loaded.check, &hermetic);
    assert!(escapes.is_empty());
}

#[test]
fn identical_tests_in_two_modules_share_one_cache_entry() {
    let (dir, loaded, hashes) = project(&[
        ("alpha.ply", "test \"same\" { assert_eq(1 + 1, 2) }\n"),
        ("beta.ply", "test \"same\" { assert_eq(1 + 1, 2) }\n"),
    ]);
    assert_eq!(hashes.tests[0], hashes.tests[1]);

    let mut store = Store::open(dir.path()).unwrap();
    let selected = ply_test::select(
        &loaded.check,
        &hashes,
        &store,
        &SimPlan::default(),
        &ply_test::Engine::Evaluator,
    );
    assert_eq!(selected.to_run.len(), 2);
    run(&loaded, &selected, &mut store);

    let store = Store::open(dir.path()).unwrap();
    let again = ply_test::select(
        &loaded.check,
        &hashes,
        &store,
        &SimPlan::default(),
        &ply_test::Engine::Evaluator,
    );
    assert!(again.to_run.is_empty());
    assert_eq!(again.cached.len(), 2);
}
