use super::descriptor::Descriptor;
use std::sync::OnceLock;

static UNIT: OnceLock<&'static Descriptor> = OnceLock::new();

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
