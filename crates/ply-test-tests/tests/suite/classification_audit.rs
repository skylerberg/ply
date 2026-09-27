use crate::fixture::Compiled;
use ply_eval::Plan;
use ply_span::{Diagnostic, Severity, SourceId, Span, codes};
use ply_store::Store;
use ply_test::{Executor, RunReport, Skipped, Status, Verdict, run_with, select};
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

/// Answers with a chosen code, so the classifier is measured against the code, not a program.
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
    let compiled = Compiled::anonymous(CORPUS);
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

/// A wall clock describes the machine, so what it stops decided nothing: no failure to attribute,
/// nothing to write, and a run that cannot be called a success.
#[test]
fn an_abandoned_run_is_no_verdict_and_is_recorded_nowhere() {
    let root = TempRoot::new();
    let mut store = root.store();
    let compiled = Compiled::anonymous(CORPUS);
    let selection = select(&compiled.check, &compiled.hashes, &store, &Plan::default());
    let report = run_with(
        &selection,
        &compiled.check,
        &compiled.hashes,
        &mut store,
        &Answering {
            diagnostic: Some(Diagnostic::warning(
                codes::RUN_ABANDONED,
                "abandoned after 300 ms of wall clock",
            )),
            unwind: false,
        },
    );
    assert_eq!(report.abandoned, 1);
    assert_eq!(report.failed, 0);
    assert_eq!(report.passed, 0);
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert!(!report.is_success(), "an undecided run is not a success");
    assert_eq!(report.results[0].status, Status::Abandoned);
    assert_eq!(report.results[0].recorded, None);
    assert_eq!(store.len(), 0, "an abandoned run writes no result");
}

#[test]
fn a_simulation_divergence_is_a_defect_in_ply() {
    let (defect, status, verdict) = classified(codes::SIMULATION_DIVERGENCE);
    assert!(defect, "a divergence is Ply's fault, not the program's");
    assert_eq!(status, Status::Panicked);
    assert_eq!(verdict, Verdict::NotAttempted(Skipped::Panicked));
}

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
