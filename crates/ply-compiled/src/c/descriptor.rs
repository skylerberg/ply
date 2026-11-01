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
pub struct NameCell {
    name: *const c_char,
    cell: *mut c_void,
}

impl NameCell {
    pub fn name(&self) -> &str {
        let bytes = unsafe { std::ffi::CStr::from_ptr(self.name) }.to_bytes();
        std::str::from_utf8(bytes).unwrap_or("")
    }

    pub fn cell(&self) -> *mut c_void {
        self.cell
    }
}

fn rows<'a>(first: *const NameCell) -> impl Iterator<Item = &'a NameCell> {
    (0..)
        .map(move |at| unsafe { &*first.add(at) })
        .take_while(|row| !row.name.is_null())
}

#[repr(C)]
pub struct Links {
    slots: *const NameCell,
    exported: *const NameCell,
    keys: *const NameCell,
    field_words: *const NameCell,
    code_names: *const *const c_char,
}

impl Links {
    pub fn slots(&self) -> impl Iterator<Item = &NameCell> {
        rows(self.slots)
    }

    pub fn exported(&self) -> impl Iterator<Item = &NameCell> {
        rows(self.exported)
    }

    pub fn keys(&self) -> impl Iterator<Item = &NameCell> {
        rows(self.keys)
    }

    pub fn field_words(&self) -> impl Iterator<Item = &NameCell> {
        rows(self.field_words)
    }

    pub fn code_names(&self) -> impl Iterator<Item = &str> {
        (0..)
            .map(|at| unsafe { *self.code_names.add(at) })
            .take_while(|name| !name.is_null())
            .map(|name| {
                let bytes = unsafe { std::ffi::CStr::from_ptr(name) }.to_bytes();
                std::str::from_utf8(bytes).unwrap_or("")
            })
    }
}

#[repr(C)]
pub struct Descriptor {
    exports: *const c_char,
    exports_len: usize,
    bind: *mut c_void,
    links: *const Links,
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

    pub fn links(&self) -> Option<&Links> {
        unsafe { self.links.as_ref() }
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
