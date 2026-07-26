//! The bound on runaway recursion, shared by both engines.
//!
//! A runaway recursion is a diagnostic and not an out-of-memory kill. While two
//! evaluators ship they must reach that diagnostic on the same programs and
//! phrase it identically, or `--engine both` reports a divergence on every
//! deeply-recursive program and the audit that exists to catch real defects
//! spends its budget on this one.
//!
//! So the bound is on **nested calls**, which both engines can count exactly:
//! the tree-walker as its own nesting, the machine as the `Frame::Call`s pending
//! on its stack. The machine keeps a second, far looser bound on total pending
//! frames — a resource limit on a heap that is nobody's native stack — but a
//! program reaches this one first, which is what keeps the two answers equal.

use ply_span::{Diagnostic, Span, Symbol, codes};

/// The most nested calls a program may hold at once.
pub const DEFAULT_MAX_CALLS: usize = 10_000;

/// How many of the innermost calls the diagnostic names. Enough to see a cycle,
/// short enough to read.
pub(crate) const NAMED_CALLS: usize = 6;

/// The message keeps the phrase "recursion limit" so that ADR 0004's
/// `AssertionKind::RecursionLimit` still classifies it, and names the innermost
/// calls — the actual recursion path.
///
/// `innermost` is innermost-first; `None` is an anonymous function.
pub(crate) fn err_recursion_limit(
    span: Span,
    what: &str,
    max: usize,
    innermost: &[Option<Symbol>],
) -> Diagnostic {
    let mut diag = Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("recursion limit of {max} {what} exceeded"),
    )
    .primary(span, "this call is too deeply nested")
    .note("check for a recursive call that never reaches its base case");

    let named: Vec<String> = innermost
        .iter()
        .take(NAMED_CALLS)
        .map(|name| match name {
            Some(n) => format!("`{n}`"),
            None => "an anonymous function".to_string(),
        })
        .collect();
    if !named.is_empty() {
        diag = diag.note(format!("innermost calls: {}", named.join(" from ")));
    }
    diag
}

/// What each bound counts, in the message. `NESTED_CALLS` is written once
/// because two engines must spell it the same way.
pub(crate) const NESTED_CALLS: &str = "nested calls";
pub(crate) const PENDING_FRAMES: &str = "pending frames";
