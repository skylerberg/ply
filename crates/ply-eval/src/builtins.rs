//! The prelude, in one definition per builtin.

use crate::arena::{Arena, Slot};
use crate::cont::Frame;
use crate::map;
use crate::semantics::arity_error;
use crate::value::{
    Decimal, Fixed, IntTy, List, Value, first_difference, type_error, values_equal,
};
use ply_core::ty::INT_TYPES;
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
    /// The list index, and the whole of it.
    ListAt,
    Map,
    Filter,
    Fold,
    /// The one loop that can stop before its bound.
    Iterate,
    Range,
    /// The three arithmetic operations that answer instead of raising, so a step defined to
    /// wrap says so in the name it calls.
    WrapAdd,
    WrapSub,
    WrapMul,
    /// The low thirty-two bits rotated right, the one step a hash written over masked words
    /// spells as two shifts, a subtraction and a mask otherwise.
    Rotr32,
    /// The same step at a fixed-width type, turning the whole word: `rotr` at `U32` is what
    /// `rotr32` was at `Int`, and at `U8` it turns eight bits.
    Rotr,
    /// `u32_of_int` and its seven siblings: an `Int` read as a fixed-width type, raising when it
    /// is not one of that type's values. Mask first if a truncation is what you meant.
    ///
    /// Sixteen field-less variants rather than two carrying an [`IntTy`], because the enum is
    /// cast to an index for the per-builtin caches and a payload would forbid the cast.
    /// [`Builtin::of_int`] and [`Builtin::int_of`] are how the rest of the tree names them.
    U8OfInt,
    U16OfInt,
    U32OfInt,
    U64OfInt,
    I8OfInt,
    I16OfInt,
    I32OfInt,
    I64OfInt,
    IntOfU8,
    IntOfU16,
    IntOfU32,
    IntOfU64,
    IntOfI8,
    IntOfI16,
    IntOfI32,
    IntOfI64,
    /// The smaller and the larger of two integers.
    Min,
    Max,
    /// One byte, from the number naming it: the inverse of `bytes_at`, which had none.
    ByteOfInt,
    IntToString,
    StringConcat,
    BytesLen,
    BytesAt,
    /// Four bytes as one `U32`, least significant first.
    ///
    /// `bytes_at` four times and shifted is the same answer and four times the bounds checking:
    /// BLAKE3's block decoding spent 128 branches and 131 comparisons per block doing exactly
    /// that, against sixteen loads for the same bytes here.
    BytesU32Le,
    BytesSlice,
    BytesConcat,
    /// One allocation over the whole list.
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
    /// `CellUpdate`'s shape over one map entry.
    MapUpdate,
    DecimalDiv,
    DecimalRound,
    DecimalOfInt,
    IntOfDecimal,
    FloatOfDecimal,
    DecimalOfFloat,
    DecimalOfString,
    DecimalToString,
    /// The IEEE 754 bit pattern, as the signed 64-bit `Int` it fits in.
    BitsOfFloat,
    FloatOfBits,
    Compare,
    /// The same order as [`Builtin::Compare`], under a name a module may not declare.
    CompareValues,
    CellGet,
    CellSet,
    /// The fused update: takes the cell's contents out of the arena, applies the function, and
    /// stores the answer. Sole ownership established at runtime, which is what an append inside
    /// the function needs and what no analysis of the caller can prove.
    CellUpdate,
    Panic,
    /// The only introduction of a [`Value::Secret`].
    SecretOfString,
    SecretVerify,
    SecretIsEmpty,
}

impl Builtin {
    pub fn from_name(name: &str) -> Option<Builtin> {
        if let Some(t) = IntTy::of_int_from_name(name) {
            return Some(Builtin::of_int(t));
        }
        if let Some(t) = IntTy::to_int_from_name(name) {
            return Some(Builtin::int_of(t));
        }
        Some(match name {
            "assert" => Builtin::Assert,
            "assert_eq" => Builtin::AssertEq,
            "len" => Builtin::Len,
            "push" => Builtin::Push,
            "list_at" => Builtin::ListAt,
            "map" => Builtin::Map,
            "filter" => Builtin::Filter,
            "fold" => Builtin::Fold,
            "range" => Builtin::Range,
            "wrap_add" => Builtin::WrapAdd,
            "min" => Builtin::Min,
            "max" => Builtin::Max,
            "wrap_sub" => Builtin::WrapSub,
            "wrap_mul" => Builtin::WrapMul,
            "rotr32" => Builtin::Rotr32,
            "rotr" => Builtin::Rotr,
            "byte_of_int" => Builtin::ByteOfInt,
            "int_to_string" => Builtin::IntToString,
            "string_concat" => Builtin::StringConcat,
            "bytes_len" => Builtin::BytesLen,
            "bytes_at" => Builtin::BytesAt,
            "bytes_u32_le" => Builtin::BytesU32Le,
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
            "map_update" => Builtin::MapUpdate,
            "iterate" => Builtin::Iterate,
            "decimal_div" => Builtin::DecimalDiv,
            "decimal_round" => Builtin::DecimalRound,
            "decimal_of_int" => Builtin::DecimalOfInt,
            "int_of_decimal" => Builtin::IntOfDecimal,
            "float_of_decimal" => Builtin::FloatOfDecimal,
            "decimal_of_float" => Builtin::DecimalOfFloat,
            "decimal_of_string" => Builtin::DecimalOfString,
            "decimal_to_string" => Builtin::DecimalToString,
            "bits_of_float" => Builtin::BitsOfFloat,
            "float_of_bits" => Builtin::FloatOfBits,
            "cell_get" => Builtin::CellGet,
            "cell_set" => Builtin::CellSet,
            "cell_update" => Builtin::CellUpdate,
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
            Builtin::ListAt => "list_at",
            Builtin::Map => "map",
            Builtin::Filter => "filter",
            Builtin::Fold => "fold",
            Builtin::Iterate => "iterate",
            Builtin::Range => "range",
            Builtin::WrapAdd => "wrap_add",
            Builtin::Min => "min",
            Builtin::Max => "max",
            Builtin::WrapSub => "wrap_sub",
            Builtin::WrapMul => "wrap_mul",
            Builtin::Rotr32 => "rotr32",
            Builtin::Rotr => "rotr",
            Builtin::U8OfInt => "u8_of_int",
            Builtin::U16OfInt => "u16_of_int",
            Builtin::U32OfInt => "u32_of_int",
            Builtin::U64OfInt => "u64_of_int",
            Builtin::I8OfInt => "i8_of_int",
            Builtin::I16OfInt => "i16_of_int",
            Builtin::I32OfInt => "i32_of_int",
            Builtin::I64OfInt => "i64_of_int",
            Builtin::IntOfU8 => "int_of_u8",
            Builtin::IntOfU16 => "int_of_u16",
            Builtin::IntOfU32 => "int_of_u32",
            Builtin::IntOfU64 => "int_of_u64",
            Builtin::IntOfI8 => "int_of_i8",
            Builtin::IntOfI16 => "int_of_i16",
            Builtin::IntOfI32 => "int_of_i32",
            Builtin::IntOfI64 => "int_of_i64",
            Builtin::ByteOfInt => "byte_of_int",
            Builtin::IntToString => "int_to_string",
            Builtin::StringConcat => "string_concat",
            Builtin::BytesLen => "bytes_len",
            Builtin::BytesAt => "bytes_at",
            Builtin::BytesU32Le => "bytes_u32_le",
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
            Builtin::MapUpdate => "map_update",
            Builtin::DecimalDiv => "decimal_div",
            Builtin::DecimalRound => "decimal_round",
            Builtin::DecimalOfInt => "decimal_of_int",
            Builtin::IntOfDecimal => "int_of_decimal",
            Builtin::FloatOfDecimal => "float_of_decimal",
            Builtin::DecimalOfFloat => "decimal_of_float",
            Builtin::DecimalOfString => "decimal_of_string",
            Builtin::DecimalToString => "decimal_to_string",
            Builtin::BitsOfFloat => "bits_of_float",
            Builtin::FloatOfBits => "float_of_bits",
            Builtin::CellGet => "cell_get",
            Builtin::CellSet => "cell_set",
            Builtin::CellUpdate => "cell_update",
            Builtin::Panic => "panic",
            Builtin::SecretOfString => "secret_of_string",
            Builtin::SecretVerify => "secret_verify",
            Builtin::SecretIsEmpty => "secret_is_empty",
        }
    }

    /// Inclusive `(min, max)` argument counts.
    pub fn arity(self) -> (usize, usize) {
        match self {
            // Every builtin is exactly applied.
            Builtin::MapNew => (0, 0),
            // Ply has no top-level constants, so the empty map is a call.
            Builtin::Len
            | Builtin::IntToString
            | Builtin::ByteOfInt
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
            | Builtin::BitsOfFloat
            | Builtin::FloatOfBits
            | Builtin::SecretOfString
            | Builtin::SecretIsEmpty
            | Builtin::U8OfInt
            | Builtin::U16OfInt
            | Builtin::U32OfInt
            | Builtin::U64OfInt
            | Builtin::I8OfInt
            | Builtin::I16OfInt
            | Builtin::I32OfInt
            | Builtin::I64OfInt
            | Builtin::IntOfU8
            | Builtin::IntOfU16
            | Builtin::IntOfU32
            | Builtin::IntOfU64
            | Builtin::IntOfI8
            | Builtin::IntOfI16
            | Builtin::IntOfI32
            | Builtin::IntOfI64 => (1, 1),
            Builtin::AssertEq
            | Builtin::Push
            | Builtin::ListAt
            | Builtin::Map
            | Builtin::Filter
            | Builtin::StringConcat
            | Builtin::CellSet
            | Builtin::CellUpdate
            | Builtin::BytesAt
            | Builtin::BytesU32Le
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
            | Builtin::SecretVerify
            | Builtin::Assert
            | Builtin::WrapAdd
            | Builtin::Min
            | Builtin::Max
            | Builtin::WrapSub
            | Builtin::WrapMul
            | Builtin::Rotr32
            | Builtin::Rotr
            | Builtin::Range => (2, 2),
            Builtin::Fold
            | Builtin::Iterate
            | Builtin::BytesSlice
            | Builtin::BytesIndexOfFrom
            | Builtin::BytesPosition
            | Builtin::StringSlice
            | Builtin::MapInsert
            | Builtin::MapFold
            | Builtin::MapUpdate
            | Builtin::DecimalRound => (3, 3),
            Builtin::BytesScan | Builtin::BytesScanUntil | Builtin::DecimalDiv => (4, 4),
        }
    }

    /// Calls user code, so [`call`] may answer [`Step::Apply`] rather than a value and the caller
    /// must be able to suspend.
    pub fn higher_order(self) -> bool {
        matches!(
            self,
            Builtin::Map
                | Builtin::Filter
                | Builtin::Fold
                | Builtin::Iterate
                | Builtin::BytesPosition
                | Builtin::MapFold
                | Builtin::CellUpdate
                | Builtin::MapUpdate
        )
    }

    /// The conversion into `t`, as a builtin.
    pub fn of_int(t: IntTy) -> Builtin {
        match t {
            IntTy::U8 => Builtin::U8OfInt,
            IntTy::U16 => Builtin::U16OfInt,
            IntTy::U32 => Builtin::U32OfInt,
            IntTy::U64 => Builtin::U64OfInt,
            IntTy::I8 => Builtin::I8OfInt,
            IntTy::I16 => Builtin::I16OfInt,
            IntTy::I32 => Builtin::I32OfInt,
            IntTy::I64 => Builtin::I64OfInt,
        }
    }

    /// The conversion out of `t`, as a builtin.
    pub fn int_of(t: IntTy) -> Builtin {
        match t {
            IntTy::U8 => Builtin::IntOfU8,
            IntTy::U16 => Builtin::IntOfU16,
            IntTy::U32 => Builtin::IntOfU32,
            IntTy::U64 => Builtin::IntOfU64,
            IntTy::I8 => Builtin::IntOfI8,
            IntTy::I16 => Builtin::IntOfI16,
            IntTy::I32 => Builtin::IntOfI32,
            IntTy::I64 => Builtin::IntOfI64,
        }
    }

    /// Which type this converts into, if it converts into one.
    pub fn converts_into(self) -> Option<IntTy> {
        INT_TYPES.into_iter().find(|t| Builtin::of_int(*t) == self)
    }

    /// Which type this converts out of, if it converts out of one.
    pub fn converts_from(self) -> Option<IntTy> {
        INT_TYPES.into_iter().find(|t| Builtin::int_of(*t) == self)
    }

    pub fn all() -> &'static [Builtin] {
        &[
            Builtin::Assert,
            Builtin::AssertEq,
            Builtin::Len,
            Builtin::Push,
            Builtin::ListAt,
            Builtin::Map,
            Builtin::Filter,
            Builtin::Fold,
            Builtin::Iterate,
            Builtin::Range,
            Builtin::WrapAdd,
            Builtin::WrapSub,
            Builtin::WrapMul,
            Builtin::Rotr32,
            Builtin::Rotr,
            Builtin::U8OfInt,
            Builtin::U16OfInt,
            Builtin::U32OfInt,
            Builtin::U64OfInt,
            Builtin::I8OfInt,
            Builtin::I16OfInt,
            Builtin::I32OfInt,
            Builtin::I64OfInt,
            Builtin::IntOfU8,
            Builtin::IntOfU16,
            Builtin::IntOfU32,
            Builtin::IntOfU64,
            Builtin::IntOfI8,
            Builtin::IntOfI16,
            Builtin::IntOfI32,
            Builtin::IntOfI64,
            Builtin::Min,
            Builtin::Max,
            Builtin::ByteOfInt,
            Builtin::IntToString,
            Builtin::StringConcat,
            Builtin::BytesLen,
            Builtin::BytesAt,
            Builtin::BytesU32Le,
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
            Builtin::MapUpdate,
            Builtin::DecimalDiv,
            Builtin::DecimalRound,
            Builtin::DecimalOfInt,
            Builtin::IntOfDecimal,
            Builtin::FloatOfDecimal,
            Builtin::DecimalOfFloat,
            Builtin::DecimalOfString,
            Builtin::DecimalToString,
            Builtin::BitsOfFloat,
            Builtin::FloatOfBits,
            Builtin::Compare,
            Builtin::CompareValues,
            Builtin::CellGet,
            Builtin::CellSet,
            Builtin::CellUpdate,
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
    /// Apply `callee` to `args`, then hand the answer to [`advance`] along with `frame`.
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

/// `push`, with the reuse that is the point of reference counting — and with the defect that reuse
/// arrives attached to.
fn push(args: &mut Vec<Value>, span: Span) -> Result<Step, Diagnostic> {
    let x = args.pop().expect("arity checked");
    let mut xs = args.pop().expect("arity checked");
    let Value::List(list) = &mut xs else {
        return Err(type_error(span, "`push`", "List", &xs));
    };
    let copied = list.push(x);
    crate::rc::note_update_of(copied.is_none(), copied.unwrap_or(0), span);
    Ok(Step::Done(xs))
}

/// `cells` is the run's live arena, threaded rather than snapshotted: `cell_get` must observe every
/// write made before this call, including one a handler clause made before resuming.
///
/// The argument vector goes back to the free list whatever the builtin did with its contents:
/// measured on the request path, builtin calls consuming their vectors were the single largest
/// source of allocations, at a third of the total.
pub fn call(
    b: Builtin,
    mut args: Vec<Value>,
    cells: &mut Arena,
    span: Span,
) -> Result<Step, Diagnostic> {
    let out = call_with(b, &mut args, cells, span);
    args.clear();
    crate::argv::give(args);
    out
}

fn call_with(
    b: Builtin,
    args: &mut Vec<Value>,
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
            Err(assert_failure(&args[1], span))
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

        Builtin::ListAt => {
            let xs = args[0].as_list(span, "`list_at`")?;
            let i = args[1].as_int(span, "`list_at`")?;
            Ok(Step::Done(option(at(xs, i).cloned())))
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

        Builtin::Iterate => {
            let budget = args[1].as_int(span, "`iterate`")?;
            if budget < 1 {
                return Err(crate::limit::err_iterate_budget_not_a_count(span, budget));
            }
            next_iterate(args[2].clone(), args[0].clone(), budget, budget, span)
        }

        Builtin::Range => {
            let lo = args[0].as_int(span, "`range`")?;
            let hi = args[1].as_int(span, "`range`")?;
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

        // Two's complement, modulo 2^64: the only arithmetic in the language that cannot raise.
        Builtin::Min | Builtin::Max => {
            let x = args[0].as_int(span, &format!("`{}`", b.name()))?;
            let y = args[1].as_int(span, &format!("`{}`", b.name()))?;
            Ok(Step::Done(Value::Int(if b == Builtin::Min {
                x.min(y)
            } else {
                x.max(y)
            })))
        }

        // The low thirty-two bits of `x` rotated right by `n` modulo thirty-two, answered as the
        // non-negative `Int` a masked word is.
        Builtin::Rotr32 => {
            let x = args[0].as_int(span, "`rotr32`")?;
            let n = args[1].as_int(span, "`rotr32`")?;
            Ok(Step::Done(Value::Int(i64::from(
                (x as u32).rotate_right((n & 31) as u32),
            ))))
        }

        // The word turned whole, at whatever width the operand's type is, and the count taken
        // modulo that width so every count names a rotation.
        Builtin::Rotr => {
            let n = args[1].as_int(span, "`rotr`")?;
            if let Value::Fixed(f) = &args[0] {
                let w = f.ty.bits();
                let k = n.rem_euclid(i64::from(w)) as u32;
                let raw = f.raw();
                let bits = if k == 0 {
                    raw
                } else {
                    (raw >> k) | (raw << (w - k))
                };
                return Ok(Step::Done(Value::Fixed(Fixed::new(f.ty, bits))));
            }
            let x = args[0].as_int(span, "`rotr`")?;
            Ok(Step::Done(Value::Int(
                (x as u64).rotate_right(n.rem_euclid(64) as u32) as i64,
            )))
        }

        // Total by construction at every type, which is the whole of why they exist beside the
        // checked operators.
        Builtin::WrapAdd | Builtin::WrapSub | Builtin::WrapMul => {
            let what = b.name();
            if let (Value::Fixed(x), Value::Fixed(y)) = (&args[0], &args[1])
                && x.ty == y.ty
            {
                let v = match b {
                    Builtin::WrapAdd => x.wrapping(*y, |a, c| a.wrapping_add(c)),
                    Builtin::WrapSub => x.wrapping(*y, |a, c| a.wrapping_sub(c)),
                    _ => x.wrapping(*y, |a, c| a.wrapping_mul(c)),
                };
                return Ok(Step::Done(Value::Fixed(v)));
            }
            let x = args[0].as_int(span, &format!("`{what}`"))?;
            let y = args[1].as_int(span, &format!("`{what}`"))?;
            Ok(Step::Done(Value::Int(match b {
                Builtin::WrapAdd => x.wrapping_add(y),
                Builtin::WrapSub => x.wrapping_sub(y),
                _ => x.wrapping_mul(y),
            })))
        }

        // Out of range raises rather than truncating, for the reason `byte_of_int` does: a value
        // silently reduced is one nobody chose. `x & 0xFFFF_FFFF` first is how a program says it
        // meant the truncation.
        Builtin::U8OfInt
        | Builtin::U16OfInt
        | Builtin::U32OfInt
        | Builtin::U64OfInt
        | Builtin::I8OfInt
        | Builtin::I16OfInt
        | Builtin::I32OfInt
        | Builtin::I64OfInt => {
            let t = b.converts_into().expect("every `_of_int` names its type");
            let n = args[0].as_int(span, &format!("`{}`", t.of_int_name()))?;
            match Fixed::of(t, i128::from(n)) {
                Some(v) => Ok(Step::Done(Value::Fixed(v))),
                None => Err(Diagnostic::error(
                    codes::RUNTIME_ERROR,
                    format!("`{}` was given {n}", t.of_int_name()),
                )
                .primary(span, format!("`{t}` holds {} to {}", t.min(), t.max()))
                .note(format!(
                    "mask the value before the call if that is what you meant: `n & 0x{:X}`",
                    t.max()
                ))),
            }
        }

        // The same value as an `Int`. Only `U64` reaches past `Int`, and only above its largest.
        Builtin::IntOfU8
        | Builtin::IntOfU16
        | Builtin::IntOfU32
        | Builtin::IntOfU64
        | Builtin::IntOfI8
        | Builtin::IntOfI16
        | Builtin::IntOfI32
        | Builtin::IntOfI64 => {
            let t = b.converts_from().expect("every `int_of_` names its type");
            let f = args[0].as_fixed(span, &format!("`{}`", t.to_int_name()))?;
            match i64::try_from(f.value()) {
                Ok(n) => Ok(Step::Done(Value::Int(n))),
                Err(_) => Err(Diagnostic::error(
                    codes::RUNTIME_ERROR,
                    format!("`{}` was given {f}", t.to_int_name()),
                )
                .primary(span, "past the largest `Int`")
                .note("an `Int` is 64 bits and signed, so it does not hold every `U64`")),
            }
        }

        // Out of range raises rather than masking, because a silent `& 0xFF` would write a
        // byte nobody chose.
        Builtin::ByteOfInt => {
            let n = args[0].as_int(span, "`byte_of_int`")?;
            match u8::try_from(n) {
                Ok(byte) => Ok(Step::Done(Value::bytes([byte]))),
                Err(_) => Err(Diagnostic::error(
                    codes::RUNTIME_ERROR,
                    format!("`byte_of_int` was given {n}"),
                )
                .primary(span, "a byte is 0 to 255")
                .note("mask the value before the call if that is what you meant: `n & 0xFF`")),
            }
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

        Builtin::BytesU32Le => {
            let b = args[0].as_bytes(span, "`bytes_u32_le`")?;
            let i = args[1].as_int(span, "`bytes_u32_le`")?;
            let four = usize::try_from(i)
                .ok()
                .and_then(|i| b.get(i..i + 4))
                .and_then(|s| <[u8; 4]>::try_from(s).ok());
            match four {
                Some(w) => Ok(Step::Done(Value::Fixed(Fixed::new(
                    IntTy::U32,
                    u64::from(u32::from_le_bytes(w)),
                )))),
                // Reported against the last index it would have read, since that is the one past
                // the end and the one the caller has to move.
                None => Err(out_of_range(span, "bytes_u32_le", i + 3, b.len())),
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
            Ok(Step::Done(Value::Int(scan(args, b, span, false)?)))
        }

        Builtin::BytesScanUntil => {
            let b = args[0].as_bytes(span, "`bytes_scan_until`")?;
            Ok(Step::Done(Value::Int(scan(args, b, span, true)?)))
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

        // `len` is `(List<a>) -> Int`, so a String needs its own name until W2 settles
        // type-directed dispatch.
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

        // These three read `std`'s Unicode tables, so their answers move if those tables do.
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

        // The order `Map` iterates in, so a derived `OrdDict` and a map's own key order are one
        // order rather than two that can drift.
        Builtin::Compare | Builtin::CompareValues => {
            // The runtime backstop the secret representation asks for.
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

        // Every one of these reaches a key through `map::key`, which is the one gate `Value::cmp`
        // is behind.
        Builtin::MapNew => Ok(Step::Done(map::new())),
        Builtin::MapInsert => {
            let (k, v) = (args.remove(1), args.remove(1));
            Ok(Step::Done(map::insert(args.remove(0), k, v, span)?))
        }
        Builtin::MapGet => Ok(Step::Done(map::get(&args[0], &args[1], span)?)),
        Builtin::MapContains => Ok(Step::Done(map::contains(&args[0], &args[1], span)?)),
        Builtin::MapRemove => {
            let k = args.remove(1);
            Ok(Step::Done(map::remove(args.remove(0), &k, span)?))
        }
        // An absent key leaves the map as it was, for `map_remove`'s reason.
        Builtin::MapUpdate => {
            let f = args.remove(2);
            let key = args.remove(1);
            let (map, taken) = map::take(args.remove(0), &key, span)?;
            Ok(match taken {
                Some(value) => Step::Apply {
                    callee: f,
                    args: crate::argv::of([value]),
                    frame: Frame::MapUpdateStep { map, key, span },
                },
                None => Step::Done(map),
            })
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
            if cells.is_taken(slot) {
                return Err(cell_in_update(span, slot, "cell_get"));
            }
            match cells.get(slot) {
                Some(v) => Ok(Step::Done(v.clone())),
                None => Err(no_such_cell(span, slot)),
            }
        }

        Builtin::CellSet => {
            let slot = args[0].as_cell(span, "`cell_set`")?;
            if cells.is_taken(slot) {
                return Err(cell_in_update(span, slot, "cell_set"));
            }
            // Reported rather than refused: refusing would change what a legal program means, and
            // the reference-counting pass accepts the leak and asks only that it be said out loud.
            crate::rc::cell_cycle(slot, &args[1], span);
            if cells.set(slot, args.remove(1)) {
                Ok(Step::Done(Value::Unit))
            } else {
                Err(no_such_cell(span, slot))
            }
        }

        // The contents leave the arena for the length of the call, so a `push` inside the function
        // sees one owner; the machine puts the answer back at `Frame::CellUpdateStep`.
        Builtin::CellUpdate => {
            let slot = args[0].as_cell(span, "`cell_update`")?;
            if cells.is_taken(slot) {
                return Err(cell_in_update(span, slot, "cell_update"));
            }
            let Some(current) = cells.take(slot) else {
                return Err(no_such_cell(span, slot));
            };
            Ok(Step::Apply {
                callee: args[1].clone(),
                args: crate::argv::of([current]),
                frame: Frame::CellUpdateStep { slot, span },
            })
        }

        // `/` on `Decimal` is `E0209` precisely so that a division names its scale and its rounding
        // mode here instead.
        Builtin::DecimalDiv => {
            let a = args[0].as_decimal(span, "`decimal_div`")?;
            let b = args[1].as_decimal(span, "`decimal_div`")?;
            let scale = decimal_scale(&args[2], span, "decimal_div")?;
            let mode = rounding(&args[3], span, "decimal_div")?;
            if b.is_zero() {
                return Err(crate::semantics::err_zero_divisor(span, "`decimal_div`"));
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

        // Lossy and total, which is the honest pair: every `Decimal` has a nearest `f64`, and
        // saying so beats an `Option` nobody can act on.
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

        // Round-trips `decimal_of_string` exactly, scale included: `1.50m` renders `1.50`, because
        // the trailing zero is what the value carries.
        Builtin::DecimalToString => {
            let d = args[0].as_decimal(span, "`decimal_to_string`")?;
            Ok(Step::Done(Value::str(d.to_string())))
        }

        // Total both ways: every bit pattern is a `Float`, NaNs included, and the pattern is what
        // content addressing hashes a literal by.
        Builtin::BitsOfFloat => {
            let f = args[0].as_float(span, "`bits_of_float`")?;
            Ok(Step::Done(Value::Int(f.to_bits() as i64)))
        }

        Builtin::FloatOfBits => {
            let n = args[0].as_int(span, "`float_of_bits`")?;
            Ok(Step::Done(Value::Float(f64::from_bits(n as u64))))
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

        // Does not consume its argument: Ply is a value language, so the plaintext is still in
        // scope and can still be traced or returned.
        Builtin::SecretOfString => {
            args[0].as_str(span, "`secret_of_string`")?;
            Ok(Step::Done(Value::secret(args[0].clone())))
        }

        // One bit per call, constant time over the compared bytes, and not rate limited — a loop
        // over candidates recovers the value, which is the program's to prevent.
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

        // Presence, never the value.
        Builtin::SecretIsEmpty => {
            let Value::Secret(held) = &args[0] else {
                return Err(type_error(span, "`secret_is_empty`", "Secret", &args[0]));
            };
            Ok(Step::Done(Value::Bool(match &**held {
                Value::Str(s) => s.is_empty(),
                Value::Bytes(b) => b.is_empty(),
                // `secret_of_string` is the only introduction, so nothing else is constructible; a
                // payload that is not a sequence has no emptiness, and answering `false` reports "a
                // credential is present" rather than inventing one.
                _ => false,
            })))
        }
    }
}

/// Where a list index becomes a position.
fn at(xs: &List, i: i64) -> Option<&Value> {
    usize::try_from(i).ok().and_then(|i| xs.get(i))
}

/// `Some(v)` or `None`, the prelude's.
fn option(v: Option<Value>) -> Value {
    match v {
        Some(v) => Value::ctor("Some", vec![v]),
        None => Value::ctor("None", Vec::new()),
    }
}

/// A `Rounding` argument as `rust_decimal`'s strategy.
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

/// A scale argument, refused outside `0..=28` rather than clamped: a scale the caller asked for and
/// did not get is a rounding they did not write down.
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

/// The **shortest decimal that round-trips the float**, and `None` for NaN, an infinity, and
/// anything outside `Decimal`'s range.
fn decimal_of_float(f: f64) -> Option<Decimal> {
    if !f.is_finite() {
        return None;
    }
    parse_decimal(&format!("{f}"))
}

/// The **nearest** `f64` to a decimal, which is what makes `float_of_decimal(decimal_of_float(f))
/// == f` for every `f` that has a `Decimal` at all.
fn float_of_decimal(d: Decimal) -> f64 {
    d.to_string()
        .parse::<f64>()
        .unwrap_or_else(|_| d.to_f64().unwrap_or(f64::NAN))
}

/// The one decimal grammar, for `decimal_of_string` and for the shortest round-tripping form of a
/// `Float` alike.
fn parse_decimal(text: &str) -> Option<Decimal> {
    if text.contains(['e', 'E']) {
        Decimal::from_scientific(text).ok()
    } else {
        Decimal::from_str_exact(text).ok()
    }
}

/// Resumes a higher-order builtin: `answer` is what the user code the frame was waiting on
/// returned.
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

        Frame::IterateStep {
            f,
            budget,
            left,
            span,
        } => match iterate_answer(&answer, span)? {
            Continued(seed) => return next_iterate(f, seed, budget, left, span),
            Stopped(r) => Step::Done(r),
        },

        Frame::MapUpdateStep { map, key, span } => Step::Done(map::insert(map, key, answer, span)?),

        Frame::BytesPositionStep {
            f,
            bytes,
            next,
            span,
        } => {
            if answer.as_bool(span, "the predicate given to `bytes_position`")? {
                // `next` is one past the byte the predicate was asked about, and the answer is that
                // byte's index rather than the byte.
                Step::Done(position(Some(next - 1)))
            } else {
                next_position(f, bytes, next, span)
            }
        }

        _ => return Err(not_a_builtin_step()),
    })
}

fn next_map(f: Value, items: List, next: usize, done: Vec<Value>, span: Span) -> Step {
    let Some(x) = items.get(next).cloned() else {
        return Step::Done(Value::list(done));
    };
    Step::Apply {
        callee: f.clone(),
        args: crate::argv::of([x]),
        frame: Frame::MapStep {
            f,
            items,
            next: next + 1,
            done,
            span,
        },
    }
}

fn next_filter(f: Value, items: List, next: usize, done: Vec<Value>, span: Span) -> Step {
    let Some(x) = items.get(next).cloned() else {
        return Step::Done(Value::list(done));
    };
    Step::Apply {
        callee: f.clone(),
        args: crate::argv::of([x]),
        frame: Frame::FilterStep {
            f,
            items,
            next: next + 1,
            done,
            span,
        },
    }
}

fn next_fold(f: Value, items: List, next: usize, acc: Value, span: Span) -> Step {
    let Some(x) = items.get(next).cloned() else {
        return Step::Done(acc);
    };
    Step::Apply {
        callee: f.clone(),
        args: crate::argv::of([acc, x]),
        frame: Frame::FoldStep {
            f,
            items,
            next: next + 1,
            span,
        },
    }
}

/// `iterate`'s step, which is `next_fold`'s shape with the list replaced by a countdown and the end
/// replaced by an answer the step itself gives.
fn next_iterate(
    f: Value,
    seed: Value,
    budget: i64,
    left: i64,
    span: Span,
) -> Result<Step, Diagnostic> {
    if left <= 0 {
        return Err(crate::limit::err_iterate_budget(span, budget));
    }
    Ok(Step::Apply {
        callee: f.clone(),
        args: crate::argv::of([seed]),
        frame: Frame::IterateStep {
            f,
            budget,
            left: left - 1,
            span,
        },
    })
}

/// What the step answered, with its payload.
enum IterAnswer {
    Continued(Value),
    Stopped(Value),
}
use IterAnswer::{Continued, Stopped};

/// Anything but the prelude's two constructors is a type error rather than a silent stop: inference
/// admits only `Iter<s, r>` here, so reaching this with another shape means a host handler or a
/// `Value` built in Rust answered something the checker never saw.
fn iterate_answer(answer: &Value, span: Span) -> Result<IterAnswer, Diagnostic> {
    if let Value::Ctor { name, args } = answer
        && args.len() == 1
    {
        match name.as_str() {
            "Continue" => return Ok(Continued(args[0].clone())),
            "Stop" => return Ok(Stopped(args[0].clone())),
            _ => {}
        }
    }
    Err(type_error(
        span,
        "the function given to `iterate`",
        "Continue or Stop",
        answer,
    ))
}

fn next_position(f: Value, bytes: std::sync::Arc<[u8]>, next: usize, span: Span) -> Step {
    let Some(byte) = bytes.get(next).copied() else {
        return Step::Done(position(None));
    };
    Step::Apply {
        callee: f.clone(),
        args: crate::argv::of([Value::Int(i64::from(byte))]),
        frame: Frame::BytesPositionStep {
            f,
            bytes,
            next: next + 1,
            span,
        },
    }
}

/// `Option<Int>`, the answer shape of every builtin that searches.
fn position(at: Option<usize>) -> Value {
    match at {
        Some(i) => Value::ctor("Some", vec![Value::Int(i as i64)]),
        None => Value::ctor("None", Vec::new()),
    }
}

/// An empty needle occurs at `from`, which is what `str::find` answers and what makes
/// `bytes_index_of(b, b"")` `Some(0)` rather than a special case every caller has to write.
fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(from);
    }
    memchr::memmem::find(&hay[from..], needle).map(|at| from + at)
}

/// A 256-bit membership test built once per call, in time proportional to the set rather than to
/// the buffer.
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

/// The bytes a bounded scan is allowed to look at: `max` of them at most, and never past the end.
fn scan_window(hay: &[u8], from: usize, max: usize) -> &[u8] {
    &hay[from..hay.len().min(from.saturating_add(max))]
}

/// The typed-argument readers the byte and decimal builtins use, quoting the operation's name only
/// when the argument is the wrong type.
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

/// Both bounded scans.
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

    // `memchr` is SIMD and a bitmap loop is not, so a small set — which is every header delimiter a
    // parser cares about — takes the fast path.
    let found = match (want, members.as_ref()) {
        // An empty class is never entered, so `bytes_scan_until` runs out the window and
        // `bytes_scan` — which stops off the class — stops at once.
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

/// A position a search may start at.
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

/// The bound that stops a 20-megabyte header line from being a denial of service.
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

/// The half-open `[start, end)` of a slicing builtin, refused rather than clamped.
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

/// The byte offset of the `n`-th character boundary.
fn char_offset(s: &str, n: usize) -> usize {
    s.char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(s.len()))
        .nth(n)
        .unwrap_or(s.len())
}

/// The offset is the first byte the decoder could not use, which is the number an author needs to
/// find the truncation — a `Bytes` cut mid-character by `bytes_slice` reports the position of the
/// character it cut.
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

/// The `ASSERTION_FAILED` an agent reads to decide what to fix: both values in full, plus the path
/// to the first place they differ.
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

/// The `ASSERTION_FAILED` for a failing `assert`, whose second argument is the message the author
/// wanted the reader to see.
pub fn assert_failure(message: &Value, span: Span) -> Diagnostic {
    let mut diag = Diagnostic::error(
        codes::ASSERTION_FAILED,
        "assertion failed: condition is false",
    )
    .primary(span, "this condition evaluated to false");
    let carried = match message {
        Value::Ctor { name, args, .. } if name.as_str() == "Some" => args.first(),
        Value::Ctor { .. } => None,
        // A message that is not an `Option` at all can only come from a call this evaluator was
        // handed without a check in front of it.
        other => Some(other),
    };
    if let Some(message) = carried {
        diag = diag.note(match message {
            Value::Str(s) => s.to_string(),
            other => other.render(),
        });
    }
    diag
}

/// A cell whose contents a `cell_update` is holding, reached before the update stored its answer:
/// through an effect the function performed, or a nested update of the same cell.
#[cold]
#[inline(never)]
pub(crate) fn cell_in_update(span: Span, slot: Slot, what: &str) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`{what}` reached cell {slot} while a `cell_update` holds its contents"),
    )
    .primary(span, "the cell is being updated here")
    .note("`cell_update` takes the contents out of the region for the length of its function, so nothing can read or write them until it stores the answer")
    .note("perform the read or write after the update, or outside the function you pass to it")
}

/// A cell whose region has closed.
#[cold]
pub(crate) fn no_such_cell(span: Span, slot: Slot) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("cell {slot} does not belong to the region this code is running in"),
    )
    .primary(span, "this cell was made by a different run")
    .note("please report this: a cell value escaped the region that allocated it")
}

/// Only the higher-order builtins suspend, so only their frames reach here.
#[cold]
fn not_a_builtin_step() -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        "internal error: a frame that is not a builtin step reached `advance`",
    )
    .primary(Span::DUMMY, "please report this")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{
        bin, block, callv, clause, discard, effect_def, handle, int, lam, letv, list, perform,
        standalone, var, with_cell,
    };
    use crate::machine::Machine;
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

    /// `Some(i)` as `i` and `None` as `-1`, which is the shape W1's folds answered in and therefore
    /// the shape a comparison against them needs.
    fn at(v: &Value) -> i64 {
        match v {
            Value::Ctor { args, .. } if !args.is_empty() => {
                args[0].as_int(Span::DUMMY, "test").unwrap()
            }
            _ => -1,
        }
    }

    /// Deterministic and dependency-free, so a failing case is a seed a reader can reproduce rather
    /// than a number that moves between runs.
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

        // Overlapping occurrences: `aa` sits at 0, 1, 4 and 5, and the first is the answer.
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

    /// The index a `_from` search answers is absolute, so it feeds straight back into
    /// `bytes_slice`.
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

    /// Required test 36.
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
        // Non-overlapping, left to right: the second `aa` starts after the first one's last byte.
        assert_eq!(
            split(b"aaaa", b"aa").unwrap().render(),
            "[b\"\", b\"\", b\"\"]"
        );
    }

    /// The limits's builtin.
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

        // The whole point of `bytes_scan` over a fold: the answer for a run that reaches the end is
        // the end, not a sentinel.
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

    /// Every set size takes a different path — `memchr`, `memchr2`, `memchr3`, then the bitmap — so
    /// the four have to agree with each other.
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

    /// Required test 37.
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

    /// Required test 38.
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

    /// The property every builtin frame owes: a suspension point captured inside it can be advanced
    /// more than once, and each resumption is its own search.
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

    /// The fold-based `index_of` from W1's `examples/hello.ply`, verbatim in Rust.
    #[test]
    fn the_scans_agree_with_the_folds_they_replace() {
        fn fold_index_of(hay: &[u8], byte: u8, from: usize) -> i64 {
            // The fold's shape, kept: it visits every remaining byte even after it has the answer,
            // which is the cost the builtins removed.
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

            // The same question through the bounded scan, whose "absent" is the end of the window
            // rather than a sentinel.
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

    /// These are byte builtins and index in bytes, which is the whole reason a request target is
    /// `Bytes`: a peer may send what is not UTF-8 at all.
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

        // A byte search may stop in the middle of a character, and the piece it cuts is refused by
        // `string_of_bytes` rather than silently replaced.
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

        // A multi-byte needle is matched whole, so a search never reports a position that splits
        // one.
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

    /// The property the frames exist for: a suspension point inside `map` can be advanced twice and
    /// each resumption completes its own list.
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

    /// `iterate` through the protocol an engine drives, with the step answered by hand: the seed is
    /// threaded, `Stop` ends it, and the value `Stop` carries is the answer rather than the seed.
    #[test]
    fn an_iterate_threads_its_seed_and_answers_what_stop_carries() {
        let stop_at = |n: i64| {
            move |args: &[Value]| {
                let Value::Int(i) = args[0] else {
                    panic!("the seed is an Int here")
                };
                if i >= n {
                    Value::ctor("Stop", vec![Value::str(format!("done at {i}"))])
                } else {
                    Value::ctor("Continue", vec![Value::Int(i + 1)])
                }
            }
        };
        let out = drive(
            Builtin::Iterate,
            vec![Value::Int(0), Value::Int(100), f()],
            stop_at(7),
        )
        .unwrap();
        assert_eq!(out.render(), "\"done at 7\"");

        // Stopping on the very first step costs one round, not none: the step has to run to say so.
        let out = drive(
            Builtin::Iterate,
            vec![Value::Int(9), Value::Int(1), f()],
            stop_at(0),
        )
        .unwrap();
        assert_eq!(out.render(), "\"done at 9\"");
    }

    /// The reason the loop is a `Frame` and not host recursion: a continuation captured inside the
    /// step can be resumed more than once, and each resumption has to continue **its own** copy of
    /// the countdown.
    #[test]
    fn one_suspension_point_inside_iterate_can_be_resumed_twice() {
        let mut cells = TaskRegions::new();
        let start = call(
            Builtin::Iterate,
            vec![Value::Int(0), Value::Int(4), f()],
            cells.arena_mut(),
            Span::DUMMY,
        )
        .unwrap();
        let Step::Apply { frame, .. } = start else {
            panic!("iterate suspends on its first round");
        };

        // Each leg runs the loop out from the same captured point.
        let run = |mut step: Step, stop_after: i64| {
            let mut seen = 0;
            loop {
                match step {
                    Step::Done(v) => return Ok(v),
                    Step::Apply { frame, .. } => {
                        seen += 1;
                        let answer = if seen >= stop_after {
                            Value::ctor("Stop", vec![Value::Int(seen)])
                        } else {
                            Value::ctor("Continue", vec![Value::Int(seen)])
                        };
                        match advance(frame, answer) {
                            Ok(next) => step = next,
                            Err(d) => return Err(d),
                        }
                    }
                }
            }
        };

        let a = run(
            advance(frame.clone(), Value::ctor("Continue", vec![Value::Int(0)])).unwrap(),
            3,
        )
        .unwrap();
        let b = run(
            advance(frame, Value::ctor("Continue", vec![Value::Int(0)])).unwrap(),
            3,
        )
        .unwrap();
        assert_eq!(a.render(), "3");
        assert_eq!(
            b.render(),
            "3",
            "the second resumption inherited a spent budget"
        );

        // And the budget above is exactly tight, which is what makes the pair above non-vacuous:
        // one leg spends all four rounds, so two legs sharing a countdown could not both finish.
        let mut cells = TaskRegions::new();
        let tight = call(
            Builtin::Iterate,
            vec![Value::Int(0), Value::Int(3), f()],
            cells.arena_mut(),
            Span::DUMMY,
        )
        .unwrap();
        let Step::Apply { frame, .. } = tight else {
            panic!("iterate suspends on its first round");
        };
        let d = run(
            advance(frame, Value::ctor("Continue", vec![Value::Int(0)])).unwrap(),
            3,
        )
        .unwrap_err();
        assert!(d.message.contains("budget of 3 steps"), "{}", d.message);
    }

    /// The budget is spent per round and exhausting it is a diagnostic, because `Stop` is the only
    /// source of an answer and there is none to give.
    #[test]
    fn an_iterate_that_never_stops_exhausts_its_budget_and_says_so() {
        let d = drive(
            Builtin::Iterate,
            vec![Value::Int(0), Value::Int(12), f()],
            |args| {
                let Value::Int(i) = args[0] else {
                    panic!("the seed is an Int here")
                };
                Value::ctor("Continue", vec![Value::Int(i + 1)])
            },
        )
        .unwrap_err();
        assert_eq!(d.code, codes::RUNTIME_ERROR);
        assert!(d.message.contains("budget of 12 steps"), "{}", d.message);
        // Nothing nested, so nothing here may say it did.
        assert!(!d.message.contains("recursion limit"), "{}", d.message);
    }

    #[test]
    fn an_iterate_budget_below_one_is_refused_before_the_loop_starts() {
        for budget in [0, -1] {
            let d = drive(
                Builtin::Iterate,
                vec![Value::Int(0), Value::Int(budget), f()],
                |_| panic!("the step must not run at all"),
            )
            .unwrap_err();
            assert_eq!(d.code, codes::RUNTIME_ERROR);
            assert!(
                d.message.contains(&format!("budget of {budget}")),
                "{}",
                d.message
            );
        }
    }

    /// Inference admits only `Iter<s, r>` in this position, so anything else arriving here came
    /// from a host handler or a `Value` built in Rust — and treating it as a silent stop would
    /// answer a value nobody asked for.
    #[test]
    fn an_iterate_step_answering_neither_continue_nor_stop_is_a_runtime_error() {
        let d = drive(
            Builtin::Iterate,
            vec![Value::Int(0), Value::Int(5), f()],
            |_| Value::ctor("Halt", vec![Value::Int(1)]),
        )
        .unwrap_err();
        assert_eq!(d.code, codes::RUNTIME_ERROR);
        assert!(d.message.contains("Continue or Stop"), "{}", d.message);
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
            callee_window: 0,
            caller_window: 0,
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

    /// The generation is what makes this a report rather than a read of the cell now living at that
    /// position: the stale slot and the live one share an index and differ in generation.
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
            [
                "bytes_position",
                "cell_update",
                "filter",
                "fold",
                "iterate",
                "map",
                "map_fold",
                "map_update"
            ]
        );
    }

    #[test]
    fn every_builtin_is_reachable_by_the_name_it_reports() {
        for b in Builtin::all() {
            assert_eq!(Builtin::from_name(b.name()), Some(*b));
        }
    }

    /// **The test this repository went its whole history without.**
    #[test]
    fn every_builtin_agrees_on_its_arity_everywhere() {
        for b in Builtin::all() {
            let (min, max) = b.arity();
            assert_eq!(
                min,
                max,
                "`{}` has a variable arity; every builtin is exactly applied, and a call \
                 that leaves an argument out is filled by `ply_syntax::defaults` before \
                 anything here sees it",
                b.name()
            );

            // A builtin the prelude does not type cannot be called at all, so the two tables have
            // to cover the same set.
            let typed = ply_core::prelude_arity(b.name()).unwrap_or_else(|| {
                panic!(
                    "`{}` is a builtin with no scheme in the prelude: no program can call it",
                    b.name()
                )
            });
            assert_eq!(
                typed,
                max,
                "`{}` takes {max} arguments here and {typed} in the prelude's scheme. \
                 Whichever is larger, the extra arm is unreachable from source.",
                b.name()
            );

            if let Some((params, defaults)) = ply_syntax::defaults::builtin_shape(b.name()) {
                assert_eq!(
                    params,
                    max,
                    "`{}`'s defaults table describes {params} parameters, its arity {max}",
                    b.name()
                );
                assert!(
                    defaults > 0,
                    "`{}` is in the defaults table with no default in it",
                    b.name()
                );
            }
        }
    }

    /// What [`Builtin::all`] lists, pinned — because until this was written, **nothing checked that
    /// it was complete**.
    #[test]
    fn builtin_all_is_complete_and_lists_each_name_once() {
        let mut names: Vec<&str> = Builtin::all().iter().map(|b| b.name()).collect();
        names.sort_unstable();
        let mut unique = names.clone();
        unique.dedup();
        assert_eq!(names, unique, "`Builtin::all()` lists a builtin twice");
        assert_eq!(
            names,
            [
                "assert",
                "assert_eq",
                "bits_of_float",
                "byte_of_int",
                "bytes_at",
                "bytes_concat",
                "bytes_concat_all",
                "bytes_ends_with",
                "bytes_index_of",
                "bytes_index_of_byte",
                "bytes_index_of_from",
                "bytes_is_utf8",
                "bytes_len",
                "bytes_of_string",
                "bytes_position",
                "bytes_scan",
                "bytes_scan_until",
                "bytes_slice",
                "bytes_split",
                "bytes_starts_with",
                "bytes_u32_le",
                "cell_get",
                "cell_set",
                "cell_update",
                "compare",
                "compare_values",
                "decimal_div",
                "decimal_of_float",
                "decimal_of_int",
                "decimal_of_string",
                "decimal_round",
                "decimal_to_string",
                "filter",
                "float_of_bits",
                "float_of_decimal",
                "fold",
                "i16_of_int",
                "i32_of_int",
                "i64_of_int",
                "i8_of_int",
                "int_of_decimal",
                "int_of_i16",
                "int_of_i32",
                "int_of_i64",
                "int_of_i8",
                "int_of_u16",
                "int_of_u32",
                "int_of_u64",
                "int_of_u8",
                "int_to_string",
                "iterate",
                "len",
                "list_at",
                "map",
                "map_contains",
                "map_entries",
                "map_fold",
                "map_get",
                "map_insert",
                "map_keys",
                "map_len",
                "map_merge",
                "map_new",
                "map_of_entries",
                "map_remove",
                "map_update",
                "map_values",
                "max",
                "min",
                "panic",
                "push",
                "range",
                "rotr",
                "rotr32",
                "secret_is_empty",
                "secret_of_string",
                "secret_verify",
                "string_concat",
                "string_contains",
                "string_ends_with",
                "string_find",
                "string_len",
                "string_lower",
                "string_of_bytes",
                "string_of_bytes_lossy",
                "string_slice",
                "string_split",
                "string_starts_with",
                "string_trim",
                "string_upper",
                "u16_of_int",
                "u32_of_int",
                "u64_of_int",
                "u8_of_int",
                "wrap_add",
                "wrap_mul",
                "wrap_sub",
            ],
            "a builtin was added to or removed from the enum without `Builtin::all()` being \
             updated — every table driven by `all()` silently skips it until this list agrees"
        );
    }

    /// The low word rotated: bits leaving the right come back on the left of a thirty-two-bit
    /// word, whatever the `Int` above that word held and whatever the count's sign.
    #[test]
    fn rotr32_turns_the_low_word_and_answers_it_non_negative() {
        let cases: &[(i64, i64, i64)] = &[
            (1, 1, 0x8000_0000),
            (0x8000_0000, 31, 1),
            (0x1234_5678, 0, 0x1234_5678),
            (0x1234_5678, 32, 0x1234_5678),
            (0x1234_5678, 4, 0x8123_4567),
            (0x1_0000_0001, 1, 0x8000_0000),
            (-1, 7, 0xFFFF_FFFF),
            (2, -1, 4),
        ];
        for &(x, n, want) in cases {
            assert_eq!(
                done(Builtin::Rotr32, vec![Value::Int(x), Value::Int(n)]).unwrap(),
                Value::Int(want),
                "rotr32({x}, {n})"
            );
        }
    }

    /// The three that answer where `+`, `-` and `*` raise, at the boundaries
    /// that are the only reason they exist. A value below 2^32 needs none of
    /// them — the shift semantics says so — so every case here is at or across the
    /// 64-bit edge.
    #[test]
    fn the_wrapping_builtins_are_modulo_two_to_the_sixty_fourth() {
        let cases: &[(Builtin, i64, i64, i64)] = &[
            (Builtin::WrapAdd, i64::MAX, 1, i64::MIN),
            (Builtin::WrapAdd, i64::MIN, -1, i64::MAX),
            (Builtin::WrapAdd, i64::MAX, i64::MAX, -2),
            (Builtin::WrapAdd, 2, 3, 5),
            (Builtin::WrapSub, i64::MIN, 1, i64::MAX),
            (Builtin::WrapSub, i64::MAX, -1, i64::MIN),
            (Builtin::WrapSub, 0, i64::MIN, i64::MIN),
            (Builtin::WrapMul, i64::MAX, 2, -2),
            (Builtin::WrapMul, i64::MIN, -1, i64::MIN),
            (Builtin::WrapMul, 1 << 32, 1 << 32, 0),
            (Builtin::WrapMul, 6, 7, 42),
        ];
        for &(b, x, y, want) in cases {
            assert_eq!(
                done(b, vec![Value::Int(x), Value::Int(y)]).unwrap(),
                Value::Int(want),
                "`{}`({x}, {y})",
                b.name()
            );
        }
    }

    /// None of the three can fail, so the only diagnostic any of them produces
    /// is about an argument that is not an `Int` — which the checker refused
    /// before it got here, leaving this as the shape an unchecked body meets.
    #[test]
    fn a_wrapping_builtin_refuses_a_non_int_and_nothing_else() {
        let d = done(Builtin::WrapAdd, vec![Value::Int(1), Value::str("2")])
            .expect_err("a `String` is not an `Int`");
        assert_eq!(d.code, codes::RUNTIME_ERROR);
        assert!(d.message.contains("wrap_add"), "{}", d.message);
    }

    fn run(items: Vec<Item>, e: Expr) -> Result<Value, Diagnostic> {
        let (program, resolved) = standalone(items);
        Machine::for_program(&program, &resolved).eval_expr_for_test(&e)
    }

    fn state() -> Item {
        effect_def("state", &[("get", Mode::Read, false)])
    }

    /// The suspension points are where a builtin is most likely to be handed a stale arena, so the
    /// handler both writes a cell and decides the answer from it: a builtin that carried its own
    /// copy would keep the count at 1 and keep the wrong elements.
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

    /// The handler is inside the callback, so it is installed and torn down once per element rather
    /// than once for the whole `map`.
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

    #[test]
    fn map_update_applies_the_function_to_a_present_key_and_leaves_an_absent_one_alone() {
        let m = done(
            Builtin::MapInsert,
            vec![map::new(), Value::str("k"), Value::Int(1)],
        )
        .unwrap();
        let out = drive(
            Builtin::MapUpdate,
            vec![m.clone(), Value::str("k"), f()],
            |args| Value::Int(args[0].as_int(Span::DUMMY, "test").unwrap() + 41),
        )
        .unwrap();
        assert_eq!(out.render(), "{\"k\": 42}");
        let untouched = drive(Builtin::MapUpdate, vec![m, Value::str("z"), f()], |_| {
            panic!("`map_update` called its function on an absent key")
        })
        .unwrap();
        assert_eq!(untouched.render(), "{\"k\": 1}");
    }
}
