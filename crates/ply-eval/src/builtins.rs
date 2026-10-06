//! The prelude, in one definition per builtin.

use crate::arena::Slot;
use crate::semantics::arity_error;
use crate::value::{
    Decimal, Fixed, FixedOp, List, Value, first_difference, type_error, values_equal,
};
use crate::{BinOp, Diagnostic, INT_TYPES, IntTy, PathStep, Plain, Span, codes, map, slot};
use rust_decimal::RoundingStrategy;
use rust_decimal::prelude::ToPrimitive;

/// A list this long is a runaway `range`, not an intent.
const MAX_RANGE_LEN: i64 = 10_000_000;

/// An array this long is a runaway length, not an intent: half a gigabyte of words.
pub const MAX_ARRAY_LEN: i64 = 1 << 26;

/// `Decimal`'s scale bound, the type's rather than a policy.
const MAX_DECIMAL_SCALE: u32 = 28;
/// Every builtin, once: its name in the prelude, the words a call passes, and whether a failure of
/// it is a raise of `abort.raise`, which the prelude's scheme for it carries in its row and a
/// `handle` answers (`raises`), or a fault that ends the run (`ends`).
macro_rules! builtins {
    ($d:tt $( $(#[$doc:meta])* $variant:ident = $name:literal, $arity:tt, $failure:ident; )*) => {
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub enum Builtin {
            $( $(#[$doc])* $variant, )*
        }

        impl Builtin {
            pub fn raises(self) -> bool {
                match self {
                    $( Builtin::$variant => builtins!(@raises $failure), )*
                }
            }

            pub fn from_name(name: &str) -> Option<Builtin> {
                match name {
                    $( $name => Some(Builtin::$variant), )*
                    _ => None,
                }
            }

            pub fn name(self) -> &'static str {
                match self {
                    $( Builtin::$variant => $name, )*
                }
            }

            /// The words a call passes.
            pub fn arity(self) -> usize {
                match self {
                    $( Builtin::$variant => $arity, )*
                }
            }

            pub fn all() -> &'static [Builtin] {
                &[ $( Builtin::$variant, )* ]
            }
        }

        /// Hands `$m` every builtin as `Variant "name" arity;`: what is made one per builtin is
        /// made from this table.
        #[macro_export]
        macro_rules! each_builtin {
            ($d m:ident) => {
                $d m! { $( $variant $name $arity; )* }
            };
        }
    };
    (@raises raises) => { true };
    (@raises ends) => { false };
}

builtins! { $
    Assert = "assert", 2, raises;
    AssertEq = "assert_eq", 2, raises;
    Len = "len", 1, ends;
    Push = "push", 2, ends;
    ListAt = "list_at", 2, ends;
    /// One element replaced, sharing the rest; raises where `list_at` answers `None`.
    ListSet = "list_set", 3, raises;
    Map = "map", 2, ends;
    Filter = "filter", 2, ends;
    Fold = "fold", 3, ends;
    Iterate = "iterate", 3, raises;
    Metered = "metered", 1, ends;
    Range = "range", 2, ends;
    WrapAdd = "wrap_add", 2, ends;
    WrapSub = "wrap_sub", 2, ends;
    WrapMul = "wrap_mul", 2, ends;
    /// The low thirty-two bits of an `Int`, rotated right.
    Rotr32 = "rotr32", 2, ends;
    Rotr = "rotr", 2, ends;
    Rotl = "rotl", 2, ends;
    /// Over any integer type: the exact answer, or the type's nearest value where it leaves it.
    SaturatingAdd = "saturating_add", 2, ends;
    SaturatingSub = "saturating_sub", 2, ends;
    SaturatingMul = "saturating_mul", 2, ends;
    // Field-less rather than carrying an `IntTy`: the enum is cast to a per-builtin cache index.
    U8OfInt = "u8_of_int", 1, raises;
    U16OfInt = "u16_of_int", 1, raises;
    U32OfInt = "u32_of_int", 1, raises;
    U64OfInt = "u64_of_int", 1, raises;
    I8OfInt = "i8_of_int", 1, raises;
    I16OfInt = "i16_of_int", 1, raises;
    I32OfInt = "i32_of_int", 1, raises;
    I64OfInt = "i64_of_int", 1, raises;
    IntOfU8 = "int_of_u8", 1, ends;
    IntOfU16 = "int_of_u16", 1, ends;
    IntOfU32 = "int_of_u32", 1, ends;
    IntOfU64 = "int_of_u64", 1, raises;
    IntOfI8 = "int_of_i8", 1, ends;
    IntOfI16 = "int_of_i16", 1, ends;
    IntOfI32 = "int_of_i32", 1, ends;
    IntOfI64 = "int_of_i64", 1, ends;
    Min = "min", 2, ends;
    Max = "max", 2, ends;
    ByteOfInt = "byte_of_int", 1, raises;
    IntToString = "int_to_string", 1, ends;
    FloatToString = "float_to_string", 1, ends;
    StringConcat = "string_concat", 2, ends;
    BytesLen = "bytes_len", 1, ends;
    BytesAt = "bytes_at", 2, raises;
    BytesU32Le = "bytes_u32_le", 2, raises;
    BytesSlice = "bytes_slice", 3, raises;
    BytesConcat = "bytes_concat", 2, ends;
    BytesConcatAll = "bytes_concat_all", 1, ends;
    BytesBlake3 = "bytes_blake3", 1, ends;
    BytesGzip = "bytes_gzip", 1, ends;
    BytesGunzip = "bytes_gunzip", 1, ends;
    BytesOfString = "bytes_of_string", 1, ends;
    BytesIsUtf8 = "bytes_is_utf8", 1, ends;
    BytesIndexOf = "bytes_index_of", 2, ends;
    BytesIndexOfFrom = "bytes_index_of_from", 3, raises;
    BytesIndexOfByte = "bytes_index_of_byte", 2, raises;
    BytesStartsWith = "bytes_starts_with", 2, ends;
    BytesEndsWith = "bytes_ends_with", 2, ends;
    BytesSplit = "bytes_split", 2, raises;
    BytesScan = "bytes_scan", 4, raises;
    BytesScanUntil = "bytes_scan_until", 4, raises;
    BytesPosition = "bytes_position", 3, raises;
    StringOfBytes = "string_of_bytes", 1, raises;
    StringOfBytesLossy = "string_of_bytes_lossy", 1, ends;
    StringLen = "string_len", 1, ends;
    StringSlice = "string_slice", 3, raises;
    StringSplit = "string_split", 2, raises;
    StringTrim = "string_trim", 1, ends;
    StringLower = "string_lower", 1, ends;
    StringUpper = "string_upper", 1, ends;
    StringStartsWith = "string_starts_with", 2, ends;
    StringEndsWith = "string_ends_with", 2, ends;
    StringContains = "string_contains", 2, ends;
    StringFind = "string_find", 2, raises;
    MapNew = "map_new", 0, ends;
    MapInsert = "map_insert", 3, ends;
    MapGet = "map_get", 2, ends;
    MapContains = "map_contains", 2, ends;
    MapRemove = "map_remove", 2, ends;
    MapLen = "map_len", 1, ends;
    MapKeys = "map_keys", 1, ends;
    MapValues = "map_values", 1, ends;
    MapEntries = "map_entries", 1, ends;
    MapOfEntries = "map_of_entries", 1, ends;
    MapMerge = "map_merge", 2, ends;
    MapFold = "map_fold", 3, ends;
    /// `CellUpdate`'s shape over one map entry.
    MapUpdate = "map_update", 3, ends;
    DecimalDiv = "decimal_div", 4, raises;
    DecimalRound = "decimal_round", 3, raises;
    DecimalOfInt = "decimal_of_int", 1, ends;
    IntOfDecimal = "int_of_decimal", 2, ends;
    FloatOfDecimal = "float_of_decimal", 1, ends;
    DecimalOfFloat = "decimal_of_float", 1, ends;
    DecimalOfString = "decimal_of_string", 1, ends;
    /// The lexer's float parse over text, reaching `Float`s no route through `Decimal` does.
    FloatOfString = "float_of_string", 1, ends;
    DecimalToString = "decimal_to_string", 1, ends;
    /// The IEEE 754 bit pattern, as the signed 64-bit `Int` it fits in.
    BitsOfFloat = "bits_of_float", 1, ends;
    FloatOfBits = "float_of_bits", 1, ends;
    Compare = "compare", 2, ends;
    /// The same order as [`Builtin::Compare`], under a name a module may not declare.
    CompareValues = "compare_values", 2, ends;
    CellGet = "cell_get", 1, ends;
    CellSet = "cell_set", 2, ends;
    /// Takes the contents out for the call, so an append inside the function owns them.
    CellUpdate = "cell_update", 2, ends;
    Panic = "panic", 1, raises;
    /// The identity a benchmark pins a measured value with: opaque, so it is not optimized away.
    Observe = "observe", 1, ends;
    /// The only introduction of a [`Value::Secret`].
    SecretOfString = "secret_of_string", 1, ends;
    SecretVerify = "secret_verify", 2, ends;
    SecretIsEmpty = "secret_is_empty", 1, ends;
    // Appended, so every earlier builtin keeps its cache index.
    U128OfInt = "u128_of_int", 1, raises;
    I128OfInt = "i128_of_int", 1, raises;
    IntOfU128 = "int_of_u128", 1, raises;
    IntOfI128 = "int_of_i128", 1, raises;
    U128ToString = "u128_to_string", 1, ends;
    I128ToString = "i128_to_string", 1, ends;
    U128OfString = "u128_of_string", 1, ends;
    I128OfString = "i128_of_string", 1, ends;
    /// Over any integer type: the exact answer, or `None` where it leaves the type.
    CheckedAdd = "checked_add", 2, ends;
    CheckedSub = "checked_sub", 2, ends;
    CheckedMul = "checked_mul", 2, ends;
    CheckedNeg = "checked_neg", 1, ends;
    /// `None` for a surrogate or past `U+10FFFF`, which no `Char` is.
    CharOfInt = "char_of_int", 1, ends;
    IntOfChar = "int_of_char", 1, ends;
    StringChars = "string_chars", 1, ends;
    StringOfChars = "string_of_chars", 1, ends;
    ArrayNew = "array_new", 2, raises;
    ArrayOfList = "array_of_list", 1, ends;
    ArrayToList = "array_to_list", 1, ends;
    ArrayLen = "array_len", 1, ends;
    ArrayAt = "array_at", 2, ends;
    /// Raises where `array_at` answers `None`.
    ArrayGet = "array_get", 2, raises;
    /// In place while the array has one holder; raises out of range.
    ArraySet = "array_set", 3, raises;
    /// The value as a `std.value.Value`, whole.
    Reflect = "reflect", 1, ends;
    /// As `reflect`, with a value whose type states a `show` as `VShown` of what that function
    /// writes: what `std.show.show` renders.
    Shown = "shown", 1, ends;
    /// BLAKE3 of the value's canonical encoding, under `derivable(hash, a)`.
    Digest = "digest", 1, ends;
    /// `bracket(acquire, release, body)`: `release` runs on every exit from `body` but a failure.
    Bracket = "bracket", 3, ends;
    /// An operator over a `numeric` or `integer` type parameter: `?numeric_binary(op, witness, x,
    /// y)`, its operands read as the type the witness names. No source can spell it; the
    /// elaboration writes it.
    NumericBinary = "?numeric_binary", 4, ends;
    NumericUnary = "?numeric_unary", 3, ends;
    /// `numeric_of_int(n)`, called with the witness the elaboration passes first.
    NumericOfInt = "numeric_of_int", 2, raises;
    /// `to_int(x)`, called with the witness the elaboration passes first.
    ToInt = "to_int", 2, ends;
    FloatOfInt = "float_of_int", 1, ends;
    /// `None` for a NaN, an infinity, or a value past `Int` once rounded.
    IntOfFloat = "int_of_float", 2, ends;
    Floor = "floor", 1, ends;
    Ceil = "ceil", 1, ends;
    Round = "round", 2, ends;
    Sqrt = "sqrt", 1, ends;
    Pow = "pow", 2, ends;
    Exp = "exp", 1, ends;
    Ln = "ln", 1, ends;
    Log2 = "log2", 1, ends;
    Log10 = "log10", 1, ends;
    Sin = "sin", 1, ends;
    Cos = "cos", 1, ends;
    Tan = "tan", 1, ends;
    Asin = "asin", 1, ends;
    Acos = "acos", 1, ends;
    Atan = "atan", 1, ends;
    Atan2 = "atan2", 2, ends;
    Hypot = "hypot", 2, ends;
    /// The entry at an end of a map's key order, as `Some({key, value})`.
    MapFirst = "map_first", 1, ends;
    MapLast = "map_last", 1, ends;
    /// The entry nearest a key from one side, the key's own counted or not.
    MapFloor = "map_floor", 2, ends;
    MapCeiling = "map_ceiling", 2, ends;
    MapBelow = "map_below", 2, ends;
    MapAbove = "map_above", 2, ends;
    /// An end's entry with the map that is left, in place while the map has one holder.
    MapPopFirst = "map_pop_first", 1, ends;
    MapPopLast = "map_pop_last", 1, ends;
    /// `map_range(m, lo, lo_inclusive, hi, hi_inclusive, limit)`.
    MapRange = "map_range", 6, ends;
    MapSplit = "map_split", 2, ends;
}

impl Builtin {
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
            IntTy::U128 => Builtin::U128OfInt,
            IntTy::I128 => Builtin::I128OfInt,
        }
    }

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
            IntTy::U128 => Builtin::IntOfU128,
            IntTy::I128 => Builtin::IntOfI128,
        }
    }

    pub fn converts_into(self) -> Option<IntTy> {
        INT_TYPES.into_iter().find(|t| Builtin::of_int(*t) == self)
    }

    pub fn converts_from(self) -> Option<IntTy> {
        INT_TYPES.into_iter().find(|t| Builtin::int_of(*t) == self)
    }
}

fn push(args: &mut Vec<Value>, span: Span) -> Result<Value, Diagnostic> {
    let x = args.pop().expect("arity checked");
    let mut xs = args.pop().expect("arity checked");
    let Value::List(list) = &mut xs else {
        return Err(type_error(span, "`push`", "List", &xs));
    };
    let copied = list.push(x);
    crate::rc::note_update_of(copied.is_none(), copied.unwrap_or(0), span);
    Ok(xs)
}

/// `list_set`, taking the list out of its arguments so the last holder writes in place.
fn list_set(args: &mut Vec<Value>, span: Span) -> Result<Value, Diagnostic> {
    let v = args.pop().expect("arity checked");
    let index = args.pop().expect("arity checked");
    let mut xs = args.pop().expect("arity checked");
    let Value::List(list) = &mut xs else {
        return Err(type_error(span, "`list_set`", "List", &xs));
    };
    let i = index.as_int(span, "`list_set`")?;
    let Some(at) = usize::try_from(i).ok().filter(|at| *at < list.len()) else {
        return Err(out_of_range(span, "list_set", i, list.len(), "elements"));
    };
    let copied = list.set(at, v);
    crate::rc::note_update_of(copied.is_none(), copied.unwrap_or(0), span);
    Ok(xs)
}

/// A value of a `numeric` or `integer` parameter, read as the type its witness names: one of the
/// runtime's ten widths, else `Int`, `Float` or `Decimal`, which every word already says it is.
fn witnessed(v: Value, w: i64, span: Span) -> Result<Value, Diagnostic> {
    match (usize::try_from(w).ok().and_then(|i| INT_TYPES.get(i)), &v) {
        (Some(t), Value::Int(n)) => Fixed::of(*t, i128::from(*n))
            .map(Value::Fixed)
            .ok_or_else(|| type_error(span, "a witnessed operand", t.name(), &v)),
        _ => Ok(v),
    }
}

/// The witness of `Int`, which is also a parameter's when nothing shows its type: the prover draws
/// every type variable as `Int`, and so does an entry whose arguments hold no value of it.
pub const INT_WITNESS: i64 = 10;
pub const FLOAT_WITNESS: i64 = 11;
pub const DECIMAL_WITNESS: i64 = 12;

/// The witness a value of a `numeric` or `integer` parameter shows: its width's place among
/// [`INT_TYPES`], else `Int`'s, `Float`'s or `Decimal`'s, which follow them.
pub fn witness_of(v: &Value) -> i64 {
    match v {
        Value::Fixed(f) => INT_TYPES
            .iter()
            .position(|t| *t == f.ty)
            .map_or(INT_WITNESS, |i| i as i64),
        Value::Float(_) => FLOAT_WITNESS,
        Value::Decimal(_) => DECIMAL_WITNESS,
        Value::Ctor { name, .. } => crate::instances::witness(name).unwrap_or(INT_WITNESS),
        _ => INT_WITNESS,
    }
}

/// The operators a witnessed parameter takes, numbered as the elaboration writes them.
pub const NUMERIC_OPS: [BinOp; 15] = [
    BinOp::Add,
    BinOp::Sub,
    BinOp::Mul,
    BinOp::Div,
    BinOp::Rem,
    BinOp::Lt,
    BinOp::Le,
    BinOp::Gt,
    BinOp::Ge,
    BinOp::BitAnd,
    BinOp::BitOr,
    BinOp::BitXor,
    BinOp::Shl,
    BinOp::Shr,
    BinOp::Ushr,
];

fn numeric_op(op: &Value, span: Span) -> Result<BinOp, Diagnostic> {
    let n = op.as_int(span, "an operator")?;
    usize::try_from(n)
        .ok()
        .and_then(|i| NUMERIC_OPS.get(i).copied())
        .ok_or_else(|| {
            Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!("operator {n} is no operator over a numeric parameter"),
            )
            .primary(span, "this is Ply's fault")
        })
}

/// `array_set`, taking the array out of its arguments so the last holder writes in place.
fn array_set(args: &mut Vec<Value>, span: Span) -> Result<Value, Diagnostic> {
    let v = args.pop().expect("arity checked");
    let index = args.pop().expect("arity checked");
    let mut xs = args.pop().expect("arity checked");
    let Value::Array(items) = &mut xs else {
        return Err(type_error(span, "`array_set`", "Array", &xs));
    };
    let i = index.as_int(span, "`array_set`")?;
    let Some(at) = usize::try_from(i).ok().filter(|at| *at < items.len()) else {
        return Err(out_of_range(span, "array_set", i, items.len(), "elements"));
    };
    let in_place = std::sync::Arc::get_mut(items).is_some();
    crate::rc::note_update_of(in_place, if in_place { 0 } else { items.len() }, span);
    std::sync::Arc::make_mut(items)[at] = v;
    Ok(xs)
}

/// A builtin over values. One that calls back into the program, or reads a cell, is the compiled
/// backend's to answer, since only it can enter a closure or reach a cell.
pub fn call(b: Builtin, mut args: Vec<Value>, span: Span) -> Result<Value, Diagnostic> {
    let out = call_with(b, &mut args, span);
    args.clear();
    crate::argv::give(args);
    out
}

fn call_with(b: Builtin, args: &mut Vec<Value>, span: Span) -> Result<Value, Diagnostic> {
    if args.len() != b.arity() {
        return Err(arity_error(
            span,
            &format!("`{}`", b.name()),
            b.arity(),
            args.len(),
        ));
    }

    match b {
        Builtin::Assert => {
            if args[0].as_bool(span, "`assert`")? {
                return Ok(Value::Unit);
            }
            Err(assert_failure(&args[1], span))
        }

        Builtin::AssertEq => {
            if values_equal(&args[0], &args[1], span)? {
                Ok(Value::Unit)
            } else {
                Err(assertion_failure(&args[0], &args[1], span))
            }
        }

        Builtin::Len => match &args[0] {
            Value::List(xs) => Ok(Value::Int(xs.len() as i64)),
            Value::Str(s) => Ok(Value::Int(s.chars().count() as i64)),
            other => Err(type_error(span, "`len`", "a List or String", other)),
        },

        Builtin::Push => push(args, span),

        Builtin::ListAt => {
            let xs = args[0].as_list(span, "`list_at`")?;
            let i = args[1].as_int(span, "`list_at`")?;
            Ok(option(at(xs, i).cloned()))
        }

        Builtin::ListSet => list_set(args, span),

        Builtin::Map
        | Builtin::Filter
        | Builtin::Fold
        | Builtin::Iterate
        | Builtin::Metered
        | Builtin::BytesPosition
        | Builtin::MapUpdate
        | Builtin::MapFold
        | Builtin::CellGet
        | Builtin::CellSet
        | Builtin::CellUpdate
        | Builtin::Bracket => Err(answered_by_the_backend(b, span)),

        Builtin::Range => {
            let lo = args[0].as_int(span, "`range`")?;
            let hi = args[1].as_int(span, "`range`")?;
            if hi <= lo {
                return Ok(Value::list(Vec::new()));
            }
            let len = hi.saturating_sub(lo);
            if len > MAX_RANGE_LEN {
                return Err(Diagnostic::error(
                    codes::RUNTIME_ERROR,
                    format!("`range` of {len} elements exceeds the limit of {MAX_RANGE_LEN}"),
                )
                .primary(span, "this range is too large to materialize"));
            }
            Ok(Value::list((lo..hi).map(Value::Int).collect()))
        }

        // In `compare`'s order, the first of two equal ones.
        Builtin::Min | Builtin::Max => {
            crate::value::secret_has_no_order(&args[0], b.name(), span)?;
            let first = match args[0].cmp(&args[1]) {
                std::cmp::Ordering::Less => b == Builtin::Min,
                std::cmp::Ordering::Greater => b == Builtin::Max,
                std::cmp::Ordering::Equal => true,
            };
            Ok(args.swap_remove(if first { 0 } else { 1 }))
        }

        Builtin::Rotr32 => {
            let x = args[0].as_int(span, "`rotr32`")?;
            let n = args[1].as_int(span, "`rotr32`")?;
            Ok(Value::Int(i64::from(
                (x as u32).rotate_right((n & 31) as u32),
            )))
        }

        // A rotation left is one right by the rest of the width.
        Builtin::Rotr | Builtin::Rotl => {
            let what = format!("`{}`", b.name());
            let n = args[1].as_int(span, &what)?;
            let right = |turn: i64, w: u32| {
                let k = turn.rem_euclid(i64::from(w)) as u32;
                if b == Builtin::Rotl { (w - k) % w } else { k }
            };
            if let Value::Fixed(f) = &args[0] {
                let w = f.ty.bits();
                let k = right(n, w);
                let raw = f.raw();
                let bits = if k == 0 {
                    raw
                } else {
                    (raw >> k) | (raw << (w - k))
                };
                return Ok(Value::Fixed(Fixed::new(f.ty, bits)));
            }
            let x = args[0].as_int(span, &what)?;
            Ok(Value::Int((x as u64).rotate_right(right(n, 64)) as i64))
        }

        Builtin::SaturatingAdd | Builtin::SaturatingSub | Builtin::SaturatingMul => {
            let what = format!("`{}`", b.name());
            if let (Value::Fixed(x), Value::Fixed(y)) = (&args[0], &args[1])
                && x.ty == y.ty
            {
                let op = match b {
                    Builtin::SaturatingAdd => FixedOp::Add,
                    Builtin::SaturatingSub => FixedOp::Sub,
                    _ => FixedOp::Mul,
                };
                return Ok(Value::Fixed(x.saturating(*y, op)));
            }
            let x = args[0].as_int(span, &what)?;
            let y = args[1].as_int(span, &what)?;
            Ok(Value::Int(match b {
                Builtin::SaturatingAdd => x.saturating_add(y),
                Builtin::SaturatingSub => x.saturating_sub(y),
                _ => x.saturating_mul(y),
            }))
        }

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
                return Ok(Value::Fixed(v));
            }
            let x = args[0].as_int(span, &format!("`{what}`"))?;
            let y = args[1].as_int(span, &format!("`{what}`"))?;
            Ok(Value::Int(match b {
                Builtin::WrapAdd => x.wrapping_add(y),
                Builtin::WrapSub => x.wrapping_sub(y),
                _ => x.wrapping_mul(y),
            }))
        }

        // Out of range raises rather than truncating; a program masks first to mean truncation.
        Builtin::U8OfInt
        | Builtin::U16OfInt
        | Builtin::U32OfInt
        | Builtin::U64OfInt
        | Builtin::I8OfInt
        | Builtin::I16OfInt
        | Builtin::I32OfInt
        | Builtin::I64OfInt
        | Builtin::U128OfInt
        | Builtin::I128OfInt => {
            let t = b.converts_into().expect("every `_of_int` names its type");
            let n = args[0].as_int(span, &format!("`{}`", b.name()))?;
            match Fixed::of(t, i128::from(n)) {
                Some(v) => Ok(Value::Fixed(v)),
                None => Err(Diagnostic::error(
                    codes::RUNTIME_ERROR,
                    format!("`{}` was given {n}", b.name()),
                )
                .primary(span, format!("`{t}` holds {} to {}", t.min(), t.max()))
                .note(format!(
                    "mask the value before the call if that is what you meant: `n & 0x{:X}`",
                    t.max()
                ))),
            }
        }

        Builtin::IntOfU8
        | Builtin::IntOfU16
        | Builtin::IntOfU32
        | Builtin::IntOfU64
        | Builtin::IntOfI8
        | Builtin::IntOfI16
        | Builtin::IntOfI32
        | Builtin::IntOfI64
        | Builtin::IntOfU128
        | Builtin::IntOfI128 => {
            let t = b.converts_from().expect("every `int_of_` names its type");
            let f = args[0].as_fixed(span, &format!("`{}`", b.name()))?;
            match f.to_i128().and_then(|v| i64::try_from(v).ok()) {
                Some(n) => Ok(Value::Int(n)),
                None => Err(Diagnostic::error(
                    codes::RUNTIME_ERROR,
                    format!("`{}` was given {}", b.name(), slot(0)),
                )
                .primary(span, "outside what an `Int` holds")
                .note(format!(
                    "an `Int` is 64 bits and signed, so it does not hold every `{t}`"
                ))
                .showing(vec![Plain::shown(&args[0])])),
            }
        }

        Builtin::U128ToString | Builtin::I128ToString => {
            let f = args[0].as_fixed(span, &format!("`{}`", b.name()))?;
            Ok(Value::str(f.to_decimal()))
        }

        Builtin::U128OfString | Builtin::I128OfString => {
            let text = args[0].as_str(span, &format!("`{}`", b.name()))?;
            let t = if b == Builtin::U128OfString {
                IntTy::U128
            } else {
                IntTy::I128
            };
            Ok(option(fixed_of_text(t, text).map(Value::Fixed)))
        }

        Builtin::CheckedAdd | Builtin::CheckedSub | Builtin::CheckedMul => {
            let answer = match (&args[0], &args[1]) {
                (Value::Fixed(x), Value::Fixed(y)) if x.ty == y.ty => {
                    let op = match b {
                        Builtin::CheckedAdd => FixedOp::Add,
                        Builtin::CheckedSub => FixedOp::Sub,
                        _ => FixedOp::Mul,
                    };
                    x.checked(*y, op).map(Value::Fixed)
                }
                _ => {
                    let x = args[0].as_int(span, &format!("`{}`", b.name()))?;
                    let y = args[1].as_int(span, &format!("`{}`", b.name()))?;
                    match b {
                        Builtin::CheckedAdd => x.checked_add(y),
                        Builtin::CheckedSub => x.checked_sub(y),
                        _ => x.checked_mul(y),
                    }
                    .map(Value::Int)
                }
            };
            Ok(option(answer))
        }

        Builtin::CheckedNeg => {
            let answer = match &args[0] {
                Value::Fixed(x) => x.checked_neg().map(Value::Fixed),
                other => other
                    .as_int(span, "`checked_neg`")?
                    .checked_neg()
                    .map(Value::Int),
            };
            Ok(option(answer))
        }

        // Raises rather than masking: a silent `& 0xFF` would write a byte nobody chose.
        Builtin::ByteOfInt => {
            let n = args[0].as_int(span, "`byte_of_int`")?;
            match u8::try_from(n) {
                Ok(byte) => Ok(Value::bytes([byte])),
                Err(_) => Err(Diagnostic::error(
                    codes::RUNTIME_ERROR,
                    format!("`byte_of_int` was given {n}"),
                )
                .primary(span, "a byte is 0 to 255")
                .note("mask the value before the call if that is what you meant: `n & 0xFF`")),
            }
        }

        Builtin::IntToString => Ok(Value::str(
            args[0].as_int(span, "`int_to_string`")?.to_string(),
        )),

        // The language's own float spelling, the one a diagnostic prints: shortest round-trip,
        // and `Infinity`/`NaN` rather than the `inf`/`NaN` Rust would write.
        Builtin::FloatToString => Ok(Value::str(crate::render_float(
            args[0].as_float(span, "`float_to_string`")?,
        ))),

        Builtin::StringConcat => {
            let a = args[0].as_str(span, "`string_concat`")?;
            let b = args[1].as_str(span, "`string_concat`")?;
            Ok(Value::str(format!("{a}{b}")))
        }

        Builtin::BytesLen => {
            let b = args[0].as_bytes(span, "`bytes_len`")?;
            Ok(Value::Int(b.len() as i64))
        }

        Builtin::BytesAt => {
            let b = args[0].as_bytes(span, "`bytes_at`")?;
            let i = args[1].as_int(span, "`bytes_at`")?;
            match usize::try_from(i).ok().and_then(|i| b.get(i)) {
                Some(byte) => Ok(Value::Int(i64::from(*byte))),
                None => Err(out_of_range(span, "bytes_at", i, b.len(), "bytes")),
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
                Some(w) => Ok(Value::Fixed(Fixed::new(
                    IntTy::U32,
                    u128::from(u32::from_le_bytes(w)),
                ))),
                // Reported at the last index it would read: that is the one past the end.
                None => Err(out_of_range(span, "bytes_u32_le", i + 3, b.len(), "bytes")),
            }
        }

        Builtin::BytesSlice => {
            let b = args[0].as_bytes(span, "`bytes_slice`")?;
            let (start, end) =
                range_args(&args[1], &args[2], b.len(), span, "bytes_slice", "bytes")?;
            Ok(Value::bytes(&b[start..end]))
        }

        Builtin::BytesConcat => {
            let a = args[0].as_bytes(span, "`bytes_concat`")?;
            let b = args[1].as_bytes(span, "`bytes_concat`")?;
            let mut out = Vec::with_capacity(a.len() + b.len());
            out.extend_from_slice(a);
            out.extend_from_slice(b);
            Ok(Value::bytes(out))
        }

        Builtin::BytesBlake3 => {
            let b = args[0].as_bytes(span, "`bytes_blake3`")?;
            Ok(Value::bytes(blake3::hash(b).as_bytes()))
        }

        // At the best level, so what a writer seals is a function of the bytes alone.
        Builtin::BytesGzip => {
            use std::io::Write;
            let b = args[0].as_bytes(span, "`bytes_gzip`")?;
            let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
            let sealed = gz.write_all(b).and_then(|()| gz.finish());
            Ok(Value::bytes(sealed.expect("writing to memory cannot fail")))
        }

        Builtin::BytesGunzip => {
            use std::io::Read;
            let b = args[0].as_bytes(span, "`bytes_gunzip`")?;
            let mut out = Vec::new();
            let read = flate2::read::GzDecoder::new(&b[..]).read_to_end(&mut out);
            Ok(option(read.ok().map(|_| Value::bytes(out))))
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
            Ok(Value::bytes(out))
        }

        Builtin::BytesOfString => {
            let s = args[0].as_str(span, "`bytes_of_string`")?;
            Ok(Value::bytes(s.as_bytes()))
        }

        Builtin::BytesIsUtf8 => {
            let b = args[0].as_bytes(span, "`bytes_is_utf8`")?;
            Ok(Value::Bool(std::str::from_utf8(b).is_ok()))
        }

        Builtin::BytesIndexOf => {
            let hay = args[0].as_bytes(span, "`bytes_index_of`")?;
            let needle = args[1].as_bytes(span, "`bytes_index_of`")?;
            Ok(position(find(hay, needle, 0)))
        }

        Builtin::BytesIndexOfFrom => {
            let hay = args[0].as_bytes(span, "`bytes_index_of_from`")?;
            let needle = args[1].as_bytes(span, "`bytes_index_of_from`")?;
            let from = start_at(&args[2], hay.len(), span, "bytes_index_of_from")?;
            Ok(position(find(hay, needle, from)))
        }

        Builtin::BytesIndexOfByte => {
            let hay = args[0].as_bytes(span, "`bytes_index_of_byte`")?;
            let byte = one_byte(&args[1], span, "bytes_index_of_byte")?;
            Ok(position(memchr::memchr(byte, hay)))
        }

        Builtin::BytesStartsWith => {
            let b = args[0].as_bytes(span, "`bytes_starts_with`")?;
            let prefix = args[1].as_bytes(span, "`bytes_starts_with`")?;
            Ok(Value::Bool(b.starts_with(prefix)))
        }

        Builtin::BytesEndsWith => {
            let b = args[0].as_bytes(span, "`bytes_ends_with`")?;
            let suffix = args[1].as_bytes(span, "`bytes_ends_with`")?;
            Ok(Value::Bool(b.ends_with(suffix)))
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
            Ok(Value::list(out))
        }

        Builtin::BytesScan => {
            let b = args[0].as_bytes(span, "`bytes_scan`")?;
            Ok(Value::Int(scan(args, b, span, false)?))
        }

        Builtin::BytesScanUntil => {
            let b = args[0].as_bytes(span, "`bytes_scan_until`")?;
            Ok(Value::Int(scan(args, b, span, true)?))
        }

        Builtin::StringOfBytes => {
            let b = args[0].as_bytes(span, "`string_of_bytes`")?;
            match std::str::from_utf8(b) {
                Ok(s) => Ok(Value::str(s)),
                Err(e) => Err(not_utf8(span, b, &e)),
            }
        }

        Builtin::StringOfBytesLossy => {
            let b = args[0].as_bytes(span, "`string_of_bytes_lossy`")?;
            Ok(Value::str(String::from_utf8_lossy(b)))
        }

        Builtin::CharOfInt => {
            let n = args[0].as_int(span, "`char_of_int`")?;
            Ok(option(
                u32::try_from(n)
                    .ok()
                    .and_then(char::from_u32)
                    .map(Value::Char),
            ))
        }

        Builtin::IntOfChar => Ok(Value::Int(i64::from(u32::from(
            args[0].as_char(span, "`int_of_char`")?,
        )))),

        Builtin::StringChars => Ok(Value::list(
            args[0]
                .as_str(span, "`string_chars`")?
                .chars()
                .map(Value::Char)
                .collect(),
        )),

        Builtin::StringOfChars => {
            let items = args[0].as_list(span, "`string_of_chars`")?;
            let mut out = String::with_capacity(items.len());
            for c in items.iter() {
                out.push(c.as_char(span, "`string_of_chars`")?);
            }
            Ok(Value::str(out))
        }

        Builtin::StringLen => Ok(Value::Int(
            args[0].as_str(span, "`string_len`")?.chars().count() as i64,
        )),

        Builtin::ArrayNew => {
            let n = args[0].as_int(span, "`array_new`")?;
            if !(0..=MAX_ARRAY_LEN).contains(&n) {
                return Err(array_length(span, n));
            }
            Ok(Value::array(vec![args[1].clone(); n as usize]))
        }

        Builtin::ArrayOfList => Ok(Value::array(
            args[0]
                .as_list(span, "`array_of_list`")?
                .iter()
                .cloned()
                .collect(),
        )),

        Builtin::ArrayToList => Ok(Value::list(
            args[0].as_array(span, "`array_to_list`")?.to_vec(),
        )),

        Builtin::ArrayLen => Ok(Value::Int(
            args[0].as_array(span, "`array_len`")?.len() as i64
        )),

        Builtin::ArrayAt => {
            let xs = args[0].as_array(span, "`array_at`")?;
            let i = args[1].as_int(span, "`array_at`")?;
            Ok(option(
                usize::try_from(i).ok().and_then(|i| xs.get(i)).cloned(),
            ))
        }

        Builtin::ArrayGet => {
            let xs = args[0].as_array(span, "`array_get`")?;
            let i = args[1].as_int(span, "`array_get`")?;
            match usize::try_from(i).ok().and_then(|at| xs.get(at)) {
                Some(x) => Ok(x.clone()),
                None => Err(out_of_range(span, "array_get", i, xs.len(), "elements")),
            }
        }

        Builtin::ArraySet => array_set(args, span),

        Builtin::Reflect => Ok(crate::reflect::value_of(&Plain::of(&args[0]))),

        Builtin::Shown => Ok(crate::reflect::shown_of(&args[0])),

        Builtin::Digest => Ok(Value::bytes(crate::digest::digest(&args[0], span)?)),

        Builtin::NumericBinary => {
            let op = numeric_op(&args[0], span)?;
            let w = args[1].as_int(span, "a witness")?;
            let l = witnessed(args[2].clone(), w, span)?;
            // A shift's count is an `Int` whatever the word is.
            let r = if matches!(op, BinOp::Shl | BinOp::Shr | BinOp::Ushr) {
                args[3].clone()
            } else {
                witnessed(args[3].clone(), w, span)?
            };
            crate::semantics::strict_binary(op, &l, &r, span, span, span)
        }

        Builtin::NumericUnary => {
            let w = args[1].as_int(span, "a witness")?;
            let x = witnessed(args[2].clone(), w, span)?;
            if args[0].as_int(span, "an operator")? == 1 {
                return Ok(match x {
                    Value::Fixed(f) => Value::Fixed(Fixed::new(f.ty, !f.bits())),
                    other => Value::Int(!other.as_int(span, "`~`")?),
                });
            }
            match x {
                Value::Ctor { .. } => {
                    crate::instances::numeric(crate::instances::NEG, std::slice::from_ref(&x))
                        .ok_or_else(|| crate::semantics::unstated(span, &x))
                }
                Value::Float(f) => Ok(Value::Float(-f)),
                Value::Decimal(d) => Ok(Value::Decimal(-d)),
                Value::Fixed(f) => f.checked_neg().map(Value::Fixed).ok_or_else(|| {
                    Diagnostic::error(codes::RUNTIME_ERROR, "negation overflowed its width")
                        .primary(span, "negated here")
                }),
                other => other
                    .as_int(span, "negation")?
                    .checked_neg()
                    .map(Value::Int)
                    .ok_or_else(|| {
                        Diagnostic::error(codes::RUNTIME_ERROR, "integer overflow in negation")
                            .primary(span, "negated here")
                    }),
            }
        }

        Builtin::NumericOfInt => {
            let w = args[0].as_int(span, "a witness")?;
            let n = args[1].as_int(span, "`numeric_of_int`")?;
            if w >= crate::instances::STATED_WITNESS {
                return crate::instances::of_int(w, n).ok_or_else(|| {
                    Diagnostic::error(
                        codes::RUNTIME_ERROR,
                        format!(
                            "`numeric_of_int({n})` did not answer at the type it was called at"
                        ),
                    )
                    .primary(span, "converted here")
                });
            }
            match usize::try_from(w).ok().and_then(|i| INT_TYPES.get(i)) {
                Some(t) => Fixed::of(*t, i128::from(n))
                    .map(Value::Fixed)
                    .ok_or_else(|| {
                        Diagnostic::error(
                            codes::RUNTIME_ERROR,
                            format!("`numeric_of_int({n})` is outside `{}`", t.name()),
                        )
                        .primary(span, "converted here")
                    }),
                None => Ok(match w {
                    FLOAT_WITNESS => Value::Float(n as f64),
                    DECIMAL_WITNESS => Value::Decimal(Decimal::from(n)),
                    _ => Value::Int(n),
                }),
            }
        }
        // A width the words carry is read as its `Int` already.
        Builtin::ToInt => Ok(option(match &args[1] {
            Value::Fixed(f) => f
                .to_i128()
                .and_then(|v| i64::try_from(v).ok())
                .map(Value::Int),
            other => Some(Value::Int(other.as_int(span, "`to_int`")?)),
        })),
        Builtin::FloatOfInt => Ok(Value::Float(args[0].as_int(span, "`float_of_int`")? as f64)),
        Builtin::IntOfFloat => {
            let x = args[0].as_float(span, "`int_of_float`")?;
            let r = rounded(x, rounding(&args[1], span, "int_of_float")?);
            Ok(option(int_of_whole(r).map(Value::Int)))
        }
        Builtin::Round => {
            let x = args[0].as_float(span, "`round`")?;
            Ok(Value::Float(rounded(x, rounding(&args[1], span, "round")?)))
        }
        Builtin::Floor => unary_float(&args[0], span, "`floor`", f64::floor),
        Builtin::Ceil => unary_float(&args[0], span, "`ceil`", f64::ceil),
        Builtin::Sqrt => unary_float(&args[0], span, "`sqrt`", f64::sqrt),
        Builtin::Exp => unary_float(&args[0], span, "`exp`", libm::exp),
        Builtin::Ln => unary_float(&args[0], span, "`ln`", libm::log),
        Builtin::Log2 => unary_float(&args[0], span, "`log2`", libm::log2),
        Builtin::Log10 => unary_float(&args[0], span, "`log10`", libm::log10),
        Builtin::Sin => unary_float(&args[0], span, "`sin`", libm::sin),
        Builtin::Cos => unary_float(&args[0], span, "`cos`", libm::cos),
        Builtin::Tan => unary_float(&args[0], span, "`tan`", libm::tan),
        Builtin::Asin => unary_float(&args[0], span, "`asin`", libm::asin),
        Builtin::Acos => unary_float(&args[0], span, "`acos`", libm::acos),
        Builtin::Atan => unary_float(&args[0], span, "`atan`", libm::atan),
        Builtin::Pow => binary_float(args, span, "`pow`", libm::pow),
        Builtin::Atan2 => binary_float(args, span, "`atan2`", libm::atan2),
        Builtin::Hypot => binary_float(args, span, "`hypot`", libm::hypot),

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
            Ok(Value::str(&s[from..to]))
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
            Ok(Value::list(s.split(sep).map(Value::str).collect()))
        }

        // These three read `std`'s Unicode tables, so their answers move if those tables do.
        Builtin::StringTrim => Ok(Value::str(args[0].as_str(span, "`string_trim`")?.trim())),

        Builtin::StringLower => Ok(Value::str(
            args[0].as_str(span, "`string_lower`")?.to_lowercase(),
        )),

        Builtin::StringUpper => Ok(Value::str(
            args[0].as_str(span, "`string_upper`")?.to_uppercase(),
        )),

        Builtin::StringStartsWith => {
            let s = args[0].as_str(span, "`string_starts_with`")?;
            let prefix = args[1].as_str(span, "`string_starts_with`")?;
            Ok(Value::Bool(s.starts_with(prefix)))
        }

        Builtin::StringEndsWith => {
            let s = args[0].as_str(span, "`string_ends_with`")?;
            let suffix = args[1].as_str(span, "`string_ends_with`")?;
            Ok(Value::Bool(s.ends_with(suffix)))
        }

        Builtin::StringContains => {
            let s = args[0].as_str(span, "`string_contains`")?;
            let needle = args[1].as_str(span, "`string_contains`")?;
            Ok(Value::Bool(s.contains(needle)))
        }

        Builtin::StringFind => {
            let s = args[0].as_str(span, "`string_find`")?;
            let needle = args[1].as_str(span, "`string_find`")?;
            match s.find(needle) {
                Some(at) => Ok(Value::Int(s[..at].chars().count() as i64)),
                None => Err(Diagnostic::error(
                    codes::RUNTIME_ERROR,
                    format!("`string_find` did not find {} in {}", slot(0), slot(1)),
                )
                .primary(span, "this substring does not occur")
                .note("guard with `string_contains`, which answers the same question as a `Bool`")
                .showing(vec![
                    Plain::Str(needle.to_string()),
                    Plain::Str(s.to_string()),
                ])),
            }
        }

        // The order `Map` iterates in, so a derived `OrdDict` and map key order cannot drift.
        Builtin::Compare | Builtin::CompareValues => {
            crate::value::secret_has_no_order(&args[0], b.name(), span)?;
            crate::value::secret_has_no_order(&args[1], b.name(), span)?;
            Ok(Value::ctor(
                match args[0].cmp(&args[1]) {
                    std::cmp::Ordering::Less => "Less",
                    std::cmp::Ordering::Equal => "Equal",
                    std::cmp::Ordering::Greater => "Greater",
                },
                Vec::new(),
            ))
        }

        // Every map builtin reaches keys through `map::key`, the one gate before `Value::cmp`.
        Builtin::MapNew => Ok(map::new()),
        Builtin::MapInsert => {
            let (k, v) = (args.remove(1), args.remove(1));
            Ok(map::insert(args.remove(0), k, v, span)?)
        }
        Builtin::MapGet => Ok(map::get(&args[0], &args[1], span)?),
        Builtin::MapContains => Ok(map::contains(&args[0], &args[1], span)?),
        Builtin::MapRemove => {
            let k = args.remove(1);
            Ok(map::remove(args.remove(0), &k, span)?)
        }
        Builtin::MapLen => Ok(map::len(&args[0], span)?),
        Builtin::MapKeys => Ok(map::keys(&args[0], span)?),
        Builtin::MapValues => Ok(map::values(&args[0], span)?),
        Builtin::MapEntries => Ok(map::entries(&args[0], span)?),
        Builtin::MapOfEntries => Ok(map::of_entries(&args[0], span)?),
        Builtin::MapMerge => Ok(map::merge(&args[0], &args[1], span)?),
        Builtin::MapFirst | Builtin::MapLast => Ok(map::end(&args[0], b, span)?),
        Builtin::MapFloor | Builtin::MapCeiling | Builtin::MapBelow | Builtin::MapAbove => {
            Ok(map::beside(&args[0], &args[1], b, span)?)
        }
        Builtin::MapPopFirst | Builtin::MapPopLast => Ok(map::pop(args.remove(0), b, span)?),
        Builtin::MapRange => Ok(map::range(args, span)?),
        Builtin::MapSplit => {
            let k = args.remove(1);
            Ok(map::split(args.remove(0), &k, span)?)
        }

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
            Ok(Value::Decimal(quotient.round_dp_with_strategy(scale, mode)))
        }

        Builtin::DecimalRound => {
            let d = args[0].as_decimal(span, "`decimal_round`")?;
            let scale = decimal_scale(&args[1], span, "decimal_round")?;
            let mode = rounding(&args[2], span, "decimal_round")?;
            Ok(Value::Decimal(d.round_dp_with_strategy(scale, mode)))
        }

        Builtin::DecimalOfInt => Ok(Value::Decimal(Decimal::from(
            args[0].as_int(span, "`decimal_of_int`")?,
        ))),

        Builtin::IntOfDecimal => {
            let d = args[0].as_decimal(span, "`int_of_decimal`")?;
            let mode = rounding(&args[1], span, "int_of_decimal")?;
            Ok(option(
                d.round_dp_with_strategy(0, mode).to_i64().map(Value::Int),
            ))
        }

        Builtin::FloatOfDecimal => {
            let d = args[0].as_decimal(span, "`float_of_decimal`")?;
            Ok(Value::Float(float_of_decimal(d)))
        }

        Builtin::DecimalOfFloat => {
            let f = args[0].as_float(span, "`decimal_of_float`")?;
            Ok(option(decimal_of_float(f).map(Value::Decimal)))
        }

        Builtin::DecimalOfString => {
            let s = args[0].as_str(span, "`decimal_of_string`")?;
            Ok(option(parse_decimal(s).map(Value::Decimal)))
        }

        Builtin::FloatOfString => {
            let s = args[0].as_str(span, "`float_of_string`")?;
            Ok(option(parse_float(s).map(Value::Float)))
        }

        // Keeps the scale (`1.50m` renders `1.50`), so it round-trips `decimal_of_string`.
        Builtin::DecimalToString => {
            let d = args[0].as_decimal(span, "`decimal_to_string`")?;
            Ok(Value::str(d.to_string()))
        }

        Builtin::BitsOfFloat => {
            let f = args[0].as_float(span, "`bits_of_float`")?;
            Ok(Value::Int(f.to_bits() as i64))
        }

        Builtin::FloatOfBits => {
            let n = args[0].as_int(span, "`float_of_bits`")?;
            Ok(Value::Float(f64::from_bits(n as u64)))
        }

        Builtin::Observe => {
            // The identity; its work is the emitter's, which makes the call opaque so the C
            // compiler cannot drop a pure computation whose value is only observed.
            Ok(args[0].clone())
        }

        Builtin::Panic => {
            let (message, values) = match &args[0] {
                Value::Str(s) => (s.to_string(), Vec::new()),
                other => (slot(0), vec![Plain::shown(other)]),
            };
            Err(
                Diagnostic::error(codes::RUNTIME_ERROR, format!("panic: {message}"))
                    .primary(span, "`panic` called here")
                    .showing(values),
            )
        }

        Builtin::SecretOfString => {
            args[0].as_str(span, "`secret_of_string`")?;
            Ok(Value::secret(args[0].clone()))
        }

        // Constant time but not rate limited: preventing a guessing loop is the program's job.
        Builtin::SecretVerify => {
            let Value::Secret(held) = &args[0] else {
                return Err(type_error(span, "`secret_verify`", "Secret", &args[0]));
            };
            let candidate = args[1].as_str(span, "`secret_verify`")?;
            let held = held.as_str(span, "`secret_verify`")?;
            Ok(Value::Bool(crate::value::constant_time_eq(
                held.as_bytes(),
                candidate.as_bytes(),
            )))
        }

        Builtin::SecretIsEmpty => {
            let Value::Secret(held) = &args[0] else {
                return Err(type_error(span, "`secret_is_empty`", "Secret", &args[0]));
            };
            Ok(Value::Bool(match &**held {
                Value::Str(s) => s.is_empty(),
                Value::Bytes(b) => b.is_empty(),
                // Only strings are constructible; `false` reports a credential as present.
                _ => false,
            }))
        }
    }
}

fn at(xs: &List, i: i64) -> Option<&Value> {
    usize::try_from(i).ok().and_then(|i| xs.get(i))
}

/// Digits, after a `-` only for a signed width, and a value the width holds.
fn fixed_of_text(t: IntTy, text: &str) -> Option<Fixed> {
    let digits = match text.strip_prefix('-') {
        Some(rest) if t.signed() => rest,
        _ => text,
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if t.signed() {
        Fixed::of(t, text.parse::<i128>().ok()?)
    } else {
        Fixed::of_unsigned(t, text.parse::<u128>().ok()?)
    }
}

fn option(v: Option<Value>) -> Value {
    match v {
        Some(v) => Value::ctor("Some", vec![v]),
        None => Value::ctor("None", Vec::new()),
    }
}

/// The NaN every float function answers, whatever NaN the machine produced.
pub const CANONICAL_NAN_BITS: u64 = 0x7FF8_0000_0000_0000;

/// Decided on the bits: the compiler may treat any NaN as any other, so a float-level choice
/// between two NaNs can be folded away and an x86 sqrt's negative NaN kept.
fn canonical(x: f64) -> f64 {
    let bits = x.to_bits();
    let nan =
        bits & 0x7FF0_0000_0000_0000 == 0x7FF0_0000_0000_0000 && bits & 0x000F_FFFF_FFFF_FFFF != 0;
    f64::from_bits(if nan { CANONICAL_NAN_BITS } else { bits })
}

fn unary_float(x: &Value, span: Span, what: &str, f: fn(f64) -> f64) -> Result<Value, Diagnostic> {
    Ok(Value::Float(canonical(f(x.as_float(span, what)?))))
}

fn binary_float(
    args: &[Value],
    span: Span,
    what: &str,
    f: fn(f64, f64) -> f64,
) -> Result<Value, Diagnostic> {
    let x = args[0].as_float(span, what)?;
    let y = args[1].as_float(span, what)?;
    Ok(Value::Float(canonical(f(x, y))))
}

/// `x` at a whole number, chosen as `mode` chooses a decimal's last digit.
fn rounded(x: f64, mode: RoundingStrategy) -> f64 {
    if !x.is_finite() {
        return canonical(x);
    }
    match mode {
        RoundingStrategy::MidpointAwayFromZero => x.round(),
        RoundingStrategy::ToZero => x.trunc(),
        RoundingStrategy::AwayFromZero if x.fract() != 0.0 => x.trunc() + x.signum(),
        RoundingStrategy::AwayFromZero => x,
        RoundingStrategy::ToPositiveInfinity => x.ceil(),
        RoundingStrategy::ToNegativeInfinity => x.floor(),
        _ => x.round_ties_even(),
    }
}

/// A whole float as an `Int`, when one holds it.
fn int_of_whole(x: f64) -> Option<i64> {
    const LIMIT: f64 = 9_223_372_036_854_775_808.0;
    (x.is_finite() && (-LIMIT..LIMIT).contains(&x)).then_some(x as i64)
}

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

/// A scale argument, refused outside `0..=28` rather than clamped.
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

/// The shortest decimal that round-trips the float; `None` if non-finite or out of range.
fn decimal_of_float(f: f64) -> Option<Decimal> {
    if !f.is_finite() {
        return None;
    }
    parse_decimal(&format!("{f}"))
}

/// The nearest `f64`, so `float_of_decimal(decimal_of_float(f)) == f` wherever defined.
fn float_of_decimal(d: Decimal) -> f64 {
    d.to_string()
        .parse::<f64>()
        .unwrap_or_else(|_| d.to_f64().unwrap_or(f64::NAN))
}

fn parse_decimal(text: &str) -> Option<Decimal> {
    if text.contains(['e', 'E']) {
        Decimal::from_scientific(text).ok()
    } else {
        Decimal::from_str_exact(text).ok()
    }
}

/// The lexer's float grammar plus a leading sign; `inf`, `NaN`, `.5` and the like are `None`.
fn parse_float(text: &str) -> Option<f64> {
    let body = text.strip_prefix(['+', '-']).unwrap_or(text);
    let (mantissa, exponent) = match body.split_once(['e', 'E']) {
        Some((m, e)) => (m, Some(e)),
        None => (body, None),
    };
    let (whole, fraction) = match mantissa.split_once('.') {
        Some((w, f)) => (w, Some(f)),
        None => (mantissa, None),
    };
    let digits = |s: &str| {
        s.starts_with(|c: char| c.is_ascii_digit())
            && s.bytes().all(|b| b.is_ascii_digit() || b == b'_')
    };
    if !digits(whole)
        || fraction.is_some_and(|f| !digits(f))
        || exponent.is_some_and(|e| !digits(e.strip_prefix(['+', '-']).unwrap_or(e)))
    {
        return None;
    }
    text.replace('_', "").parse().ok()
}

fn position(at: Option<usize>) -> Value {
    match at {
        Some(i) => Value::ctor("Some", vec![Value::Int(i as i64)]),
        None => Value::ctor("None", Vec::new()),
    }
}

/// An empty needle occurs at `from`, as with `str::find`.
fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(from);
    }
    memchr::memmem::find(&hay[from..], needle).map(|at| from + at)
}

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

pub fn scan_window(hay: &[u8], from: usize, max: usize) -> &[u8] {
    &hay[from..hay.len().min(from.saturating_add(max))]
}

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

    // `memchr` is SIMD and the bitmap loop is not, so small sets take it.
    let found = match (want, members.as_ref()) {
        // Empty class: `bytes_scan_until` runs out the window, `bytes_scan` stops at once.
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

fn start_at(v: &Value, len: usize, span: Span, what: &str) -> Result<usize, Diagnostic> {
    let from = int_arg(v, span, what)?;
    match usize::try_from(from) {
        Ok(from) if from <= len => Ok(from),
        _ => Err(start_outside(from, len, span, what)),
    }
}

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

/// A scan's byte budget, which bounds the work hostile input can cause.
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
pub fn array_length(span: Span, n: i64) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`array_new` of {n} elements is outside `0` to `{MAX_ARRAY_LEN}`"),
    )
    .primary(span, "no array has this length")
}

fn out_of_range(span: Span, what: &str, index: i64, len: usize, unit: &str) -> Diagnostic {
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

pub fn assertion_failure(actual: &Value, expected: &Value, span: Span) -> Diagnostic {
    let (e, a) = (slot(0), slot(1));
    let mut values = vec![Plain::shown(expected), Plain::shown(actual)];
    let mut diag = Diagnostic::error(
        codes::ASSERTION_FAILED,
        format!("assertion failed: expected {e}, found {a}"),
    )
    .primary(span, "these values are not equal")
    .note(format!("expected: {e}"))
    .note(format!("actual:   {a}"));

    if let Some(d) = first_difference(actual, expected) {
        let mut path = String::new();
        for step in d.path {
            match step {
                PathStep::Index(i) => path.push_str(&format!("[{i}]")),
                PathStep::Key(k) => {
                    path.push_str(&format!("[{}]", slot(values.len())));
                    values.push(Plain::shown(&k));
                }
                PathStep::Field(name) => path.push_str(&format!(".{name}")),
                PathStep::Arg(ctor, i) => path.push_str(&format!(".{ctor}.{i}")),
            }
        }
        let (de, da) = (slot(values.len()), slot(values.len() + 1));
        values.push(Plain::shown(&d.expected));
        values.push(Plain::shown(&d.actual));
        diag = diag.note(format!(
            "first difference at `{path}`: expected {de}, found {da}"
        ));
    }
    diag.showing(values)
}

pub fn assert_failure(message: &Value, span: Span) -> Diagnostic {
    let diag = Diagnostic::error(
        codes::ASSERTION_FAILED,
        "assertion failed: condition is false",
    )
    .primary(span, "this condition evaluated to false");
    let carried = match message {
        Value::Ctor { name, args, .. } if name.as_str() == "Some" => args.first(),
        Value::Ctor { .. } => None,
        // A non-`Option` message comes only from an unchecked call.
        other => Some(other),
    };
    match carried {
        Some(Value::Str(s)) => diag.note(s.to_string()),
        Some(other) => diag.note(slot(0)).showing(vec![Plain::shown(other)]),
        None => diag,
    }
}

#[cold]
#[inline(never)]
pub fn cell_in_update(span: Span, slot: Slot, what: &str) -> Diagnostic {
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
pub fn no_such_cell(span: Span, slot: Slot) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("cell {slot} does not belong to the region this code is running in"),
    )
    .primary(span, "this cell was made by a different run")
    .note("please report this: a cell value escaped the region that allocated it")
}

/// The C backend answers every builtin that calls back into the program or reads a cell.
#[cold]
fn answered_by_the_backend(b: Builtin, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!(
            "`{}` reached the value builtins, and only compiled code can answer it",
            b.name()
        ),
    )
    .primary(span, "it calls back into the program or reads a cell")
    .note("this is Ply's fault: the C backend answers this builtin over its own words")
}
