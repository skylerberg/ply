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
//!
//! A call is not the only thing a program can nest, though. A *value* nests too,
//! and walking one structurally — comparing two, diffing two — is host recursion
//! that no count of calls bounds. That is [`MAX_VALUE_DEPTH`], and it sits where
//! it does so that anything the call bound lets a program build, it can compare.

use ply_span::{Diagnostic, Span, Symbol, codes};

/// The most nested calls a program may hold at once.
pub const DEFAULT_MAX_CALLS: usize = 10_000;

/// The deepest a value may nest before a structural walk over it refuses.
///
/// Equal to [`DEFAULT_MAX_CALLS`]: a value built by recursion is at most as deep
/// as the recursion that built it, so no value a program can construct is one
/// this bound then refuses to compare. Deeper is reachable only by iteration,
/// and answering that with a diagnostic is the point — the alternative is a host
/// stack overflow, which aborts the process and takes every sibling test's
/// result with it.
pub const MAX_VALUE_DEPTH: usize = DEFAULT_MAX_CALLS;

/// One step of a native recursion costs several frames and an unoptimized build
/// makes each of them large, so a worker's default 2 MiB runs out long before
/// either bound above does. Growing on demand means the bound is what a user
/// hits, and it is reported as a diagnostic instead of aborting the process (and
/// with it every unrelated test sharing it).
pub(crate) fn grow<R>(f: impl FnOnce() -> R) -> R {
    const RED_ZONE: usize = 256 * 1024;
    const NEW_SEGMENT: usize = 2 * 1024 * 1024;
    stacker::maybe_grow(RED_ZONE, NEW_SEGMENT, f)
}

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

/// A value too deep to walk. Phrased through the same message as a runaway
/// call so that a consumer classifying on "recursion limit" catches both, and
/// spelled once because both engines share the walk that raises it.
pub(crate) fn err_value_depth(span: Span, max: usize) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("recursion limit of {max} {NESTED_VALUES} exceeded"),
    )
    .primary(span, "this value nests too deeply to walk")
    .note("a value this deep is reachable only by iteration; compare its parts instead")
}

/// What each bound counts, in the message. `NESTED_CALLS` is written once
/// because two engines must spell it the same way.
pub(crate) const NESTED_CALLS: &str = "nested calls";
pub(crate) const PENDING_FRAMES: &str = "pending frames";
pub(crate) const NESTED_VALUES: &str = "nested values";
