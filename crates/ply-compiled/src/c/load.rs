use super::descriptor::{self, Descriptor};
use anyhow::{Result, bail};
use std::ffi::{CString, c_char, c_int, c_void};
use std::path::{Path, PathBuf};

unsafe extern "C" {
    fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
    fn dlerror() -> *const c_char;
}

const RTLD_NOW: c_int = 2;

pub struct Library {
    handle: Handle,
    path: PathBuf,
}

enum Handle {
    Opened(*mut c_void),
    Linked(&'static Descriptor),
}

impl Library {
    pub fn descriptor(&self) -> Option<&Descriptor> {
        match self.handle {
            Handle::Linked(unit) => Some(unit),
            Handle::Opened(handle) => {
                let name = CString::new(descriptor::SYMBOL).ok()?;
                let p = unsafe { dlsym(handle, name.as_ptr()) };
                unsafe { p.cast::<Descriptor>().as_ref() }
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn linked() -> Option<Library> {
        Some(Library {
            handle: Handle::Linked(super::linked::unit()?),
            path: std::env::current_exe().unwrap_or_default(),
        })
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        let Handle::Opened(handle) = self.handle else {
            return;
        };
        unsafe { dlclose(handle) };
    }
}

unsafe impl Send for Library {}

pub fn cache_dir() -> std::path::PathBuf {
    super::cache_dir_for("cache", "PLY_C_CACHE")
}

impl Library {
    pub fn open(so: &std::path::Path) -> Result<Library> {
        let path = CString::new(so.to_string_lossy().as_bytes())?;
        let handle = unsafe { dlopen(path.as_ptr(), RTLD_NOW) };
        if handle.is_null() {
            let e = unsafe { dlerror() };
            let message = if e.is_null() {
                "no reason given".to_string()
            } else {
                unsafe { std::ffi::CStr::from_ptr(e) }
                    .to_string_lossy()
                    .to_string()
            };
            bail!("could not load the unit the C backend built: {message}");
        }
        Ok(Library {
            handle: Handle::Opened(handle),
            path: so.to_path_buf(),
        })
    }
}
