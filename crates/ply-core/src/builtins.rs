use crate::codec::Writer;
use crate::{Diagnostic, Span, codes, slot};

const MAX_RANGE_LEN: i64 = 10_000_000;

pub const MAX_ARRAY_LEN: i64 = 1 << 26;

pub fn start_outside(from: i64, len: usize, span: Span, what: &str) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`{what}` start {from} is outside a value of {len} bytes"),
    )
    .primary(span, "this position does not exist")
    .note(format!(
        "a start must satisfy `0 <= from <= {len}`; it is never clamped"
    ))
}

#[cold]
pub fn negative_budget(span: Span, what: &str, max: i64) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`{what}` was given a negative budget of {max}"),
    )
    .primary(span, "a scan cannot examine a negative number of bytes")
    .note("pass `0` to examine nothing, or `Bytes::len(b)` to leave it unbounded")
}

#[cold]
pub fn not_a_byte(span: Span, what: &str, byte: i64) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`{what}` was given {byte}, which is not a byte"),
    )
    .primary(span, "a byte is `0` to `255`")
    .note("`Bytes::at` returns values in that range, and so does a byte literal's element")
}

#[cold]
pub fn byte_of_int_rejected(span: Span, n: i64) -> Diagnostic {
    Diagnostic::error(codes::RUNTIME_ERROR, format!("`byte_of_int` was given {n}"))
        .primary(span, "a byte is 0 to 255")
        .note("mask the value before the call if that is what you meant: `n & 0xFF`")
}

#[cold]
pub fn empty_separator(span: Span, split: &str) -> Diagnostic {
    let example = if split == "String::split" {
        "pass the text that actually separates the parts, as in \"\\r\\n\""
    } else {
        "pass the bytes that actually separate the parts, as in `b\"\\r\\n\"`"
    };
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`{split}` needs a separator, and this one is empty"),
    )
    .primary(span, "an empty separator matches everywhere and nowhere")
    .note(example)
}

#[cold]
pub fn range_too_long(span: Span, lo: i64, hi: i64) -> Diagnostic {
    let len = hi.saturating_sub(lo);
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`range` of {len} elements exceeds the limit of {MAX_RANGE_LEN}"),
    )
    .primary(span, "this range is too large to materialize")
}

#[cold]
pub fn not_found(span: Span, s: &str, needle: &str) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`String::find` did not find {} in {}", slot(0), slot(1)),
    )
    .primary(span, "this substring does not occur")
    .note("guard with `String::contains`, which answers the same question as a `Bool`")
    .showing(vec![encode_text(needle), encode_text(s)])
}

fn encode_text(s: &str) -> Vec<u8> {
    let mut w = Writer::new();
    w.str(s.as_bytes());
    w.finish()
}

#[cold]
pub fn range_outside(
    span: Span,
    what: &str,
    start: i64,
    end: i64,
    len: usize,
    unit: &str,
) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`{what}` range {start}..{end} is outside a value of {len} {unit}"),
    )
    .primary(span, "this range does not fit")
    .note(format!(
        "a range must satisfy `0 <= start <= end <= {len}`; it is never clamped"
    ))
}

#[cold]
pub fn array_length(span: Span, what: &str, n: i64) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`{what}` of {n} elements is outside `0` to `{MAX_ARRAY_LEN}`"),
    )
    .primary(span, "no array has this length")
}

#[cold]
pub fn out_of_range(span: Span, what: &str, index: i64, len: usize, unit: &str) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`{what}` index {index} is outside a value of {len} {unit}"),
    )
    .primary(span, "this index does not exist")
    .note(format!(
        "valid indices are `0` to `{}`",
        len.saturating_sub(1)
    ))
}

pub fn assert_failure(message: Option<&str>, span: Span) -> Diagnostic {
    let diag = Diagnostic::error(
        codes::ASSERTION_FAILED,
        "assertion failed: condition is false",
    )
    .primary(span, "this condition evaluated to false");
    match message {
        Some(s) => diag.note(s),
        None => diag,
    }
}

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
