use std::ffi::{c_char, c_void};

pub const SYMBOL: &str = "ply_descriptor";

#[repr(C)]
pub struct FunctionRow {
    name: *const c_char,
    entry: *mut c_void,
    arity: usize,
}

impl FunctionRow {
    pub fn name(&self) -> &str {
        let bytes = unsafe { std::ffi::CStr::from_ptr(self.name) }.to_bytes();
        std::str::from_utf8(bytes).unwrap_or("")
    }

    pub fn entry(&self) -> Option<*mut c_void> {
        (!self.entry.is_null()).then_some(self.entry)
    }

    pub fn arity(&self) -> usize {
        self.arity
    }
}

#[repr(C)]
pub struct Descriptor {
    exports: *const c_char,
    exports_len: usize,
    bind: *mut c_void,
    _reserved: *mut c_void,
    code: *const *mut c_void,
    code_count: usize,
    taken: *const FunctionRow,
    taken_count: usize,
}

unsafe impl Sync for Descriptor {}
unsafe impl Send for Descriptor {}

impl Descriptor {
    pub fn exports(&self) -> Option<&str> {
        let bytes =
            unsafe { std::slice::from_raw_parts(self.exports.cast::<u8>(), self.exports_len) };
        std::str::from_utf8(bytes).ok()
    }

    pub fn bind(&self) -> *mut c_void {
        self.bind
    }

    pub fn code(&self) -> &[*mut c_void] {
        unsafe { std::slice::from_raw_parts(self.code, self.code_count) }
    }

    pub fn taken(&self) -> &[FunctionRow] {
        unsafe { std::slice::from_raw_parts(self.taken, self.taken_count) }
    }

    pub fn taking(&self, name: &str) -> Option<&FunctionRow> {
        let rows = self.taken();
        rows.binary_search_by(|row| row.name().as_bytes().cmp(name.as_bytes()))
            .ok()
            .map(|at| &rows[at])
    }
}
