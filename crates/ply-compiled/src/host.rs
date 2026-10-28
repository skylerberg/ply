//! What the runtime keeps of the host operations an entry performed: the at-most-once one a second
//! resumption would replay.

use ply_core::{Diagnostic, Span, codes};

/// An at-most-once host operation an entry performed, which a second resumption would replay.
pub(crate) struct HostMark {
    pub(crate) operation: String,
    pub(crate) path: &'static str,
    pub(crate) span: Span,
}

/// A second resumption across an at-most-once host operation.
#[cold]
#[inline(never)]
pub(crate) fn err_continuation_resumed(
    span: Span,
    resumes: u32,
    last: Option<&HostMark>,
) -> Diagnostic {
    let mut diagnostic = Diagnostic::error(
        codes::HOST_CONTINUATION_RESUMED,
        "this continuation is resumed again across a host operation",
    )
    .primary(span, format!("resumption {resumes} of one continuation"));
    if let Some(mark) = last {
        if mark.span != Span::DUMMY {
            diagnostic = diagnostic.secondary(
                mark.span,
                format!("`{}` was performed here, after the capture", mark.operation),
            );
        }
        diagnostic = diagnostic.note(format!(
            "`{}` is registered `at-most-once`, so replaying this control would perform it again",
            mark.path
        ));
    }
    diagnostic
        .note("multi-shot resumption stays available for pure and in-memory handlers; the restriction is on the boundary, not on the feature")
        .note("resume at most once across a host operation, or -- only if replaying it really changes nothing outside the program -- register the operation `Linearity::Repeatable`")
        .note("the rule is conservative: it refuses when any at-most-once host operation happened after the capture, including in another task")
}
