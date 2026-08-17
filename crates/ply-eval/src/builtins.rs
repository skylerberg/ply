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

use crate::arena::{Arena, Slot};
use crate::cont::Frame;
use crate::interp::{Interp, arity_error};
use crate::map;
use crate::value::{Decimal, Value, Vector, first_difference, type_error, values_equal};
use ply_span::{Diagnostic, Span, codes};
use rust_decimal::RoundingStrategy;
use rust_decimal::prelude::ToPrimitive;
use std::fmt;

/// A list this long is a runaway `range`, not an intent.
const MAX_RANGE_LEN: i64 = 10_000_000;

/// `Decimal`'s scale bound, the type's rather than a policy.
const MAX_DECIMAL_SCALE: u32 = 28;

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
    /// One allocation over the whole list.
    ///
    /// A read loop that folds `bytes_concat` across N answers copies the
    /// accumulated prefix N times, which is O(total²) — quadratic in the size of
    /// a request an unauthenticated peer chooses. This copies each byte once.
    BytesConcatAll,
    BytesOfString,
    BytesIsUtf8,
    BytesIndexOf,
    BytesIndexOfFrom,
    BytesIndexOfByte,
    BytesStartsWith,
    BytesEndsWith,
    BytesSplit,
    BytesScan,
    BytesScanUntil,
    BytesPosition,
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
    MapNew,
    MapInsert,
    MapGet,
    MapContains,
    MapRemove,
    MapLen,
    MapKeys,
    MapValues,
    MapEntries,
    MapOfEntries,
    MapMerge,
    MapFold,
    DecimalDiv,
    DecimalRound,
    DecimalOfInt,
    IntOfDecimal,
    FloatOfDecimal,
    DecimalOfFloat,
    DecimalOfString,
    DecimalToString,
    Compare,
    /// The same order as [`Builtin::Compare`], under a name a module may not
    /// declare.
    ///
    /// `derive ord for T` generates a dictionary whose `compare` field is this
    /// call. A bare `compare` would be an ordinary name, and ADR 0001 says a
    /// module's own items shadow the prelude — so a module that happened to
    /// declare `fn compare` would supply the order of every dictionary derived
    /// in it, while `derivable(ord, T)` still called the type ordered. That is
    /// the second order ADR 0012 §2 rests on not existing.
    CompareValues,
    CellGet,
    CellSet,
    Panic,
    /// The only introduction of a [`Value::Secret`]. There is no elimination:
    /// the three below answer a `Bool` or a `Secret`, never the payload's type,
    /// so no plaintext leaves except by a host operation declaring it may
    /// receive one.
    SecretOfString,
    SecretVerify,
    SecretIsEmpty,
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
            "bytes_concat_all" => Builtin::BytesConcatAll,
            "bytes_of_string" => Builtin::BytesOfString,
            "bytes_is_utf8" => Builtin::BytesIsUtf8,
            "bytes_index_of" => Builtin::BytesIndexOf,
            "bytes_index_of_from" => Builtin::BytesIndexOfFrom,
            "bytes_index_of_byte" => Builtin::BytesIndexOfByte,
            "bytes_starts_with" => Builtin::BytesStartsWith,
            "bytes_ends_with" => Builtin::BytesEndsWith,
            "bytes_split" => Builtin::BytesSplit,
            "bytes_scan" => Builtin::BytesScan,
            "bytes_scan_until" => Builtin::BytesScanUntil,
            "bytes_position" => Builtin::BytesPosition,
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
            "compare" => Builtin::Compare,
            "compare_values" => Builtin::CompareValues,
            "map_new" => Builtin::MapNew,
            "map_insert" => Builtin::MapInsert,
            "map_get" => Builtin::MapGet,
            "map_contains" => Builtin::MapContains,
            "map_remove" => Builtin::MapRemove,
            "map_len" => Builtin::MapLen,
            "map_keys" => Builtin::MapKeys,
            "map_values" => Builtin::MapValues,
            "map_entries" => Builtin::MapEntries,
            "map_of_entries" => Builtin::MapOfEntries,
            "map_merge" => Builtin::MapMerge,
            "map_fold" => Builtin::MapFold,
            "decimal_div" => Builtin::DecimalDiv,
            "decimal_round" => Builtin::DecimalRound,
            "decimal_of_int" => Builtin::DecimalOfInt,
            "int_of_decimal" => Builtin::IntOfDecimal,
            "float_of_decimal" => Builtin::FloatOfDecimal,
            "decimal_of_float" => Builtin::DecimalOfFloat,
            "decimal_of_string" => Builtin::DecimalOfString,
            "decimal_to_string" => Builtin::DecimalToString,
            "cell_get" => Builtin::CellGet,
            "cell_set" => Builtin::CellSet,
            "panic" => Builtin::Panic,
            "secret_of_string" => Builtin::SecretOfString,
            "secret_verify" => Builtin::SecretVerify,
            "secret_is_empty" => Builtin::SecretIsEmpty,
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
            Builtin::BytesConcatAll => "bytes_concat_all",
            Builtin::BytesOfString => "bytes_of_string",
            Builtin::BytesIsUtf8 => "bytes_is_utf8",
            Builtin::BytesIndexOf => "bytes_index_of",
            Builtin::BytesIndexOfFrom => "bytes_index_of_from",
            Builtin::BytesIndexOfByte => "bytes_index_of_byte",
            Builtin::BytesStartsWith => "bytes_starts_with",
            Builtin::BytesEndsWith => "bytes_ends_with",
            Builtin::BytesSplit => "bytes_split",
            Builtin::BytesScan => "bytes_scan",
            Builtin::BytesScanUntil => "bytes_scan_until",
            Builtin::BytesPosition => "bytes_position",
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
            Builtin::Compare => "compare",
            Builtin::CompareValues => "compare_values",
            Builtin::MapNew => "map_new",
            Builtin::MapInsert => "map_insert",
            Builtin::MapGet => "map_get",
            Builtin::MapContains => "map_contains",
            Builtin::MapRemove => "map_remove",
            Builtin::MapLen => "map_len",
            Builtin::MapKeys => "map_keys",
            Builtin::MapValues => "map_values",
            Builtin::MapEntries => "map_entries",
            Builtin::MapOfEntries => "map_of_entries",
            Builtin::MapMerge => "map_merge",
            Builtin::MapFold => "map_fold",
            Builtin::DecimalDiv => "decimal_div",
            Builtin::DecimalRound => "decimal_round",
            Builtin::DecimalOfInt => "decimal_of_int",
            Builtin::IntOfDecimal => "int_of_decimal",
            Builtin::FloatOfDecimal => "float_of_decimal",
            Builtin::DecimalOfFloat => "decimal_of_float",
            Builtin::DecimalOfString => "decimal_of_string",
            Builtin::DecimalToString => "decimal_to_string",
            Builtin::CellGet => "cell_get",
            Builtin::CellSet => "cell_set",
            Builtin::Panic => "panic",
            Builtin::SecretOfString => "secret_of_string",
            Builtin::SecretVerify => "secret_verify",
            Builtin::SecretIsEmpty => "secret_is_empty",
        }
    }

    /// Inclusive `(min, max)` argument counts.
    pub fn arity(self) -> (usize, usize) {
        match self {
            Builtin::Assert => (1, 2),
            Builtin::Range => (1, 2),
            // Ply has no top-level constants, so the empty map is a call.
            Builtin::MapNew => (0, 0),
            Builtin::Len
            | Builtin::IntToString
            | Builtin::CellGet
            | Builtin::Panic
            | Builtin::BytesLen
            | Builtin::BytesOfString
            | Builtin::BytesIsUtf8
            | Builtin::BytesConcatAll
            | Builtin::StringOfBytes
            | Builtin::StringOfBytesLossy
            | Builtin::StringLen
            | Builtin::StringTrim
            | Builtin::StringLower
            | Builtin::StringUpper
            | Builtin::MapLen
            | Builtin::MapKeys
            | Builtin::MapValues
            | Builtin::MapEntries
            | Builtin::MapOfEntries
            | Builtin::DecimalOfInt
            | Builtin::FloatOfDecimal
            | Builtin::DecimalOfFloat
            | Builtin::DecimalOfString
            | Builtin::DecimalToString
            | Builtin::SecretOfString
            | Builtin::SecretIsEmpty => (1, 1),
            Builtin::AssertEq
            | Builtin::Push
            | Builtin::Map
            | Builtin::Filter
            | Builtin::StringConcat
            | Builtin::CellSet
            | Builtin::BytesAt
            | Builtin::BytesConcat
            | Builtin::BytesIndexOf
            | Builtin::BytesIndexOfByte
            | Builtin::BytesStartsWith
            | Builtin::BytesEndsWith
            | Builtin::BytesSplit
            | Builtin::StringSplit
            | Builtin::StringStartsWith
            | Builtin::StringEndsWith
            | Builtin::StringContains
            | Builtin::StringFind
            | Builtin::MapGet
            | Builtin::MapContains
            | Builtin::MapRemove
            | Builtin::MapMerge
            | Builtin::Compare
            | Builtin::CompareValues
            | Builtin::IntOfDecimal
            | Builtin::SecretVerify => (2, 2),
            Builtin::Fold
            | Builtin::BytesSlice
            | Builtin::BytesIndexOfFrom
            | Builtin::BytesPosition
            | Builtin::StringSlice
            | Builtin::MapInsert
            | Builtin::MapFold
            | Builtin::DecimalRound => (3, 3),
            Builtin::BytesScan | Builtin::BytesScanUntil | Builtin::DecimalDiv => (4, 4),
        }
    }

    /// Calls user code, so [`call`] may answer [`Step::Apply`] rather than a
    /// value and the caller must be able to suspend.
    pub fn higher_order(self) -> bool {
        matches!(
            self,
            Builtin::Map
                | Builtin::Filter
                | Builtin::Fold
                | Builtin::BytesPosition
                | Builtin::MapFold
        )
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
            Builtin::BytesConcatAll,
            Builtin::BytesOfString,
            Builtin::BytesIsUtf8,
            Builtin::BytesIndexOf,
            Builtin::BytesIndexOfFrom,
            Builtin::BytesIndexOfByte,
            Builtin::BytesStartsWith,
            Builtin::BytesEndsWith,
            Builtin::BytesSplit,
            Builtin::BytesScan,
            Builtin::BytesScanUntil,
            Builtin::BytesPosition,
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
            Builtin::MapNew,
            Builtin::MapInsert,
            Builtin::MapGet,
            Builtin::MapContains,
            Builtin::MapRemove,
            Builtin::MapLen,
            Builtin::MapKeys,
            Builtin::MapValues,
            Builtin::MapEntries,
            Builtin::MapOfEntries,
            Builtin::MapMerge,
            Builtin::MapFold,
            Builtin::DecimalDiv,
            Builtin::DecimalRound,
            Builtin::DecimalOfInt,
            Builtin::IntOfDecimal,
            Builtin::FloatOfDecimal,
            Builtin::DecimalOfFloat,
            Builtin::DecimalOfString,
            Builtin::DecimalToString,
            Builtin::Compare,
            Builtin::CompareValues,
            Builtin::CellGet,
            Builtin::CellSet,
            Builtin::Panic,
            Builtin::SecretOfString,
            Builtin::SecretVerify,
            Builtin::SecretIsEmpty,
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

/// `push`, with the reuse that is the point of reference counting.
///
/// A list the caller is the last owner of grows in place; one anybody else can
/// still see is copied. The two answer the same value — that is what
/// [`Arc::get_mut`] checks and why the choice is invisible — and they do not
/// cost the same: appending in a fold copies the whole accumulator per element
/// without this, which is quadratic in the list being built.
///
/// [`Arc::get_mut`]: std::sync::Arc::get_mut
fn push(mut args: Vec<Value>, span: Span) -> Result<Step, Diagnostic> {
    let x = args.pop().expect("arity checked");
    let mut xs = args.pop().expect("arity checked");
    let copied = match &mut xs {
        Value::List(list) => match std::sync::Arc::get_mut(list) {
            Some(items) => {
                items.push(x);
                crate::rc::note_update(true);
                return Ok(Step::Done(xs));
            }
            None => {
                let mut out = Vec::with_capacity(list.len() + 1);
                out.extend(list.iter().cloned());
                out.push(x);
                out
            }
        },
        other => return Err(type_error(span, "`push`", "List", other)),
    };
    crate::rc::note_update(false);
    Ok(Step::Done(Value::list(copied)))
}

/// `cells` is the run's live arena, threaded rather than snapshotted:
/// `cell_get` must observe every write made before this call, including one a
/// handler clause made before resuming.
pub fn call(
    b: Builtin,
    args: Vec<Value>,
    cells: &mut Arena,
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

        Builtin::Push => push(args, span),

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

        Builtin::BytesConcatAll => {
            let pieces = args[0].as_list(span, "`bytes_concat_all`")?.clone();
            let mut total = 0usize;
            for piece in pieces.iter() {
                total += piece.as_bytes(span, "`bytes_concat_all`")?.len();
            }
            let mut out = Vec::with_capacity(total);
            for piece in pieces.iter() {
                out.extend_from_slice(piece.as_bytes(span, "`bytes_concat_all`")?);
            }
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

        Builtin::BytesIndexOf => {
            let hay = args[0].as_bytes(span, "`bytes_index_of`")?;
            let needle = args[1].as_bytes(span, "`bytes_index_of`")?;
            Ok(Step::Done(position(find(hay, needle, 0))))
        }

        Builtin::BytesIndexOfFrom => {
            let hay = args[0].as_bytes(span, "`bytes_index_of_from`")?;
            let needle = args[1].as_bytes(span, "`bytes_index_of_from`")?;
            let from = start_at(&args[2], hay.len(), span, "bytes_index_of_from")?;
            Ok(Step::Done(position(find(hay, needle, from))))
        }

        Builtin::BytesIndexOfByte => {
            let hay = args[0].as_bytes(span, "`bytes_index_of_byte`")?;
            let byte = one_byte(&args[1], span, "bytes_index_of_byte")?;
            Ok(Step::Done(position(memchr::memchr(byte, hay))))
        }

        Builtin::BytesStartsWith => {
            let b = args[0].as_bytes(span, "`bytes_starts_with`")?;
            let prefix = args[1].as_bytes(span, "`bytes_starts_with`")?;
            Ok(Step::Done(Value::Bool(b.starts_with(prefix))))
        }

        Builtin::BytesEndsWith => {
            let b = args[0].as_bytes(span, "`bytes_ends_with`")?;
            let suffix = args[1].as_bytes(span, "`bytes_ends_with`")?;
            Ok(Step::Done(Value::Bool(b.ends_with(suffix))))
        }

        Builtin::BytesSplit => {
            let b = args[0].as_bytes(span, "`bytes_split`")?;
            let sep = args[1].as_bytes(span, "`bytes_split`")?;
            if sep.is_empty() {
                return Err(Diagnostic::error(
                    codes::RUNTIME_ERROR,
                    "`bytes_split` needs a separator, and this one is empty",
                )
                .primary(span, "an empty separator matches everywhere and nowhere")
                .note("pass the bytes that actually separate the parts, as in `b\"\\r\\n\"`"));
            }
            let mut out = Vec::new();
            let mut at = 0;
            for found in memchr::memmem::find_iter(b, sep.as_ref()) {
                out.push(Value::bytes(&b[at..found]));
                at = found + sep.len();
            }
            out.push(Value::bytes(&b[at..]));
            Ok(Step::Done(Value::list(out)))
        }

        Builtin::BytesScan => {
            let b = args[0].as_bytes(span, "`bytes_scan`")?;
            Ok(Step::Done(Value::Int(scan(&args, b, span, false)?)))
        }

        Builtin::BytesScanUntil => {
            let b = args[0].as_bytes(span, "`bytes_scan_until`")?;
            Ok(Step::Done(Value::Int(scan(&args, b, span, true)?)))
        }

        Builtin::BytesPosition => {
            let b = args[0].as_bytes(span, "`bytes_position`")?.clone();
            let from = start_at(&args[1], b.len(), span, "bytes_position")?;
            Ok(next_position(args[2].clone(), b, from, span))
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

        // The order `Map` iterates in, so a derived `OrdDict` and a map's own
        // key order are one order rather than two that can drift. `Float` is
        // refused by `derivable(ord, ·)` at the signature, which is what keeps
        // `total_cmp` out of reach of a program that could observe it
        // disagreeing with `==`.
        Builtin::Compare | Builtin::CompareValues => {
            // The runtime backstop ADR 0015 §2.2 asks for. `derivable(ord, ·)`
            // already refuses a `Secret` at both walks, so a well-typed program
            // cannot arrive here; this is what a defect in either walk meets
            // instead of an ordering oracle over a credential.
            crate::value::secret_has_no_order(&args[0], b.name(), span)?;
            crate::value::secret_has_no_order(&args[1], b.name(), span)?;
            Ok(Step::Done(Value::ctor(
                match args[0].cmp(&args[1]) {
                    std::cmp::Ordering::Less => "Less",
                    std::cmp::Ordering::Equal => "Equal",
                    std::cmp::Ordering::Greater => "Greater",
                },
                Vec::new(),
            )))
        }

        // Every one of these reaches a key through `map::key`, which is the one
        // gate `Value::cmp` is behind. The check is not repeated here, because a
        // backstop written once per call site is a backstop a seventh map
        // builder forgets — which is exactly how `map_of_entries` and
        // `map_merge` came to be an ordering oracle over a credential.
        Builtin::MapNew => Ok(Step::Done(map::new())),
        Builtin::MapInsert => {
            let mut args = args;
            let (k, v) = (args.remove(1), args.remove(1));
            Ok(Step::Done(map::insert(args.remove(0), k, v, span)?))
        }
        Builtin::MapGet => Ok(Step::Done(map::get(&args[0], &args[1], span)?)),
        Builtin::MapContains => Ok(Step::Done(map::contains(&args[0], &args[1], span)?)),
        Builtin::MapRemove => {
            let mut args = args;
            let k = args.remove(1);
            Ok(Step::Done(map::remove(args.remove(0), &k, span)?))
        }
        Builtin::MapLen => Ok(Step::Done(map::len(&args[0], span)?)),
        Builtin::MapKeys => Ok(Step::Done(map::keys(&args[0], span)?)),
        Builtin::MapValues => Ok(Step::Done(map::values(&args[0], span)?)),
        Builtin::MapEntries => Ok(Step::Done(map::entries(&args[0], span)?)),
        Builtin::MapOfEntries => Ok(Step::Done(map::of_entries(&args[0], span)?)),
        Builtin::MapMerge => Ok(Step::Done(map::merge(&args[0], &args[1], span)?)),

        Builtin::MapFold => {
            let entries = map::fold_entries(&args[0], span)?;
            Ok(map::next_fold(
                args[2].clone(),
                entries,
                0,
                args[1].clone(),
                span,
            ))
        }

        Builtin::CellGet => {
            let slot = args[0].as_cell(span, "`cell_get`")?;
            match cells.get(slot) {
                Some(v) => Ok(Step::Done(v.clone())),
                None => Err(no_such_cell(span, slot)),
            }
        }

        Builtin::CellSet => {
            let slot = args[0].as_cell(span, "`cell_set`")?;
            // Reported rather than refused: refusing would change what a legal
            // program means, and ADR 0017 §4 accepts the leak and asks only that
            // it be said out loud.
            crate::rc::cell_cycle(slot, &args[1], span);
            let mut args = args;
            if cells.set(slot, args.remove(1)) {
                Ok(Step::Done(Value::Unit))
            } else {
                Err(no_such_cell(span, slot))
            }
        }

        // `/` on `Decimal` is `E0209` precisely so that a division names its
        // scale and its rounding mode here instead.
        Builtin::DecimalDiv => {
            let a = args[0].as_decimal(span, "`decimal_div`")?;
            let b = args[1].as_decimal(span, "`decimal_div`")?;
            let scale = decimal_scale(&args[2], span, "decimal_div")?;
            let mode = rounding(&args[3], span, "decimal_div")?;
            if b.is_zero() {
                return Err(crate::interp::err_zero_divisor(span, "`decimal_div`"));
            }
            let quotient = a
                .checked_div(b)
                .ok_or_else(|| decimal_overflow(span, "division"))?;
            Ok(Step::Done(Value::Decimal(
                quotient.round_dp_with_strategy(scale, mode),
            )))
        }

        Builtin::DecimalRound => {
            let d = args[0].as_decimal(span, "`decimal_round`")?;
            let scale = decimal_scale(&args[1], span, "decimal_round")?;
            let mode = rounding(&args[2], span, "decimal_round")?;
            Ok(Step::Done(Value::Decimal(
                d.round_dp_with_strategy(scale, mode),
            )))
        }

        // Total: every `Int` is a `Decimal`, at scale 0.
        Builtin::DecimalOfInt => Ok(Step::Done(Value::Decimal(Decimal::from(
            args[0].as_int(span, "`decimal_of_int`")?,
        )))),

        Builtin::IntOfDecimal => {
            let d = args[0].as_decimal(span, "`int_of_decimal`")?;
            let mode = rounding(&args[1], span, "int_of_decimal")?;
            Ok(Step::Done(option(
                d.round_dp_with_strategy(0, mode).to_i64().map(Value::Int),
            )))
        }

        // Lossy and total, which is the honest pair: every `Decimal` has a
        // nearest `f64`, and saying so beats an `Option` nobody can act on.
        Builtin::FloatOfDecimal => {
            let d = args[0].as_decimal(span, "`float_of_decimal`")?;
            Ok(Step::Done(Value::Float(float_of_decimal(d))))
        }

        Builtin::DecimalOfFloat => {
            let f = args[0].as_float(span, "`decimal_of_float`")?;
            Ok(Step::Done(option(decimal_of_float(f).map(Value::Decimal))))
        }

        Builtin::DecimalOfString => {
            let s = args[0].as_str(span, "`decimal_of_string`")?;
            Ok(Step::Done(option(parse_decimal(s).map(Value::Decimal))))
        }

        // Round-trips `decimal_of_string` exactly, scale included: `1.50m`
        // renders `1.50`, because the trailing zero is what the value carries.
        Builtin::DecimalToString => {
            let d = args[0].as_decimal(span, "`decimal_to_string`")?;
            Ok(Step::Done(Value::str(d.to_string())))
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

        // Does not consume its argument: Ply is a value language, so the
        // plaintext is still in scope and can still be traced or returned.
        // Containment starts here, and ADR 0015 §2.5 (2) says so out loud.
        Builtin::SecretOfString => {
            args[0].as_str(span, "`secret_of_string`")?;
            Ok(Step::Done(Value::secret(args[0].clone())))
        }

        // One bit per call, constant time over the compared bytes, and not rate
        // limited — a loop over candidates recovers the value, which is the
        // program's to prevent (§2.5 (3)).
        Builtin::SecretVerify => {
            let Value::Secret(held) = &args[0] else {
                return Err(type_error(span, "`secret_verify`", "Secret", &args[0]));
            };
            let candidate = args[1].as_str(span, "`secret_verify`")?;
            let held = held.as_str(span, "`secret_verify`")?;
            Ok(Step::Done(Value::Bool(crate::value::constant_time_eq(
                held.as_bytes(),
                candidate.as_bytes(),
            ))))
        }

        // Presence, never the value. An operator must be able to tell a missing
        // credential from a wrong one, and that is metadata.
        Builtin::SecretIsEmpty => {
            let Value::Secret(held) = &args[0] else {
                return Err(type_error(span, "`secret_is_empty`", "Secret", &args[0]));
            };
            Ok(Step::Done(Value::Bool(match &**held {
                Value::Str(s) => s.is_empty(),
                Value::Bytes(b) => b.is_empty(),
                // `secret_of_string` is the only introduction, so nothing else
                // is constructible; a payload that is not a sequence has no
                // emptiness, and answering `false` reports "a credential is
                // present" rather than inventing one.
                _ => false,
            })))
        }
    }
}

/// `Some(v)` or `None`, the prelude's.
fn option(v: Option<Value>) -> Value {
    match v {
        Some(v) => Value::ctor("Some", vec![v]),
        None => Value::ctor("None", Vec::new()),
    }
}

/// A `Rounding` argument as `rust_decimal`'s strategy.
///
/// The six are the prelude's constructors, and an argument that is not one of
/// them is a runtime error rather than a default: silently choosing a rounding
/// is exactly what refusing `/` was for.
fn rounding(v: &Value, span: Span, what: &str) -> Result<RoundingStrategy, Diagnostic> {
    let name = match v {
        Value::Ctor { name, args } if args.is_empty() => name.as_str(),
        other => return Err(type_error(span, &format!("`{what}`"), "Rounding", other)),
    };
    match name {
        "HalfEven" => Ok(RoundingStrategy::MidpointNearestEven),
        "HalfUp" => Ok(RoundingStrategy::MidpointAwayFromZero),
        "Down" => Ok(RoundingStrategy::ToZero),
        "Up" => Ok(RoundingStrategy::AwayFromZero),
        "Ceiling" => Ok(RoundingStrategy::ToPositiveInfinity),
        "Floor" => Ok(RoundingStrategy::ToNegativeInfinity),
        other => Err(Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("`{other}` is not a rounding mode"),
        )
        .primary(span, format!("`{what}` was given `{other}`"))
        .note("the modes are `HalfEven`, `HalfUp`, `Down`, `Up`, `Ceiling` and `Floor`")),
    }
}

/// A scale argument, refused outside `0..=28` rather than clamped: a scale the
/// caller asked for and did not get is a rounding they did not write down.
fn decimal_scale(v: &Value, span: Span, what: &str) -> Result<u32, Diagnostic> {
    let scale = int_arg(v, span, what)?;
    u32::try_from(scale)
        .ok()
        .filter(|s| *s <= MAX_DECIMAL_SCALE)
        .ok_or_else(|| {
            Diagnostic::error(
                codes::RUNTIME_ERROR,
                format!("`{what}` needs a scale in 0..={MAX_DECIMAL_SCALE}, not {scale}"),
            )
            .primary(span, "`Decimal` holds at most 28 decimal places")
        })
}

fn decimal_overflow(span: Span, what: &str) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`Decimal` overflow in {what}"),
    )
    .primary(span, "the result needs more than 96 bits of mantissa")
    .note("`Decimal` is exact and bounded; it will not round to make room")
}

/// The **shortest decimal that round-trips the float**, and `None` for NaN, an
/// infinity, and anything outside `Decimal`'s range.
///
/// Shortest is the only defensible choice: any other is an arbitrary number of
/// digits of a binary approximation, and `0.1` would decode as
/// `0.1000000000000000055511151231257827` — technically the value, and not what
/// anybody wrote. Rust's own `f64` formatting already produces exactly that
/// shortest form, so this is a format and a parse rather than an algorithm.
fn decimal_of_float(f: f64) -> Option<Decimal> {
    if !f.is_finite() {
        return None;
    }
    parse_decimal(&format!("{f}"))
}

/// The **nearest** `f64` to a decimal, which is what makes
/// `float_of_decimal(decimal_of_float(f)) == f` for every `f` that has a
/// `Decimal` at all.
///
/// Through the decimal's own digits rather than through `Decimal::to_f64`,
/// which divides a mantissa by a power of ten in binary and is therefore off by
/// an ulp or two for a value with a long scale — enough that a `Float` field's
/// derived JSON codec silently changed the value it round-tripped. Rust's
/// `f64` parser is correctly rounded, and `Decimal::to_string` is exact, so the
/// pair is.
fn float_of_decimal(d: Decimal) -> f64 {
    d.to_string()
        .parse::<f64>()
        .unwrap_or_else(|_| d.to_f64().unwrap_or(f64::NAN))
}

/// The one decimal grammar, for `decimal_of_string` and for the shortest
/// round-tripping form of a `Float` alike.
///
/// The two parsers are disjoint rather than layered: `from_str_exact` preserves
/// the scale the text was written with, which is what makes `1.50` a different
/// value from `1.5`, and it refuses an exponent; `from_scientific` requires one.
/// Dispatching on `e` therefore never costs the trailing zero of a plain
/// literal, and `1e3` — which JSON's grammar admits and which is well inside
/// `Decimal`'s range — parses instead of being reported as out of range.
/// Anything needing a scale past 28 or a mantissa past 96 bits is still `None`.
fn parse_decimal(text: &str) -> Option<Decimal> {
    if text.contains(['e', 'E']) {
        Decimal::from_scientific(text).ok()
    } else {
        Decimal::from_str_exact(text).ok()
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

        Frame::MapFoldStep {
            f,
            entries,
            next,
            span,
        } => map::next_fold(f, entries, next, answer, span),

        Frame::BytesPositionStep {
            f,
            bytes,
            next,
            span,
        } => {
            if answer.as_bool(span, "the predicate given to `bytes_position`")? {
                // `next` is one past the byte the predicate was asked about,
                // and the answer is that byte's index rather than the byte.
                Step::Done(position(Some(next - 1)))
            } else {
                next_position(f, bytes, next, span)
            }
        }

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

fn next_position(f: Value, bytes: std::sync::Arc<[u8]>, next: usize, span: Span) -> Step {
    let Some(byte) = bytes.get(next).copied() else {
        return Step::Done(position(None));
    };
    Step::Apply {
        callee: f.clone(),
        args: vec![Value::Int(i64::from(byte))],
        frame: Frame::BytesPositionStep {
            f,
            bytes,
            next: next + 1,
            span,
        },
    }
}

/// `Option<Int>`, the answer shape of every builtin that searches. `Some` and
/// `None` are the prelude's constructors, so these names are program-wide and
/// need no module.
fn position(at: Option<usize>) -> Value {
    match at {
        Some(i) => Value::ctor("Some", vec![Value::Int(i as i64)]),
        None => Value::ctor("None", Vec::new()),
    }
}

/// An empty needle occurs at `from`, which is what `str::find` answers and what
/// makes `bytes_index_of(b, b"")` `Some(0)` rather than a special case every
/// caller has to write.
fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(from);
    }
    memchr::memmem::find(&hay[from..], needle).map(|at| from + at)
}

/// A 256-bit membership test built once per call, in time proportional to the
/// set rather than to the buffer. This is the whole reason `bytes_scan` takes
/// its byte class as a `Bytes` and still costs nothing per byte.
struct ByteSet([u64; 4]);

impl ByteSet {
    fn new(members: &[u8]) -> ByteSet {
        let mut bits = [0u64; 4];
        for &b in members {
            bits[usize::from(b >> 6)] |= 1 << (b & 63);
        }
        ByteSet(bits)
    }

    fn contains(&self, b: u8) -> bool {
        self.0[usize::from(b >> 6)] >> (b & 63) & 1 == 1
    }
}

/// The bytes a bounded scan is allowed to look at: `max` of them at most, and
/// never past the end. Returning the window rather than a pair of indices is
/// what makes the budget structural — a scan cannot examine a byte it was not
/// handed.
fn scan_window(hay: &[u8], from: usize, max: usize) -> &[u8] {
    &hay[from..hay.len().min(from.saturating_add(max))]
}

/// The typed-argument readers the byte and decimal builtins use, quoting the
/// operation's name only when the argument is the wrong type.
///
/// `v.as_int(span, &format!("`{what}`"))` builds that `String` on the success
/// path, which is every call: W6 counted 540 such allocations per request, all
/// of them for a message no request produced.
fn int_arg(v: &Value, span: Span, what: &str) -> Result<i64, Diagnostic> {
    match v {
        Value::Int(i) => Ok(*i),
        other => Err(crate::value::type_error(
            span,
            &format!("`{what}`"),
            "Int",
            other,
        )),
    }
}

fn bytes_arg<'a>(
    v: &'a Value,
    span: Span,
    what: &str,
) -> Result<&'a std::sync::Arc<[u8]>, Diagnostic> {
    match v {
        Value::Bytes(b) => Ok(b),
        other => Err(crate::value::type_error(
            span,
            &format!("`{what}`"),
            "Bytes",
            other,
        )),
    }
}

/// Both bounded scans. `want` is whether it stops on a member of the set or on
/// a non-member; the answer is the index it stopped at, or the end of the
/// window when it did not, so a caller tells "the class ended" from "the budget
/// ran out" by comparing against `from + max`.
fn scan(args: &[Value], hay: &[u8], span: Span, want: bool) -> Result<i64, Diagnostic> {
    let what = if want {
        "bytes_scan_until"
    } else {
        "bytes_scan"
    };
    let from = start_at(&args[1], hay.len(), span, what)?;
    let members = bytes_arg(&args[2], span, what)?;
    let max = budget(&args[3], span, what)?;
    let window = scan_window(hay, from, max);

    // `memchr` is SIMD and a bitmap loop is not, so a small set — which is
    // every header delimiter a parser cares about — takes the fast path.
    let found = match (want, members.as_ref()) {
        // An empty class is never entered, so `bytes_scan_until` runs out the
        // window and `bytes_scan` — which stops off the class — stops at once.
        (true, []) => None,
        (true, [a]) => memchr::memchr(*a, window),
        (true, [a, b]) => memchr::memchr2(*a, *b, window),
        (true, [a, b, c]) => memchr::memchr3(*a, *b, *c, window),
        _ => {
            let set = ByteSet::new(members);
            window.iter().position(|&b| set.contains(b) == want)
        }
    };
    Ok(match found {
        Some(at) => (from + at) as i64,
        None => (from + window.len()) as i64,
    })
}

/// A position a search may start at. `len` itself is admissible — an empty
/// window is a real answer — and anything else is refused rather than clamped,
/// for the reason [`range_args`] gives.
fn start_at(v: &Value, len: usize, span: Span, what: &str) -> Result<usize, Diagnostic> {
    let from = int_arg(v, span, what)?;
    match usize::try_from(from) {
        Ok(from) if from <= len => Ok(from),
        _ => Err(Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("`{what}` start {from} is outside a value of {len} bytes"),
        )
        .primary(span, "this position does not exist")
        .note(format!(
            "a start must satisfy `0 <= from <= {len}`; it is never clamped"
        ))),
    }
}

/// The bound that stops a 20-megabyte header line from being a denial of
/// service. Negative is refused rather than treated as zero: a caller that
/// computed a negative budget has a bug, and answering `from` for it hides it.
fn budget(v: &Value, span: Span, what: &str) -> Result<usize, Diagnostic> {
    let max = int_arg(v, span, what)?;
    usize::try_from(max).map_err(|_| {
        Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("`{what}` was given a negative budget of {max}"),
        )
        .primary(span, "a scan cannot examine a negative number of bytes")
        .note("pass `0` to examine nothing, or `bytes_len(b)` to leave it unbounded")
    })
}

fn one_byte(v: &Value, span: Span, what: &str) -> Result<u8, Diagnostic> {
    let byte = int_arg(v, span, what)?;
    u8::try_from(byte).map_err(|_| {
        Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("`{what}` was given {byte}, which is not a byte"),
        )
        .primary(span, "a byte is `0` to `255`")
        .note("`bytes_at` answers in that range, and so does a byte literal's element")
    })
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
    let start = int_arg(start, span, what)?;
    let end = int_arg(end, span, what)?;
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

/// A cell whose region has closed. Reachable only by carrying a value out of
/// the run that made it, which no source program can express; the generation in
/// the slot is what turns it into this report rather than a read of whatever
/// now lives at that position.
#[cold]
fn no_such_cell(span: Span, slot: Slot) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("cell {slot} does not belong to the region this code is running in"),
    )
    .primary(span, "this cell was made by a different run")
    .note("please report this: a cell value escaped the region that allocated it")
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
        let mut step = call(b, args, self.cells_mut(), span)?;
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
    use crate::task_regions::TaskRegions;
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
        let mut cells = TaskRegions::new();
        let mut step = call(b, args, cells.arena_mut(), Span::DUMMY)?;
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

    /// A builtin that cannot suspend, called the way an engine calls it.
    fn done(b: Builtin, args: Vec<Value>) -> Result<Value, Diagnostic> {
        let mut cells = TaskRegions::new();
        match call(b, args, cells.arena_mut(), Span::DUMMY)? {
            Step::Done(v) => Ok(v),
            Step::Apply { .. } => panic!("`{}` suspended", b.name()),
        }
    }

    fn bytes(b: &[u8]) -> Value {
        Value::bytes(b)
    }

    /// `Some(i)` or `None`, rendered, which is what a Ply program sees.
    fn found(b: Builtin, args: Vec<Value>) -> String {
        done(b, args).unwrap().render()
    }

    fn some(i: i64) -> String {
        format!("Some({i})")
    }

    /// `Some(i)` as `i` and `None` as `-1`, which is the shape W1's folds
    /// answered in and therefore the shape a comparison against them needs.
    fn at(v: &Value) -> i64 {
        match v {
            Value::Ctor { args, .. } if !args.is_empty() => {
                args[0].as_int(Span::DUMMY, "test").unwrap()
            }
            _ => -1,
        }
    }

    /// Deterministic and dependency-free, so a failing case is a seed a reader
    /// can reproduce rather than a number that moves between runs.
    struct Xorshift(u64);

    impl Xorshift {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, bound: usize) -> usize {
            (self.next() % bound as u64) as usize
        }
    }

    // ------------------------------------------------------------ the searches

    #[test]
    fn index_of_covers_empty_absent_at_the_start_at_the_end_and_overlapping() {
        let hay = bytes(b"aaabaaab");
        assert_eq!(
            found(Builtin::BytesIndexOf, vec![hay.clone(), bytes(b"")]),
            some(0)
        );
        assert_eq!(
            found(Builtin::BytesIndexOf, vec![bytes(b""), bytes(b"")]),
            some(0)
        );
        assert_eq!(
            found(Builtin::BytesIndexOf, vec![bytes(b""), bytes(b"a")]),
            "None"
        );
        assert_eq!(
            found(Builtin::BytesIndexOf, vec![hay.clone(), bytes(b"z")]),
            "None"
        );
        assert_eq!(
            found(
                Builtin::BytesIndexOf,
                vec![hay.clone(), bytes(b"aaabaaabx")]
            ),
            "None",
            "a needle longer than the haystack cannot occur"
        );
        assert_eq!(
            found(Builtin::BytesIndexOf, vec![hay.clone(), bytes(b"aaa")]),
            some(0)
        );
        assert_eq!(
            found(Builtin::BytesIndexOf, vec![hay.clone(), bytes(b"aab")]),
            some(1)
        );
        assert_eq!(
            found(Builtin::BytesIndexOf, vec![hay.clone(), bytes(b"b")]),
            some(3)
        );

        // Overlapping occurrences: `aa` sits at 0, 1, 4 and 5, and the first is
        // the answer.
        assert_eq!(
            found(Builtin::BytesIndexOf, vec![hay.clone(), bytes(b"aa")]),
            some(0)
        );
        assert_eq!(
            found(
                Builtin::BytesIndexOfFrom,
                vec![hay.clone(), bytes(b"aa"), Value::Int(1)]
            ),
            some(1)
        );
        assert_eq!(
            found(
                Builtin::BytesIndexOfFrom,
                vec![hay.clone(), bytes(b"aa"), Value::Int(2)]
            ),
            some(4)
        );
    }

    /// The index a `_from` search answers is absolute, so it feeds straight
    /// back into `bytes_slice`. A relative one would be an off-by-`from` in
    /// every caller that resumed a scan.
    #[test]
    fn index_of_from_answers_an_absolute_index_and_admits_the_end() {
        let hay = bytes(b"GET / HTTP/1.1");
        assert_eq!(
            found(
                Builtin::BytesIndexOfFrom,
                vec![hay.clone(), bytes(b" "), Value::Int(0)]
            ),
            some(3)
        );
        assert_eq!(
            found(
                Builtin::BytesIndexOfFrom,
                vec![hay.clone(), bytes(b" "), Value::Int(4)]
            ),
            some(5)
        );
        assert_eq!(
            found(
                Builtin::BytesIndexOfFrom,
                vec![hay.clone(), bytes(b""), Value::Int(14)]
            ),
            some(14),
            "an empty needle occurs where the search started, the end included"
        );
        assert_eq!(
            found(
                Builtin::BytesIndexOfFrom,
                vec![hay.clone(), bytes(b" "), Value::Int(14)]
            ),
            "None"
        );
    }

    #[test]
    fn a_start_outside_the_buffer_is_named_rather_than_clamped() {
        for (b, args) in [
            (
                Builtin::BytesIndexOfFrom,
                vec![bytes(b"abc"), bytes(b"a"), Value::Int(4)],
            ),
            (
                Builtin::BytesIndexOfFrom,
                vec![bytes(b"abc"), bytes(b"a"), Value::Int(-1)],
            ),
            (
                Builtin::BytesScan,
                vec![bytes(b"abc"), Value::Int(9), bytes(b"a"), Value::Int(1)],
            ),
        ] {
            let d = done(b, args).unwrap_err();
            assert_eq!(d.code, codes::RUNTIME_ERROR, "{}", b.name());
            assert!(
                d.message.contains("outside a value of 3 bytes"),
                "{}",
                d.message
            );
        }
    }

    #[test]
    fn index_of_byte_takes_a_byte_and_refuses_anything_else() {
        assert_eq!(
            found(
                Builtin::BytesIndexOfByte,
                vec![bytes(b"a\r\nb"), Value::Int(13)]
            ),
            some(1)
        );
        assert_eq!(
            found(
                Builtin::BytesIndexOfByte,
                vec![bytes(b"abc"), Value::Int(255)]
            ),
            "None"
        );
        for out_of_range in [-1, 256] {
            let d = done(
                Builtin::BytesIndexOfByte,
                vec![bytes(b"abc"), Value::Int(out_of_range)],
            )
            .unwrap_err();
            assert_eq!(d.code, codes::RUNTIME_ERROR);
            assert!(d.message.contains("not a byte"), "{}", d.message);
        }
    }

    /// Required test 36. A naive search is obviously correct and obviously
    /// slow, which is exactly what a SIMD one should be checked against.
    #[test]
    fn index_of_agrees_with_a_naive_search_over_ten_thousand_pairs() {
        fn naive(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
            if needle.is_empty() {
                return Some(from);
            }
            (from..=hay.len().checked_sub(needle.len())?)
                .find(|&i| &hay[i..i + needle.len()] == needle)
        }

        let mut rng = Xorshift(0x5eed_1234_9abc_def1);
        for case in 0..10_000 {
            // A three-letter alphabet, so needles hit often and overlap.
            let hay: Vec<u8> = (0..rng.below(40))
                .map(|_| b'a' + rng.below(3) as u8)
                .collect();
            let needle: Vec<u8> = (0..rng.below(4))
                .map(|_| b'a' + rng.below(3) as u8)
                .collect();
            let from = if hay.is_empty() {
                0
            } else {
                rng.below(hay.len() + 1)
            };

            assert_eq!(
                done(Builtin::BytesIndexOf, vec![bytes(&hay), bytes(&needle)])
                    .unwrap()
                    .render(),
                match naive(&hay, &needle, 0) {
                    Some(i) => some(i as i64),
                    None => "None".to_string(),
                },
                "case {case}: {hay:?} / {needle:?}"
            );
            assert_eq!(
                done(
                    Builtin::BytesIndexOfFrom,
                    vec![bytes(&hay), bytes(&needle), Value::Int(from as i64)]
                )
                .unwrap()
                .render(),
                match naive(&hay, &needle, from) {
                    Some(i) => some(i as i64),
                    None => "None".to_string(),
                },
                "case {case}: {hay:?} / {needle:?} from {from}"
            );
        }
    }

    // ------------------------------------------------------- prefix and suffix

    #[test]
    fn starts_with_and_ends_with_agree_with_the_empty_and_whole_cases() {
        let b = bytes(b"HTTP/1.1");
        for (builtin, hits, misses) in [
            (
                Builtin::BytesStartsWith,
                [&b""[..], b"H", b"HTTP/1.1"],
                [&b"HTTP/1.10"[..], b"T", b"1.1"],
            ),
            (
                Builtin::BytesEndsWith,
                [&b""[..], b"1", b"HTTP/1.1"],
                [&b"0HTTP/1.1"[..], b"T", b"HTTP"],
            ),
        ] {
            for hit in hits {
                assert_eq!(
                    done(builtin, vec![b.clone(), bytes(hit)]).unwrap().render(),
                    "true",
                    "{} {hit:?}",
                    builtin.name()
                );
            }
            for miss in misses {
                assert_eq!(
                    done(builtin, vec![b.clone(), bytes(miss)])
                        .unwrap()
                        .render(),
                    "false",
                    "{} {miss:?}",
                    builtin.name()
                );
            }
        }
        assert_eq!(
            done(Builtin::BytesStartsWith, vec![bytes(b""), bytes(b"")])
                .unwrap()
                .render(),
            "true"
        );
    }

    // ------------------------------------------------------------------ splits

    #[test]
    fn split_keeps_the_empty_pieces_a_join_needs_to_round_trip() {
        let split =
            |hay: &[u8], sep: &[u8]| done(Builtin::BytesSplit, vec![bytes(hay), bytes(sep)]);
        assert_eq!(
            split(b"a,b,c", b",").unwrap().render(),
            "[b\"a\", b\"b\", b\"c\"]"
        );
        assert_eq!(split(b"", b",").unwrap().render(), "[b\"\"]");
        assert_eq!(split(b",", b",").unwrap().render(), "[b\"\", b\"\"]");
        assert_eq!(split(b"abc", b",").unwrap().render(), "[b\"abc\"]");
        assert_eq!(
            split(b"a\r\n\r\nb", b"\r\n").unwrap().render(),
            "[b\"a\", b\"\", b\"b\"]"
        );
        // Non-overlapping, left to right: the second `aa` starts after the
        // first one's last byte.
        assert_eq!(
            split(b"aaaa", b"aa").unwrap().render(),
            "[b\"\", b\"\", b\"\"]"
        );
    }

    /// ADR 0013 §4's builtin. The claim is the allocation count, and what is
    /// asserted here is the observable half of it: the answer is the
    /// concatenation, the empty list is `b""`, and a list holding anything but
    /// `Bytes` is refused rather than skipped.
    #[test]
    fn concat_all_joins_every_piece_in_order() {
        let empty = done(Builtin::BytesConcatAll, vec![Value::list(vec![])]).unwrap();
        assert_eq!(empty, Value::bytes([]));

        let pieces = Value::list(vec![
            bytes(b"GET "),
            bytes(b""),
            bytes(b"/x"),
            bytes(b" HTTP"),
        ]);
        assert_eq!(
            done(Builtin::BytesConcatAll, vec![pieces]).unwrap(),
            Value::bytes(b"GET /x HTTP")
        );

        let mut rng = Xorshift(0x00c0_ffee_0bad_f00d);
        for _ in 0..500 {
            let raw: Vec<Vec<u8>> = (0..rng.below(12))
                .map(|_| {
                    (0..rng.below(9))
                        .map(|_| b'a' + rng.below(4) as u8)
                        .collect()
                })
                .collect();
            let expected: Vec<u8> = raw.concat();
            let list = Value::list(raw.iter().map(|p| bytes(p)).collect());
            assert_eq!(
                done(Builtin::BytesConcatAll, vec![list]).unwrap(),
                Value::bytes(expected)
            );
        }

        let mixed = Value::list(vec![bytes(b"a"), Value::Int(1)]);
        assert_eq!(
            done(Builtin::BytesConcatAll, vec![mixed])
                .expect_err("an Int is not a piece")
                .code,
            codes::RUNTIME_ERROR
        );
    }

    /// Required test 39, both halves.
    #[test]
    fn split_round_trips_against_a_join_and_refuses_an_empty_separator() {
        let mut rng = Xorshift(0xfeed_face_dead_b0d1);
        for case in 0..2_000 {
            let hay: Vec<u8> = (0..rng.below(30))
                .map(|_| b'a' + rng.below(3) as u8)
                .collect();
            let sep: Vec<u8> = (0..1 + rng.below(3))
                .map(|_| b'a' + rng.below(3) as u8)
                .collect();
            let split = done(Builtin::BytesSplit, vec![bytes(&hay), bytes(&sep)]).unwrap();
            let Value::List(pieces) = &split else {
                panic!("`bytes_split` answers a list");
            };
            let mut joined: Vec<u8> = Vec::new();
            for (i, piece) in pieces.iter().enumerate() {
                if i > 0 {
                    joined.extend_from_slice(&sep);
                }
                let Value::Bytes(p) = piece else {
                    panic!("`bytes_split` answers a list of Bytes");
                };
                joined.extend_from_slice(p);
            }
            assert_eq!(joined, hay, "case {case}: separator {sep:?}");
        }

        let d = done(Builtin::BytesSplit, vec![bytes(b"abc"), bytes(b"")]).unwrap_err();
        assert_eq!(d.code, codes::RUNTIME_ERROR);
        assert!(d.message.contains("empty"), "{}", d.message);
    }

    // ------------------------------------------------------------ the scans

    fn scan(b: Builtin, hay: &[u8], from: i64, set: &[u8], max: i64) -> Result<i64, Diagnostic> {
        Ok(done(
            b,
            vec![bytes(hay), Value::Int(from), bytes(set), Value::Int(max)],
        )?
        .as_int(Span::DUMMY, "test")
        .unwrap())
    }

    #[test]
    fn a_scan_stops_on_the_class_and_the_other_stops_off_it() {
        let head = b"GET /orders?id=7 HTTP/1.1";
        let digits = b"0123456789";
        let big = head.len() as i64;

        assert_eq!(
            scan(Builtin::BytesScanUntil, head, 0, b" ", big).unwrap(),
            3
        );
        assert_eq!(
            scan(Builtin::BytesScanUntil, head, 4, b" ", big).unwrap(),
            16
        );
        assert_eq!(
            scan(Builtin::BytesScan, head, 4, b"/ordes?=i", big).unwrap(),
            15,
            "the target's own bytes run out at the `7`"
        );
        assert_eq!(
            scan(Builtin::BytesScan, head, 15, digits, big).unwrap(),
            16,
            "a digit run ends at the space after it"
        );
        assert_eq!(
            scan(Builtin::BytesScan, head, 14, digits, big).unwrap(),
            14,
            "`=` is not a digit, so the scan stops where it started"
        );

        // The whole point of `bytes_scan` over a fold: the answer for a run
        // that reaches the end is the end, not a sentinel.
        assert_eq!(
            scan(Builtin::BytesScanUntil, head, 0, b"z", big).unwrap(),
            big
        );
        assert_eq!(scan(Builtin::BytesScan, head, 0, b"", big).unwrap(), 0);
        assert_eq!(
            scan(Builtin::BytesScanUntil, head, 0, b"", big).unwrap(),
            big,
            "an empty class is never entered"
        );
        assert_eq!(scan(Builtin::BytesScanUntil, b"", 0, b"a", 10).unwrap(), 0);
        assert_eq!(scan(Builtin::BytesScan, head, big, b"a", big).unwrap(), big);
    }

    /// Every set size takes a different path — `memchr`, `memchr2`, `memchr3`,
    /// then the bitmap — so the four have to agree with each other.
    #[test]
    fn every_set_size_takes_its_own_path_and_they_all_agree() {
        fn naive(hay: &[u8], from: usize, set: &[u8], max: usize, want: bool) -> i64 {
            let limit = hay.len().min(from + max);
            for (i, b) in hay.iter().enumerate().take(limit).skip(from) {
                if set.contains(b) == want {
                    return i as i64;
                }
            }
            limit as i64
        }

        let mut rng = Xorshift(0x0123_4567_89ab_cdef);
        for case in 0..5_000 {
            let hay: Vec<u8> = (0..rng.below(50))
                .map(|_| b'a' + rng.below(6) as u8)
                .collect();
            let set: Vec<u8> = (0..rng.below(7))
                .map(|_| b'a' + rng.below(6) as u8)
                .collect();
            let from = if hay.is_empty() {
                0
            } else {
                rng.below(hay.len() + 1)
            };
            let max = rng.below(60);
            for (builtin, want) in [(Builtin::BytesScan, false), (Builtin::BytesScanUntil, true)] {
                assert_eq!(
                    scan(builtin, &hay, from as i64, &set, max as i64).unwrap(),
                    naive(&hay, from, &set, max, want),
                    "case {case}: {} over {hay:?} from {from} set {set:?} max {max}",
                    builtin.name()
                );
            }
        }
    }

    /// Required test 37. The bound is structural rather than timed: the scan is
    /// handed the window and cannot look outside it, so a marker placed one
    /// byte past the budget is invisible however the search is implemented.
    #[test]
    fn a_scan_examines_at_most_max_bytes() {
        for max in 0..40usize {
            assert!(scan_window(&[0u8; 64], 3, max).len() <= max);
            assert!(scan_window(&[0u8; 8], 3, max).len() <= max);
        }

        let mut hay = vec![b'a'; 1024];
        hay[100] = b'!';
        assert_eq!(
            scan(Builtin::BytesScanUntil, &hay, 0, b"!", 100).unwrap(),
            100,
            "the budget ran out exactly at the marker, which is one byte too far"
        );
        assert_eq!(
            scan(Builtin::BytesScanUntil, &hay, 0, b"!", 101).unwrap(),
            100
        );
        assert_eq!(
            scan(Builtin::BytesScanUntil, &hay, 0, b"!", 20).unwrap(),
            20,
            "a caller tells this from a hit by comparing against `from + max`"
        );
        assert_eq!(
            scan(Builtin::BytesScan, &hay, 0, b"a", 7).unwrap(),
            7,
            "the same bound applies to the complement scan"
        );
    }

    #[test]
    fn a_negative_budget_is_a_bug_in_the_caller_and_is_named() {
        let d = scan(Builtin::BytesScan, b"abc", 0, b"a", -1).unwrap_err();
        assert_eq!(d.code, codes::RUNTIME_ERROR);
        assert!(d.message.contains("negative budget"), "{}", d.message);
    }

    // --------------------------------------------------------- the escape hatch

    #[test]
    fn position_finds_the_first_byte_its_predicate_accepts() {
        let hay = bytes(b"abcXdef");
        let is_upper = |args: &[Value]| {
            let b = args[0].as_int(Span::DUMMY, "test").unwrap();
            Value::Bool((65..=90).contains(&b))
        };
        assert_eq!(
            drive(
                Builtin::BytesPosition,
                vec![hay.clone(), Value::Int(0), f()],
                is_upper
            )
            .unwrap()
            .render(),
            some(3)
        );
        assert_eq!(
            drive(
                Builtin::BytesPosition,
                vec![hay.clone(), Value::Int(4), f()],
                is_upper
            )
            .unwrap()
            .render(),
            "None"
        );
        assert_eq!(
            drive(
                Builtin::BytesPosition,
                vec![bytes(b""), Value::Int(0), f()],
                |_| panic!("an empty buffer calls no predicate")
            )
            .unwrap()
            .render(),
            "None"
        );
    }

    /// Required test 38. The whole reason this builtin exists beside the fold
    /// it replaces: it stops.
    #[test]
    fn position_calls_its_predicate_once_for_a_match_at_the_start_of_a_megabyte() {
        let mut calls = 0;
        let out = drive(
            Builtin::BytesPosition,
            vec![Value::bytes(vec![7u8; 1 << 20]), Value::Int(0), f()],
            |_| {
                calls += 1;
                Value::Bool(true)
            },
        )
        .unwrap();
        assert_eq!(out.render(), some(0));
        assert_eq!(calls, 1);
    }

    #[test]
    fn position_reports_a_non_boolean_answer_rather_than_reading_past_it() {
        let d = drive(
            Builtin::BytesPosition,
            vec![bytes(b"ab"), Value::Int(0), f()],
            |_| Value::Int(1),
        )
        .unwrap_err();
        assert_eq!(d.code, codes::RUNTIME_ERROR);
        assert!(d.message.contains("Bool"), "{}", d.message);
    }

    /// The property every builtin frame owes: a suspension point captured
    /// inside it can be advanced more than once, and each resumption is its own
    /// search.
    #[test]
    fn one_suspension_point_inside_position_can_be_resumed_twice() {
        let mut cells = TaskRegions::new();
        let start = call(
            Builtin::BytesPosition,
            vec![bytes(b"abc"), Value::Int(0), f()],
            cells.arena_mut(),
            Span::DUMMY,
        )
        .unwrap();
        let Step::Apply { frame, .. } = start else {
            panic!("`bytes_position` suspends on its first byte");
        };

        let finish = |mut step: Step, fill: bool| loop {
            match step {
                Step::Done(v) => return v,
                Step::Apply { frame, .. } => step = advance(frame, Value::Bool(fill)).unwrap(),
            }
        };
        assert_eq!(
            finish(advance(frame.clone(), Value::Bool(true)).unwrap(), false).render(),
            some(0)
        );
        assert_eq!(
            finish(advance(frame, Value::Bool(false)).unwrap(), true).render(),
            some(1)
        );
    }

    // ---------------------------------------------------- against what they replace

    /// The fold-based `index_of` from W1's `examples/hello.ply`, verbatim in
    /// Rust. The new builtins have to answer what it answered — the point of
    /// the milestone is that they answer it in one pass instead of `n`.
    #[test]
    fn the_scans_agree_with_the_folds_they_replace() {
        fn fold_index_of(hay: &[u8], byte: u8, from: usize) -> i64 {
            // The fold's shape, kept: it visits every remaining byte even after
            // it has the answer, which is the cost the builtins removed.
            let mut found: i64 = -1;
            for (i, &b) in hay.iter().enumerate().skip(from) {
                if found < 0 && b == byte {
                    found = i as i64;
                }
            }
            found
        }

        fn fold_head_end(head: &[u8]) -> i64 {
            let mut found: i64 = -1;
            for i in 0..head.len().saturating_sub(3) {
                if found < 0 && &head[i..i + 4] == b"\r\n\r\n" {
                    found = (i + 4) as i64;
                }
            }
            found
        }

        fn fold_all_upper(b: &[u8]) -> bool {
            b.iter().all(|c| (65..=90).contains(c))
        }

        let mut rng = Xorshift(0xabcd_ef01_2345_6789);
        for case in 0..5_000 {
            let head: Vec<u8> = (0..rng.below(60))
                .map(|_| [b'G', b'E', b'T', b' ', b'/', b'\r', b'\n', b'A'][rng.below(8)])
                .collect();
            let len = head.len() as i64;

            // `bytes_index_of_byte`, absent as `None` rather than as `-1`.
            let byte = b'\n';
            let native = at(&done(
                Builtin::BytesIndexOfByte,
                vec![bytes(&head), Value::Int(i64::from(byte))],
            )
            .unwrap());
            assert_eq!(
                native,
                fold_index_of(&head, byte, 0),
                "case {case}: {head:?}"
            );

            // The same question through the bounded scan, whose "absent" is the
            // end of the window rather than a sentinel.
            let stopped = scan(Builtin::BytesScanUntil, &head, 0, &[byte], len).unwrap();
            assert_eq!(
                if stopped == len { -1 } else { stopped },
                fold_index_of(&head, byte, 0),
                "case {case}: {head:?}"
            );

            // `head_end`, which was the most expensive of the five folds.
            let found = at(&done(
                Builtin::BytesIndexOf,
                vec![bytes(&head), bytes(b"\r\n\r\n")],
            )
            .unwrap());
            let end = if found < 0 { -1 } else { found + 4 };
            assert_eq!(end, fold_head_end(&head), "case {case}: {head:?}");

            // `all_upper`, as a complement scan that reaches the end.
            let upper = scan(
                Builtin::BytesScan,
                &head,
                0,
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZ",
                len,
            )
            .unwrap();
            assert_eq!(upper == len, fold_all_upper(&head), "case {case}: {head:?}");
        }
    }

    /// These are byte builtins and index in bytes, which is the whole reason a
    /// request target is `Bytes`: a peer may send what is not UTF-8 at all. The
    /// `String` pair indexes in characters, and the two answers differ on the
    /// same text — so this pins the distinction rather than leaving a caller to
    /// discover it on the first non-ASCII request.
    #[test]
    fn the_byte_searches_index_in_bytes_where_the_string_ones_index_in_characters() {
        let text = "héllo=wörld";
        assert_eq!(
            found(
                Builtin::BytesIndexOfByte,
                vec![bytes(text.as_bytes()), Value::Int(i64::from(b'='))]
            ),
            some(6),
            "`é` is two bytes, so the byte index is one past the character index"
        );
        assert_eq!(
            done(Builtin::StringFind, vec![Value::str(text), Value::str("=")])
                .unwrap()
                .render(),
            "5"
        );

        // A byte search may stop in the middle of a character, and the piece it
        // cuts is refused by `string_of_bytes` rather than silently replaced.
        let cut = done(
            Builtin::BytesSlice,
            vec![bytes(text.as_bytes()), Value::Int(0), Value::Int(2)],
        )
        .unwrap();
        assert_eq!(
            done(Builtin::BytesIsUtf8, vec![cut.clone()])
                .unwrap()
                .render(),
            "false"
        );
        assert_eq!(
            done(Builtin::StringOfBytes, vec![cut]).unwrap_err().code,
            codes::RUNTIME_ERROR
        );

        // A multi-byte needle is matched whole, so a search never reports a
        // position that splits one.
        assert_eq!(
            found(
                Builtin::BytesIndexOf,
                vec![bytes(text.as_bytes()), bytes("ö".as_bytes())]
            ),
            some(8)
        );
        assert_eq!(
            found(
                Builtin::BytesIndexOf,
                vec![bytes(text.as_bytes()), bytes(&"é".as_bytes()[1..])]
            ),
            some(2),
            "a needle that is half a character still occurs where those bytes do"
        );
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
        let mut cells = TaskRegions::new();
        let start = call(
            Builtin::Map,
            vec![ints(&[1, 2, 3]), f()],
            cells.arena_mut(),
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
            memo: false,
        };
        let d = advance(frame, Value::Unit).unwrap_err();
        assert_eq!(d.code, codes::INTERNAL_ERROR);
        assert!(d.message.contains("internal error"), "{}", d.message);
    }

    #[test]
    fn cell_builtins_read_and_write_the_arena_they_are_given() {
        let mut cells = TaskRegions::new();
        let slot = cells.alloc_cell(Value::Int(1));
        let set = call(
            Builtin::CellSet,
            vec![Value::Cell(slot), Value::Int(2)],
            cells.arena_mut(),
            Span::DUMMY,
        )
        .unwrap();
        assert!(matches!(set, Step::Done(Value::Unit)));

        let got = call(
            Builtin::CellGet,
            vec![Value::Cell(slot)],
            cells.arena_mut(),
            Span::DUMMY,
        )
        .unwrap();
        let Step::Done(v) = got else {
            panic!("cell_get does not suspend");
        };
        assert_eq!(v.render(), "2");
    }

    /// The generation is what makes this a report rather than a read of the cell
    /// now living at that position: the stale slot and the live one share an
    /// index and differ in generation.
    #[test]
    fn a_cell_from_another_region_stack_is_named_rather_than_silently_read() {
        let mut other = TaskRegions::new();
        other.alloc_cell(Value::Int(1));
        other.reset();
        let stale = other.alloc_cell(Value::Int(2));

        let mut cells = TaskRegions::new();
        let live = cells.alloc_cell(Value::Int(3));
        assert_eq!(stale.index(), live.index());
        assert_ne!(stale.generation(), live.generation());

        let d = call(
            Builtin::CellGet,
            vec![Value::Cell(stale)],
            cells.arena_mut(),
            Span::DUMMY,
        )
        .unwrap_err();
        assert_eq!(d.code, codes::INTERNAL_ERROR);
        assert!(d.message.contains("does not belong"), "{}", d.message);
    }

    #[test]
    fn exactly_the_callback_builtins_are_higher_order() {
        let names: Vec<&str> = Builtin::all()
            .iter()
            .filter(|b| b.higher_order())
            .map(|b| b.name())
            .collect();
        let mut names = names;
        names.sort_unstable();
        assert_eq!(
            names,
            ["bytes_position", "filter", "fold", "map", "map_fold"]
        );
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
    /// stale arena, so the handler both writes a cell and decides the answer
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
