use crate::c::{MachineNative, runtime_native};
use crate::native::{Address, F64, I64};
use crate::rt::{Context, borrowed};
use ply_core::{Diagnostic, Span, codes};

pub fn natives() -> Vec<MachineNative> {
    vec![runtime_native(
        "?blake3",
        rt_blake3 as unsafe extern "C" fn(_, _, _) -> _,
    )]
    .into_iter()
    .chain(float_natives())
    .collect()
}

macro_rules! float_functions {
    ($($name:literal => $f:ident = $($math:ident)::+ ($($a:ident),+);)+) => {
        $(unsafe extern "C" fn $f(_context: *mut Context, $($a: F64),+) -> F64 {
            F64(ply_core::float::bits($($math)::+($(f64::from_bits($a.0)),+)))
        })+

        fn float_natives() -> Vec<MachineNative> {
            vec![$(runtime_native($name, $f as unsafe extern "C" fn(*mut Context, $(float_functions!(@f64 $a)),+) -> F64)),+]
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

impl Context {
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

pub unsafe extern "C" fn rt_blake3(context: *mut Context, p: Address, n: I64) -> I64 {
    let digest = blake3::hash(unsafe { borrowed(p, n) });
    unsafe { &mut *context }.give(digest.as_bytes().to_vec())
}
