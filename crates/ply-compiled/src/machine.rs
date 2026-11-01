//! The natives over machine data that strings and byte strings emitted code lays out reach the
//! runtime through: each takes the bytes it reads as an address and a count, borrowed for the call,
//! and returns raw integers, a `Float`'s bits, a runtime object, or bytes left as the context's
//! taken bytes, which emitted code copies out into a block of its own. None reads or makes a string,
//! a byte string or a list. What each returns is what the value builtin of the same name returns.

use crate::c::{MachineNative, machine_native};
use crate::native::{Addr, F64, I64};
use crate::rt::{Ctx, borrowed};
use ply_core::{Diagnostic, Span, codes};

/// Every native here, under the name the compiler declares it by.
pub fn natives() -> Vec<MachineNative> {
    vec![machine_native(
        "?blake3",
        rt_blake3 as unsafe extern "C" fn(_, _, _) -> _,
    )]
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

/// The BLAKE3 digest of the `n` bytes at `p`, as taken bytes.
pub unsafe extern "C" fn rt_blake3(ctx: *mut Ctx, p: Addr, n: I64) -> I64 {
    let digest = blake3::hash(unsafe { borrowed(p, n) });
    unsafe { &mut *ctx }.give(digest.as_bytes().to_vec())
}
