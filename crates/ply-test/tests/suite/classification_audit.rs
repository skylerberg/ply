//! What the defect/program-error split does with *every* code an evaluator can attach to a failure,
//! and what each `Skipped` variant is actually applied to.

use ply_core::CheckOutput;
use ply_eval::Plan;
use ply_hash::HashOutput;
use ply_span::{Diagnostic, Severity, SourceId, Span, codes};
use ply_store::Store;
use ply_syntax::resolve::Resolved;
use ply_test::{
    Baseline, Bisection, Executor, Gate, Mode, Options, RunReport, Skipped, Status, Verdict,
    precheck, run_with, select,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new() -> TempRoot {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ply-classification-audit-{}-{}",
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

const CORPUS: &str = r#"
fn double(x: Int) -> Int = x * 2

test "double doubles" { assert_eq(double(4), 8) }
"#;

struct Compiled {
    check: CheckOutput,
    hashes: HashOutput,
}

impl Compiled {
    fn new(src: &str) -> Compiled {
        let module = ply_syntax::parse(SourceId(0), src).expect("the fixture must parse");
        let mut program = ply_syntax::ast::Program::single(module);
        let resolved: Resolved = ply_syntax::resolve(&mut program)
            .unwrap_or_else(|d| panic!("the fixture must resolve: {d:#?}"));
        let check = ply_core::check_program(&program, &resolved)
            .unwrap_or_else(|d| panic!("the fixture must typecheck: {d:#?}"));
        let hashes = ply_hash::hash_program(&program, &resolved, &check)
            .unwrap_or_else(|d| panic!("the fixture must hash: {d:#?}"));
        Compiled { check, hashes }
    }
}

/// Answers with whatever the case wants the evaluator to have said, so the classifier is measured
/// against a code rather than against whichever program happens to produce it today.
struct Answering {
    diagnostic: Option<Diagnostic>,
    unwind: bool,
}

impl Executor for Answering {
    type Worker = ();

    fn worker(&self) {}

    fn execute(&self, _worker: &mut (), _index: usize) -> Result<(), Diagnostic> {
        if self.unwind {
            panic!("the evaluator lost its footing");
        }
        match &self.diagnostic {
            Some(d) => Err(d.clone()),
            None => Ok(()),
        }
    }
}

fn report_for(executor: &Answering) -> RunReport {
    let root = TempRoot::new();
    let mut store = root.store();
    let compiled = Compiled::new(CORPUS);
    let selection = select(&compiled.check, &compiled.hashes, &store, &Plan::default());
    run_with(
        &selection,
        &compiled.check,
        &compiled.hashes,
        &mut store,
        executor,
    )
}

fn classified(code: &'static str) -> (bool, Status, Verdict) {
    let report = report_for(&Answering {
        diagnostic: Some(
            Diagnostic::error(code, "the fixture's failure")
                .primary(Span::new(SourceId(0), 0, 1), "here"),
        ),
        unwind: false,
    });
    assert_eq!(report.failures.len(), 1, "{code} must produce one failure");
    (
        report.failures[0].defect,
        report.results[0].status,
        report.failures[0].attribution.bisection.verdict,
    )
}

/// Every code the language uses for "the program did something the language defines it may do".
#[test]
fn no_program_level_code_is_read_as_a_defect_in_ply() {
    for code in [
        codes::ASSERTION_FAILED,
        codes::RUNTIME_ERROR,
        codes::NON_EXHAUSTIVE_MATCH,
        codes::ARITY_MISMATCH,
        codes::UNHANDLED_EFFECT,
        codes::RESOURCE_REQUIRED,
        codes::UNKNOWN_NAME,
        codes::UNKNOWN_OPERATION,
    ] {
        let (defect, status, verdict) = classified(code);
        assert!(!defect, "{code} was read as a defect in Ply");
        assert_eq!(status, Status::Failed, "{code}");
        assert_ne!(
            verdict,
            Verdict::NotAttempted(Skipped::Panicked),
            "{code} had its bisection suppressed"
        );
    }
}

/// The other direction, and the more expensive one.
#[test]
fn an_internal_error_and_an_unwind_are_both_defects() {
    let (defect, status, verdict) = classified(codes::INTERNAL_ERROR);
    assert!(defect);
    assert_eq!(status, Status::Panicked);
    assert_eq!(verdict, Verdict::NotAttempted(Skipped::Panicked));

    let report = report_for(&Answering {
        diagnostic: None,
        unwind: true,
    });
    assert!(report.failures[0].defect, "an unwind is a defect");
    assert_eq!(report.results[0].status, Status::Panicked);
    assert_eq!(
        report.failures[0].attribution.bisection.verdict,
        Verdict::NotAttempted(Skipped::Panicked)
    );
    assert_eq!(
        report.failures[0].diagnostic.code,
        codes::INTERNAL_ERROR,
        "an unwind is rendered as the internal-error code so a JSON consumer \
         reading only the code agrees with `defect`"
    );
}

/// A warning is not a failure, and the severity is not what the split reads.
#[test]
fn a_non_error_severity_is_still_a_failure() {
    let report = report_for(&Answering {
        diagnostic: Some(Diagnostic::warning(codes::RUNTIME_ERROR, "odd but red")),
        unwind: false,
    });
    assert_eq!(report.failed, 1);
    assert_eq!(report.results[0].status, Status::Failed);
    assert!(!report.failures[0].defect);
}

/// **A divergence between a backend and the machine is a defect in Ply.**
#[test]
fn an_engine_divergence_is_a_defect_in_ply() {
    let (defect, status, verdict) = classified(codes::ENGINE_DIVERGENCE);
    assert!(defect, "a divergence is Ply's fault, not the program's");
    assert_eq!(status, Status::Panicked);
    assert_eq!(verdict, Verdict::NotAttempted(Skipped::Panicked));
}

// ---------------------------------------------- one variant, one description

fn hash(seed: &str) -> ply_hash::DefHash {
    ply_hash::DefHash::from_hex(&seed.repeat(32)).expect("a well-formed hash")
}

fn baseline() -> Baseline {
    Baseline::with_decls(hash("a1"), BTreeMap::new(), BTreeMap::new())
}

/// Each variant answers a different question for a consumer, so each has to be reachable from
/// exactly the condition its `describe` claims — and the order has to be the order the answers are
/// worth.
#[test]
fn precheck_maps_each_condition_to_exactly_its_own_variant() {
    struct Case {
        what: &'static str,
        mode: Mode,
        defect: bool,
        host: bool,
        nondet: bool,
        passed_before: bool,
        want: Result<(), Skipped>,
    }
    let case = |what, mode, defect, host, nondet, passed_before, want| Case {
        what,
        mode,
        defect,
        host,
        nondet,
        passed_before,
        want,
    };
    let cases = [
        case(
            "nothing wrong",
            Mode::Auto,
            false,
            false,
            false,
            true,
            Ok(()),
        ),
        case(
            "bisection turned off",
            Mode::Never,
            false,
            false,
            false,
            true,
            Err(Skipped::NotRequested),
        ),
        case(
            "a defect in Ply",
            Mode::Auto,
            true,
            false,
            false,
            true,
            Err(Skipped::Panicked),
        ),
        case(
            "a failure a host handler answered",
            Mode::Auto,
            false,
            true,
            false,
            true,
            Err(Skipped::Host),
        ),
        case(
            "a nondet test",
            Mode::Auto,
            false,
            false,
            true,
            true,
            Err(Skipped::Nondet),
        ),
        case(
            "no recorded pass",
            Mode::Auto,
            false,
            false,
            false,
            false,
            Err(Skipped::NeverPassed),
        ),
    ];

    for c in &cases {
        let base = baseline();
        let got = precheck(
            Gate::new(c.mode, c.defect, c.nondet, c.passed_before.then_some(&base)).hosted(c.host),
        );
        assert_eq!(got, c.want, "{}", c.what);
    }

    // Precedence, stated as the cases that would be ambiguous otherwise.
    let base = baseline();
    assert_eq!(
        precheck(Gate::new(Mode::Never, true, true, None).hosted(true)),
        Err(Skipped::NotRequested),
        "a run that did not ask learns nothing about why it would have skipped"
    );
    assert_eq!(
        precheck(Gate::new(Mode::Auto, true, true, Some(&base)).hosted(true)),
        Err(Skipped::Panicked),
        "a defect in Ply outranks `nondet` and `host`: the outcome is not the program's either way"
    );
    assert_eq!(
        precheck(Gate::new(Mode::Auto, true, false, None)),
        Err(Skipped::Panicked),
        "a defect in Ply outranks a missing baseline"
    );
    assert_eq!(
        precheck(Gate::new(Mode::Auto, false, true, Some(&base)).hosted(true)),
        Err(Skipped::Host),
        "`host` outranks `nondet`, which nearly every host-backed test also is: \
         one says a hybrid would prove nothing, the other says asking is an act on the world"
    );
    assert_eq!(
        precheck(Gate::new(Mode::Auto, false, false, None).hosted(true)),
        Err(Skipped::Host),
        "`host` outranks a missing baseline: a baseline would not make re-running it safe"
    );
    assert_eq!(
        precheck(Gate::new(Mode::Auto, false, true, None)),
        Err(Skipped::Nondet),
        "`nondet` outranks a missing baseline: a baseline would not have helped"
    );
    assert_eq!(
        precheck(Gate::new(Mode::Always, false, false, None)),
        Err(Skipped::NeverPassed),
        "`--bisect always` lifts the budget, not the need for a baseline"
    );
}

/// `Mode::Always` must not be able to talk the gate out of a skip that is about evidence rather
/// than about effort.
#[test]
fn bisect_always_does_not_override_a_missing_premise() {
    for (defect, host, nondet) in [
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        assert!(
            precheck(Gate::new(Mode::Always, defect, nondet, Some(&baseline())).hosted(host))
                .is_err(),
            "defect={defect} host={host} nondet={nondet}"
        );
    }
}

/// The strings are the artifact's contract with a consumer that branches on them, so no two may
/// collide and none may be empty.
#[test]
fn every_skipped_variant_has_its_own_tag_and_its_own_description() {
    let all = [
        Skipped::NotRequested,
        Skipped::NeverPassed,
        Skipped::Host,
        Skipped::Nondet,
        Skipped::Panicked,
        Skipped::NoChanges,
        Skipped::NoBodies,
        Skipped::NoHybrids,
    ];

    // `all` is a hand-written list and this is what keeps it honest: the `match` makes a new
    // variant a compile error, and the ordinals make forgetting to add it to `all` a failure here
    // rather than a variant nothing ever tested.
    let ordinal = |s: Skipped| match s {
        Skipped::NotRequested => 0,
        Skipped::NeverPassed => 1,
        Skipped::Host => 2,
        Skipped::Nondet => 3,
        Skipped::Panicked => 4,
        Skipped::NoChanges => 5,
        Skipped::NoBodies => 6,
        Skipped::NoHybrids => 7,
    };
    let mut seen: Vec<usize> = all.iter().map(|s| ordinal(*s)).collect();
    seen.sort_unstable();
    assert_eq!(
        seen,
        (0..8).collect::<Vec<_>>(),
        "a `Skipped` variant exists that this audit never sees"
    );

    let mut tags: Vec<&str> = all.iter().map(|s| s.as_str()).collect();
    let mut descriptions: Vec<&str> = all.iter().map(|s| s.describe()).collect();
    assert!(tags.iter().all(|t| !t.is_empty()));
    assert!(descriptions.iter().all(|d| d.len() > 20));
    tags.sort_unstable();
    let before = tags.len();
    tags.dedup();
    assert_eq!(before, tags.len(), "two variants share a tag");
    descriptions.sort_unstable();
    let before = descriptions.len();
    descriptions.dedup();
    assert_eq!(
        before,
        descriptions.len(),
        "two variants share a description"
    );

    // A skip is never a verdict about the program: a consumer must be able to tell "no answer" from
    // "the answer is the empty set".
    for skipped in all {
        let bisection = Bisection::not_attempted(skipped);
        assert_eq!(bisection.verdict, Verdict::NotAttempted(skipped));
        assert_eq!(bisection.verdict.skipped(), Some(skipped));
        assert!(
            bisection.culprits().is_empty(),
            "{} named a definition without running anything",
            skipped.as_str()
        );
        assert_eq!(bisection.reason, skipped.describe());
    }
}

/// `NoChanges` is the one skip that is a statement about the *program* rather than about the run or
/// the build, so it has to be reachable from an empty delta and from nothing else.
#[test]
fn no_changes_is_reached_only_when_nothing_moved() {
    use ply_test::bisect::{NoHybrid, bisect};
    use ply_test::{Budget, Change, Confidence, Delta, DepEdges};

    let empty = Delta::default();
    let out = bisect(&empty, &mut NoHybrid, Budget::DEFAULT);
    assert_eq!(out.verdict, Verdict::NotAttempted(Skipped::NoChanges));
    assert!(out.culprits().is_empty());
    assert_eq!(out.confidence, Confidence::None);
    assert_eq!(out.search.evaluated, 0, "a skip runs nothing");

    let name = ply_span::Symbol::new("m.f");
    let edited = Change::edited(name.clone(), hash("a1"), hash("b2"), true);
    let moved = Delta::new(None, vec![edited], &DepEdges::from(&HashOutput::default()));
    let out = bisect(&moved, &mut NoHybrid, Budget::DEFAULT);
    assert_eq!(
        out.verdict,
        Verdict::Sole,
        "one change that moved is an answer, not `no_changes`"
    );
    assert_eq!(out.groups, vec![vec![name]], "{:?}", out.groups);

    // A change that is only `Derived` is no candidate, so the delta is again empty — and
    // `no_changes` is the honest answer even though a hash moved.
    let derived = Change::derived(ply_span::Symbol::new("m.g"), hash("c3"), hash("d4"));
    let only_derived = Delta::new(None, vec![derived], &DepEdges::from(&HashOutput::default()));
    assert_eq!(
        bisect(&only_derived, &mut NoHybrid, Budget::DEFAULT).verdict,
        Verdict::NotAttempted(Skipped::NoChanges)
    );
}

/// `NoHybrids` says "this build cannot mix two eras", which is only an honest answer to a question
/// that needed a mixture.
#[test]
fn no_hybrids_is_reported_only_where_a_mixture_was_actually_needed() {
    use ply_span::Symbol;
    use ply_test::bisect::Unknown;
    use ply_test::{DepEdges, Evidence, diagnose};

    let hashes = HashOutput::default();
    let edges = DepEdges::from(&hashes);
    let key = Symbol::new("m.a test");
    let base = baseline();

    let attribution = diagnose(
        Evidence {
            key: &key,
            test_hash: None,
            nondet: false,
            defect: false,
            host: false,
            suspects: &[],
            hashes: &hashes,
            baseline: Some(&base),
            slice: None,
        },
        &Options::default(),
        &edges,
        &mut Unknown,
        None,
        Skipped::NoHybrids,
    );
    assert_eq!(
        attribution.bisection.verdict,
        Verdict::NotAttempted(Skipped::NoChanges),
        "an empty closure moved nothing, so the missing hybrid is beside the point"
    );
}

/// `Skipped::Panicked` still says "the interpreter failed rather than the program".
#[test]
fn the_panicked_description_still_describes_only_ply_defects() {
    let described = Skipped::Panicked.describe();
    assert!(described.contains("defect in Ply"), "{described}");
    assert!(
        described.contains("no change in the program explains it"),
        "{described}"
    );
    for code in [codes::RUNTIME_ERROR, codes::ASSERTION_FAILED] {
        assert_ne!(
            classified(code).2,
            Verdict::NotAttempted(Skipped::Panicked),
            "{code} would be handed a sentence that is false about it"
        );
    }
}

/// `Options::never()` is the only way a caller asks for no bisection at all, and it has to reach
/// `NotRequested` rather than any of the reasons that are about the failure.
#[test]
fn options_never_is_the_only_thing_that_reads_as_not_requested() {
    let never = Options::never();
    assert_eq!(
        precheck(Gate::new(never.bisect, false, false, Some(&baseline()))),
        Err(Skipped::NotRequested)
    );
    let default = Options::default();
    assert_eq!(
        precheck(Gate::new(default.bisect, false, false, Some(&baseline()))),
        Ok(())
    );
}

/// A diagnostic with no span at all still has to be classified — the split reads the code, and a
/// defensive path that forgot its span is exactly where a missing `primary` would come from.
#[test]
fn a_spanless_diagnostic_is_classified_by_its_code_alone() {
    let report = report_for(&Answering {
        diagnostic: Some(Diagnostic::error(codes::RUNTIME_ERROR, "no span here")),
        unwind: false,
    });
    assert!(!report.failures[0].defect);
    assert_eq!(report.results[0].status, Status::Failed);
    assert_eq!(report.failures[0].diagnostic.severity, Severity::Error);
}
