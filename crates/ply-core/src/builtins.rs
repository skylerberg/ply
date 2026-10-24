//! The prelude, in one definition per builtin.

use crate::semantics::arity_error;
use crate::value::{Decimal, Fixed, FixedOp, Value, first_difference, type_error, values_equal};
use crate::{BinOp, Diagnostic, INT_TYPES, IntTy, PathStep, Plain, Span, codes, float, map, slot};
use rust_decimal::RoundingStrategy;
use rust_decimal::prelude::ToPrimitive;

/// A list this long is a runaway `range`, not an intent.
const MAX_RANGE_LEN: i64 = 10_000_000;

/// An array this long is a runaway length, not an intent: half a gigabyte of words.
pub const MAX_ARRAY_LEN: i64 = 1 << 26;

/// `Decimal`'s scale bound, the type's rather than a policy.
const MAX_DECIMAL_SCALE: u32 = 28;
/// Every builtin, once: its name in the prelude, and the type of the function compiled code calls it
/// through, off which `ply_compiled::native` reads what the runtime reports of it. That function
/// takes a word for each parameter, witnesses first, and answers a `Raising` where a failure of it is
/// a raise of `abort.raise`, which the prelude's scheme for it carries in its row and a `handle`
/// answers, or a `Word` where a failure ends the run.
macro_rules! builtins {
    (@count) => { 0 };
    (@count $first:ident $($rest:ident)*) => { 1 + builtins!(@count $($rest)*) };
    ($d:tt $( $(#[$doc:meta])* $variant:ident = $name:literal ($($param:ident),*) -> $answer:ident; )*) => {
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub enum Builtin {
            $( $(#[$doc])* $variant, )*
        }

        impl Builtin {
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

            /// The parameters of the function compiled code calls it through.
            pub fn words(self) -> usize {
                match self {
                    $( Builtin::$variant => builtins!(@count $($param)*), )*
                }
            }

            pub fn all() -> &'static [Builtin] {
                &[ $( Builtin::$variant, )* ]
            }
        }

        /// Hands `$m` every builtin as `Variant "name" (parameters) Answer;`: what is made one per
        /// builtin is made from this table.
        #[macro_export]
        macro_rules! each_builtin {
            ($d m:ident) => {
                $d m! { $( $variant $name ($($param),*) $answer; )* }
            };
        }
    };
}

builtins! { $
    Assert = "assert" (cond, message) -> Raising;
    AssertEq = "assert_eq" (actual, expected) -> Raising;
    /// `assert_eq` failing where compiled code compared the two and found them unequal: raises with
    /// both shown, comparing nothing. No source can spell it.
    AssertFailed = "?assert_failed" (actual, expected) -> Raising;
    Len = "List::len" (xs) -> Word;
    WithCapacity = "List::with_capacity" (n) -> Word;
    /// The leaf of a list's trie that holds its items `j * 32` on, counting from the leaf a `rest`
    /// cut starts in, as compiled code holds an `Option`: the leaf's word with a count of the
    /// caller's, or zero past the last.
    ListLeafOf = "ListNode::leaf_of" (xs, j) -> Word;
    Iterate = "iterate" (seed, budget, step) -> Raising;
    Metered = "metered" (f) -> Word;
    Range = "range" (lo, hi) -> Word;
    WrapAdd = "wrap_add" (a, b) -> Word;
    WrapSub = "wrap_sub" (a, b) -> Word;
    WrapMul = "wrap_mul" (a, b) -> Word;
    /// The low thirty-two bits of an `Int`, rotated right.
    Rotr32 = "rotr32" (x, n) -> Word;
    Rotr = "rotr" (x, n) -> Word;
    Rotl = "rotl" (x, n) -> Word;
    /// Over any integer type: the exact answer, or the type's nearest value where it leaves it.
    SaturatingAdd = "saturating_add" (a, b) -> Word;
    SaturatingSub = "saturating_sub" (a, b) -> Word;
    SaturatingMul = "saturating_mul" (a, b) -> Word;
    // Field-less rather than carrying an `IntTy`: the enum is cast to a per-builtin cache index.
    U8OfInt = "U8::of_int" (n) -> Raising;
    U16OfInt = "U16::of_int" (n) -> Raising;
    U32OfInt = "U32::of_int" (n) -> Raising;
    U64OfInt = "U64::of_int" (n) -> Raising;
    I8OfInt = "I8::of_int" (n) -> Raising;
    I16OfInt = "I16::of_int" (n) -> Raising;
    I32OfInt = "I32::of_int" (n) -> Raising;
    I64OfInt = "I64::of_int" (n) -> Word;
    IntOfU8 = "Int::of_u8" (n) -> Word;
    IntOfU16 = "Int::of_u16" (n) -> Word;
    IntOfU32 = "Int::of_u32" (n) -> Word;
    IntOfU64 = "Int::of_u64" (n) -> Raising;
    IntOfI8 = "Int::of_i8" (n) -> Word;
    IntOfI16 = "Int::of_i16" (n) -> Word;
    IntOfI32 = "Int::of_i32" (n) -> Word;
    IntOfI64 = "Int::of_i64" (n) -> Word;
    Min = "min" (a, b) -> Word;
    Max = "max" (a, b) -> Word;
    ByteOfInt = "byte_of_int" (n) -> Raising;
    IntToString = "Int::to_string" (n) -> Word;
    FloatToString = "Float::to_string" (f) -> Word;
    StringConcat = "String::concat" (a, b) -> Word;
    BytesLen = "Bytes::len" (b) -> Word;
    BytesAt = "Bytes::at" (b, i) -> Raising;
    BytesU32Le = "Bytes::u32_le" (b, i) -> Raising;
    BytesSlice = "Bytes::slice" (b, start, end) -> Raising;
    BytesConcat = "Bytes::concat" (a, b) -> Word;
    BytesConcatAll = "Bytes::concat_all" (bs) -> Word;
    BytesBlake3 = "Bytes::blake3" (b) -> Word;
    BytesGzip = "Bytes::gzip" (b) -> Word;
    BytesGunzip = "Bytes::gunzip" (b) -> Word;
    BytesZstd = "Bytes::zstd" (b) -> Word;
    BytesOfString = "Bytes::of_string" (s) -> Word;
    BytesIsUtf8 = "Bytes::is_utf8" (b) -> Word;
    BytesIndexOf = "Bytes::index_of" (hay, needle) -> Word;
    BytesIndexOfFrom = "Bytes::index_of_from" (hay, needle, from) -> Raising;
    BytesIndexOfByte = "Bytes::index_of_byte" (hay, byte) -> Raising;
    BytesStartsWith = "Bytes::starts_with" (b, prefix) -> Word;
    BytesEndsWith = "Bytes::ends_with" (b, suffix) -> Word;
    BytesSplit = "Bytes::split" (b, sep) -> Raising;
    BytesScan = "Bytes::scan" (hay, from, class, budget) -> Raising;
    BytesScanUntil = "Bytes::scan_until" (hay, from, class, budget) -> Raising;
    BytesPosition = "Bytes::position" (b, from, f) -> Raising;
    StringOfBytes = "String::of_bytes" (b) -> Raising;
    StringOfBytesLossy = "String::of_bytes_lossy" (b) -> Word;
    StringLen = "String::len" (s) -> Word;
    StringSlice = "String::slice" (s, start, end) -> Raising;
    StringSplit = "String::split" (s, sep) -> Raising;
    StringTrim = "String::trim" (s) -> Word;
    StringLower = "String::lower" (s) -> Word;
    StringUpper = "String::upper" (s) -> Word;
    StringStartsWith = "String::starts_with" (s, prefix) -> Word;
    StringEndsWith = "String::ends_with" (s, suffix) -> Word;
    StringContains = "String::contains" (s, t) -> Word;
    StringFind = "String::find" (s, needle) -> Raising;
    MapNew = "Map::new" () -> Word;
    MapLen = "Map::len" (m) -> Word;
    DecimalDiv = "Decimal::div" (a, b, scale, mode) -> Raising;
    DecimalRound = "Decimal::round" (d, scale, mode) -> Raising;
    DecimalOfInt = "Decimal::of_int" (n) -> Word;
    IntOfDecimal = "Int::of_decimal" (d, mode) -> Word;
    FloatOfDecimal = "Float::of_decimal" (d) -> Word;
    DecimalOfFloat = "Decimal::of_float" (f) -> Word;
    DecimalOfString = "Decimal::of_string" (s) -> Word;
    /// The lexer's float parse over text, reaching `Float`s no route through `Decimal` does.
    FloatOfString = "Float::of_string" (s) -> Word;
    DecimalToString = "Decimal::to_string" (d) -> Word;
    DecimalScale = "Decimal::scale" (d) -> Word;
    /// The IEEE 754 bit pattern, as the signed 64-bit `Int` it fits in.
    BitsOfFloat = "bits_of_float" (f) -> Word;
    FloatOfBits = "Float::of_bits" (n) -> Word;
    Compare = "compare" (x, y) -> Word;
    /// The same order as [`Builtin::Compare`], under a name a module may not declare.
    CompareValues = "compare_values" (x, y) -> Word;
    Panic = "panic" (message) -> Raising;
    /// The identity a benchmark pins a measured value with: opaque, so it is not optimized away.
    Observe = "observe" (value) -> Word;
    SecretOfString = "Secret::of_string" (s) -> Word;
    SecretVerify = "Secret::verify" (stored, supplied) -> Word;
    SecretIsEmpty = "Secret::is_empty" (s) -> Word;
    // Appended, so every earlier builtin keeps its cache index.
    U128OfInt = "U128::of_int" (n) -> Raising;
    I128OfInt = "I128::of_int" (n) -> Word;
    IntOfU128 = "Int::of_u128" (n) -> Raising;
    IntOfI128 = "Int::of_i128" (n) -> Raising;
    U128ToString = "U128::to_string" (n) -> Word;
    I128ToString = "I128::to_string" (n) -> Word;
    U128OfString = "U128::of_string" (s) -> Word;
    I128OfString = "I128::of_string" (s) -> Word;
    /// Over any integer type: the exact answer, or `None` where it leaves the type.
    CheckedAdd = "checked_add" (a, b) -> Word;
    CheckedSub = "checked_sub" (a, b) -> Word;
    CheckedMul = "checked_mul" (a, b) -> Word;
    CheckedNeg = "checked_neg" (a) -> Word;
    /// `None` for a surrogate or past `U+10FFFF`, which no `Char` is.
    CharOfInt = "Char::of_int" (n) -> Word;
    IntOfChar = "Int::of_char" (c) -> Word;
    StringChars = "String::chars" (s) -> Word;
    StringOfChars = "String::of_chars" (cs) -> Word;
    ArrayNew = "Array::new" (n, x) -> Raising;
    ArrayOfList = "Array::of_list" (xs) -> Word;
    ArrayLen = "Array::len" (xs) -> Word;
    ArrayAt = "Array::at" (xs, i) -> Word;
    /// Raises where `Array::at` answers `None`.
    ArrayGet = "Array::get" (xs, i) -> Raising;
    /// In place while the array has one holder; raises out of range.
    ArraySet = "Array::set" (xs, i, v) -> Raising;
    /// An array of records holding each field apart, the compiled runtime's columns; as a value it
    /// is the array of its records, and each builtin answers as its `array_` namesake does.
    ColumnsNew = "Columns::new" (n, x) -> Raising;
    ColumnsOfList = "Columns::of_list" (xs) -> Word;
    ColumnsLen = "Columns::len" (xs) -> Word;
    ColumnsAt = "Columns::at" (xs, i) -> Word;
    ColumnsGet = "Columns::get" (xs, i) -> Raising;
    ColumnsSet = "Columns::set" (xs, i, v) -> Raising;
    /// The value as a `std.value.Value`, whole.
    Reflect = "reflect" (x) -> Word;
    /// As `reflect`, with a value whose type states a `show` as `VShown` of what that function
    /// writes: what `std.show.show` renders.
    Shown = "shown" (x) -> Word;
    /// BLAKE3 of the value's canonical encoding, under `derivable(hash, a)`.
    Digest = "digest" (x) -> Word;
    /// The value's canonical encoding, whose BLAKE3 is its `digest`: what code written from a type
    /// reads of a part it does not lay out itself. No source can spell it.
    Encoded = "?encoded" (x) -> Word;
    /// `bracket(acquire, release, body)`: `release` runs on every exit from `body` but a failure.
    Bracket = "bracket" (acquire, release, body) -> Word;
    /// An operator over a `numeric` or `integer` type parameter: `?numeric_binary(op, witness, x,
    /// y)`, its operands read as the type the witness names. No source can spell it; the
    /// elaboration writes it.
    NumericBinary = "?numeric_binary" (op, witness, x, y) -> Word;
    NumericUnary = "?numeric_unary" (op, witness, x) -> Word;
    /// `numeric_of_int(n)`, called with the witness the elaboration passes first.
    NumericOfInt = "numeric_of_int" (witness, n) -> Raising;
    /// `to_int(x)`, called with the witness the elaboration passes first.
    ToInt = "to_int" (witness, x) -> Word;
    FloatOfInt = "Float::of_int" (n) -> Word;
    /// `None` for a NaN, an infinity, or a value past `Int` once rounded.
    IntOfFloat = "Int::of_float" (x, mode) -> Word;
    Floor = "Float::floor" (x) -> Word;
    Ceil = "Float::ceil" (x) -> Word;
    Round = "Float::round" (x, mode) -> Word;
    Sqrt = "Float::sqrt" (x) -> Word;
    Pow = "Float::pow" (x, y) -> Word;
    Exp = "Float::exp" (x) -> Word;
    Ln = "Float::ln" (x) -> Word;
    Log2 = "Float::log2" (x) -> Word;
    Log10 = "Float::log10" (x) -> Word;
    Sin = "Float::sin" (x) -> Word;
    Cos = "Float::cos" (x) -> Word;
    Tan = "Float::tan" (x) -> Word;
    Asin = "Float::asin" (x) -> Word;
    Acos = "Float::acos" (x) -> Word;
    Atan = "Float::atan" (x) -> Word;
    Atan2 = "Float::atan2" (y, x) -> Word;
    Hypot = "Float::hypot" (x, y) -> Word;
    /// What a hold holds, while its region has not let it go.
    HoldGet = "Hold::get" (h) -> Word;
    /// A hold's contents moved out for its release, so the next read finds them gone. No source
    /// can spell it; the lowering of `with_hold` writes it.
    HoldTake = "?hold_take" (h) -> Word;
    /// [`Builtin::Bracket`] under a name no module rebinds, for the lowering of `with_hold`.
    HoldBracket = "?hold_bracket" (acquire, release, body) -> Word;
    /// Bytes as a credential, and what is made of one without opening it.
    SecretOfBytes = "Secret::of_bytes" (b) -> Word;
    SecretBytes = "Secret::bytes" (s) -> Word;
    SecretLen = "Secret::len" (s) -> Word;
    SecretConcat = "Secret::concat" (a, b) -> Word;
    /// `Secret::decode(encoding, text)`: `None` for a text the encoding does not write.
    SecretDecode = "Secret::decode" (encoding, text) -> Raising;
    /// The private key a DER document holds, with the scheme it is a key of.
    SecretPrivateKey = "Secret::private_key" (der) -> Word;
    /// Keyed hashing under a credential; `None` for a hash the runtime does not key.
    SecretHmac = "Secret::hmac" (hash, key, message) -> Word;
    SecretHkdfExtract = "Secret::hkdf_extract" (hash, salt, key) -> Word;
    SecretHkdfExpand = "Secret::hkdf_expand" (hash, key, info, length) -> Word;
    SecretPbkdf2 = "Secret::pbkdf2" (hash, password, salt, iterations, length) -> Word;
    /// `crypto_seal(cipher, key, nonce, associated, plaintext)` and its inverse.
    CryptoSeal = "crypto_seal" (cipher, key, nonce, associated, plaintext) -> Raising;
    CryptoOpen = "crypto_open" (cipher, key, nonce, associated, sealed) -> Raising;
    CryptoPublic = "crypto_public" (scheme, key) -> Raising;
    CryptoSign = "crypto_sign" (scheme, key, message) -> Raising;
    CryptoVerify = "crypto_verify" (scheme, public, message, signature) -> Raising;
    CryptoAgree = "crypto_agree" (scheme, key, peer) -> Raising;
    /// `sqlite_run(image, sql, params, reads_only, strict, limits)`: one statement over the
    /// database an image holds, answering the image it leaves.
    SqliteRun = "sqlite_run" (image, sql, params, reads_only, strict, limits) -> Word;
    SqliteFunctions = "sqlite_functions" () -> Word;
    /// `Secret::encode(encoding, bytes)`: the text a credential's bytes are in an encoding, sealed.
    SecretEncode = "Secret::encode" (encoding, bytes) -> Raising;
    /// `Secret::rsa_jwk(n, e, [d, p, q, dp, dq, qi])`: the PKCS #1 document an RSA JSON Web Key's
    /// numbers make, each private one its base64url text sealed; `None` for numbers that are no key.
    SecretRsaJwk = "Secret::rsa_jwk" (n, e, private) -> Raising;
    /// Variable `i` of the `handle` whose body starts, given its first value. No source can spell
    /// these three; the lowering of a handler's `var`s writes them.
    HandlerInit = "?handler_init" (i, x) -> Word;
    /// Variable `i` of the handler whose clause runs.
    HandlerVar = "?handler_var" (i) -> Word;
    /// Variable `i` of the handler whose clause runs, given a new value.
    HandlerSet = "?handler_set" (i, x) -> Word;
    /// A hold of what it is given, which a bracket's release lets go. No source can spell it; the
    /// lowering of `with_hold` writes it.
    HoldNew = "?hold_new" (x) -> Word;
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

/// `Array::set` or `Columns::set`, taking the array out of its arguments so the last holder writes in
/// place.
fn array_set(args: &mut Vec<Value>, span: Span, what: &str) -> Result<Value, Diagnostic> {
    let v = args.pop().expect("arity checked");
    let index = args.pop().expect("arity checked");
    let mut xs = args.pop().expect("arity checked");
    let Value::Array(items) = &mut xs else {
        return Err(type_error(span, &format!("`{what}`"), "Array", &xs));
    };
    let i = index.as_int(span, &format!("`{what}`"))?;
    let Some(at) = usize::try_from(i).ok().filter(|at| *at < items.len()) else {
        return Err(out_of_range(span, what, i, items.len(), "elements"));
    };
    let in_place = std::sync::Arc::get_mut(items).is_some();
    crate::rc::note_update_of(in_place, if in_place { 0 } else { items.len() }, span);
    std::sync::Arc::make_mut(items)[at] = v;
    Ok(xs)
}

/// A builtin over values. One that calls back into the program, or reaches a hold or a handler's
/// variables, is the compiled backend's to answer, since only it holds them.
pub fn call(b: Builtin, mut args: Vec<Value>, span: Span) -> Result<Value, Diagnostic> {
    let out = call_with(b, &mut args, span);
    args.clear();
    crate::argv::give(args);
    out
}

fn call_with(b: Builtin, args: &mut Vec<Value>, span: Span) -> Result<Value, Diagnostic> {
    if args.len() != b.words() {
        return Err(arity_error(
            span,
            &format!("`{}`", b.name()),
            b.words(),
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

        Builtin::AssertFailed => Err(assertion_failure(&args[0], &args[1], span)),

        Builtin::Len => match &args[0] {
            Value::List(xs) => Ok(Value::Int(xs.len() as i64)),
            Value::Str(s) => Ok(Value::Int(s.chars().count() as i64)),
            other => Err(type_error(span, "`List::len`", "a List or String", other)),
        },

        Builtin::WithCapacity => {
            args[0].as_int(span, "`List::with_capacity`")?;
            Ok(Value::list(Vec::new()))
        }

        // Compiled code searches in place and calls this only to raise for a start past the end.
        Builtin::BytesPosition => {
            let bytes = args[0].as_bytes(span, "`Bytes::position`")?;
            start_at(&args[1], bytes.len(), span, "Bytes::position")?;
            Err(answered_by_the_backend(b, span))
        }

        Builtin::Iterate
        | Builtin::Metered
        | Builtin::Bracket
        | Builtin::HoldGet
        | Builtin::HoldTake
        | Builtin::HoldBracket
        | Builtin::HandlerInit
        | Builtin::HandlerVar
        | Builtin::HandlerSet
        | Builtin::HoldNew
        | Builtin::ListLeafOf => Err(answered_by_the_backend(b, span)),

        Builtin::Range => {
            let lo = args[0].as_int(span, "`range`")?;
            let hi = args[1].as_int(span, "`range`")?;
            if hi <= lo {
                return Ok(Value::list(Vec::new()));
            }
            if hi.saturating_sub(lo) > MAX_RANGE_LEN {
                return Err(range_too_long(span, lo, hi));
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
                Err(_) => Err(byte_of_int_refused(span, n)),
            }
        }

        Builtin::IntToString => Ok(Value::str(
            args[0].as_int(span, "`Int::to_string`")?.to_string(),
        )),

        // The language's own float spelling, the one a diagnostic prints: shortest round-trip,
        // and `Infinity`/`NaN` rather than the `inf`/`NaN` Rust would write.
        Builtin::FloatToString => Ok(Value::str(crate::render_float(
            args[0].as_float(span, "`Float::to_string`")?,
        ))),

        Builtin::StringConcat => {
            let a = args[0].as_str(span, "`String::concat`")?;
            let b = args[1].as_str(span, "`String::concat`")?;
            Ok(Value::str(format!("{a}{b}")))
        }

        Builtin::BytesLen => {
            let b = args[0].as_bytes(span, "`Bytes::len`")?;
            Ok(Value::Int(b.len() as i64))
        }

        Builtin::BytesAt => {
            let b = args[0].as_bytes(span, "`Bytes::at`")?;
            let i = args[1].as_int(span, "`Bytes::at`")?;
            match usize::try_from(i).ok().and_then(|i| b.get(i)) {
                Some(byte) => Ok(Value::Int(i64::from(*byte))),
                None => Err(out_of_range(span, "Bytes::at", i, b.len(), "bytes")),
            }
        }

        Builtin::BytesU32Le => {
            let b = args[0].as_bytes(span, "`Bytes::u32_le`")?;
            let i = args[1].as_int(span, "`Bytes::u32_le`")?;
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
                None => Err(out_of_range(span, "Bytes::u32_le", i + 3, b.len(), "bytes")),
            }
        }

        Builtin::BytesSlice => {
            let b = args[0].as_bytes(span, "`Bytes::slice`")?;
            let (start, end) =
                range_args(&args[1], &args[2], b.len(), span, "Bytes::slice", "bytes")?;
            Ok(Value::bytes(&b[start..end]))
        }

        Builtin::BytesConcat => {
            let a = args[0].as_bytes(span, "`Bytes::concat`")?;
            let b = args[1].as_bytes(span, "`Bytes::concat`")?;
            let mut out = Vec::with_capacity(a.len() + b.len());
            out.extend_from_slice(a);
            out.extend_from_slice(b);
            Ok(Value::bytes(out))
        }

        Builtin::BytesBlake3 => {
            let b = args[0].as_bytes(span, "`Bytes::blake3`")?;
            Ok(Value::bytes(blake3::hash(b).as_bytes()))
        }

        // At the best level, so what a writer seals is a function of the bytes alone.
        Builtin::BytesGzip => {
            use std::io::Write;
            let b = args[0].as_bytes(span, "`Bytes::gzip`")?;
            let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
            let sealed = gz.write_all(b).and_then(|()| gz.finish());
            Ok(Value::bytes(sealed.expect("writing to memory cannot fail")))
        }

        Builtin::BytesGunzip => {
            use std::io::Read;
            let b = args[0].as_bytes(span, "`Bytes::gunzip`")?;
            let mut out = Vec::new();
            let read = flate2::read::GzDecoder::new(&b[..]).read_to_end(&mut out);
            Ok(option(read.ok().map(|_| Value::bytes(out))))
        }

        // One pass, so the frame says how long it is and a reader's window is no larger.
        Builtin::BytesZstd => {
            let b = args[0].as_bytes(span, "`Bytes::zstd`")?;
            let sealed = zstd::bulk::Compressor::new(3).and_then(|mut z| {
                z.set_parameter(zstd::zstd_safe::CParameter::ChecksumFlag(true))?;
                z.compress(b)
            });
            Ok(Value::bytes(
                sealed.expect("compressing in memory cannot fail"),
            ))
        }

        Builtin::BytesConcatAll => {
            let pieces = args[0].as_list(span, "`Bytes::concat_all`")?.clone();
            let mut total = 0usize;
            for piece in pieces.iter() {
                total += piece.as_bytes(span, "`Bytes::concat_all`")?.len();
            }
            let mut out = Vec::with_capacity(total);
            for piece in pieces.iter() {
                out.extend_from_slice(piece.as_bytes(span, "`Bytes::concat_all`")?);
            }
            Ok(Value::bytes(out))
        }

        Builtin::BytesOfString => {
            let s = args[0].as_str(span, "`Bytes::of_string`")?;
            Ok(Value::bytes(s.as_bytes()))
        }

        Builtin::BytesIsUtf8 => {
            let b = args[0].as_bytes(span, "`Bytes::is_utf8`")?;
            Ok(Value::Bool(std::str::from_utf8(b).is_ok()))
        }

        Builtin::BytesIndexOf => {
            let hay = args[0].as_bytes(span, "`Bytes::index_of`")?;
            let needle = args[1].as_bytes(span, "`Bytes::index_of`")?;
            Ok(position(find(hay, needle, 0)))
        }

        Builtin::BytesIndexOfFrom => {
            let hay = args[0].as_bytes(span, "`Bytes::index_of_from`")?;
            let needle = args[1].as_bytes(span, "`Bytes::index_of_from`")?;
            let from = start_at(&args[2], hay.len(), span, "Bytes::index_of_from")?;
            Ok(position(find(hay, needle, from)))
        }

        Builtin::BytesIndexOfByte => {
            let hay = args[0].as_bytes(span, "`Bytes::index_of_byte`")?;
            let byte = one_byte(&args[1], span, "Bytes::index_of_byte")?;
            Ok(position(memchr::memchr(byte, hay)))
        }

        Builtin::BytesStartsWith => {
            let b = args[0].as_bytes(span, "`Bytes::starts_with`")?;
            let prefix = args[1].as_bytes(span, "`Bytes::starts_with`")?;
            Ok(Value::Bool(b.starts_with(prefix)))
        }

        Builtin::BytesEndsWith => {
            let b = args[0].as_bytes(span, "`Bytes::ends_with`")?;
            let suffix = args[1].as_bytes(span, "`Bytes::ends_with`")?;
            Ok(Value::Bool(b.ends_with(suffix)))
        }

        Builtin::BytesSplit => {
            let b = args[0].as_bytes(span, "`Bytes::split`")?;
            let sep = args[1].as_bytes(span, "`Bytes::split`")?;
            if sep.is_empty() {
                return Err(empty_separator(span, Builtin::BytesSplit));
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
            let b = args[0].as_bytes(span, "`Bytes::scan`")?;
            Ok(Value::Int(scan(args, b, span, false)?))
        }

        Builtin::BytesScanUntil => {
            let b = args[0].as_bytes(span, "`Bytes::scan_until`")?;
            Ok(Value::Int(scan(args, b, span, true)?))
        }

        Builtin::StringOfBytes => {
            let b = args[0].as_bytes(span, "`String::of_bytes`")?;
            match std::str::from_utf8(b) {
                Ok(s) => Ok(Value::str(s)),
                Err(e) => Err(not_utf8(span, b, &e)),
            }
        }

        Builtin::StringOfBytesLossy => {
            let b = args[0].as_bytes(span, "`String::of_bytes_lossy`")?;
            Ok(Value::str(String::from_utf8_lossy(b)))
        }

        Builtin::CharOfInt => {
            let n = args[0].as_int(span, "`Char::of_int`")?;
            Ok(option(
                u32::try_from(n)
                    .ok()
                    .and_then(char::from_u32)
                    .map(Value::Char),
            ))
        }

        Builtin::IntOfChar => Ok(Value::Int(i64::from(u32::from(
            args[0].as_char(span, "`Int::of_char`")?,
        )))),

        Builtin::StringChars => Ok(Value::list(
            args[0]
                .as_str(span, "`String::chars`")?
                .chars()
                .map(Value::Char)
                .collect(),
        )),

        Builtin::StringOfChars => {
            let items = args[0].as_list(span, "`String::of_chars`")?;
            let mut out = String::with_capacity(items.len());
            for c in items.iter() {
                out.push(c.as_char(span, "`String::of_chars`")?);
            }
            Ok(Value::str(out))
        }

        Builtin::StringLen => Ok(Value::Int(
            args[0].as_str(span, "`String::len`")?.chars().count() as i64,
        )),

        Builtin::ArrayNew | Builtin::ColumnsNew => {
            let n = args[0].as_int(span, &format!("`{}`", b.name()))?;
            if !(0..=MAX_ARRAY_LEN).contains(&n) {
                return Err(array_length(span, b.name(), n));
            }
            Ok(Value::array(vec![args[1].clone(); n as usize]))
        }

        Builtin::ArrayOfList | Builtin::ColumnsOfList => Ok(Value::array(
            args[0]
                .as_list(span, &format!("`{}`", b.name()))?
                .iter()
                .cloned()
                .collect(),
        )),

        Builtin::ArrayLen | Builtin::ColumnsLen => Ok(Value::Int(
            args[0].as_array(span, &format!("`{}`", b.name()))?.len() as i64,
        )),

        Builtin::ArrayAt | Builtin::ColumnsAt => {
            let xs = args[0].as_array(span, &format!("`{}`", b.name()))?;
            let i = args[1].as_int(span, &format!("`{}`", b.name()))?;
            Ok(option(
                usize::try_from(i).ok().and_then(|i| xs.get(i)).cloned(),
            ))
        }

        Builtin::ArrayGet | Builtin::ColumnsGet => {
            let xs = args[0].as_array(span, &format!("`{}`", b.name()))?;
            let i = args[1].as_int(span, &format!("`{}`", b.name()))?;
            match usize::try_from(i).ok().and_then(|at| xs.get(at)) {
                Some(x) => Ok(x.clone()),
                None => Err(out_of_range(span, b.name(), i, xs.len(), "elements")),
            }
        }

        Builtin::ArraySet | Builtin::ColumnsSet => array_set(args, span, b.name()),

        Builtin::Reflect => Ok(crate::reflect::value_of(&Plain::of(&args[0]))),

        Builtin::Shown => Ok(crate::reflect::shown_of(&args[0])),

        Builtin::Digest => Ok(Value::bytes(crate::digest::digest(&args[0], span)?)),
        Builtin::Encoded => Ok(Value::bytes(&crate::digest::encoded(&args[0], span)?)),

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
        Builtin::FloatOfInt => Ok(Value::Float(args[0].as_int(span, "`Float::of_int`")? as f64)),
        Builtin::IntOfFloat => {
            let x = args[0].as_float(span, "`Int::of_float`")?;
            let r = rounded(x, rounding(&args[1], span, "Int::of_float")?);
            Ok(option(int_of_whole(r).map(Value::Int)))
        }
        Builtin::Round => {
            let x = args[0].as_float(span, "`Float::round`")?;
            Ok(Value::Float(rounded(x, rounding(&args[1], span, "round")?)))
        }
        Builtin::Floor => unary_float(&args[0], span, "`Float::floor`", f64::floor),
        Builtin::Ceil => unary_float(&args[0], span, "`Float::ceil`", f64::ceil),
        Builtin::Sqrt => unary_float(&args[0], span, "`Float::sqrt`", f64::sqrt),
        Builtin::Exp => unary_float(&args[0], span, "`Float::exp`", float::exp),
        Builtin::Ln => unary_float(&args[0], span, "`Float::ln`", float::ln),
        Builtin::Log2 => unary_float(&args[0], span, "`Float::log2`", float::log2),
        Builtin::Log10 => unary_float(&args[0], span, "`Float::log10`", float::log10),
        Builtin::Sin => unary_float(&args[0], span, "`Float::sin`", float::sin),
        Builtin::Cos => unary_float(&args[0], span, "`Float::cos`", float::cos),
        Builtin::Tan => unary_float(&args[0], span, "`Float::tan`", float::tan),
        Builtin::Asin => unary_float(&args[0], span, "`Float::asin`", float::asin),
        Builtin::Acos => unary_float(&args[0], span, "`Float::acos`", float::acos),
        Builtin::Atan => unary_float(&args[0], span, "`Float::atan`", float::atan),
        Builtin::Pow => binary_float(args, span, "`Float::pow`", float::pow),
        Builtin::Atan2 => binary_float(args, span, "`Float::atan2`", float::atan2),
        Builtin::Hypot => binary_float(args, span, "`Float::hypot`", float::hypot),

        Builtin::StringSlice => {
            let s = args[0].as_str(span, "`String::slice`")?;
            let chars = s.chars().count();
            let (start, end) = range_args(
                &args[1],
                &args[2],
                chars,
                span,
                "String::slice",
                "characters",
            )?;
            let from = char_offset(s, start);
            let to = char_offset(s, end);
            Ok(Value::str(&s[from..to]))
        }

        Builtin::StringSplit => {
            let s = args[0].as_str(span, "`String::split`")?;
            let sep = args[1].as_str(span, "`String::split`")?;
            if sep.is_empty() {
                return Err(empty_separator(span, Builtin::StringSplit));
            }
            Ok(Value::list(s.split(sep).map(Value::str).collect()))
        }

        // These three read `std`'s Unicode tables, so their answers move if those tables do.
        Builtin::StringTrim => Ok(Value::str(args[0].as_str(span, "`String::trim`")?.trim())),

        Builtin::StringLower => Ok(Value::str(
            args[0].as_str(span, "`String::lower`")?.to_lowercase(),
        )),

        Builtin::StringUpper => Ok(Value::str(
            args[0].as_str(span, "`String::upper`")?.to_uppercase(),
        )),

        Builtin::StringStartsWith => {
            let s = args[0].as_str(span, "`String::starts_with`")?;
            let prefix = args[1].as_str(span, "`String::starts_with`")?;
            Ok(Value::Bool(s.starts_with(prefix)))
        }

        Builtin::StringEndsWith => {
            let s = args[0].as_str(span, "`String::ends_with`")?;
            let suffix = args[1].as_str(span, "`String::ends_with`")?;
            Ok(Value::Bool(s.ends_with(suffix)))
        }

        Builtin::StringContains => {
            let s = args[0].as_str(span, "`String::contains`")?;
            let needle = args[1].as_str(span, "`String::contains`")?;
            Ok(Value::Bool(s.contains(needle)))
        }

        Builtin::StringFind => {
            let s = args[0].as_str(span, "`String::find`")?;
            let needle = args[1].as_str(span, "`String::find`")?;
            match s.find(needle) {
                Some(at) => Ok(Value::Int(s[..at].chars().count() as i64)),
                None => Err(not_found(span, s, needle)),
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

        Builtin::MapNew => Ok(map::new()),
        Builtin::MapLen => Ok(map::len(&args[0], span)?),

        Builtin::DecimalDiv => {
            let a = args[0].as_decimal(span, "`Decimal::div`")?;
            let b = args[1].as_decimal(span, "`Decimal::div`")?;
            let scale = decimal_scale(&args[2], span, "Decimal::div")?;
            let mode = rounding(&args[3], span, "Decimal::div")?;
            if b.is_zero() {
                return Err(crate::semantics::err_zero_divisor(span, "`Decimal::div`"));
            }
            decimal_quotient(a, b, scale, mode)
                .map(Value::Decimal)
                .ok_or_else(|| decimal_overflow(span, "division"))
        }

        Builtin::DecimalRound => {
            let d = args[0].as_decimal(span, "`Decimal::round`")?;
            let scale = decimal_scale(&args[1], span, "Decimal::round")?;
            let mode = rounding(&args[2], span, "Decimal::round")?;
            decimal_quotient(d, Decimal::ONE, scale, mode)
                .map(Value::Decimal)
                .ok_or_else(|| decimal_overflow(span, "rounding"))
        }

        Builtin::DecimalOfInt => Ok(Value::Decimal(Decimal::from(
            args[0].as_int(span, "`Decimal::of_int`")?,
        ))),

        Builtin::IntOfDecimal => {
            let d = args[0].as_decimal(span, "`Int::of_decimal`")?;
            let mode = rounding(&args[1], span, "Int::of_decimal")?;
            Ok(option(
                d.round_dp_with_strategy(0, mode).to_i64().map(Value::Int),
            ))
        }

        Builtin::FloatOfDecimal => {
            let d = args[0].as_decimal(span, "`Float::of_decimal`")?;
            Ok(Value::Float(float_of_decimal(d)))
        }

        Builtin::DecimalOfFloat => {
            let f = args[0].as_float(span, "`Decimal::of_float`")?;
            Ok(option(decimal_of_float(f).map(Value::Decimal)))
        }

        Builtin::DecimalOfString => {
            let s = args[0].as_str(span, "`Decimal::of_string`")?;
            Ok(option(parse_decimal(s).map(Value::Decimal)))
        }

        Builtin::FloatOfString => {
            let s = args[0].as_str(span, "`Float::of_string`")?;
            Ok(option(parse_float(s).map(Value::Float)))
        }

        // Keeps the scale (`1.50m` renders `1.50`), so it round-trips `Decimal::of_string`.
        Builtin::DecimalToString => {
            let d = args[0].as_decimal(span, "`Decimal::to_string`")?;
            Ok(Value::str(d.to_string()))
        }

        Builtin::DecimalScale => {
            let d = args[0].as_decimal(span, "`Decimal::scale`")?;
            Ok(Value::Int(i64::from(d.scale())))
        }

        Builtin::BitsOfFloat => {
            let f = args[0].as_float(span, "`bits_of_float`")?;
            Ok(Value::Int(float::bits(f) as i64))
        }

        Builtin::FloatOfBits => {
            let n = args[0].as_int(span, "`Float::of_bits`")?;
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

        Builtin::SecretOfString => Ok(Value::secret_text(
            args[0].as_str(span, "`Secret::of_string`")?,
        )),

        // Constant time but not rate limited: preventing a guessing loop is the program's job.
        Builtin::SecretVerify => {
            let Value::Secret(held) = &args[0] else {
                return Err(type_error(span, "`Secret::verify`", "Secret", &args[0]));
            };
            let candidate = args[1].as_str(span, "`Secret::verify`")?;
            Ok(Value::Bool(
                held.is_text()
                    && crate::value::constant_time_eq(held.bytes(), candidate.as_bytes()),
            ))
        }

        Builtin::SecretIsEmpty => {
            let Value::Secret(held) = &args[0] else {
                return Err(type_error(span, "`Secret::is_empty`", "Secret", &args[0]));
            };
            Ok(Value::Bool(held.bytes().is_empty()))
        }

        Builtin::SecretOfBytes => Ok(Value::secret_bytes(
            args[0].as_bytes(span, "`Secret::of_bytes`")?,
        )),
        Builtin::SecretEncode => crate::crypto::encode(&args[0], &args[1], span),
        Builtin::SecretRsaJwk => crate::crypto::rsa_jwk(args, span),
        Builtin::SecretBytes => crate::crypto::utf8(&args[0], span),
        Builtin::SecretLen => crate::crypto::len(&args[0], span),
        Builtin::SecretConcat => crate::crypto::concat(&args[0], &args[1], span),
        Builtin::SecretDecode => crate::crypto::decode(&args[0], &args[1], span),
        Builtin::SecretPrivateKey => crate::crypto::private_key(&args[0], span),
        Builtin::SecretHmac => crate::crypto::hmac(&args[0], &args[1], &args[2], span),
        Builtin::SecretHkdfExtract => {
            crate::crypto::hkdf_extract(&args[0], &args[1], &args[2], span)
        }
        Builtin::SecretHkdfExpand => {
            crate::crypto::hkdf_expand(&args[0], &args[1], &args[2], &args[3], span)
        }
        Builtin::SecretPbkdf2 => crate::crypto::pbkdf2(args, span),
        Builtin::CryptoSeal => crate::crypto::seal(args, span),
        Builtin::CryptoOpen => crate::crypto::open(args, span),
        Builtin::CryptoPublic => crate::crypto::public(&args[0], &args[1], span),
        Builtin::CryptoSign => crate::crypto::sign(&args[0], &args[1], &args[2], span),
        Builtin::CryptoVerify => crate::crypto::verify(args, span),
        Builtin::CryptoAgree => crate::crypto::agree(&args[0], &args[1], &args[2], span),
        Builtin::SqliteRun => crate::sqlite::run_image(args, span),
        Builtin::SqliteFunctions => Ok(crate::sqlite::functions()),
    }
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

fn unary_float(x: &Value, span: Span, what: &str, f: fn(f64) -> f64) -> Result<Value, Diagnostic> {
    Ok(Value::Float(f(x.as_float(span, what)?)))
}

fn binary_float(
    args: &[Value],
    span: Span,
    what: &str,
    f: fn(f64, f64) -> f64,
) -> Result<Value, Diagnostic> {
    let x = args[0].as_float(span, what)?;
    let y = args[1].as_float(span, what)?;
    Ok(Value::Float(f(x, y)))
}

/// `x` at a whole number, chosen as `mode` chooses a decimal's last digit.
fn rounded(x: f64, mode: RoundingStrategy) -> f64 {
    if !x.is_finite() {
        return x;
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

/// `a / b`, for a `b` that is not zero, with exactly `scale` digits after the point: the exact
/// quotient rounded once, there, by `mode`. `None` where that mantissa is past 96 bits.
///
/// The mantissas are divided as whole numbers, a digit at a time, so no digit past the last one
/// kept is rounded before `mode` reads what is left over.
fn decimal_quotient(a: Decimal, b: Decimal, scale: u32, mode: RoundingStrategy) -> Option<Decimal> {
    use std::cmp::Ordering::{Equal, Greater, Less};
    const LIMIT: u128 = 1 << 96;
    let negative = a.is_sign_negative() != b.is_sign_negative();
    let (dividend, divisor) = (a.mantissa().unsigned_abs(), b.mantissa().unsigned_abs());
    // The answer's mantissa is `dividend * 10^shift / divisor`.
    let shift = i64::from(scale) + i64::from(b.scale()) - i64::from(a.scale());
    let (mut quotient, mut rest) = (dividend / divisor, dividend % divisor);
    // What the quotient leaves out, against half a unit of its last digit.
    let (exact, half) = if shift >= 0 {
        for _ in 0..shift {
            // `rest` is below `divisor`, which is below 2^96, so neither product wraps.
            rest *= 10;
            quotient = quotient * 10 + rest / divisor;
            rest %= divisor;
            if quotient >= LIMIT {
                return None;
            }
        }
        (rest == 0, (rest * 2).cmp(&divisor))
    } else {
        let unit = 10u128.pow(u32::try_from(-shift).ok()?);
        let dropped = quotient % unit;
        quotient /= unit;
        let half = match dropped.cmp(&(unit / 2)) {
            Equal if rest > 0 => Greater,
            other => other,
        };
        (dropped == 0 && rest == 0, half)
    };
    let away = !exact
        && match mode {
            RoundingStrategy::ToZero => false,
            RoundingStrategy::AwayFromZero => true,
            RoundingStrategy::ToPositiveInfinity => !negative,
            RoundingStrategy::ToNegativeInfinity => negative,
            RoundingStrategy::MidpointAwayFromZero => half != Less,
            _ => half == Greater || (half == Equal && quotient % 2 == 1),
        };
    let mantissa = i128::try_from(quotient + u128::from(away)).ok()?;
    Decimal::try_from_i128_with_scale(if negative { -mantissa } else { mantissa }, scale).ok()
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

/// The nearest `f64`, so `Float::of_decimal(decimal_of_float(f)) == f` wherever defined.
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
        "Bytes::scan_until"
    } else {
        "Bytes::scan"
    };
    let from = start_at(&args[1], hay.len(), span, what)?;
    let members = bytes_arg(&args[2], span, what)?;
    let max = budget(&args[3], span, what)?;
    let window = scan_window(hay, from, max);

    // `memchr` is SIMD and the bitmap loop is not, so small sets take it.
    let found = match (want, members.as_ref()) {
        // Empty class: `Bytes::scan_until` runs out the window, `Bytes::scan` stops at once.
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
    usize::try_from(max).map_err(|_| negative_budget(span, what, max))
}

/// `what`, a scan, given a budget `max` below 0.
#[cold]
pub fn negative_budget(span: Span, what: &str, max: i64) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`{what}` was given a negative budget of {max}"),
    )
    .primary(span, "a scan cannot examine a negative number of bytes")
    .note("pass `0` to examine nothing, or `Bytes::len(b)` to leave it unbounded")
}

fn one_byte(v: &Value, span: Span, what: &str) -> Result<u8, Diagnostic> {
    let byte = int_arg(v, span, what)?;
    u8::try_from(byte).map_err(|_| not_a_byte(span, what, byte))
}

/// `what` given `byte` to search for, which is no byte.
#[cold]
pub fn not_a_byte(span: Span, what: &str, byte: i64) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`{what}` was given {byte}, which is not a byte"),
    )
    .primary(span, "a byte is `0` to `255`")
    .note("`Bytes::at` answers in that range, and so does a byte literal's element")
}

/// `byte_of_int` given `n`, which is no byte.
#[cold]
pub fn byte_of_int_refused(span: Span, n: i64) -> Diagnostic {
    Diagnostic::error(codes::RUNTIME_ERROR, format!("`byte_of_int` was given {n}"))
        .primary(span, "a byte is 0 to 255")
        .note("mask the value before the call if that is what you meant: `n & 0xFF`")
}

/// `Bytes::split` or `String::split`, as `split` names it, given an empty separator.
#[cold]
pub fn empty_separator(span: Span, split: Builtin) -> Diagnostic {
    let example = if split == Builtin::StringSplit {
        "pass the text that actually separates the parts, as in \"\\r\\n\""
    } else {
        "pass the bytes that actually separate the parts, as in `b\"\\r\\n\"`"
    };
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!(
            "`{}` needs a separator, and this one is empty",
            split.name()
        ),
    )
    .primary(span, "an empty separator matches everywhere and nowhere")
    .note(example)
}

/// `range(lo, hi)` holding more than the most items a range materializes.
#[cold]
pub fn range_too_long(span: Span, lo: i64, hi: i64) -> Diagnostic {
    let len = hi.saturating_sub(lo);
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`range` of {len} elements exceeds the limit of {MAX_RANGE_LEN}"),
    )
    .primary(span, "this range is too large to materialize")
}

/// `String::find` not finding `needle` in `s`.
#[cold]
pub fn not_found(span: Span, s: &str, needle: &str) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`String::find` did not find {} in {}", slot(0), slot(1)),
    )
    .primary(span, "this substring does not occur")
    .note("guard with `String::contains`, which answers the same question as a `Bool`")
    .showing(vec![
        Plain::Str(needle.to_string()),
        Plain::Str(s.to_string()),
    ])
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
        return Err(range_outside(span, what, start, end, len, unit));
    }
    Ok((start as usize, end as usize))
}

/// `what`, a slicing builtin, given the range `start..end` of a value of `len` `unit`, which does not
/// satisfy `0 <= start <= end <= len`.
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

/// The byte offset of the `n`-th character boundary.
fn char_offset(s: &str, n: usize) -> usize {
    s.char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(s.len()))
        .nth(n)
        .unwrap_or(s.len())
}

/// `String::of_bytes` given the bytes `b`, which `e` says are not UTF-8.
#[cold]
pub fn not_utf8(span: Span, b: &[u8], e: &std::str::Utf8Error) -> Diagnostic {
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
        format!("`String::of_bytes` was given bytes that are not UTF-8: {what}"),
    )
    .primary(span, format!("byte {at} of {} is where it fails", b.len()))
    .note("guard with `Bytes::is_utf8`, or use `String::of_bytes_lossy` to accept U+FFFD")
}

#[cold]
pub fn array_length(span: Span, what: &str, n: i64) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`{what}` of {n} elements is outside `0` to `{MAX_ARRAY_LEN}`"),
    )
    .primary(span, "no array has this length")
}

/// `what` was given `index` into a value of `len` `unit`, which has no such index.
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

/// A hold reached after its region let it go, which only a continuation captured inside the region
/// and resumed after the release can do.
#[cold]
pub fn hold_released(span: Span, doing: &str) -> Diagnostic {
    Diagnostic::error(
        codes::HOLD_RELEASED,
        format!("a hold is {doing} after its region released what it held"),
    )
    .primary(span, "the hold holds nothing now")
    .note("a `with_hold` releases what it holds once, when its body first ends; a continuation captured inside the body and resumed after that runs the rest of the body against a hold already let go")
    .note("resume a continuation captured inside a `with_hold` at most once, or open the `with_hold` after the choice the continuation replays")
}

/// The C backend answers every builtin that calls back into the program or reaches what only it
/// holds, a list's trie nodes among them.
#[cold]
fn answered_by_the_backend(b: Builtin, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!(
            "`{}` reached the value builtins, and only compiled code can answer it",
            b.name()
        ),
    )
    .primary(
        span,
        "it calls back into the program or reaches what only compiled code holds",
    )
    .note("this is Ply's fault: the C backend answers this builtin over its own words")
}
