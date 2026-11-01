//! The failure the runtime raises for a scoped value it holds.

use crate::{Diagnostic, Span, codes};

/// A scoped value used after its scope let it go, which only a continuation captured in the scope
/// and resumed after the release can do.
#[cold]
pub fn scoped_released(span: Span, doing: &str) -> Diagnostic {
    Diagnostic::error(
        codes::SCOPED_VALUE_RELEASED,
        format!("a scoped value is {doing} after its scope released what it held"),
    )
    .primary(span, "the scoped value holds nothing now")
    .note("a `with_scope` releases its value once, when its body first ends; a continuation captured inside the body and resumed after that runs the rest of the body against a value already released")
    .note("resume a continuation captured inside a `with_scope` at most once, or open the `with_scope` after the choice the continuation replays")
}
