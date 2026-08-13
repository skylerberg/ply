//! What the defect/program-error split does with *every* code an evaluator can
//! attach to a failure, and what each `Skipped` variant is actually applied to.
//!
//! The classifier is two clauses — the run watched an unwind, or the diagnostic
//! said `INTERNAL_ERROR` — so its correctness is entirely a claim about the
//! codes the nine crates emit. A code that arrives here and means "Ply is
//! broken" without saying `E0505` is silently reported as the user's bug; a
//! code that means "your program is wrong" and says `E0505` costs the failure
//! its bisection. Both directions are enumerated below rather than sampled,
//! because the cost of the split being wrong is paid per class and not per run.
//!
//! Where a case shows the split getting it wrong the test is named `documents_`
//! and its doc comment says what the right answer is.

use ply_core::CheckOutput;
use ply_hash::HashOutput;
use ply_span::{Diagnostic, Severity, SourceId, Span, codes};
use ply_store::Store;
use ply_syntax::resolve::Resolved;
use ply_test::{
    Baseline, Bisection, Executor, Mode, Options, RunReport, Skipped, Status, Verdict, precheck,
    run_with, select,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

// ------------------------------------------------------------------ harness

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
        let program = ply_syntax::ast::Program::single(module);
        let resolved: Resolved = ply_syntax::resolve(&program)
            .unwrap_or_else(|d| panic!("the fixture must resolve: {d:#?}"));
        let check = ply_core::check_program(&program, &resolved)
            .unwrap_or_else(|d| panic!("the fixture must typecheck: {d:#?}"));
        let hashes = ply_hash::hash_program(&program, &resolved, &check)
            .unwrap_or_else(|d| panic!("the fixture must hash: {d:#?}"));
        Compiled { check, hashes }
    }
}

/// Answers with whatever the case wants the evaluator to have said, so the
/// classifier is measured against a code rather than against whichever program
/// happens to produce it today.
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
    let selection = select(&compiled.check, &compiled.hashes, &store);
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

// ------------------------------------------------- the two clauses, directly

/// Every code the language uses for "the program did something the language
/// defines it may do". None of them may set `defect`, because each is a
/// behaviour an edit can introduce and remove, and the bisection is the whole
/// value of the artifact.
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

/// The other direction, and the more expensive one. `INTERNAL_ERROR` is the
/// only code that says Ply broke its own invariant, and an unwind is the only
/// evidence that needs no code at all.
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

/// A warning is not a failure, and the severity is not what the split reads. If
/// an evaluator ever hands back `Err` on a non-error diagnostic the run still
/// has to call it a failure — silently passing would cache a green result for a
/// test that did not finish.
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

/// **A divergence between the two engines is a defect in Ply.** `E0503` is
/// emitted only by the `--engine both` audit, and it means precisely that two
/// evaluators of one language disagree — the failure mode the code's own doc
/// comment calls "never a warning" because the result cache would make it
/// sticky. There is nothing in the definition graph to attribute it to, and an
/// agent handed `defect: false` goes looking for the bug in source that is
/// correct.
///
/// So it lands where `INTERNAL_ERROR` lands: both are "Ply is broken", and the
/// classifier used to distinguish them only because it matched one constant
/// instead of a set.
#[test]
fn an_engine_divergence_is_a_defect_in_ply() {
    let (defect, status, verdict) = classified(codes::ENGINE_DIVERGENCE);
    assert!(defect, "a divergence is Ply's fault, not the program's");
    assert_eq!(status, Status::Panicked);
    assert_eq!(verdict, Verdict::NotAttempted(Skipped::Panicked));
}

/// **A refusal to run is classified as the program's fault.** `E0504` is the
/// tree-walker declining to start on a handler clause that binds a
/// continuation; the diagnostic itself says "run this with `--engine machine`".
/// Nothing about the program is wrong, and a bisection that names a definition
/// for it is naming the definition that happens to contain the clause.
///
/// The code's own doc comment says it exists "so that a consumer can tell a
/// refusal to run from a defect while running: the two call for opposite
/// responses" — and the one consumer that has to act on the difference does not
/// read it. A third classification is what this wants; a red test is the wrong
/// one of the two that exist.
#[test]
fn documents_a_machine_only_clause_is_classified_as_the_programs_fault() {
    let (defect, status, verdict) = classified(codes::MACHINE_ONLY_CLAUSE);
    assert!(
        !defect,
        "a refusal to run is now classified apart from a red test — good; assert \
         the new shape instead of deleting this test"
    );
    assert_eq!(status, Status::Failed);
    assert_ne!(verdict, Verdict::NotAttempted(Skipped::Panicked));
}

// ---------------------------------------------- one variant, one description

fn hash(seed: &str) -> ply_hash::DefHash {
    ply_hash::DefHash::from_hex(&seed.repeat(32)).expect("a well-formed hash")
}

fn baseline() -> Baseline {
    Baseline::with_decls(hash("a1"), BTreeMap::new(), BTreeMap::new())
}

/// Each variant answers a different question for a consumer, so each has to be
/// reachable from exactly the condition its `describe` claims — and the order
/// has to be the order the answers are worth. `NotRequested` outranks
/// everything: a run told not to look has learned nothing about the failure.
/// `Panicked` outranks `Nondet` and `NeverPassed` because a defect in Ply makes
/// both of those questions moot.
#[test]
fn precheck_maps_each_condition_to_exactly_its_own_variant() {
    struct Case {
        what: &'static str,
        mode: Mode,
        defect: bool,
        nondet: bool,
        passed_before: bool,
        want: Result<(), Skipped>,
    }
    let case = |what, mode, defect, nondet, passed_before, want| Case {
        what,
        mode,
        defect,
        nondet,
        passed_before,
        want,
    };
    let cases = [
        case("nothing wrong", Mode::Auto, false, false, true, Ok(())),
        case(
            "bisection turned off",
            Mode::Never,
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
            true,
            Err(Skipped::Panicked),
        ),
        case(
            "a nondet test",
            Mode::Auto,
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
            Err(Skipped::NeverPassed),
        ),
    ];

    for c in &cases {
        let base = baseline();
        let got = precheck(c.mode, c.defect, c.nondet, c.passed_before.then_some(&base));
        assert_eq!(got, c.want, "{}", c.what);
    }

    // Precedence, stated as the cases that would be ambiguous otherwise.
    let base = baseline();
    assert_eq!(
        precheck(Mode::Never, true, true, None),
        Err(Skipped::NotRequested),
        "a run that did not ask learns nothing about why it would have skipped"
    );
    assert_eq!(
        precheck(Mode::Auto, true, true, Some(&base)),
        Err(Skipped::Panicked),
        "a defect in Ply outranks `nondet`: the outcome is not the program's either way"
    );
    assert_eq!(
        precheck(Mode::Auto, true, false, None),
        Err(Skipped::Panicked),
        "a defect in Ply outranks a missing baseline"
    );
    assert_eq!(
        precheck(Mode::Auto, false, true, None),
        Err(Skipped::Nondet),
        "`nondet` outranks a missing baseline: a baseline would not have helped"
    );
    assert_eq!(
        precheck(Mode::Always, false, false, None),
        Err(Skipped::NeverPassed),
        "`--bisect always` lifts the budget, not the need for a baseline"
    );
}

/// `Mode::Always` must not be able to talk the gate out of a skip that is about
/// evidence rather than about effort. Every one of these is "there is nothing to
/// compare", and running an unlimited search over nothing is still nothing.
#[test]
fn bisect_always_does_not_override_a_missing_premise() {
    for (defect, nondet) in [(true, false), (false, true)] {
        assert!(
            precheck(Mode::Always, defect, nondet, Some(&baseline())).is_err(),
            "defect={defect} nondet={nondet}"
        );
    }
}

/// The strings are the artifact's contract with a consumer that branches on
/// them, so no two may collide and none may be empty. A duplicate would make
/// two different situations indistinguishable in JSON, which is the one thing
/// the enum exists to prevent.
#[test]
fn every_skipped_variant_has_its_own_tag_and_its_own_description() {
    let all = [
        Skipped::NotRequested,
        Skipped::NeverPassed,
        Skipped::Nondet,
        Skipped::Panicked,
        Skipped::NoChanges,
        Skipped::NoBodies,
        Skipped::NoHybrids,
    ];
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

    // A skip is never a verdict about the program: a consumer must be able to
    // tell "no answer" from "the answer is the empty set".
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

/// `NoChanges` is the one skip that is a statement about the *program* rather
/// than about the run or the build, so it has to be reachable from an empty
/// delta and from nothing else. One cluster is an answer, not a skip.
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

    // A change that is only `Derived` is no candidate, so the delta is again
    // empty — and `no_changes` is the honest answer even though a hash moved.
    let derived = Change::derived(ply_span::Symbol::new("m.g"), hash("c3"), hash("d4"));
    let only_derived = Delta::new(None, vec![derived], &DepEdges::from(&HashOutput::default()));
    assert_eq!(
        bisect(&only_derived, &mut NoHybrid, Budget::DEFAULT).verdict,
        Verdict::NotAttempted(Skipped::NoChanges)
    );
}

/// `NoHybrids` says "this build cannot mix two eras", which is only an honest
/// answer to a question that needed a mixture. With one cluster and an unedited
/// test the search decides by counting, and withholding that answer because no
/// hybrid could be built would make the default useless on the commonest
/// failure there is.
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

/// `Skipped::Panicked` still says "the interpreter failed rather than the
/// program". That sentence is what an agent acts on, and it is now reached only
/// by an unwind or `INTERNAL_ERROR` — so the class of failures it describes and
/// the class that can produce it have to stay the same class. This pins the
/// wording against a future edit that widens the trigger without widening the
/// sentence.
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

/// `Options::never()` is the only way a caller asks for no bisection at all, and
/// it has to reach `NotRequested` rather than any of the reasons that are about
/// the failure. Asserted through `Options` because that is the value the CLI
/// builds, and a default that drifted would be invisible to `precheck` alone.
#[test]
fn options_never_is_the_only_thing_that_reads_as_not_requested() {
    let never = Options::never();
    assert_eq!(
        precheck(never.bisect, false, false, Some(&baseline())),
        Err(Skipped::NotRequested)
    );
    let default = Options::default();
    assert_eq!(
        precheck(default.bisect, false, false, Some(&baseline())),
        Ok(())
    );
}

/// A diagnostic with no span at all still has to be classified — the split
/// reads the code, and a defensive path that forgot its span is exactly where a
/// missing `primary` would come from.
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
