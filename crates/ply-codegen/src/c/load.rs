//! Compiling a unit's C and loading it: the process and the link ADR 0037 priced, done once per
//! unit rather than once per definition.

use anyhow::{Result, anyhow, bail};
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
/// which is why `Bodies` keeps one alive for as long as it holds an [`crate::jit::Entry`].
pub struct Library {
    handle: *mut c_void,
    /// Kept so a unit that outlives its build directory still names where it came from in a
    /// diagnostic, and so the file can be removed with the library.
    path: PathBuf,
}

impl Library {
    pub fn symbol(&self, name: &str) -> Option<*mut c_void> {
        let c = CString::new(name).ok()?;
        let p = unsafe { dlsym(self.handle, c.as_ptr()) };
        (!p.is_null()).then_some(p)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        unsafe { dlclose(self.handle) };
        // `PLY_C_KEEP` leaves the unit and its source where they were built, which is how the
        // emitted code is read: this tier's output is a file a disassembler can open, and the
        // other tier's is not.
        if std::env::var("PLY_C_KEEP").is_err() {
            let _ = std::fs::remove_file(&self.path);
            let _ = std::fs::remove_file(self.path.with_file_name("unit.c"));
        } else {
            eprintln!("c tier kept {}", self.path.display());
        }
    }
}

// SAFETY: the handle is only read, and every entry taken from it is called on the thread that
// holds the `Bodies` it belongs to.
unsafe impl Send for Library {}

/// The C compiler this tier shells out to. `cc` rather than a pinned name, for the reason ADR 0037
/// gives for preferring C over LLVM in the first place: the dependency should be the one every
/// machine already has.
fn compiler() -> String {
    std::env::var("PLY_CC").unwrap_or_else(|_| "cc".to_string())
}

/// Compile `source` into a shared object beside it and load it.
///
/// One process and one link for the whole unit, which is the shape `benches/c-floor/` found is a
/// constant rather than an exponent — and the opposite of the per-definition image it refused.
pub fn compile_and_load(source: &str, stem: &str) -> Result<Library> {
    let dir = std::env::temp_dir().join(format!("ply-c-{}-{}", std::process::id(), stem));
    std::fs::create_dir_all(&dir)?;
    let c = dir.join("unit.c");
    let so = dir.join(if cfg!(target_os = "macos") {
        "unit.dylib"
    } else {
        "unit.so"
    });
    std::fs::write(&c, source)?;
    let level = std::env::var("PLY_CC_OPT").unwrap_or_else(|_| "-O2".to_string());
    let out = std::process::Command::new(compiler())
        .arg(&level)
        .arg("-fPIC")
        .arg("-shared")
        .arg("-fno-strict-aliasing")
        .arg("-o")
        .arg(&so)
        .arg(&c)
        .output()
        .map_err(|e| anyhow!("could not run {}: {e}", compiler()))?;
    if !out.status.success() {
        bail!(
            "the C tier's compiler refused the unit it emitted ({}):\n{}",
            so.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
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
        bail!("could not load the unit the C tier built: {message}");
    }
    Ok(Library { handle, path: so })
}
