//! `std.pace`'s natives: what a test's entry has cost so far, its calls, the objects it allocated
//! and the bytes those took. They read the entry's own counts, so unlike a host operation they
//! answer inside a `simulate` region and in a test the search re-runs alike; outside a test's entry
//! they fail it.

use crate::c::{MachineNative, machine_native};
use crate::native::I64;
use crate::rt::{Cost, Ctx};
use ply_core::{Diagnostic, Span, codes};

pub(super) fn natives() -> Vec<MachineNative> {
    vec![
        machine_native(
            "meter_steps",
            rt_meter_steps as unsafe extern "C" fn(_) -> _,
        ),
        machine_native(
            "meter_allocations",
            rt_meter_allocations as unsafe extern "C" fn(_) -> _,
        ),
        machine_native(
            "meter_bytes",
            rt_meter_bytes as unsafe extern "C" fn(_) -> _,
        ),
    ]
}

/// The calls the entry has made so far.
pub unsafe extern "C" fn rt_meter_steps(ctx: *mut Ctx) -> I64 {
    reading(ctx, |c| c.steps)
}

/// The objects the entry has allocated so far.
pub unsafe extern "C" fn rt_meter_allocations(ctx: *mut Ctx) -> I64 {
    reading(ctx, |c| c.allocations)
}

/// The bytes the objects the entry has allocated so far took.
pub unsafe extern "C" fn rt_meter_bytes(ctx: *mut Ctx) -> I64 {
    reading(ctx, |c| c.bytes)
}

/// What `read` takes of the entry's cost so far, where the entry is a test's; any other fails.
fn reading(ctx: *mut Ctx, read: impl FnOnce(Cost) -> i64) -> I64 {
    let c = unsafe { &mut *ctx };
    if c.tested {
        I64(read(c.cost()))
    } else {
        c.fail(err_untested());
        I64(0)
    }
}

/// `E0424`: what a computation costs is its compiler's to choose, so a program reads it only where
/// it is a test.
#[cold]
fn err_untested() -> Diagnostic {
    Diagnostic::error(
        codes::HOST_UNBOUND,
        "`pace::meter.reading` reached a run that is no test's, and only `ply test` answers it",
    )
    .primary(Span::DUMMY, "read here")
    .note("what a computation costs is its compiler's to choose, so no program's answer may hang on it")
}
