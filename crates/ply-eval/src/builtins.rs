//! The prelude, in one definition per builtin that both engines run.
//!
//! `map`, `filter` and `fold` call back into user code. On an explicit control
//! stack they cannot recurse into the host: a continuation captured inside the
//! function passed to `map` would be captured across a native frame that cannot
//! be re-entered, and the second resumption would have nowhere to return to. So
//! they are expressed as a step protocol — [`call`] starts one, [`advance`]
//! resumes it — where every suspension point is a [`Frame`] the machine can
//! push, capture and splice like any other.
//!
//! The tree-walker drives the same protocol with a host loop. That is
//! deliberate: two engines that disagree about what `map` means is exactly the
//! defect `crate::differential` exists to catch, and sharing the definition
//! means the audit is comparing the machinery rather than two transcriptions.

use crate::cont::Frame;
use crate::interp::{Interp, arity_error};
use crate::value::{Value, Vector, first_difference, type_error, values_equal};
use crate::world::{CellId, World};
use ply_span::{Diagnostic, Span, codes};
use std::fmt;

/// A list this long is a runaway `range`, not an intent.
const MAX_RANGE_LEN: i64 = 10_000_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Builtin {
    Assert,
    AssertEq,
    Len,
    Push,
    Map,
    Filter,
    Fold,
    Range,
    IntToString,
    StringConcat,
    BytesLen,
    BytesAt,
    BytesSlice,
    BytesConcat,
    BytesOfString,
    BytesIsUtf8,
    StringOfBytes,
    StringOfBytesLossy,
    StringLen,
    StringSlice,
    StringSplit,
    StringTrim,
    StringLower,
    StringUpper,
    StringStartsWith,
    StringEndsWith,
    StringContains,
    StringFind,
    CellGet,
    CellSet,
    Panic,
}

impl Builtin {
    pub fn from_name(name: &str) -> Option<Builtin> {
        Some(match name {
            "assert" => Builtin::Assert,
            "assert_eq" => Builtin::AssertEq,
            "len" => Builtin::Len,
            "push" => Builtin::Push,
            "map" => Builtin::Map,
            "filter" => Builtin::Filter,
            "fold" => Builtin::Fold,
            "range" => Builtin::Range,
            "int_to_string" => Builtin::IntToString,
            "string_concat" => Builtin::StringConcat,
            "bytes_len" => Builtin::BytesLen,
            "bytes_at" => Builtin::BytesAt,
            "bytes_slice" => Builtin::BytesSlice,
            "bytes_concat" => Builtin::BytesConcat,
            "bytes_of_string" => Builtin::BytesOfString,
            "bytes_is_utf8" => Builtin::BytesIsUtf8,
            "string_of_bytes" => Builtin::StringOfBytes,
            "string_of_bytes_lossy" => Builtin::StringOfBytesLossy,
            "string_len" => Builtin::StringLen,
            "string_slice" => Builtin::StringSlice,
            "string_split" => Builtin::StringSplit,
            "string_trim" => Builtin::StringTrim,
            "string_lower" => Builtin::StringLower,
            "string_upper" => Builtin::StringUpper,
            "string_starts_with" => Builtin::StringStartsWith,
            "string_ends_with" => Builtin::StringEndsWith,
            "string_contains" => Builtin::StringContains,
            "string_find" => Builtin::StringFind,
            "cell_get" => Builtin::CellGet,
            "cell_set" => Builtin::CellSet,
            "panic" => Builtin::Panic,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Builtin::Assert => "assert",
            Builtin::AssertEq => "assert_eq",
            Builtin::Len => "len",
            Builtin::Push => "push",
            Builtin::Map => "map",
            Builtin::Filter => "filter",
            Builtin::Fold => "fold",
            Builtin::Range => "range",
            Builtin::IntToString => "int_to_string",
            Builtin::StringConcat => "string_concat",
            Builtin::BytesLen => "bytes_len",
            Builtin::BytesAt => "bytes_at",
            Builtin::BytesSlice => "bytes_slice",
            Builtin::BytesConcat => "bytes_concat",
            Builtin::BytesOfString => "bytes_of_string",
            Builtin::BytesIsUtf8 => "bytes_is_utf8",
            Builtin::StringOfBytes => "string_of_bytes",
            Builtin::StringOfBytesLossy => "string_of_bytes_lossy",
            Builtin::StringLen => "string_len",
            Builtin::StringSlice => "string_slice",
            Builtin::StringSplit => "string_split",
            Builtin::StringTrim => "string_trim",
            Builtin::StringLower => "string_lower",
            Builtin::StringUpper => "string_upper",
            Builtin::StringStartsWith => "string_starts_with",
            Builtin::StringEndsWith => "string_ends_with",
            Builtin::StringContains => "string_contains",
            Builtin::StringFind => "string_find",
            Builtin::CellGet => "cell_get",
            Builtin::CellSet => "cell_set",
            Builtin::Panic => "panic",
        }
    }

    /// Inclusive `(min, max)` argument counts.
    pub fn arity(self) -> (usize, usize) {
        match self {
            Builtin::Assert => (1, 2),
            Builtin::Range => (1, 2),
            Builtin::Len
            | Builtin::IntToString
            | Builtin::CellGet
            | Builtin::Panic
            | Builtin::BytesLen
            | Builtin::BytesOfString
            | Builtin::BytesIsUtf8
            | Builtin::StringOfBytes
            | Builtin::StringOfBytesLossy
            | Builtin::StringLen
            | Builtin::StringTrim
            | Builtin::StringLower
            | Builtin::StringUpper => (1, 1),
            Builtin::AssertEq
            | Builtin::Push
            | Builtin::Map
            | Builtin::Filter
            | Builtin::StringConcat
            | Builtin::CellSet
            | Builtin::BytesAt
            | Builtin::BytesConcat
            | Builtin::StringSplit
            | Builtin::StringStartsWith
            | Builtin::StringEndsWith
            | Builtin::StringContains
            | Builtin::StringFind => (2, 2),
            Builtin::Fold | Builtin::BytesSlice | Builtin::StringSlice => (3, 3),
        }
    }

    /// Calls user code, so [`call`] may answer [`Step::Apply`] rather than a
    /// value and the caller must be able to suspend.
    pub fn higher_order(self) -> bool {
        matches!(self, Builtin::Map | Builtin::Filter | Builtin::Fold)
    }

    pub fn all() -> &'static [Builtin] {
        &[
            Builtin::Assert,
            Builtin::AssertEq,
            Builtin::Len,
            Builtin::Push,
            Builtin::Map,
            Builtin::Filter,
            Builtin::Fold,
            Builtin::Range,
            Builtin::IntToString,
            Builtin::StringConcat,
            Builtin::BytesLen,
            Builtin::BytesAt,
            Builtin::BytesSlice,
            Builtin::BytesConcat,
            Builtin::BytesOfString,
            Builtin::BytesIsUtf8,
            Builtin::StringOfBytes,
            Builtin::StringOfBytesLossy,
            Builtin::StringLen,
            Builtin::StringSlice,
            Builtin::StringSplit,
            Builtin::StringTrim,
            Builtin::StringLower,
            Builtin::StringUpper,
            Builtin::StringStartsWith,
            Builtin::StringEndsWith,
            Builtin::StringContains,
            Builtin::StringFind,
            Builtin::CellGet,
            Builtin::CellSet,
            Builtin::Panic,
        ]
    }
}

/// What a builtin needs before it can produce a value.
pub enum Step {
    Done(Value),
    /// Apply `callee` to `args`, then hand the answer to [`advance`] along with
    /// `frame`. The machine pushes `frame` and enters the application; the
    /// tree-walker keeps the application on the host stack.
    Apply {
        callee: Value,
        args: Vec<Value>,
        frame: Frame,
    },
}

impl fmt::Debug for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Step::Done(v) => write!(f, "Done({v})"),
            Step::Apply { callee, args, .. } => {
                write!(f, "Apply({callee} to {} arguments)", args.len())
            }
        }
    }
}

/// `world` is the current world, threaded rather than snapshotted: `cell_get`
/// must observe every write made before this call, including one a handler
/// clause made before resuming.
pub fn call(
    b: Builtin,
    args: Vec<Value>,
    world: &mut World,
    span: Span,
) -> Result<Step, Diagnostic> {
    let (min, max) = b.arity();
    if args.len() < min || args.len() > max {
        let expected = if min == max { min } else { max };
        return Err(arity_error(
            span,
            &format!("`{}`", b.name()),
            expected,
            args.len(),
        ));
    }

    match b {
        Builtin::Assert => {
            if args[0].as_bool(span, "`assert`")? {
                return Ok(Step::Done(Value::Unit));
            }
            Err(assert_failure(args.get(1), span))
        }

        Builtin::AssertEq => {
            if values_equal(&args[0], &args[1], span)? {
                Ok(Step::Done(Value::Unit))
            } else {
                Err(assertion_failure(&args[0], &args[1], span))
            }
        }

        Builtin::Len => match &args[0] {
            Value::List(xs) => Ok(Step::Done(Value::Int(xs.len() as i64))),
            Value::Str(s) => Ok(Step::Done(Value::Int(s.chars().count() as i64))),
            other => Err(type_error(span, "`len`", "a List or String", other)),
        },

        Builtin::Push => {
            let xs = args[0].as_list(span, "`push`")?;
            let mut out = Vec::with_capacity(xs.len() + 1);
            out.extend(xs.iter().cloned());
            out.push(args[1].clone());
            Ok(Step::Done(Value::list(out)))
        }

        Builtin::Map => {
            let items = args[0].as_list(span, "`map`")?.clone();
            Ok(next_map(args[1].clone(), items, 0, Vec::new(), span))
        }

        Builtin::Filter => {
            let items = args[0].as_list(span, "`filter`")?.clone();
            Ok(next_filter(args[1].clone(), items, 0, Vec::new(), span))
        }

        Builtin::Fold => {
            let items = args[0].as_list(span, "`fold`")?.clone();
            Ok(next_fold(args[2].clone(), items, 0, args[1].clone(), span))
        }

        Builtin::Range => {
            let (lo, hi) = match args.len() {
                1 => (0, args[0].as_int(span, "`range`")?),
                _ => (
                    args[0].as_int(span, "`range`")?,
                    args[1].as_int(span, "`range`")?,
                ),
            };
            if hi <= lo {
                return Ok(Step::Done(Value::list(Vec::new())));
            }
            let len = hi.saturating_sub(lo);
            if len > MAX_RANGE_LEN {
                return Err(Diagnostic::error(
                    codes::RUNTIME_ERROR,
                    format!("`range` of {len} elements exceeds the limit of {MAX_RANGE_LEN}"),
                )
                .primary(span, "this range is too large to materialize"));
            }
            Ok(Step::Done(Value::list((lo..hi).map(Value::Int).collect())))
        }

        Builtin::IntToString => Ok(Step::Done(Value::str(
            args[0].as_int(span, "`int_to_string`")?.to_string(),
        ))),

        Builtin::StringConcat => {
            let a = args[0].as_str(span, "`string_concat`")?;
            let b = args[1].as_str(span, "`string_concat`")?;
            Ok(Step::Done(Value::str(format!("{a}{b}"))))
        }

        Builtin::BytesLen => {
            let b = args[0].as_bytes(span, "`bytes_len`")?;
            Ok(Step::Done(Value::Int(b.len() as i64)))
        }

        Builtin::BytesAt => {
            let b = args[0].as_bytes(span, "`bytes_at`")?;
            let i = args[1].as_int(span, "`bytes_at`")?;
            match usize::try_from(i).ok().and_then(|i| b.get(i)) {
                Some(byte) => Ok(Step::Done(Value::Int(i64::from(*byte)))),
                None => Err(out_of_range(span, "bytes_at", i, b.len())),
            }
        }

        Builtin::BytesSlice => {
            let b = args[0].as_bytes(span, "`bytes_slice`")?;
            let (start, end) =
                range_args(&args[1], &args[2], b.len(), span, "bytes_slice", "bytes")?;
            Ok(Step::Done(Value::bytes(&b[start..end])))
        }

        Builtin::BytesConcat => {
            let a = args[0].as_bytes(span, "`bytes_concat`")?;
            let b = args[1].as_bytes(span, "`bytes_concat`")?;
            let mut out = Vec::with_capacity(a.len() + b.len());
            out.extend_from_slice(a);
            out.extend_from_slice(b);
            Ok(Step::Done(Value::bytes(out)))
        }

        Builtin::BytesOfString => {
            let s = args[0].as_str(span, "`bytes_of_string`")?;
            Ok(Step::Done(Value::bytes(s.as_bytes())))
        }

        Builtin::BytesIsUtf8 => {
            let b = args[0].as_bytes(span, "`bytes_is_utf8`")?;
            Ok(Step::Done(Value::Bool(std::str::from_utf8(b).is_ok())))
        }

        Builtin::StringOfBytes => {
            let b = args[0].as_bytes(span, "`string_of_bytes`")?;
            match std::str::from_utf8(b) {
                Ok(s) => Ok(Step::Done(Value::str(s))),
                Err(e) => Err(not_utf8(span, b, &e)),
            }
        }

        Builtin::StringOfBytesLossy => {
            let b = args[0].as_bytes(span, "`string_of_bytes_lossy`")?;
            Ok(Step::Done(Value::str(String::from_utf8_lossy(b))))
        }

        // `len` is `(List<a>) -> Int`, so a String needs its own name until W2
        // settles type-directed dispatch. Characters rather than bytes: it is
        // the number that pairs with `string_slice`.
        Builtin::StringLen => Ok(Step::Done(Value::Int(
            args[0].as_str(span, "`string_len`")?.chars().count() as i64,
        ))),

        Builtin::StringSlice => {
            let s = args[0].as_str(span, "`string_slice`")?;
            let chars = s.chars().count();
            let (start, end) = range_args(
                &args[1],
                &args[2],
                chars,
                span,
                "string_slice",
                "characters",
            )?;
            let from = char_offset(s, start);
            let to = char_offset(s, end);
            Ok(Step::Done(Value::str(&s[from..to])))
        }

        Builtin::StringSplit => {
            let s = args[0].as_str(span, "`string_split`")?;
            let sep = args[1].as_str(span, "`string_split`")?;
            if sep.is_empty() {
                return Err(Diagnostic::error(
                    codes::RUNTIME_ERROR,
                    "`string_split` needs a separator, and this one is empty",
                )
                .primary(span, "an empty separator matches everywhere and nowhere")
                .note("pass the text that actually separates the parts, as in \"\\r\\n\""));
            }
            Ok(Step::Done(Value::list(
                s.split(sep).map(Value::str).collect(),
            )))
        }

        // These three read `std`'s Unicode tables, so their answers move if
        // those tables do. A toolchain upgrade is therefore a
        // `RUNTIME_VERSION` bump — a cached `Pass` is a claim about what the
        // evaluator did, and this is the first thing in the language whose
        // behaviour is not settled by this repository alone.
        Builtin::StringTrim => Ok(Step::Done(Value::str(
            args[0].as_str(span, "`string_trim`")?.trim(),
        ))),

        Builtin::StringLower => Ok(Step::Done(Value::str(
            args[0].as_str(span, "`string_lower`")?.to_lowercase(),
        ))),

        Builtin::StringUpper => Ok(Step::Done(Value::str(
            args[0].as_str(span, "`string_upper`")?.to_uppercase(),
        ))),

        Builtin::StringStartsWith => {
            let s = args[0].as_str(span, "`string_starts_with`")?;
            let prefix = args[1].as_str(span, "`string_starts_with`")?;
            Ok(Step::Done(Value::Bool(s.starts_with(prefix))))
        }

        Builtin::StringEndsWith => {
            let s = args[0].as_str(span, "`string_ends_with`")?;
            let suffix = args[1].as_str(span, "`string_ends_with`")?;
            Ok(Step::Done(Value::Bool(s.ends_with(suffix))))
        }

        Builtin::StringContains => {
            let s = args[0].as_str(span, "`string_contains`")?;
            let needle = args[1].as_str(span, "`string_contains`")?;
            Ok(Step::Done(Value::Bool(s.contains(needle))))
        }

        Builtin::StringFind => {
            let s = args[0].as_str(span, "`string_find`")?;
            let needle = args[1].as_str(span, "`string_find`")?;
            match s.find(needle) {
                Some(at) => Ok(Step::Done(Value::Int(s[..at].chars().count() as i64))),
                None => Err(Diagnostic::error(
                    codes::RUNTIME_ERROR,
                    format!(
                        "`string_find` did not find {} in {}",
                        Value::str(needle).render(),
                        Value::str(s).render()
                    ),
                )
                .primary(span, "this substring does not occur")
                .note("guard with `string_contains`, which answers the same question as a `Bool`")),
            }
        }

        Builtin::CellGet => {
            let id = args[0].as_cell(span, "`cell_get`")?;
            match world.get(id) {
                Some(v) => Ok(Step::Done(v.clone())),
                None => Err(no_such_cell(span, id)),
            }
        }

        Builtin::CellSet => {
            let id = args[0].as_cell(span, "`cell_set`")?;
            if world.set(id, args[1].clone()) {
                Ok(Step::Done(Value::Unit))
            } else {
                Err(no_such_cell(span, id))
            }
        }

        Builtin::Panic => {
            let message = match &args[0] {
                Value::Str(s) => s.to_string(),
                other => other.render(),
            };
            Err(
                Diagnostic::error(codes::RUNTIME_ERROR, format!("panic: {message}"))
                    .primary(span, "`panic` called here"),
            )
        }
    }
}

/// Resumes a higher-order builtin: `answer` is what the user code the frame was
/// waiting on returned.
///
/// The frame is consumed by value but it is `Clone`, so a machine that captured
/// it inside a continuation may advance the same suspension point more than
/// once — each resumption continuing its own copy of the list being built.
pub fn advance(frame: Frame, answer: Value) -> Result<Step, Diagnostic> {
    Ok(match frame {
        Frame::MapStep {
            f,
            items,
            next,
            mut done,
            span,
        } => {
            done.push(answer);
            next_map(f, items, next, done, span)
        }

        Frame::FilterStep {
            f,
            items,
            next,
            mut done,
            span,
        } => {
            if answer.as_bool(span, "the predicate given to `filter`")?
                && let Some(kept) = next.checked_sub(1).and_then(|i| items.get(i))
            {
                done.push(kept.clone());
            }
            next_filter(f, items, next, done, span)
        }

        Frame::FoldStep {
            f,
            items,
            next,
            span,
        } => next_fold(f, items, next, answer, span),

        _ => return Err(not_a_builtin_step()),
    })
}

fn next_map(f: Value, items: Vector<Value>, next: usize, done: Vec<Value>, span: Span) -> Step {
    let Some(x) = items.get(next).cloned() else {
        return Step::Done(Value::list(done));
    };
    Step::Apply {
        callee: f.clone(),
        args: vec![x],
        frame: Frame::MapStep {
            f,
            items,
            next: next + 1,
            done,
            span,
        },
    }
}

fn next_filter(f: Value, items: Vector<Value>, next: usize, done: Vec<Value>, span: Span) -> Step {
    let Some(x) = items.get(next).cloned() else {
        return Step::Done(Value::list(done));
    };
    Step::Apply {
        callee: f.clone(),
        args: vec![x],
        frame: Frame::FilterStep {
            f,
            items,
            next: next + 1,
            done,
            span,
        },
    }
}

fn next_fold(f: Value, items: Vector<Value>, next: usize, acc: Value, span: Span) -> Step {
    let Some(x) = items.get(next).cloned() else {
        return Step::Done(acc);
    };
    Step::Apply {
        callee: f.clone(),
        args: vec![acc, x],
        frame: Frame::FoldStep {
            f,
            items,
            next: next + 1,
            span,
        },
    }
}

/// The half-open `[start, end)` of a slicing builtin, refused rather than
/// clamped.
///
/// Clamping is what turns an off-by-one into a shorter answer that every later
/// assertion agrees with, which is the silent-wrong-answer shape this project
/// exists to refuse. `len` and `unit` are whatever the builtin indexes in:
/// bytes for `bytes_slice`, characters for `string_slice`.
fn range_args(
    start: &Value,
    end: &Value,
    len: usize,
    span: Span,
    what: &str,
    unit: &str,
) -> Result<(usize, usize), Diagnostic> {
    let start = start.as_int(span, &format!("`{what}`"))?;
    let end = end.as_int(span, &format!("`{what}`"))?;
    if start < 0 || end < start || !usize::try_from(end).is_ok_and(|e| e <= len) {
        return Err(Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("`{what}` range {start}..{end} is outside a value of {len} {unit}"),
        )
        .primary(span, "this range does not fit")
        .note(format!(
            "a range must satisfy `0 <= start <= end <= {len}`; it is never clamped"
        )));
    }
    Ok((start as usize, end as usize))
}

/// The byte offset of the `n`-th character boundary. `n` has already been
/// checked against the character count, so the fallback is unreachable for a
/// caller that went through [`range_args`].
fn char_offset(s: &str, n: usize) -> usize {
    s.char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(s.len()))
        .nth(n)
        .unwrap_or(s.len())
}

/// The offset is the first byte the decoder could not use, which is the number
/// an author needs to find the truncation — a `Bytes` cut mid-character by
/// `bytes_slice` reports the position of the character it cut.
fn not_utf8(span: Span, b: &[u8], e: &std::str::Utf8Error) -> Diagnostic {
    let at = e.valid_up_to();
    let what = match e.error_len() {
        Some(n) => format!(
            "{n} byte{} at offset {at} are not a UTF-8 sequence",
            if n == 1 { "" } else { "s" }
        ),
        None => format!("a UTF-8 sequence starting at offset {at} is cut short"),
    };
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`string_of_bytes` was given bytes that are not UTF-8: {what}"),
    )
    .primary(span, format!("byte {at} of {} is where it fails", b.len()))
    .note("guard with `bytes_is_utf8`, or use `string_of_bytes_lossy` to accept U+FFFD")
}

#[cold]
fn out_of_range(span: Span, what: &str, index: i64, len: usize) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`{what}` index {index} is outside a value of {len} bytes"),
    )
    .primary(span, "this index does not exist")
    .note(format!(
        "valid indices are `0` to `{}`",
        len.saturating_sub(1)
    ))
}

/// The `ASSERTION_FAILED` an agent reads to decide what to fix: both values in
/// full, plus the path to the first place they differ.
///
/// Argument order follows `assert_eq(actual, expected)`.
pub fn assertion_failure(actual: &Value, expected: &Value, span: Span) -> Diagnostic {
    let mut diag = Diagnostic::error(
        codes::ASSERTION_FAILED,
        format!(
            "assertion failed: expected {}, found {}",
            expected.render(),
            actual.render()
        ),
    )
    .primary(span, "these values are not equal")
    .note(format!("expected: {}", expected.render()))
    .note(format!("actual:   {}", actual.render()));

    if let Some((path, exp, act)) = first_difference(actual, expected) {
        diag = diag.note(format!(
            "first difference at `{path}`: expected {exp}, found {act}"
        ));
    }
    diag
}

/// The `ASSERTION_FAILED` for a bare `assert`, whose optional second argument
/// is the message the author wanted the reader to see.
pub fn assert_failure(message: Option<&Value>, span: Span) -> Diagnostic {
    let mut diag = Diagnostic::error(
        codes::ASSERTION_FAILED,
        "assertion failed: condition is false",
    )
    .primary(span, "this condition evaluated to false");
    if let Some(message) = message {
        diag = diag.note(match message {
            Value::Str(s) => s.to_string(),
            other => other.render(),
        });
    }
    diag
}

/// A cell that is not in the current world. Reachable only by carrying a value
/// out of the world that made it, which no source program can express;
/// reporting it beats reading a neighbouring cell's state.
#[cold]
fn no_such_cell(span: Span, id: CellId) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("cell {id} does not belong to the world this code is running in"),
    )
    .primary(span, "this cell was made by a different run")
    .note("please report this: a cell value escaped the world that allocated it")
}

/// Only the three higher-order builtins suspend, so only their frames reach
/// here. An engine that routes another one in has a dispatch bug, and saying so
/// beats folding an unrelated frame into a list.
#[cold]
fn not_a_builtin_step() -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        "internal error: a frame that is not a builtin step reached `advance`",
    )
    .primary(Span::DUMMY, "please report this")
}

impl Interp<'_> {
    /// The tree-walker's driver for the step protocol. It calls back into user
    /// code on the host stack, which is precisely why it can never run a
    /// continuation captured inside `map`; the machine pushes the same frames
    /// onto the heap instead and can.
    pub(crate) fn call_builtin(
        &mut self,
        b: Builtin,
        args: Vec<Value>,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        let mut step = call(b, args, self.world_mut(), span)?;
        loop {
            match step {
                Step::Done(v) => return Ok(v),
                Step::Apply {
                    callee,
                    args,
                    frame,
                } => {
                    let answer = self.apply(callee, args, span)?;
                    step = advance(frame, answer)?;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{
        bin, block, callv, clause, discard, effect_def, handle, int, lam, letv, list, perform,
        standalone, var, with_cell,
    };
    use ply_syntax::ast::{BinOp, Expr, Item, Mode};

    fn ints(xs: &[i64]) -> Value {
        Value::list(xs.iter().copied().map(Value::Int).collect())
    }

    fn f() -> Value {
        Value::builtin(Builtin::IntToString)
    }

    /// Drives the protocol the way an engine does, answering every callback.
    fn drive(
        b: Builtin,
        args: Vec<Value>,
        mut answer: impl FnMut(&[Value]) -> Value,
    ) -> Result<Value, Diagnostic> {
        let mut world = World::new();
        let mut step = call(b, args, &mut world, Span::DUMMY)?;
        loop {
            match step {
                Step::Done(v) => return Ok(v),
                Step::Apply { args, frame, .. } => {
                    let v = answer(&args);
                    step = advance(frame, v)?;
                }
            }
        }
    }

    #[test]
    fn map_visits_every_element_in_order() {
        let mut seen = Vec::new();
        let out = drive(Builtin::Map, vec![ints(&[1, 2, 3]), f()], |args| {
            let n = args[0].as_int(Span::DUMMY, "test").unwrap();
            seen.push(n);
            Value::Int(n * 10)
        })
        .unwrap();
        assert_eq!(seen, [1, 2, 3]);
        assert_eq!(out.render(), "[10, 20, 30]");
    }

    #[test]
    fn an_empty_list_never_calls_the_callback() {
        let out = drive(Builtin::Map, vec![ints(&[]), f()], |_| {
            panic!("map called its function on an empty list")
        })
        .unwrap();
        assert_eq!(out.render(), "[]");
        let out = drive(Builtin::Fold, vec![ints(&[]), Value::Int(7), f()], |_| {
            panic!("fold called its function on an empty list")
        })
        .unwrap();
        assert_eq!(out.render(), "7");
    }

    #[test]
    fn filter_keeps_the_element_its_predicate_accepted() {
        let out = drive(Builtin::Filter, vec![ints(&[1, 2, 3, 4]), f()], |args| {
            let n = args[0].as_int(Span::DUMMY, "test").unwrap();
            Value::Bool(n % 2 == 0)
        })
        .unwrap();
        assert_eq!(out.render(), "[2, 4]");
    }

    #[test]
    fn fold_threads_the_accumulator_leftwards() {
        let out = drive(
            Builtin::Fold,
            vec![ints(&[1, 2, 3]), Value::Int(0), f()],
            |args| {
                let acc = args[0].as_int(Span::DUMMY, "test").unwrap();
                let x = args[1].as_int(Span::DUMMY, "test").unwrap();
                Value::Int(acc * 10 + x)
            },
        )
        .unwrap();
        assert_eq!(out.render(), "123");
    }

    /// The property the frames exist for: a suspension point inside `map` can
    /// be advanced twice and each resumption completes its own list. Host
    /// recursion cannot do this, which is why `map` is a frame.
    #[test]
    fn one_suspension_point_inside_map_can_be_resumed_twice() {
        let mut world = World::new();
        let start = call(
            Builtin::Map,
            vec![ints(&[1, 2, 3]), f()],
            &mut world,
            Span::DUMMY,
        )
        .unwrap();
        let Step::Apply { frame, .. } = start else {
            panic!("map suspends on its first element");
        };

        let finish = |mut step: Step, fill: i64| loop {
            match step {
                Step::Done(v) => return v,
                Step::Apply { frame, .. } => step = advance(frame, Value::Int(fill)).unwrap(),
            }
        };

        let a = finish(advance(frame.clone(), Value::Int(7)).unwrap(), 0);
        let b = finish(advance(frame, Value::Int(9)).unwrap(), 1);
        assert_eq!(a.render(), "[7, 0, 0]");
        assert_eq!(b.render(), "[9, 1, 1]");
    }

    #[test]
    fn a_non_boolean_from_a_filter_predicate_is_a_runtime_error() {
        let d = drive(Builtin::Filter, vec![ints(&[1]), f()], |_| Value::Int(1)).unwrap_err();
        assert_eq!(d.code, codes::RUNTIME_ERROR);
        assert!(d.message.contains("Bool"), "{}", d.message);
    }

    #[test]
    fn advancing_a_frame_that_is_not_a_builtin_step_is_reported_not_ignored() {
        let frame = Frame::Call {
            name: None,
            call_site: Span::DUMMY,
        };
        let d = advance(frame, Value::Unit).unwrap_err();
        assert_eq!(d.code, codes::INTERNAL_ERROR);
        assert!(d.message.contains("internal error"), "{}", d.message);
    }

    #[test]
    fn cell_builtins_read_and_write_the_world_they_are_given() {
        let mut world = World::new();
        let id = world.alloc(Value::Int(1));
        let set = call(
            Builtin::CellSet,
            vec![Value::Cell(id), Value::Int(2)],
            &mut world,
            Span::DUMMY,
        )
        .unwrap();
        assert!(matches!(set, Step::Done(Value::Unit)));

        let got = call(
            Builtin::CellGet,
            vec![Value::Cell(id)],
            &mut world,
            Span::DUMMY,
        )
        .unwrap();
        let Step::Done(v) = got else {
            panic!("cell_get does not suspend");
        };
        assert_eq!(v.render(), "2");
    }

    #[test]
    fn a_cell_from_another_world_is_named_rather_than_silently_read() {
        let mut other = World::new();
        let id = other.alloc(Value::Int(1));
        let mut world = World::new();
        let d = call(
            Builtin::CellGet,
            vec![Value::Cell(id)],
            &mut world,
            Span::DUMMY,
        )
        .unwrap_err();
        assert_eq!(d.code, codes::INTERNAL_ERROR);
        assert!(d.message.contains("does not belong"), "{}", d.message);
    }

    #[test]
    fn exactly_the_three_callback_builtins_are_higher_order() {
        let names: Vec<&str> = Builtin::all()
            .iter()
            .filter(|b| b.higher_order())
            .map(|b| b.name())
            .collect();
        assert_eq!(names, ["map", "filter", "fold"]);
    }

    #[test]
    fn every_builtin_is_reachable_by_the_name_it_reports() {
        for b in Builtin::all() {
            assert_eq!(Builtin::from_name(b.name()), Some(*b));
        }
    }

    fn run(items: Vec<Item>, e: Expr) -> Result<Value, Diagnostic> {
        let (program, resolved) = standalone(items);
        Interp::for_program(&program, &resolved).eval_expr_for_test(&e)
    }

    fn state() -> Item {
        effect_def("state", &[("get", Mode::Read, false)])
    }

    /// The suspension points are where a builtin is most likely to be handed a
    /// stale world, so the handler both writes a cell and decides the answer
    /// from it: a builtin that carried its own copy would keep the count at 1
    /// and keep the wrong elements.
    #[test]
    fn a_predicate_that_performs_sees_every_write_the_handler_made_before_it() {
        let bump = block(
            vec![discard(callv(
                "cell_set",
                vec![
                    var("c"),
                    bin(BinOp::Add, callv("cell_get", vec![var("c")]), int(1)),
                ],
            ))],
            Some(bin(
                BinOp::Eq,
                bin(BinOp::Rem, callv("cell_get", vec![var("c")]), int(2)),
                int(0),
            )),
        );
        let kept = handle(
            callv(
                "filter",
                vec![
                    list(vec![int(10), int(20), int(30), int(40)]),
                    lam(&["x"], perform("state", "get", None, vec![])),
                ],
            ),
            vec![clause("state", "get", None, &[], bump)],
        );
        let e = with_cell(
            "s",
            int(0),
            "c",
            block(
                vec![letv("kept", kept)],
                Some(bin(
                    BinOp::Add,
                    bin(BinOp::Mul, callv("len", vec![var("kept")]), int(100)),
                    callv("cell_get", vec![var("c")]),
                )),
            ),
        );
        assert_eq!(run(vec![state()], e).unwrap().render(), "204");
    }

    #[test]
    fn a_fold_function_may_perform_and_the_accumulator_still_threads() {
        let e = handle(
            callv(
                "fold",
                vec![
                    list(vec![int(1), int(2), int(3)]),
                    int(0),
                    lam(
                        &["acc", "x"],
                        bin(
                            BinOp::Add,
                            bin(BinOp::Add, var("acc"), var("x")),
                            perform("state", "get", None, vec![]),
                        ),
                    ),
                ],
            ),
            vec![clause("state", "get", None, &[], int(100))],
        );
        assert_eq!(run(vec![state()], e).unwrap().render(), "306");
    }

    /// The handler is inside the callback, so it is installed and torn down once
    /// per element rather than once for the whole `map`.
    #[test]
    fn a_handler_installed_inside_a_map_callback_does_not_leak_to_the_next_element() {
        let inner = handle(
            perform("state", "get", None, vec![]),
            vec![clause("state", "get", None, &[], var("x"))],
        );
        let e = callv("map", vec![list(vec![int(1), int(2)]), lam(&["x"], inner)]);
        assert_eq!(run(vec![state()], e).unwrap().render(), "[1, 2]");

        let leaked = block(
            vec![letv(
                "ys",
                callv(
                    "map",
                    vec![
                        list(vec![int(1)]),
                        lam(
                            &["x"],
                            handle(var("x"), vec![clause("state", "get", None, &[], int(9))]),
                        ),
                    ],
                ),
            )],
            Some(perform("state", "get", None, vec![])),
        );
        assert_eq!(
            run(vec![state()], leaked).unwrap_err().code,
            codes::UNHANDLED_EFFECT
        );
    }

    #[test]
    fn an_assertion_inside_a_callback_keeps_its_structured_failure() {
        let e = callv(
            "map",
            vec![
                list(vec![int(1), int(2)]),
                lam(&["x"], callv("assert_eq", vec![var("x"), int(1)])),
            ],
        );
        let d = run(Vec::new(), e).unwrap_err();
        assert_eq!(d.code, codes::ASSERTION_FAILED);
        assert_eq!(d.message, "assertion failed: expected 1, found 2");
        assert!(
            d.notes.contains(&"actual:   2".to_string()),
            "{:?}",
            d.notes
        );
    }

    #[test]
    fn an_unhandled_effect_from_a_callback_reaches_the_caller_unchanged() {
        let e = callv(
            "map",
            vec![
                list(vec![int(1)]),
                lam(&["x"], perform("state", "get", None, vec![])),
            ],
        );
        let d = run(vec![state()], e).unwrap_err();
        assert_eq!(d.code, codes::UNHANDLED_EFFECT);
    }
}
