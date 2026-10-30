//! The program an executable was linked with. Its `main` hands the runtime the unit's descriptor,
//! so nothing of the program is exported for `dlsym` to find, and a unit the program loads later
//! binds only to its own code.

use super::descriptor::Descriptor;
use std::sync::OnceLock;

static UNIT: OnceLock<&'static Descriptor> = OnceLock::new();

/// Installs the descriptor `main` hands over. Rejected when one is installed already.
///
/// # Safety
/// `unit` points at a unit's descriptor, which lives as long as the process.
pub unsafe fn install(unit: *const Descriptor) -> Result<(), String> {
    let Some(unit) = (unsafe { unit.as_ref() }) else {
        return Err("the linked program handed over no unit".to_string());
    };
    UNIT.set(unit)
        .map_err(|_| "a linked program's unit is installed already".to_string())
}

pub(super) fn unit() -> Option<&'static Descriptor> {
    UNIT.get().copied()
}
