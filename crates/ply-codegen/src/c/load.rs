//! Loading a unit: a library its C was compiled and linked into, opened with `dlopen`, or the
//! program this executable was linked with.

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

/// `RTLD_NOW | RTLD_LOCAL`: every symbol resolved at load, and nothing added to the global
/// namespace, so two units of the same program cannot see each other's definitions.
const RTLD_NOW: c_int = 2;

/// A loaded unit. Dropping it closes the library, which invalidates every entry taken from it —
/// which is why `Bodies` keeps one alive for as long as it holds an [`crate::rt::Entry`].
pub struct Library {
    handle: Handle,
    /// Kept so a unit that outlives its build directory still names where it came from in a
    /// diagnostic, and so the file can be removed with the library.
    path: PathBuf,
}

enum Handle {
    Opened(*mut c_void),
    /// The program this executable was linked with, by the descriptor its `main` handed over.
    Linked(&'static Descriptor),
}

impl Library {
    /// What the unit's C lays out for its loader; `None` for a library that is no unit.
    pub fn descriptor(&self) -> Option<&Descriptor> {
        match self.handle {
            Handle::Linked(unit) => Some(unit),
            Handle::Opened(handle) => {
                let name = CString::new(descriptor::SYMBOL).ok()?;
                let p = unsafe { dlsym(handle, name.as_ptr()) };
                // SAFETY: a unit defines the symbol as its descriptor, which lives as long as
                // the library is open.
                unsafe { p.cast::<Descriptor>().as_ref() }
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The program linked into this executable, where its `main` installed one.
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

// SAFETY: the handle is only read, and every entry taken from it is called on the thread that
// holds the `Bodies` it belongs to.
unsafe impl Send for Library {}

/// Where compiled units are kept between runs: the directory `PLY_C_CACHE` names, or the
/// user's own. Libraries sit at its root, bucket objects under `obj/`, the emitter's answers under
/// `bodies/`.
///
/// **Nothing about that directory bounds it**: an entry is keyed by its content, so a changed
/// definition writes a new one beside the old rather than replacing it, and the system sweeps
/// nothing there. `super::sweep` is what bounds it, once a launch, and `PLY_C_CACHE_MAX` is the
/// bound.
pub fn cache_dir() -> std::path::PathBuf {
    super::kept_dir("cache", "PLY_C_CACHE")
}

impl Library {
    /// `dlopen` one library, whether it was linked just now or last week.
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
