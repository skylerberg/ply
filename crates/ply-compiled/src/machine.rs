//! The natives over machine data that strings and byte strings emitted code lays out reach the
//! runtime through: each takes the bytes it reads as an address and a count, borrowed for the call,
//! and returns raw integers, a `Float`'s bits, a runtime object, or bytes left as the context's
//! taken bytes, which emitted code copies out into a block of its own. None reads or makes a string,
//! a byte string or a list. A native whose builtin can fail returns a value that says so, and
//! emitted code hands the rejection to [`rt_refused`](crate::rt::rt_refused), or to
//! [`rt_refused_text`] where it names text. What each returns is what the value builtin of the same
//! name returns.

use crate::c::{MachineNative, machine_native};
use crate::native::{Addr, F64, I64, Raises};
use crate::rt::{Ctx, Rejected, borrowed, refuse, rejected_numbered, rejected_unknown};
use ply_core::builtins::{assert_failure, not_found};
use ply_core::{Decimal, Diagnostic, Span, codes};

/// Every native here, under the name the compiler declares it by.
pub fn natives() -> Vec<MachineNative> {
    vec![
        machine_native("?blake3", rt_blake3 as unsafe extern "C" fn(_, _, _) -> _),
        machine_native(
            "?scalar_encoded",
            rt_scalar_encoded as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        machine_native(
            "?text_encoded",
            rt_text_encoded as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        machine_native(
            "?functions_compared",
            rt_functions_compared as unsafe extern "C" fn(_) -> _,
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

/// Each `Float` function as compiled code calls it: its operands' bits in, its result's out,
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
fn err_machine(reason: &str) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("a string's bytes handed to the runtime: {reason}"),
    )
    .primary(Span::DUMMY, "in compiled code")
    .note("this is Ply's fault, not the program's")
}

impl Ctx {
    /// The UTF-8 of a string, borrowed, which emitted code only ever makes valid.
    pub(crate) fn text<'a>(&mut self, bytes: &'a [u8]) -> Option<&'a str> {
        match std::str::from_utf8(bytes) {
            Ok(s) => Some(s),
            Err(_) => {
                self.fail(err_machine("a string that is not UTF-8"));
                None
            }
        }
    }
}

/// The encoding `digest` hashes of a scalar, left as taken bytes, returning their count: `kind` 0 a
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
    use ply_core::digest::{Scalar, scalar};
    let c = unsafe { &mut *ctx };
    let s = match kind.0 {
        0 => Some(Scalar::Unit),
        1 => Some(Scalar::Bool(lo.0 != 0)),
        2 => Some(Scalar::Int(lo.0)),
        3 => usize::try_from(t.0)
            .ok()
            .and_then(|i| ply_core::INT_TYPES.get(i))
            .map(|ty| {
                Scalar::Width(
                    *ty,
                    (u128::from(hi.0 as u64) << 64) | u128::from(lo.0 as u64),
                )
            }),
        4 => u32::try_from(lo.0)
            .ok()
            .filter(|c| char::from_u32(*c).is_some())
            .map(Scalar::Char),
        5 => Some(Scalar::Decimal(decimal_of(lo, hi))),
        _ => None,
    };
    match s {
        Some(s) => c.give(scalar(s)),
        None => I64(c.fail(err_machine("a scalar's encoding of no scalar"))),
    }
}

/// The encoding `digest` hashes of the string, where `text` is not 0, or else the byte string, of
/// the `n` bytes at `p`, left as taken bytes, returning their count.
pub unsafe extern "C" fn rt_text_encoded(ctx: *mut Ctx, text: I64, p: Addr, n: I64) -> I64 {
    let c = unsafe { &mut *ctx };
    let bytes = unsafe { borrowed(p, n) };
    if text.0 != 0 && c.text(bytes).is_none() {
        return I64(0);
    }
    c.give(ply_core::digest::text(text.0 != 0, bytes))
}

/// The BLAKE3 digest of the `n` bytes at `p`, as taken bytes.
pub unsafe extern "C" fn rt_blake3(ctx: *mut Ctx, p: Addr, n: I64) -> I64 {
    let digest = blake3::hash(unsafe { borrowed(p, n) });
    unsafe { &mut *ctx }.give(digest.as_bytes().to_vec())
}

/// Fails the run: two functions were compared for equality, which no function has. Returns 0.
pub unsafe extern "C" fn rt_functions_compared(ctx: *mut Ctx) -> I64 {
    let c = unsafe { &mut *ctx };
    c.fail(
        Diagnostic::error(
            codes::RUNTIME_ERROR,
            "cannot compare functions for equality",
        )
        .primary(Span::DUMMY, "in compiled code"),
    );
    I64(0)
}

/// [`rt_refused`](crate::rt::rt_refused) for a builtin whose rejection names text, `what` numbered
/// as [`REJECTED`](crate::rt::REJECTED) numbers it: `String::find`, not finding the `m` bytes at
/// `q` among the `n` at `p`; `String::of_bytes`, given the `n` bytes at `p`, which are not UTF-8;
/// `assert`, whose message is the string of `n` bytes at `p`, or none where `n` is below 0;
/// `Secret::decode` and `Secret::encode`, whose encoding, the `n` bytes at `p`, is none they read or
/// write. Any other `what` fails the entry as Ply's fault.
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
    let Some(r) = rejected_numbered(what) else {
        rejected_unknown(c, what);
        return Raises(());
    };
    let rejected = match r {
        Rejected::StringFind => {
            let (Some(s), Some(needle)) =
                (c.text(unsafe { borrowed(p, n) }), c.text(unsafe { borrowed(q, m) }))
            else {
                return Raises(());
            };
            not_found(site, s, needle)
        }
        Rejected::Assert => {
            let message = if n.0 < 0 {
                None
            } else {
                match c.text(unsafe { borrowed(p, n) }) {
                    Some(s) => Some(s),
                    None => return Raises(()),
                }
            };
            assert_failure(message, site)
        }
        Rejected::SecretDecode | Rejected::SecretEncode => {
            let Some(encoding) = c.text(unsafe { borrowed(p, n) }) else {
                return Raises(());
            };
            let (what, does) = if r == Rejected::SecretDecode {
                ("`Secret::decode`", "reads")
            } else {
                ("`Secret::encode`", "writes")
            };
            let reason = format!("`{encoding}` is no encoding it {does}");
            ply_core::crypto::misuse(site, what, reason)
        }
        _ => {
            rejected_unknown(c, what);
            return Raises(());
        }
    };
    refuse(c, r, rejected);
    Raises(())
}

fn decimal_of(lo: I64, hi: I64) -> Decimal {
    let mut d = [0u8; 16];
    d[..8].copy_from_slice(&lo.0.to_le_bytes());
    d[8..].copy_from_slice(&hi.0.to_le_bytes());
    Decimal::deserialize(d)
}
