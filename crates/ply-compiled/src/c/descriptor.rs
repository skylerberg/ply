//! What a unit's C lays out for its loader, `ply_descriptor`: the table it says about itself, the
//! functions that bind it to a runtime, its code table and the definitions it holds, each by
//! address. A compiler and a linker place all of it, so loading such a unit looks no symbol up by
//! name and writes no table of its own.

use crate::emitted::{Handed, PlyOp};
use crate::rt::Ctx;
use std::ffi::{c_char, c_void};

/// The symbol a library's descriptor is read from.
pub const SYMBOL: &str = "ply_descriptor";

/// One definition a unit holds, a `struct ply_taken`: its Ply name, the entry the runtime calls it
/// through, null for one only compiled code calls, and how many words it takes.
#[repr(C)]
pub struct TakenRow {
    name: *const c_char,
    entry: *mut c_void,
    arity: usize,
}

impl TakenRow {
    pub fn name(&self) -> &str {
        // SAFETY: a unit writes each name as a NUL-terminated literal that lives as long as it does.
        let bytes = unsafe { std::ffi::CStr::from_ptr(self.name) }.to_bytes();
        std::str::from_utf8(bytes).unwrap_or("")
    }

    /// The entry, or `None` for a definition the runtime cannot call.
    pub fn entry(&self) -> Option<*mut c_void> {
        (!self.entry.is_null()).then_some(self.entry)
    }

    pub fn arity(&self) -> usize {
        self.arity
    }
}

/// The functions of a unit's C the runtime calls.
#[repr(C)]
pub struct Callbacks {
    /// Leaves a raise nothing handles pending, the operation's arguments its to take: not 0 where
    /// the perform is a raise, which is then done with.
    pub raised: Raised,
}

/// `Callbacks::raised`: the context, the operation's descriptor, its mode and its arguments' words.
pub type Raised = unsafe extern "C" fn(*mut Ctx, *const PlyOp, i64, *const Handed) -> i64;

/// A `struct ply_descriptor`, as `compiler.unit` writes one after a unit's runtime. Its rows of
/// [`TakenRow`] ascend by the bytes of their names, and each table is as long as its count.
#[repr(C)]
pub struct Descriptor {
    exports: *const c_char,
    exports_len: usize,
    bind: *mut c_void,
    /// What the runtime calls back into: [`Callbacks`]. Null in a unit of the release's compiler.
    callbacks: *const Callbacks,
    code: *const *mut c_void,
    code_count: usize,
    taken: *const TakenRow,
    taken_count: usize,
}

// SAFETY: a descriptor and all it points at are a unit's read-only data, which nothing writes.
unsafe impl Sync for Descriptor {}
unsafe impl Send for Descriptor {}

impl Descriptor {
    /// What the unit says about itself, or `None` where it is not UTF-8.
    pub fn exports(&self) -> Option<&str> {
        // SAFETY: the unit lays `exports_len` bytes out at `exports`.
        let bytes =
            unsafe { std::slice::from_raw_parts(self.exports.cast::<u8>(), self.exports_len) };
        std::str::from_utf8(bytes).ok()
    }

    /// The unit's `ply_bind`, which takes the address of each helper it calls.
    pub fn bind(&self) -> *mut c_void {
        self.bind
    }

    /// The unit's raise function, where its compiler writes one.
    pub fn raised(&self) -> Option<Raised> {
        // SAFETY: a unit lays its callbacks out as `Callbacks` does.
        unsafe { self.callbacks.as_ref() }.map(|c| c.raised)
    }

    /// Each compiled function's address, by its position in the unit's code table.
    pub fn code(&self) -> &[*mut c_void] {
        // SAFETY: the unit lays `code_count` addresses out at `code`.
        unsafe { std::slice::from_raw_parts(self.code, self.code_count) }
    }

    /// The definitions the unit holds, ascending by name.
    pub fn taken(&self) -> &[TakenRow] {
        // SAFETY: the unit lays `taken_count` rows out at `taken`.
        unsafe { std::slice::from_raw_parts(self.taken, self.taken_count) }
    }

    /// The row of the definition `name`, where the unit holds it.
    pub fn taking(&self, name: &str) -> Option<&TakenRow> {
        let rows = self.taken();
        rows.binary_search_by(|row| row.name().as_bytes().cmp(name.as_bytes()))
            .ok()
            .map(|at| &rows[at])
    }
}
