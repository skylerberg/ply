//! `std.random`'s natives: the operating system's entropy, each draw independent of every other.

use super::{Nondeterministic, asked};
use crate::c::{MachineNative, machine_native};
use crate::emitted::SecretHandle;
use crate::native::I64;
use crate::rt::Ctx;
use crate::secret;

pub(super) fn natives() -> Vec<MachineNative> {
    vec![
        machine_native(
            "entropy_next",
            rt_entropy_next as unsafe extern "C" fn(_) -> _,
        ),
        machine_native(
            "entropy_secret",
            rt_entropy_secret as unsafe extern "C" fn(_, _) -> _,
        ),
    ]
}

/// Sixty-three bits of the operating system's entropy; 0, with the run failed, where it gave none.
pub unsafe extern "C" fn rt_entropy_next(ctx: *mut Ctx) -> I64 {
    asked(
        ctx,
        "entropy_next",
        Nondeterministic,
        I64(0),
        |c| match ply_host::random::next() {
            Ok(n) => I64(n),
            Err(d) => {
                c.fail(d);
                I64(0)
            }
        },
    )
}

/// `n` bytes of the operating system's entropy drawn into a secret, `n` in the range the real
/// handler checks.
pub unsafe extern "C" fn rt_entropy_secret(ctx: *mut Ctx, n: I64) -> SecretHandle {
    asked(
        ctx,
        "entropy_secret",
        Nondeterministic,
        SecretHandle::of(0),
        |c| match ply_host::random::drawn(usize::try_from(n.0).unwrap_or(0)) {
            Ok(bytes) => secret::sealed(c, false, bytes),
            Err(d) => {
                c.fail(d);
                SecretHandle::of(0)
            }
        },
    )
}
