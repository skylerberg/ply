use crate::fixture::Compiled;
use ply_eval::{Diagnostic, Severity, SourceId, Span, codes};
use ply_store::Store;
use ply_test::{Executed, RunReport, Status};
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

/// A run that ended with a chosen diagnostic, so the classifier is measured against the code, not a
/// program; `unwind` when Ply unwound rather than the program failing.
struct Answering {
    diagnostic: Option<Diagnostic>,
    unwind: bool,
}

fn report_for(answer: &Answering) -> RunReport {
    let root = TempRoot::new();
    let mut store = root.store();
    concluded(answer, &mut store)
}

fn concluded(answer: &Answering, store: &mut Store) -> RunReport {
    let compiled = Compiled::anonymous(CORPUS);
    let failure = match (&answer.diagnostic, answer.unwind) {
        (Some(d), _) => Some(d.clone()),
        (None, true) => Some(Diagnostic::error(
            codes::INTERNAL_ERROR,
            "test `double doubles` panicked: the evaluator lost its footing",
        )),
        (None, false) => None,
    };
    let ran = vec![Executed {
        failure,
        panicked: answer.unwind,
        ..Executed::refused(0, Diagnostic::error(codes::INTERNAL_ERROR, "overwritten"))
    }];
    ply_test::concluded(
        &compiled.every(),
        &compiled.check,
        &compiled.hashes,
        store,
        ran,
        std::time::Duration::ZERO,
    )
}

/// The runtime's two marks on a failure; the verdict they lead to is `suite.bisect`'s.
fn classified(code: &'static str) -> (bool, Status) {
    let report = report_for(&Answering {
        diagnostic: Some(
            Diagnostic::error(code, "the fixture's failure")
                .primary(Span::new(SourceId(0), 0, 1), "here"),
        ),
        unwind: false,
    });
    assert_eq!(report.failures.len(), 1, "{code} must produce one failure");
    (report.failures[0].defect, report.results[0].status)
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
        let (defect, status) = classified(code);
        assert!(!defect, "{code} was read as a defect in Ply");
        assert_eq!(status, Status::Failed, "{code}");
    }
}

#[test]
fn an_internal_error_and_an_unwind_are_both_defects() {
    let (defect, status) = classified(codes::INTERNAL_ERROR);
    assert!(defect);
    assert_eq!(status, Status::Panicked);

    let report = report_for(&Answering {
        diagnostic: Some(Diagnostic::error(codes::RUNTIME_ERROR, "a program's code")),
        unwind: true,
    });
    assert!(
        report.failures[0].defect,
        "an unwind is a defect whatever code it carries"
    );
    assert_eq!(report.results[0].status, Status::Panicked);
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
    let report = concluded(
        &Answering {
            diagnostic: Some(Diagnostic::warning(
                codes::RUN_ABANDONED,
                "abandoned after 300 ms of wall clock",
            )),
            unwind: false,
        },
        &mut store,
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
    let (defect, status) = classified(codes::SIMULATION_DIVERGENCE);
    assert!(defect, "a divergence is Ply's fault, not the program's");
    assert_eq!(status, Status::Panicked);
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
