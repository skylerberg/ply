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
//! on its stack. It is the only bound on what a program may do, and it is the
//! only one `Compiled::enter` has to be handed, because it is the only one a
//! natively compiled body could honour.
//!
//! A machine may additionally be asked for a ceiling on its own pending frames
//! — `Machine::with_max_frames`. That is a resource guard on a heap that is
//! nobody's native stack, it is off unless asked for, and it is not semantics:
//! a machine carrying one enters no compiled body, and no shipping command sets
//! one.
//!
//! > **Corrected (2026-08-24): the paragraph above used to describe the frame
//! > ceiling as a shipping default, and the two corrections stacked on it are
//! > both discharged.** It read: *"The machine keeps a second bound on total
//! > pending frames — a resource limit on a heap that is nobody's native stack —
//! > which the tree-walker has no counterpart for"*, and before an R5 review it
//! > had ended *"— but a program reaches this one first, which is what keeps the
//! > two answers equal."* R5 withdrew that clause correctly: *"`DEFAULT_MAX_FRAMES`
//! > is 1,000,000 against `DEFAULT_MAX_CALLS`'s 10,000, so a recursion whose body
//! > pends more than **100 frames per level** hits the frame bound first, the
//! > tree-walker passes where the machine raises, and `--engine both` reports a
//! > divergence with no backend attached … Open"*. It is no longer open: the
//! > default is gone, which is the only reading that can hold for the machine,
//! > the tree-walker **and** a machine with a backend attached. The alternatives
//! > were checked and refused — copying the ceiling into the tree-walker makes
//! > both engines refuse a program over how its additions are spelled and still
//! > leaves a backend answering where both raise, and charging a compiled entry
//! > a frame estimate makes the seam decline every recursive body. See
//! > `Machine::with_max_frames` for the measurement that settles it and
//! > `equivalence_audit.rs::the_two_engines_and_a_backend_agree_however_many_
//! > frames_a_body_pends` for the test.
//!
//! The bound below is not the only bound a loop can be held to, and since ADR
//! 0022 it is not the one an early-terminating loop is held to. `iterate` takes
//! its budget as an **argument**: the number is in the source, so it is in the
//! definition's `DefHash`, and a program that raises its own bound invalidates
//! its own cached results rather than silently keeping them. That is the
//! property a `--max-calls` flag could not have — see [`err_iterate_budget`].
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

/// The message keeps the phrase "recursion limit", and names the innermost calls
/// — the actual recursion path.
///
/// > **Corrected (2026-08-24): the reason given for that phrasing does not
/// > hold.** This read *"The message keeps the phrase "recursion limit" so that
/// > ADR 0004's `AssertionKind::RecursionLimit` still classifies it"*. Nothing
/// > classifies it: `ply_test::slice::AssertionKind::RecursionLimit` is declared
/// > and mapped in `as_str`, and **constructed nowhere** — `grep -rn
/// > 'AssertionKind::' --include=*.rs` finds `Eq` built at `slice.rs:326` and no
/// > other variant built anywhere. So the phrase is load-bearing only for
/// > whatever matches on the string, which is four tests
/// > (`ply-cli/tests/failure_classification_audit.rs`,
/// > `ply-test/tests/hybrid.rs`, `ply-test/src/tests.rs`,
/// > `ply-eval/src/tests.rs`). Kept as-is because those match on it; recorded
/// > because a variant that exists to classify and never does is this
/// > repository's `E0435` pattern, and `CONTRIBUTING.md` §"Things known to be
/// > broken" now carries it. Found while splitting the frame ceiling's
/// > diagnostic out of this builder, which is why it was looked at.
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

/// A machine ran out of the frame ceiling it was asked for.
///
/// Deliberately **not** phrased through [`err_recursion_limit`]: this is one
/// engine's heap running out, not a statement about the program, and a reader
/// who sees "recursion limit" will reach for the program. The note says which
/// bound the program is actually held to, because that is the question anyone
/// reading this is about to ask.
pub(crate) fn err_frame_ceiling(
    span: Span,
    max: usize,
    max_calls: usize,
    innermost: &[Option<Symbol>],
) -> Diagnostic {
    let mut diag = Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("this engine's ceiling of {max} {PENDING_FRAMES} was reached"),
    )
    .primary(span, "evaluating this ran the machine out of frames")
    .note(format!(
        "a frame ceiling is a resource guard on the control-stack machine's own heap, not a \
         bound on the program: the program's bound is {max_calls} {NESTED_CALLS}"
    ));

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

/// An `iterate` whose step never answered `Stop`.
///
/// Deliberately **not** phrased through [`err_recursion_limit`], for the reason
/// [`err_frame_ceiling`] already records and one more: this bound nests nothing
/// — the loop is one frame however long it runs — and it is a number the
/// *program* wrote down rather than one this evaluator chose. A reader who sees
/// "recursion limit" will go looking for a missing base case; the base case
/// here is the `Stop` the step never gave.
pub(crate) fn err_iterate_budget(span: Span, budget: i64) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`iterate` took its budget of {budget} steps without stopping"),
    )
    .primary(span, "this loop never answered `Stop`")
    .note("raise the budget if the loop is right, or check the step that should have stopped")
}

/// A budget that is not a count of steps. Refused at the door rather than
/// treated as an immediate exhaustion, because zero steps is a bound nobody
/// writes on purpose and a negative one is arithmetic that went wrong upstream
/// — neither is a loop that ran too long.
pub(crate) fn err_iterate_budget_not_a_count(span: Span, budget: i64) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`iterate` was given a budget of {budget}"),
    )
    .primary(span, "a budget is the most steps the loop may take")
    .note("it must be at least 1")
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
