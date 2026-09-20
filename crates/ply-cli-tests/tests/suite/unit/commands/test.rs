use ply_cli::cli::{TestArgs, When};
use ply_cli::commands::common::{backend_spec, exit_code, run_on_tier};
use ply_cli::commands::test::*;
use ply_cli::hosts::Hosts;
use ply_cli::load::{Loaded, load};
use ply_cli::style::Style;
use ply_eval::Plan as SimPlan;
use ply_span::{Symbol, codes};
use ply_store::{Outcome, Store};
use ply_test::{
    Bisection, Isolation, Reason, Record, RunReport, Selection, Skipped, Status, Suspect,
    TestResult, Verdict,
};
use ply_ty::HashOutput;
use serde_json::{Value, json};
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

/// Opens the binding as `execute` does, so a test cannot assert about a shape the real command does not produce.
fn json_report(
    loaded: &Loaded,
    hashes: &HashOutput,
    plan: &Plan,
    report: &RunReport,
    args: &TestArgs,
    workers: usize,
) -> Value {
    let hosts = Hosts::open(
        &loaded.check,
        args.host,
        &[],
        &[],
        None,
        ply_cli::config::Configuration::default(),
        &ply_cli::trace::TraceOptions::silent(),
        None,
    )
    .expect("the fixture binds");
    let view = HostView::of(&hosts, plan, &loaded.check, report);
    let backend = BackendView::of(None, None, report, &ply_test::Engine::Evaluator);
    let ok = report.is_success() && view.escapes.is_empty();
    report_json(
        loaded,
        hashes,
        plan,
        report,
        args,
        workers,
        &[],
        &view,
        &backend,
        ok,
    )
}

fn args_for(filter: Option<&str>) -> TestArgs {
    TestArgs {
        timeout: 60_000,
        profile: "development".to_string(),
        watch: false,
        path: PathBuf::from("."),
        json: true,
        explain: false,
        no_incremental: false,
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
        db: ply_cli::db::DbOptions::default(),
        config: ply_cli::config::ConfigOptions::default(),
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
fn a_finished_run_holds_what_it_loaded() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "m.ply",
        "fn f() -> Int = 1\ntest \"f is one\" { assert_eq(f(), 1) }\n",
    );
    let args = TestArgs {
        path: dir.path().to_path_buf(),
        ..args_for(None)
    };
    let mut warm = ply_cli::warm::Warm::default();
    assert_eq!(
        execute_holding(&args, Style::plain(), &mut warm),
        exit_code(true)
    );
    let loaded = warm
        .held
        .as_ref()
        .expect("a finished run holds what it loaded");
    assert!(loaded.check.tests.iter().any(|t| t.name == "f is one"));
}

/// The hashes do not cover layout, so `--watch` keeps the unit across a save that moves a test,
/// and the unit has to report the failure where the test now is.
#[test]
fn a_held_unit_reports_a_failure_where_its_moved_test_now_is() {
    let dir = tempfile::tempdir().unwrap();
    let args = TestArgs {
        path: dir.path().to_path_buf(),
        ..args_for(None)
    };
    let spec = backend_spec(None).unwrap().expect("a default tier");
    let mut warm = ply_cli::warm::Warm::default();
    let mut save_and_run = |text: &str| {
        write(dir.path(), "m.ply", text);
        assert_eq!(
            execute_holding(&args, Style::plain(), &mut warm),
            ply_cli::EXIT_FAILED
        );
        let loaded = warm
            .held
            .as_ref()
            .expect("a finished run holds what it loaded");
        let unit = warm
            .unit_for(&spec, &loaded.front, &loaded.sources)
            .expect("a finished run holds the unit it ran on");
        let mut machine = ply_eval::Machine::new(&loaded.front);
        machine.set_compiled(unit.attach(&spec));
        let failure = machine
            .eval_test(index_of(loaded, "wrong"))
            .expect_err("the test fails");
        // Not a refusal, whose span is the front's and would move with no unit at all.
        assert_eq!(failure.code, codes::ASSERTION_FAILED);
        let site = failure
            .primary_span()
            .expect("a compiled failure names its site");
        let at = loaded
            .sources
            .get(site.source)
            .unwrap()
            .line_col(site.start);
        (unit, at)
    };

    let (first, (line, column)) =
        save_and_run("fn f() -> Int = 1\ntest \"wrong\" { assert_eq(f(), 2) }\n");
    let (moved, at) =
        save_and_run("fn f() -> Int = 1\n\n\n\ntest \"wrong\" { assert_eq(f(), 2) }\n");
    assert!(
        std::ptr::addr_eq(first, moved),
        "only the layout moved, so the second run reused the unit"
    );
    assert_eq!((line, at), (2, (5, column)));

    // A site is an offset into its test's own text, so an edit there is rebuilt rather than moved.
    let (edited, at) =
        save_and_run("fn f() -> Int = 1\n\n\n\ntest \"wrong\" {  assert_eq(f(), 2) }\n");
    assert!(!std::ptr::addr_eq(moved, edited));
    assert_eq!(at, (5, column + 1));
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
    let (_dir, loaded, _h, plan) = plan_for(Some("no such test"));
    assert_eq!(plan.selection.total, 0);
    assert!(plan.selection.to_run.is_empty());
    assert!(plan.selection.groups.is_empty());
    assert!(no_tests_note(&loaded, &args_for(Some("no such test"))).contains("substring"));
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

#[test]
fn the_json_report_is_one_object_with_selection_results_and_failures() {
    let (dir, loaded, hashes) = project(&[(
        "m.ply",
        "fn f() -> Int = 1\n\
         test \"good\" { assert_eq(f(), 1) }\n\
         test \"bad\" { assert_eq(f(), 2) }\n",
    )]);
    let mut store = Store::open(dir.path()).unwrap();
    let plan = Plan::new(
        ply_test::select(
            &loaded.check,
            &hashes,
            &store,
            &SimPlan::default(),
            &ply_test::Engine::Evaluator,
        ),
        &loaded.check,
        None,
        false,
    );
    let report = run(&loaded, &plan.selection, &mut store);

    let v = json_report(&loaded, &hashes, &plan, &report, &args_for(None), 4);

    assert_eq!(v["command"], "test");
    assert_eq!(v["ok"], false);
    assert_eq!(v["exit_code"], 1);
    assert_eq!(v["selection"]["total"], 2);
    assert_eq!(v["selection"]["selected"], 2);
    assert_eq!(v["selection"]["tests"][0]["reason"], "new");
    assert_eq!(v["selection"]["tests"][0]["key"], "m.good");
    assert_eq!(v["selection"]["tests"][0]["module"], "m");
    assert_eq!(
        v["selection"]["tests"][0]["hash"].as_str().unwrap().len(),
        64
    );
    assert_eq!(v["modules"][0]["name"], "m");
    assert_eq!(v["failures"].as_array().unwrap().len(), 1);
    assert_eq!(
        v["failures"][0]["diagnostic"]["code"],
        codes::ASSERTION_FAILED
    );
    assert_eq!(
        v["failures"][0]["diagnostic"]["labels"][0]["start"]["line"],
        3
    );
    // `f` is in the failing test's closure and has never gone green.
    assert_eq!(v["failures"][0]["suspects"][0]["name"], "m.f");
    assert_eq!(v["failures"][0]["suspects"][0]["culprit"], false);
    let at = &v["failures"][0]["location"];
    assert!(at["file"].as_str().unwrap().ends_with("m.ply"));
    assert_eq!(at["line"], 3);
    assert_eq!(v["summary"]["failed"], 1);
    assert_eq!(v["summary"]["passed"], 1);
}

#[test]
fn the_failure_artifact_carries_the_v4_shape_an_agent_branches_on() {
    let (dir, loaded, hashes) = project(&[(
        "ledger.ply",
        "fn balance() -> Int = 0 - 5\n\
         test \"balance never goes negative\" { assert_eq(balance(), 0) }\n",
    )]);
    let mut store = Store::open(dir.path()).unwrap();
    let plan = Plan::new(
        ply_test::select(
            &loaded.check,
            &hashes,
            &store,
            &SimPlan::default(),
            &ply_test::Engine::Evaluator,
        ),
        &loaded.check,
        None,
        false,
    );
    let report = run(&loaded, &plan.selection, &mut store);

    let v = json_report(&loaded, &hashes, &plan, &report, &args_for(None), 1);
    assert_eq!(v["schema_version"], 4);

    let f = &v["failures"][0];
    assert_eq!(f["key"], "ledger.balance never goes negative");
    assert_eq!(f["name"], "balance never goes negative");
    assert_eq!(f["module"], "ledger");
    assert_eq!(f["nondet"], false);
    assert_eq!(f["status"], "failed");
    assert_eq!(f["test_hash"].as_str().unwrap().len(), 64);
    assert_eq!(f["diagnostic"]["code"], codes::ASSERTION_FAILED);
    assert!(
        f["location"]["file"]
            .as_str()
            .unwrap()
            .ends_with("ledger.ply")
    );

    // Present even without evidence: a vanished field and a "not known" field are different answers.
    assert!(f["culprit"]["verdict"].is_string());
    assert!(f["culprit"]["confidence"].is_string());
    assert!(f["culprit"]["definitions"].is_array());
    assert!(f["culprit"]["groups"].is_array());
    assert!(f["culprit"]["reason"].is_string());
    assert_eq!(f["culprit"]["search"]["evaluated"], 0);
    assert!(
        f["assertion"].is_null(),
        "the evaluator carries no payload yet"
    );
    assert!(f["seed"].is_null());
    assert!(f["replay"].is_null());
    assert!(f["race"].is_null());
    assert!(f["causal_slice"].is_null(), "nothing traced this run");
    assert_eq!(f["footprint"]["declared"], json!([]));
    assert!(
        f["footprint"]["observed"].is_null(),
        "an untraced run must not claim an empty observed footprint"
    );
}

#[test]
fn the_artifact_is_byte_identical_across_two_runs_over_one_failure() {
    let (dir, loaded, hashes) = project(&[(
        "m.ply",
        "fn a() -> Int = 1\nfn b() -> Int = 2\n\
         test \"wrong\" { assert_eq(a() + b(), 4) }\n",
    )]);
    let render = || {
        let mut store = Store::open(dir.path()).unwrap();
        let plan = Plan::new(
            ply_test::select(
                &loaded.check,
                &hashes,
                &store,
                &SimPlan::default(),
                &ply_test::Engine::Evaluator,
            ),
            &loaded.check,
            None,
            false,
        );
        let report = run(&loaded, &plan.selection, &mut store);
        serde_json::to_string(
            &json_report(&loaded, &hashes, &plan, &report, &args_for(None), 1)["failures"],
        )
        .unwrap()
    };
    assert_eq!(render(), render());
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
    let (_dir, loaded, hashes, mut report) = failing(ONE_FAILURE);
    let mut args = args_for(None);
    args.bisect = When::Never;
    ply_test::diagnose_failures(
        &mut report,
        &loaded.texts(),
        &loaded.front,
        &mut Store::open(_dir.path()).unwrap(),
        &diagnosis_options(&args),
    );

    let bisection = &report.failures[0].attribution.bisection;
    assert_eq!(
        bisection.verdict,
        Verdict::NotAttempted(Skipped::NotRequested)
    );
    assert_eq!(bisection.search.evaluated, 0);

    let plan = Plan::new(
        ply_test::select(
            &loaded.check,
            &hashes,
            &Store::open(_dir.path()).unwrap(),
            &SimPlan::default(),
            &ply_test::Engine::Evaluator,
        ),
        &loaded.check,
        None,
        false,
    );
    let v = json_report(&loaded, &hashes, &plan, &report, &args, 1);
    assert_eq!(v["failures"][0]["culprit"]["verdict"], "not_attempted");
    assert_eq!(v["failures"][0]["culprit"]["skipped"], "not_requested");
    assert_eq!(v["options"]["bisect"], "never");
}

#[test]
fn the_culprit_line_comes_above_the_assertion_and_is_absent_when_there_is_none() {
    let (_dir, loaded, _h, mut report) = failing(ONE_FAILURE);

    let rendered = |report: &RunReport| failure_lines(&report.failures[0], &loaded, Style::plain());

    let quiet = rendered(&report);
    assert!(
        !quiet.iter().any(|l| l.contains("culprit")),
        "an unrequested bisection has nothing to apologize for: {quiet:?}"
    );
    assert!(quiet.iter().any(|l| l.contains("assertion failed")));

    let slice = report.failures[0].attribution.slice.clone();
    report.failures[0].attribution.resolve(
        Bisection {
            verdict: Verdict::Sole,
            confidence: ply_test::Confidence::Minimal,
            groups: vec![vec![Symbol::new("m.f")]],
            reason: "one definition changed".into(),
            search: ply_test::SearchStats::default(),
        },
        slice,
    );
    let loud = rendered(&report);
    let culprit = loud
        .iter()
        .position(|l| l.contains("culprit: m.f"))
        .expect("a conclusive bisection must name its culprit");
    let assertion = loud
        .iter()
        .position(|l| l.contains("assertion failed"))
        .expect("the diff is still the evidence");
    assert!(
        culprit < assertion,
        "the culprit is the answer and must precede the evidence: {loud:?}"
    );
    assert!(loud.iter().any(|l| l.contains("one definition changed")));

    let v = failure_json(&report.failures[0], &loaded, &_h, &report);
    assert_eq!(v["culprit"]["verdict"], "sole");
    assert_eq!(v["culprit"]["definitions"], json!(["m.f"]));
    assert_eq!(v["suspects"][0]["name"], "m.f");
    assert_eq!(v["suspects"][0]["culprit"], true);
}

/// E0414's whole value is the cycle, and the cycle lives in the secondary labels.
#[test]
fn a_deadlock_names_every_blocked_task_and_what_it_waits_on() {
    const DEADLOCK: &str = "\
type Slot =
  | Empty
  | Peer(Task<Int>)

test \"stuck\" {
  simulate {
with_cell[slot](Empty) { peer -> {
  let first = task.spawn(|| {
    clock.sleep(1);
    match cell_get(peer) {
      Peer(other) -> task.join(other),
      Empty -> 0,
    }
  });
  let second = task.spawn(|| task.join(first));
  cell_set(peer, Peer(second));
  task.join(first)
} }
  }
}
";
    let (_dir, loaded, _hashes, report) = failing(DEADLOCK);
    assert_eq!(report.failures[0].diagnostic.code, codes::DEADLOCK);

    let lines = failure_lines(&report.failures[0], &loaded, Style::plain());
    for waiting in [
        "@0 waits here for @1 to finish",
        "@1 waits here for @2 to finish",
        "@2 waits here for @1 to finish",
    ] {
        let line = lines
            .iter()
            .find(|l| l.contains(waiting))
            .unwrap_or_else(|| panic!("`{waiting}` is missing from {lines:?}"));
        assert!(
            line.contains("m.ply:"),
            "a wait without a location is not actionable: {line}"
        );
    }
}

#[test]
fn a_suspect_reads_as_a_reason_to_skip_it_rather_than_a_bare_name() {
    let plain = Suspect::new(Symbol::new("m.f"), None);
    assert_eq!(describe_suspect(&plain), "m.f");

    let mut derived = Suspect::new(Symbol::new("m.post"), None);
    derived.change = Some(ply_test::ChangeKind::Derived);
    derived.ran = Some(false);
    assert_eq!(describe_suspect(&derived), "m.post (derived, did not run)");

    let mut returned = Suspect::new(Symbol::new("m.setup"), None);
    returned.change = Some(ply_test::ChangeKind::Edited);
    returned.ran = Some(true);
    assert_eq!(
        describe_suspect(&returned),
        "m.setup (edited, ran, then returned)"
    );
}

#[test]
fn two_failures_sharing_a_label_are_told_apart_by_their_key() {
    let (dir, loaded, hashes) = project(&[
        (
            "alpha.ply",
            "fn one() -> Int = 1\ntest \"it adds up\" { assert_eq(one(), 2) }\n",
        ),
        (
            "beta.ply",
            "fn two() -> Int = 2\ntest \"it adds up\" { assert_eq(two(), 3) }\n",
        ),
    ]);
    let mut store = Store::open(dir.path()).unwrap();
    let plan = Plan::new(
        ply_test::select(
            &loaded.check,
            &hashes,
            &store,
            &SimPlan::default(),
            &ply_test::Engine::Evaluator,
        ),
        &loaded.check,
        None,
        false,
    );
    let report = run(&loaded, &plan.selection, &mut store);
    assert_eq!(report.failed, 2);

    let v = json_report(&loaded, &hashes, &plan, &report, &args_for(None), 4);
    let keys: Vec<&str> = v["failures"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["alpha.it adds up", "beta.it adds up"]);
    for f in v["failures"].as_array().unwrap() {
        assert_eq!(f["name"], "it adds up");
    }
}

#[test]
fn every_reason_has_a_distinct_explanation() {
    let all = [
        Reason::New,
        Reason::Nondet,
        Reason::PreviousFailure,
        Reason::Cached,
        Reason::Unhashed,
    ];
    let mut seen: Vec<&str> = all.iter().map(|r| why(*r)).collect();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), all.len());
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
fn a_label_is_shown_bare_in_one_module_and_qualified_across_several() {
    let (_dir, one, _h) = project(&[("m.ply", "test \"only\" { assert_eq(1, 1) }\n")]);
    assert_eq!(display_name(&one.check, 0, "fallback"), "only");

    let (_dir, two, _h) = project(&[
        ("alpha.ply", "test \"shared\" { assert_eq(1, 1) }\n"),
        ("beta.ply", "test \"shared\" { assert_eq(2, 2) }\n"),
    ]);
    assert_eq!(display_name(&two.check, 0, "fallback"), "alpha.shared");
    assert_eq!(display_name(&two.check, 1, "fallback"), "beta.shared");
    assert_eq!(display_name(&two.check, 9, "fallback"), "fallback");
}

#[test]
fn marks_are_ascii_when_unstyled_and_glyphs_when_styled() {
    let result = TestResult {
        index: 0,
        name: "balance never goes negative".into(),
        hash: None,
        group: 0,
        duration: Duration::from_micros(2100),
        status: Status::Failed,
        failure: None,
        simulation: None,
        recorded: None,
        backend: None,
    };
    let plain = result_line(&result, &result.name, 44, Style::plain());
    assert!(plain.starts_with("FAIL "));
    assert!(!plain.contains('\x1b'));
    assert!(plain.contains("balance never goes negative"));
    assert!(plain.trim_end().ends_with("2.1ms"));

    let styled = result_line(&result, &result.name, 44, Style::new(true));
    assert!(styled.contains('✗'));
    assert!(styled.contains('\x1b'));
}

#[test]
fn a_reduction_over_a_bounded_naive_count_is_reported_as_a_bound() {
    let line = |naive: ply_eval::Naive| {
        simulation_line(&TestResult {
            index: 0,
            name: "n".into(),
            hash: None,
            group: 0,
            duration: Duration::from_millis(1),
            status: Status::Passed,
            failure: None,
            simulation: Some(ply_eval::Exploration {
                explored: 1,
                exhaustive: true,
                naive: Some(naive),
                ..Default::default()
            }),
            recorded: None,
            backend: None,
        })
        .expect("a simulated test has a line")
    };

    let exact = line(ply_eval::Naive {
        explored: 720,
        bounded: false,
    });
    assert!(exact.contains("naive 720"), "{exact}");
    assert!(exact.contains("720× reduction"), "{exact}");
    assert!(!exact.contains(">="), "{exact}");

    let bounded = line(ply_eval::Naive {
        explored: 4096,
        bounded: true,
    });
    assert!(bounded.contains("naive >= 4096"), "{bounded}");
    assert!(bounded.contains(">= 4096× reduction"), "{bounded}");
}

#[test]
fn the_pass_and_panic_marks_line_up_with_the_failure_mark() {
    let make = |status| TestResult {
        index: 0,
        name: "n".into(),
        hash: None,
        group: 0,
        duration: Duration::from_millis(1),
        status,
        failure: None,
        simulation: None,
        recorded: None,
        backend: None,
    };
    let column = |status| {
        result_line(&make(status), "n", 24, Style::plain())
            .find("n ")
            .unwrap()
    };
    assert_eq!(column(Status::Passed), column(Status::Failed));
    assert_eq!(column(Status::Passed), column(Status::Panicked));
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
fn a_host_backed_test_is_reported_as_host_rather_than_world() {
    let (_dir, loaded, _h, plan) = plan_for(None);
    let hosts = bound(&loaded);
    let view = HostView::of(&hosts, &plan, &loaded.check, &report_over(Vec::new()));
    let label = |name: &str| {
        let index = index_of(&loaded, name);
        let footprint = &loaded.check.tests[index].footprint;
        isolation_label(&plan, &view, &loaded.check, index, footprint)
    };

    assert_eq!(label("reads orders only"), "host");
    assert_eq!(label("reads orders only again"), "host");
    assert_eq!(label("writes users when asked"), "shared");
    assert_eq!(label("pure arithmetic"), "region");

    assert_eq!(view.counts.total, 4);
    assert_eq!(view.counts.host, 2);
    assert_eq!(view.counts.isolated, 1);
    assert_eq!(view.counts.shared, 1);
}

#[test]
fn a_hermetic_run_reports_exactly_what_it_did_before() {
    let (_dir, loaded, _h, plan) = plan_for(None);
    let hosts = Hosts::open(
        &loaded.check,
        false,
        &[],
        &[],
        None,
        ply_cli::config::Configuration::default(),
        &ply_cli::trace::TraceOptions::silent(),
        None,
    )
    .unwrap();
    let view = HostView::of(&hosts, &plan, &loaded.check, &report_over(Vec::new()));

    for (index, test) in loaded.check.tests.iter().enumerate() {
        let expected = plan
            .selection
            .isolation_of(index)
            .unwrap_or_else(|| Isolation::of(&test.footprint));
        assert_eq!(
            isolation_label(&plan, &view, &loaded.check, index, &test.footprint),
            expected.as_str()
        );
    }
    let parallelism = &plan.selection.parallelism;
    assert_eq!(view.counts.host, 0);
    assert_eq!(view.counts.isolated, parallelism.isolated);
    assert_eq!(view.counts.shared, parallelism.shared);
    assert_eq!(view.counts.total, parallelism.total);
    assert!(view.escapes.is_empty());
}

#[test]
fn a_cached_pass_over_the_host_fails_the_run_that_wrote_it() {
    let (_dir, loaded, hashes, plan) = plan_for(None);
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
    let view = HostView::of(&hosts, &plan, &loaded.check, &escaped);
    assert_eq!(view.escapes.len(), 1, "{:?}", view.escapes);
    assert_eq!(view.escapes[0].code, codes::INTERNAL_ERROR);
    assert!(view.escapes[0].message.contains("reads orders only"));
    assert!(
        view.escapes[0]
            .notes
            .iter()
            .any(|n| n.contains("ply cache clear"))
    );

    // `--host` is not `--no-cache`: a test the binding cannot reach is cached as always.
    let ordinary = recorded(index_of(&loaded, "pure arithmetic"));
    let view = HostView::of(&hosts, &plan, &loaded.check, &ordinary);
    assert!(view.escapes.is_empty());

    // And hermetically the check costs nothing, because nothing is reachable.
    let hermetic = Hosts::open(
        &loaded.check,
        false,
        &[],
        &[],
        None,
        ply_cli::config::Configuration::default(),
        &ply_cli::trace::TraceOptions::silent(),
        None,
    )
    .unwrap();
    let view = HostView::of(&hermetic, &plan, &loaded.check, &escaped);
    assert!(view.escapes.is_empty());
}

#[test]
fn a_database_backed_test_is_host_backed_never_cached_and_says_which_database() {
    use crate::unit::hosts::fixture::{deterministic, named, op, registry};
    let (_dir, loaded, hashes, plan) = plan_for(None);
    let config = ply_cli::db::DbOptions {
        url: Some("postgres://ply:hunter2@127.0.0.1:5433/desk".to_string()),
        ..ply_cli::db::DbOptions::default()
    }
    .resolve_with(true, &|_| None)
    .expect("the fixture URL parses");
    let hosts = Hosts::bind_with(
        registry(vec![deterministic(op(
            "db",
            "all",
            named("users"),
            ply_eval::host::Linearity::AtMostOnce,
            true,
            "ply_host::db::query",
        ))]),
        &loaded.check,
        true,
        config,
    )
    .expect("the fixture binds");

    assert!(hosts.is_live_database());
    let line = ply_cli::hosts::database_line(&hosts).expect("a live database is reported");
    assert!(
        line.contains("postgres://ply:****@127.0.0.1:5433/desk"),
        "{line}"
    );
    assert!(!line.contains("hunter2"), "{line}");

    let index = index_of(&loaded, "reads orders only");
    let view = HostView::of(&hosts, &plan, &loaded.check, &report_over(Vec::new()));
    assert_eq!(view.counts.host, 2);
    assert_eq!(
        isolation_label(
            &plan,
            &view,
            &loaded.check,
            index,
            &loaded.check.tests[index].footprint
        ),
        "host"
    );

    let recorded = report_over(vec![TestResult {
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
    }]);
    let view = HostView::of(&hosts, &plan, &loaded.check, &recorded);
    assert_eq!(
        view.escapes.len(),
        1,
        "a pass over a real database was kept"
    );
    assert_eq!(view.escapes[0].code, codes::INTERNAL_ERROR);
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
