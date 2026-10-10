//! What a handler's refusal comes to: its own code kept and stamped with where it came from, and
//! never one of the codes only the run itself may raise.

use ply_eval::host::{attribute, is_reserved_code};
use ply_eval::{Diagnostic, Span, codes};

/// A handler cannot pass its own failure off as a defect in Ply, or as one of the run's verdicts.
#[test]
fn a_handler_may_not_choose_the_code_its_failure_is_classified_under() {
    for claimed in [
        codes::INTERNAL_ERROR,
        codes::REGION_ESCAPE_AT_BOUNDARY,
        codes::HERMETIC_BOUNDARY,
    ] {
        assert!(
            is_reserved_code(claimed),
            "`{claimed}` is the run's to raise"
        );
        let d = attribute(
            Diagnostic::error(claimed, "the evaluator is broken"),
            "audit::impersonates",
            "t.disk.peek[log]",
            Span::DUMMY,
        );
        assert_eq!(d.code, codes::RUNTIME_ERROR, "`{claimed}` was kept");
        assert_eq!(d.message, "the evaluator is broken");
        assert!(
            d.notes
                .iter()
                .any(|n| n.contains(claimed) && n.contains("audit::impersonates")),
            "nothing names the code it claimed and the handler that claimed it: {:?}",
            d.notes
        );
    }
}

#[test]
fn an_unreserved_code_from_a_handler_is_kept_and_attributed() {
    let d = attribute(
        Diagnostic::error(codes::RUNTIME_ERROR, "the socket refused the connection"),
        "audit::hostile",
        "t.net.send[socket]",
        Span::DUMMY,
    );
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    assert_eq!(d.message, "the socket refused the connection");
    assert!(
        d.notes
            .iter()
            .any(|n| n.contains("audit::hostile") && n.contains("t.net.send[socket]")),
        "an honest refusal is not attributed: {:?}",
        d.notes
    );
    assert!(
        !d.notes.iter().any(|n| n.contains("only the run itself")),
        "an unreserved code was reported as though it had been rewritten: {:?}",
        d.notes
    );
}
