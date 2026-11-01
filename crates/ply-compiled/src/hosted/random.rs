use super::{Nondeterministic, if_host_allowed};
use crate::c::{MachineNative, runtime_native};
use crate::emitted::SecretHandle;
use crate::native::I64;
use crate::rt::Ctx;
use crate::secret;

pub(super) fn natives() -> Vec<MachineNative> {
    vec![
        runtime_native(
            "entropy_next",
            rt_entropy_next as unsafe extern "C" fn(_) -> _,
        ),
        runtime_native(
            "entropy_secret",
            rt_entropy_secret as unsafe extern "C" fn(_, _) -> _,
        ),
    ]
}

pub unsafe extern "C" fn rt_entropy_next(ctx: *mut Ctx) -> I64 {
    if_host_allowed(
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

pub unsafe extern "C" fn rt_entropy_secret(ctx: *mut Ctx, n: I64) -> SecretHandle {
    if_host_allowed(
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
