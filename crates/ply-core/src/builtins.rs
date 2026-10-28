//! The prelude, in one definition per builtin.

use crate::codec::Writer;
use crate::crypto::over_values;
use crate::semantics::arity_error;
use crate::value::{Decimal, Fixed, FixedOp, Value, type_error};
use crate::{BinOp, Diagnostic, INT_TYPES, IntTy, Span, codes, slot};

/// A list this long is a runaway `range`, not an intent.
const MAX_RANGE_LEN: i64 = 10_000_000;

/// An array this long is a runaway length, not an intent: half a gigabyte of words.
pub const MAX_ARRAY_LEN: i64 = 1 << 26;

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
    WrapAdd = "wrap_add" (a, b) -> Word;
    WrapSub = "wrap_sub" (a, b) -> Word;
    WrapMul = "wrap_mul" (a, b) -> Word;
    Rotr = "rotr" (x, n) -> Word;
    Rotl = "rotl" (x, n) -> Word;
    /// Over any integer type: the exact answer, or the type's nearest value where it leaves it.
    SaturatingAdd = "saturating_add" (a, b) -> Word;
    SaturatingSub = "saturating_sub" (a, b) -> Word;
    SaturatingMul = "saturating_mul" (a, b) -> Word;
    IntOfU8 = "Int::of_u8" (n) -> Word;
    IntOfU16 = "Int::of_u16" (n) -> Word;
    IntOfU32 = "Int::of_u32" (n) -> Word;
    IntOfU64 = "Int::of_u64" (n) -> Raising;
    IntOfI8 = "Int::of_i8" (n) -> Word;
    IntOfI16 = "Int::of_i16" (n) -> Word;
    IntOfI32 = "Int::of_i32" (n) -> Word;
    IntOfI64 = "Int::of_i64" (n) -> Word;
    Compare = "compare" (x, y) -> Word;
    /// The same order as [`Builtin::Compare`], under a name a module may not declare.
    CompareValues = "compare_values" (x, y) -> Word;
    Panic = "panic" (message) -> Raising;
    /// The identity a benchmark pins a measured value with: opaque, so it is not optimized away.
    Observe = "observe" (value) -> Word;
    SecretOfString = "Secret::of_string" (s) -> Word;
    SecretVerify = "Secret::verify" (stored, supplied) -> Word;
    IntOfU128 = "Int::of_u128" (n) -> Raising;
    IntOfI128 = "Int::of_i128" (n) -> Raising;
    /// Over any integer type: the exact answer, or `None` where it leaves the type.
    CheckedAdd = "checked_add" (a, b) -> Word;
    CheckedSub = "checked_sub" (a, b) -> Word;
    CheckedMul = "checked_mul" (a, b) -> Word;
    CheckedNeg = "checked_neg" (a) -> Word;
    ArrayOfList = "Array::of_list" (xs) -> Word;
    ArrayLen = "Array::len" (xs) -> Word;
    ArrayAt = "Array::at" (xs, i) -> Word;
    /// Raises where `Array::at` answers `None`.
    ArrayGet = "Array::get" (xs, i) -> Raising;
    ColumnsOfList = "Columns::of_list" (xs) -> Word;
    ColumnsLen = "Columns::len" (xs) -> Word;
    ColumnsAt = "Columns::at" (xs, i) -> Word;
    ColumnsGet = "Columns::get" (xs, i) -> Raising;
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
    /// An operator over a `numeric` or `integer` type parameter: `?numeric_binary(op, witness, x,
    /// y)`, its operands read as the type the witness names. No source can spell it; the
    /// elaboration writes it.
    NumericBinary = "?numeric_binary" (op, witness, x, y) -> Word;
    NumericUnary = "?numeric_unary" (op, witness, x) -> Word;
    /// `numeric_of_int(n)`, called with the witness the elaboration passes first.
    NumericOfInt = "numeric_of_int" (witness, n) -> Raising;
    /// `to_int(x)`, called with the witness the elaboration passes first.
    ToInt = "to_int" (witness, x) -> Word;
    /// Bytes as a credential, and what is made of one without opening it.
    SecretOfBytes = "Secret::of_bytes" (b) -> Word;
    SecretBytes = "Secret::bytes" (s) -> Word;
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
    /// `Secret::encode(encoding, bytes)`: the text a credential's bytes are in an encoding, sealed.
    SecretEncode = "Secret::encode" (encoding, bytes) -> Raising;
    /// `Secret::rsa_jwk(n, e, [d, p, q, dp, dq, qi])`: the PKCS #1 document an RSA JSON Web Key's
    /// numbers make, each private one its base64url text sealed; `None` for numbers that are no key.
    SecretRsaJwk = "Secret::rsa_jwk" (n, e, private) -> Raising;
}

impl Builtin {
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

/// A builtin over values. One that calls back into the program, or reaches a hold or a handler's
/// variables, is the compiled backend's to answer, since only it holds them.
pub fn call(b: Builtin, mut args: Vec<Value>, span: Span) -> Result<Value, Diagnostic> {
    let out = call_with(b, &mut args, span);
    args.clear();
    crate::argv::give(args);
    out
}

fn call_with(b: Builtin, args: &mut [Value], span: Span) -> Result<Value, Diagnostic> {
    if args.len() != b.words() {
        return Err(arity_error(
            span,
            &format!("`{}`", b.name()),
            b.words(),
            args.len(),
        ));
    }

    match b {
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
                .showing(vec![fixed_shown(f)])),
            }
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

        Builtin::Reflect => Ok(crate::reflect::value_of(&args[0])),

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

        Builtin::Observe => {
            // The identity; its work is the emitter's, which makes the call opaque so the C
            // compiler cannot drop a pure computation whose value is only observed.
            Ok(args[0].clone())
        }

        Builtin::Panic => {
            let message = args[0].as_str(span, "`panic`")?;
            Err(
                Diagnostic::error(codes::RUNTIME_ERROR, format!("panic: {message}"))
                    .primary(span, "`panic` called here"),
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

        Builtin::SecretOfBytes => Ok(Value::secret_bytes(
            args[0].as_bytes(span, "`Secret::of_bytes`")?,
        )),
        Builtin::SecretEncode => over_values::encode(&args[0], &args[1], span),
        Builtin::SecretRsaJwk => over_values::rsa_jwk(args, span),
        Builtin::SecretBytes => over_values::utf8(&args[0], span),
        Builtin::SecretConcat => over_values::concat(&args[0], &args[1], span),
        Builtin::SecretDecode => over_values::decode(&args[0], &args[1], span),
        Builtin::SecretPrivateKey => over_values::private_key(&args[0], span),
        Builtin::SecretHmac => over_values::hmac(&args[0], &args[1], &args[2], span),
        Builtin::SecretHkdfExtract => over_values::hkdf_extract(&args[0], &args[1], &args[2], span),
        Builtin::SecretHkdfExpand => {
            over_values::hkdf_expand(&args[0], &args[1], &args[2], &args[3], span)
        }
        Builtin::SecretPbkdf2 => over_values::pbkdf2(args, span),
        Builtin::CryptoSeal => over_values::seal(args, span),
        Builtin::CryptoOpen => over_values::open(args, span),
        Builtin::CryptoPublic => over_values::public(&args[0], &args[1], span),
        Builtin::CryptoSign => over_values::sign(&args[0], &args[1], &args[2], span),
        Builtin::CryptoVerify => over_values::verify(args, span),
        Builtin::CryptoAgree => over_values::agree(&args[0], &args[1], &args[2], span),
    }
}

fn option(v: Option<Value>) -> Value {
    match v {
        Some(v) => Value::ctor("Some", vec![v]),
        None => Value::ctor("None", Vec::new()),
    }
}

pub fn scan_window(hay: &[u8], from: usize, max: usize) -> &[u8] {
    &hay[from..hay.len().min(from.saturating_add(max))]
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
    .showing(vec![text_shown(needle), text_shown(s)])
}

/// The PLV1 bytes of the `String` `s`, as a diagnostic shows it.
fn text_shown(s: &str) -> Vec<u8> {
    let mut w = Writer::new();
    w.str(s.as_bytes());
    w.finish()
}

/// The PLV1 bytes of the width `f`, as a diagnostic shows it.
pub(crate) fn fixed_shown(f: Fixed) -> Vec<u8> {
    let mut w = Writer::new();
    w.fixed(f);
    w.finish()
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

/// `assert` given a condition that is false, noting `message` where it has one.
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
