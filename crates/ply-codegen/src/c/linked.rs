//! The program an executable was linked with. Its `main` hands the runtime the unit's descriptor,
//! so nothing of the program is exported for `dlsym` to find, and a unit the program loads later
//! binds only to its own code. A `main` the last release wrote hands over a table of symbols by
//! name instead, which holds the descriptor where the unit lays one out.

use super::descriptor::Descriptor;
use std::ffi::{CStr, c_char, c_void};
use std::sync::OnceLock;

static UNIT: OnceLock<&'static Descriptor> = OnceLock::new();

/// Installs the descriptor `main` hands over. Refused when one is installed already.
///
/// # Safety
/// `unit` points at a unit's descriptor, which lives as long as the process.
pub unsafe fn install_unit(unit: *const Descriptor) -> Result<(), String> {
    let Some(unit) = (unsafe { unit.as_ref() }) else {
        return Err("the linked program handed over no unit".to_string());
    };
    UNIT.set(unit)
        .map_err(|_| "a linked program's unit is installed already".to_string())
}

pub(super) fn unit() -> Option<&'static Descriptor> {
    UNIT.get().copied()
}

/// One row of the table a native program's `main` hands over.
#[repr(C)]
pub struct Row {
    pub name: *const c_char,
    pub address: *mut c_void,
}

/// The rows, ascending by the bytes of their names.
pub struct Symbols(&'static [Row]);

// SAFETY: the rows name and point at the executable's own code and data, which outlive the process's
// every thread, and nothing writes through this table.
unsafe impl Sync for Symbols {}
unsafe impl Send for Symbols {}

static INSTALLED: OnceLock<Symbols> = OnceLock::new();

/// Installs the table `main` hands over. Refused when one is installed already or the rows are not
/// ascending, which would hide symbols from the search.
///
/// # Safety
/// `rows` points at `count` rows whose names are NUL-terminated and which live as long as the process.
pub unsafe fn install(rows: *const Row, count: usize) -> Result<(), String> {
    let rows: &'static [Row] = if count == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(rows, count) }
    };
    let name = |row: &Row| unsafe { CStr::from_ptr(row.name) }.to_bytes();
    if let Some(pair) = rows
        .windows(2)
        .find(|pair| name(&pair[0]) >= name(&pair[1]))
    {
        return Err(format!(
            "the linked program's symbols are out of order at `{}`",
            String::from_utf8_lossy(name(&pair[1]))
        ));
    }
    INSTALLED
        .set(Symbols(rows))
        .map_err(|_| "a linked program's symbols are installed already".to_string())
}

pub(super) fn installed() -> Option<&'static Symbols> {
    INSTALLED.get()
}

impl Symbols {
    pub fn find(&self, name: &str) -> Option<*mut c_void> {
        self.0
            .binary_search_by(|row| {
                unsafe { CStr::from_ptr(row.name) }
                    .to_bytes()
                    .cmp(name.as_bytes())
            })
            .ok()
            .map(|at| self.0[at].address)
    }
}
