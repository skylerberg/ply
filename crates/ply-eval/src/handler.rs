//! The diagnostic for a `perform` no handler answers.

use ply_span::{Diagnostic, Span, Symbol, codes};

/// Deliberately not `E0424`: inference should have prevented this perform, so it is a bug-catcher.
#[cold]
#[inline(never)]
pub fn err_unhandled(
    span: Span,
    effect: &Symbol,
    op: &Symbol,
    resource: Option<&Symbol>,
) -> Diagnostic {
    let label = match resource {
        Some(r) => format!("{effect}.{op}[{r}]"),
        None => format!("{effect}.{op}"),
    };
    Diagnostic::error(codes::UNHANDLED_EFFECT, format!("no handler for `{label}`"))
        .primary(span, "performed here with no enclosing handler")
        .note("wrap this in a `handle ... with { ... }` that names the operation")
}
