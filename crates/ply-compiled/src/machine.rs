//! The natives over machine data that strings and byte strings emitted code lays out reach the
//! runtime through: each takes the bytes it reads as an address and a count, borrowed for the call,
//! and answers raw integers, a `Float`'s bits, a runtime object, or bytes left as the context's
//! taken bytes, which emitted code copies out into a block of its own. None reads or makes a string,
//! a byte string or a list. A native whose builtin can fail answers a value that says so, and
//! emitted code hands the refusal to [`rt_refused`](crate::rt::rt_refused), or to
//! [`rt_refused_text`] where it names text. What each answers is what the value builtin of the same
//! name answers.

use crate::c::{MachineNative, machine_native};
use crate::emitted::Handle;
use crate::heap::{KIND_BRIDGE, bridged, obj};
use crate::native::{Addr, F64, I64, Raises};
use crate::rt::{Ctx, lent, refuse, refused_builtin, refused_unknown};
use ply_core::builtins::{assert_failure, call, not_found, not_utf8};
use ply_core::{Builtin, Decimal, Diagnostic, Fixed, IntTy, Span, Value, codes};

/// Every native here, under the name the compiler declares it by.
pub fn natives() -> Vec<MachineNative> {
    vec![
        machine_native(
            "?utf8_valid",
            rt_utf8_valid as unsafe extern "C" fn(_, _, _) -> _,
        ),
        machine_native(
            "?utf8_lossy",
            rt_utf8_lossy as unsafe extern "C" fn(_, _, _) -> _,
        ),
        machine_native(
            "?str_lower",
            rt_str_lower as unsafe extern "C" fn(_, _, _) -> _,
        ),
        machine_native(
            "?str_upper",
            rt_str_upper as unsafe extern "C" fn(_, _, _) -> _,
        ),
        machine_native(
            "?trim_start",
            rt_trim_start as unsafe extern "C" fn(_, _, _) -> _,
        ),
        machine_native(
            "?trim_end",
            rt_trim_end as unsafe extern "C" fn(_, _, _) -> _,
        ),
        machine_native("?int_text", rt_int_text as unsafe extern "C" fn(_, _) -> _),
        machine_native(
            "?float_text",
            rt_float_text as unsafe extern "C" fn(_, _) -> _,
        ),
        machine_native(
            "?decimal_text",
            rt_decimal_text as unsafe extern "C" fn(_, _, _) -> _,
        ),
        machine_native(
            "?decimal_float",
            rt_decimal_float as unsafe extern "C" fn(_, _, _) -> _,
        ),
        machine_native(
            "?wide_text",
            rt_wide_text as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        machine_native(
            "?float_parse",
            rt_float_parse as unsafe extern "C" fn(_, _, _) -> _,
        ),
        machine_native(
            "?decimal_parse",
            rt_decimal_parse as unsafe extern "C" fn(_, _, _) -> _,
        ),
        machine_native(
            "?wide_parse",
            rt_wide_parse as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        machine_native(
            "?taken_word",
            rt_taken_word as unsafe extern "C" fn(_, _) -> _,
        ),
        machine_native("?blake3", rt_blake3 as unsafe extern "C" fn(_, _, _) -> _),
        machine_native(
            "?scalar_encoded",
            rt_scalar_encoded as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        machine_native(
            "?text_encoded",
            rt_text_encoded as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        machine_native("?gzip", rt_gzip as unsafe extern "C" fn(_, _, _) -> _),
        machine_native("?gunzip", rt_gunzip as unsafe extern "C" fn(_, _, _) -> _),
        machine_native("?zstd", rt_zstd as unsafe extern "C" fn(_, _, _) -> _),
        machine_native(
            "?secret_of",
            rt_secret_of as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        machine_native(
            "?secret_verify",
            rt_secret_verify as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        machine_native(
            "?secret_len",
            rt_secret_len as unsafe extern "C" fn(_, _) -> _,
        ),
        machine_native(
            "?secret_bytes",
            rt_secret_bytes as unsafe extern "C" fn(_, _) -> _,
        ),
        machine_native(
            "?secret_concat",
            rt_secret_concat as unsafe extern "C" fn(_, _, _) -> _,
        ),
        machine_native(
            "?refused_text",
            rt_refused_text as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
    ]
    .into_iter()
    .chain(float_natives())
    .collect()
}

/// Each `Float` function as compiled code calls it: its operands' bits in, its answer's out,
/// computed as the value builtin of the same name computes it.
macro_rules! float_functions {
    ($($name:literal => $f:ident = $($math:ident)::+ ($($a:ident),+);)+) => {
        $(unsafe extern "C" fn $f(_ctx: *mut Ctx, $($a: F64),+) -> F64 {
            F64(ply_core::float::bits($($math)::+($(f64::from_bits($a.0)),+)))
        })+

        fn float_natives() -> Vec<MachineNative> {
            vec![$(machine_native($name, $f as unsafe extern "C" fn(*mut Ctx, $(float_functions!(@f64 $a)),+) -> F64)),+]
        }
    };
    (@f64 $a:ident) => { F64 };
}

float_functions! {
    "?float_floor" => rt_float_floor = f64::floor(x);
    "?float_ceil" => rt_float_ceil = f64::ceil(x);
    "?float_sqrt" => rt_float_sqrt = f64::sqrt(x);
    "?float_exp" => rt_float_exp = ply_core::float::exp(x);
    "?float_ln" => rt_float_ln = ply_core::float::ln(x);
    "?float_log2" => rt_float_log2 = ply_core::float::log2(x);
    "?float_log10" => rt_float_log10 = ply_core::float::log10(x);
    "?float_sin" => rt_float_sin = ply_core::float::sin(x);
    "?float_cos" => rt_float_cos = ply_core::float::cos(x);
    "?float_tan" => rt_float_tan = ply_core::float::tan(x);
    "?float_asin" => rt_float_asin = ply_core::float::asin(x);
    "?float_acos" => rt_float_acos = ply_core::float::acos(x);
    "?float_atan" => rt_float_atan = ply_core::float::atan(x);
    "?float_pow" => rt_float_pow = ply_core::float::pow(x, y);
    "?float_atan2" => rt_float_atan2 = ply_core::float::atan2(y, x);
    "?float_hypot" => rt_float_hypot = ply_core::float::hypot(x, y);
}

#[cold]
#[inline(never)]
fn err_machine(why: &str) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("a string's bytes handed to the runtime: {why}"),
    )
    .primary(Span::DUMMY, "in compiled code")
    .note("this is Ply's fault, not the program's")
}

impl Ctx {
    /// What value builtin `b` answers over `args`; `None` with the context failed where it errs,
    /// which none of the builtins called here does over arguments of their types.
    fn builtin(&mut self, b: Builtin, args: Vec<Value>) -> Option<Value> {
        match call(b, args, Span::DUMMY) {
            Ok(v) => Some(v),
            Err(d) => {
                self.fail(d);
                None
            }
        }
    }

    /// The UTF-8 a string lends, which emitted code only ever makes valid.
    fn text<'a>(&mut self, bytes: &'a [u8]) -> Option<&'a str> {
        match std::str::from_utf8(bytes) {
            Ok(s) => Some(s),
            Err(_) => {
                self.fail(err_machine("a string that is not UTF-8"));
                None
            }
        }
    }

    /// The bytes of the string or byte string builtin `b` answers over `args`, as taken bytes.
    fn taken_of(&mut self, b: Builtin, args: Vec<Value>) -> I64 {
        match &self.builtin(b, args) {
            Some(Value::Str(s)) => self.give(s.as_bytes().to_vec()),
            Some(Value::Bytes(b)) => self.give(b.to_vec()),
            Some(other) => {
                self.fail(err_machine(&format!(
                    "{} answered where bytes were",
                    other.type_name()
                )));
                I64(0)
            }
            None => I64(0),
        }
    }

    /// An `Option` builtin `b` answers over `args`: its payload's words, little-endian, as taken
    /// bytes, and how many bytes; -1 for `None`.
    fn parsed(&mut self, b: Builtin, args: Vec<Value>) -> I64 {
        let payload = match &self.builtin(b, args) {
            Some(Value::Ctor { name, args }) if name.as_str() == "Some" => args.first().cloned(),
            _ => return I64(-1),
        };
        let words: Vec<u8> = match &payload {
            Some(Value::Float(f)) => ply_core::float::bits(*f).to_le_bytes().to_vec(),
            Some(Value::Decimal(d)) => d.serialize().to_vec(),
            Some(Value::Fixed(f)) => f.bits().to_le_bytes().to_vec(),
            _ => {
                self.fail(err_machine("a parse answered no number"));
                return I64(-1);
            }
        };
        self.give(words)
    }
}

/// 1 where the `n` bytes at `p` are UTF-8, else 0.
pub unsafe extern "C" fn rt_utf8_valid(_: *mut Ctx, p: Addr, n: I64) -> I64 {
    I64(i64::from(
        std::str::from_utf8(unsafe { lent(p, n) }).is_ok(),
    ))
}

/// The `n` bytes at `p` as UTF-8, each run that is not replaced by U+FFFD, as taken bytes.
pub unsafe extern "C" fn rt_utf8_lossy(ctx: *mut Ctx, p: Addr, n: I64) -> I64 {
    let text = String::from_utf8_lossy(unsafe { lent(p, n) }).into_owned();
    unsafe { &mut *ctx }.give(text.into_bytes())
}

/// The string at `p` in lower case, as taken bytes.
pub unsafe extern "C" fn rt_str_lower(ctx: *mut Ctx, p: Addr, n: I64) -> I64 {
    cased(ctx, p, n, Builtin::StringLower)
}

/// The string at `p` in upper case, as taken bytes.
pub unsafe extern "C" fn rt_str_upper(ctx: *mut Ctx, p: Addr, n: I64) -> I64 {
    cased(ctx, p, n, Builtin::StringUpper)
}

fn cased(ctx: *mut Ctx, p: Addr, n: I64, b: Builtin) -> I64 {
    let c = unsafe { &mut *ctx };
    match c.text(unsafe { lent(p, n) }) {
        Some(s) => c.taken_of(b, vec![Value::str(s)]),
        None => I64(0),
    }
}

/// Where the string at `p` begins once its leading white space is trimmed, in bytes.
pub unsafe extern "C" fn rt_trim_start(ctx: *mut Ctx, p: Addr, n: I64) -> I64 {
    let c = unsafe { &mut *ctx };
    match c.text(unsafe { lent(p, n) }) {
        Some(s) => I64((s.len() - s.trim_start().len()) as i64),
        None => I64(0),
    }
}

/// Where the string at `p` ends once its trailing white space is trimmed, in bytes.
pub unsafe extern "C" fn rt_trim_end(ctx: *mut Ctx, p: Addr, n: I64) -> I64 {
    let c = unsafe { &mut *ctx };
    match c.text(unsafe { lent(p, n) }) {
        Some(s) => I64(s.trim_end().len() as i64),
        None => I64(0),
    }
}

/// `Int::to_string` of `v`, as taken bytes.
pub unsafe extern "C" fn rt_int_text(ctx: *mut Ctx, v: I64) -> I64 {
    unsafe { &mut *ctx }.taken_of(Builtin::IntToString, vec![Value::Int(v.0)])
}

/// `Float::to_string` of the `Float` whose bits are `bits`, as taken bytes.
pub unsafe extern "C" fn rt_float_text(ctx: *mut Ctx, bits: F64) -> I64 {
    unsafe { &mut *ctx }.taken_of(
        Builtin::FloatToString,
        vec![Value::Float(f64::from_bits(bits.0))],
    )
}

/// `Decimal::to_string` of the `Decimal` whose two words, as `rust_decimal` serializes it, are `lo`
/// and `hi`, as taken bytes.
pub unsafe extern "C" fn rt_decimal_text(ctx: *mut Ctx, lo: I64, hi: I64) -> I64 {
    let d = decimal_of(lo, hi);
    unsafe { &mut *ctx }.taken_of(Builtin::DecimalToString, vec![Value::Decimal(d)])
}

/// `Float::of_decimal` of the `Decimal` whose two words are `lo` and `hi`, as its bits.
pub unsafe extern "C" fn rt_decimal_float(_ctx: *mut Ctx, lo: I64, hi: I64) -> F64 {
    F64(ply_core::float::bits(ply_core::builtins::float_of_decimal(
        decimal_of(lo, hi),
    )))
}

/// `I128::to_string` where `signed` is not 0, else `U128::to_string`, of the 128 bits `hi` then
/// `lo`, as taken bytes.
pub unsafe extern "C" fn rt_wide_text(ctx: *mut Ctx, signed: I64, lo: I64, hi: I64) -> I64 {
    let (ty, b) = wide(signed);
    let bits = (u128::from(hi.0 as u64) << 64) | u128::from(lo.0 as u64);
    let builtin = if b {
        Builtin::I128ToString
    } else {
        Builtin::U128ToString
    };
    unsafe { &mut *ctx }.taken_of(builtin, vec![Value::Fixed(Fixed::new(ty, bits))])
}

/// `Float::of_string` of the string at `p`: 8, its bits left as taken bytes, or -1 for `None`.
pub unsafe extern "C" fn rt_float_parse(ctx: *mut Ctx, p: Addr, n: I64) -> I64 {
    parse(ctx, p, n, Builtin::FloatOfString)
}

/// `Decimal::of_string` of the string at `p`: 16, its two words left as taken bytes, or -1 for
/// `None`.
pub unsafe extern "C" fn rt_decimal_parse(ctx: *mut Ctx, p: Addr, n: I64) -> I64 {
    parse(ctx, p, n, Builtin::DecimalOfString)
}

/// `I128::of_string` where `signed` is not 0, else `U128::of_string`, of the string at `p`: 16, its
/// low word then its high word left as taken bytes, or -1 for `None`.
pub unsafe extern "C" fn rt_wide_parse(ctx: *mut Ctx, signed: I64, p: Addr, n: I64) -> I64 {
    let b = if wide(signed).1 {
        Builtin::I128OfString
    } else {
        Builtin::U128OfString
    };
    parse(ctx, p, n, b)
}

fn parse(ctx: *mut Ctx, p: Addr, n: I64, b: Builtin) -> I64 {
    let c = unsafe { &mut *ctx };
    match c.text(unsafe { lent(p, n) }) {
        Some(s) => c.parsed(b, vec![Value::str(s)]),
        None => I64(-1),
    }
}

/// Word `k` of the taken bytes, little-endian, which stay taken: how emitted code reads a number a
/// parse answered.
pub unsafe extern "C" fn rt_taken_word(ctx: *mut Ctx, k: I64) -> I64 {
    let c = unsafe { &mut *ctx };
    let at = usize::try_from(k.0).ok().and_then(|k| k.checked_mul(8));
    match at.and_then(|at| c.taken.get(at..at + 8)) {
        Some(w) => I64(i64::from_le_bytes(w.try_into().expect("eight bytes"))),
        None => {
            let held = c.taken.len();
            c.fail(err_machine(&format!(
                "word {} of {held} taken bytes read",
                k.0
            )));
            I64(0)
        }
    }
}

/// The BLAKE3 digest of the `n` bytes at `p`, as taken bytes.
/// The encoding `digest` hashes of a scalar, left as taken bytes, answering their count: `kind` 0 a
/// `()`, 1 a `Bool` that is `lo`, 2 an `Int` or a narrower width that reads as `lo`, 3 the width at
/// `t` in [`ply_core::INT_TYPES`] whose bits are `lo` and above them `hi`, 4 the `Char` of scalar
/// value `lo`, 5 the `Decimal` whose two words are `lo` and `hi`.
pub unsafe extern "C" fn rt_scalar_encoded(
    ctx: *mut Ctx,
    kind: I64,
    t: I64,
    lo: I64,
    hi: I64,
) -> I64 {
    let c = unsafe { &mut *ctx };
    let v = match kind.0 {
        0 => Some(Value::Unit),
        1 => Some(Value::Bool(lo.0 != 0)),
        2 => Some(Value::Int(lo.0)),
        3 => usize::try_from(t.0)
            .ok()
            .and_then(|i| ply_core::INT_TYPES.get(i))
            .map(|ty| {
                let bits = (u128::from(hi.0 as u64) << 64) | u128::from(lo.0 as u64);
                Value::Fixed(Fixed::new(*ty, bits))
            }),
        4 => u32::try_from(lo.0)
            .ok()
            .and_then(char::from_u32)
            .map(Value::Char),
        5 => Some(Value::Decimal(decimal_of(lo, hi))),
        _ => None,
    };
    let Some(v) = v else {
        return I64(c.fail(err_machine("a scalar's encoding of no scalar")));
    };
    encoded_given(c, &v)
}

/// The encoding `digest` hashes of the string, where `text` is not 0, or else the byte string, of
/// the `n` bytes at `p`, left as taken bytes, answering their count.
pub unsafe extern "C" fn rt_text_encoded(ctx: *mut Ctx, text: I64, p: Addr, n: I64) -> I64 {
    let c = unsafe { &mut *ctx };
    let bytes = unsafe { lent(p, n) };
    let v = if text.0 != 0 {
        match c.text(bytes) {
            Some(s) => Value::str(s),
            None => return I64(0),
        }
    } else {
        Value::bytes(bytes)
    };
    encoded_given(c, &v)
}

/// `v`'s encoding left as taken bytes, its count answered.
fn encoded_given(c: &mut Ctx, v: &Value) -> I64 {
    match ply_core::digest::encoded(v, Span::DUMMY) {
        Ok(bytes) => c.give(bytes),
        Err(d) => I64(c.fail(d)),
    }
}

pub unsafe extern "C" fn rt_blake3(ctx: *mut Ctx, p: Addr, n: I64) -> I64 {
    let digest = blake3::hash(unsafe { lent(p, n) });
    unsafe { &mut *ctx }.give(digest.as_bytes().to_vec())
}

/// `Bytes::gzip` of the `n` bytes at `p`, as taken bytes.
pub unsafe extern "C" fn rt_gzip(ctx: *mut Ctx, p: Addr, n: I64) -> I64 {
    let b = Value::bytes(unsafe { lent(p, n) });
    unsafe { &mut *ctx }.taken_of(Builtin::BytesGzip, vec![b])
}

/// `Bytes::zstd` of the `n` bytes at `p`, as taken bytes.
pub unsafe extern "C" fn rt_zstd(ctx: *mut Ctx, p: Addr, n: I64) -> I64 {
    let b = Value::bytes(unsafe { lent(p, n) });
    unsafe { &mut *ctx }.taken_of(Builtin::BytesZstd, vec![b])
}

/// `Bytes::gunzip` of the `n` bytes at `p`: how many bytes it unpacked, left as taken bytes, or -1
/// for `None`.
pub unsafe extern "C" fn rt_gunzip(ctx: *mut Ctx, p: Addr, n: I64) -> I64 {
    let c = unsafe { &mut *ctx };
    match &c.builtin(
        Builtin::BytesGunzip,
        vec![Value::bytes(unsafe { lent(p, n) })],
    ) {
        Some(Value::Ctor { name, args }) if name.as_str() == "Some" => match args.first() {
            Some(Value::Bytes(b)) => c.give(b.to_vec()),
            _ => I64(-1),
        },
        _ => I64(-1),
    }
}

/// A credential holding the `n` bytes at `p`, a `Secret<String>` where `text` is not 0 and a
/// `Secret<Bytes>` otherwise, with the one count emitted code takes.
pub unsafe extern "C" fn rt_secret_of(ctx: *mut Ctx, p: Addr, n: I64, text: I64) -> Handle {
    let c = unsafe { &mut *ctx };
    let bytes = unsafe { lent(p, n) };
    let made = if text.0 != 0 {
        match c.text(bytes) {
            Some(s) => c.builtin(Builtin::SecretOfString, vec![Value::str(s)]),
            None => None,
        }
    } else {
        c.builtin(Builtin::SecretOfBytes, vec![Value::bytes(bytes)])
    };
    match made {
        Some(v @ Value::Secret(_)) => Handle::of(c.heap.bridge(v)),
        _ => Handle::of(0),
    }
}

/// 1 where the credential `stored`, which is lent, is a `Secret<String>` holding the string at `p`,
/// compared in constant time; else 0.
pub unsafe extern "C" fn rt_secret_verify(ctx: *mut Ctx, stored: Handle, p: Addr, n: I64) -> I64 {
    let c = unsafe { &mut *ctx };
    let Some(held) = secret_held(c, &stored) else {
        return I64(0);
    };
    let Some(s) = c.text(unsafe { lent(p, n) }) else {
        return I64(0);
    };
    match c.builtin(Builtin::SecretVerify, vec![held, Value::str(s)]) {
        Some(Value::Bool(b)) => I64(i64::from(b)),
        _ => I64(0),
    }
}

/// The secret the lent handle `s` holds, or none where it is no runtime object, which fails the
/// entry as Ply's fault.
fn secret_held(c: &mut Ctx, s: &Handle) -> Option<Value> {
    let w = s.word();
    if w == 0 || crate::heap::kind(w) != KIND_BRIDGE {
        c.fail(err_machine("a credential that is no runtime object"));
        return None;
    }
    Some(unsafe { bridged(obj(w)) }.clone())
}

/// How many bytes the secret the lent handle `s` holds; 0 where it holds none.
pub unsafe extern "C" fn rt_secret_len(ctx: *mut Ctx, s: Handle) -> I64 {
    let c = unsafe { &mut *ctx };
    match &secret_held(c, &s) {
        Some(Value::Secret(held)) => I64(held.bytes().len() as i64),
        _ => I64(0),
    }
}

/// `Secret::bytes` of the secret the lent handle `s` holds, as a handle of its own.
pub unsafe extern "C" fn rt_secret_bytes(ctx: *mut Ctx, s: Handle) -> Handle {
    let c = unsafe { &mut *ctx };
    let made = secret_held(c, &s).and_then(|v| c.builtin(Builtin::SecretBytes, vec![v]));
    match made {
        Some(v @ Value::Secret(_)) => Handle::of(c.heap.bridge(v)),
        _ => Handle::of(0),
    }
}

/// `Secret::concat` of the secrets the lent handles `a` and `b` hold, as a handle of its own.
pub unsafe extern "C" fn rt_secret_concat(ctx: *mut Ctx, a: Handle, b: Handle) -> Handle {
    let c = unsafe { &mut *ctx };
    let made = match (secret_held(c, &a), secret_held(c, &b)) {
        (Some(x), Some(y)) => c.builtin(Builtin::SecretConcat, vec![x, y]),
        _ => None,
    };
    match made {
        Some(v @ Value::Secret(_)) => Handle::of(c.heap.bridge(v)),
        _ => Handle::of(0),
    }
}

/// [`rt_refused`](crate::rt::rt_refused) for a builtin whose refusal names text, `what` numbered as
/// [`REFUSED`](crate::rt::REFUSED) numbers it: `String::find`, which did not find the `m` bytes at
/// `q` among the `n` at `p`; `String::of_bytes`, given the `n` bytes at `p`, which are not UTF-8;
/// `assert`, whose message is the string of `n` bytes at `p`, or none where `n` is below 0. Any other
/// `what` fails the entry as Ply's fault.
#[allow(improper_ctypes_definitions)]
pub unsafe extern "C" fn rt_refused_text(
    ctx: *mut Ctx,
    what: I64,
    p: Addr,
    n: I64,
    q: Addr,
    m: I64,
) -> Raises {
    let c = unsafe { &mut *ctx };
    let site = c.site();
    let Some(builtin) = refused_builtin(what) else {
        refused_unknown(c, what);
        return Raises(());
    };
    let refused = match builtin {
        Builtin::StringFind => {
            let (Some(s), Some(needle)) =
                (c.text(unsafe { lent(p, n) }), c.text(unsafe { lent(q, m) }))
            else {
                return Raises(());
            };
            not_found(site, s, needle)
        }
        Builtin::StringOfBytes => {
            let bytes = unsafe { lent(p, n) };
            match std::str::from_utf8(bytes) {
                Err(e) => not_utf8(site, bytes, &e),
                Ok(_) => {
                    c.fail(err_machine("bytes refused as not UTF-8 that are"));
                    return Raises(());
                }
            }
        }
        Builtin::Assert => {
            let message = if n.0 < 0 {
                Value::ctor("None", Vec::new())
            } else {
                match c.text(unsafe { lent(p, n) }) {
                    Some(s) => Value::ctor("Some", vec![Value::str(s)]),
                    None => return Raises(()),
                }
            };
            assert_failure(&message, site)
        }
        _ => {
            refused_unknown(c, what);
            return Raises(());
        }
    };
    refuse(c, builtin, refused);
    Raises(())
}

fn decimal_of(lo: I64, hi: I64) -> Decimal {
    let mut d = [0u8; 16];
    d[..8].copy_from_slice(&lo.0.to_le_bytes());
    d[8..].copy_from_slice(&hi.0.to_le_bytes());
    Decimal::deserialize(d)
}

/// The 128-bit width `signed` names, and whether it is signed.
fn wide(signed: I64) -> (IntTy, bool) {
    if signed.0 != 0 {
        (IntTy::I128, true)
    } else {
        (IntTy::U128, false)
    }
}
