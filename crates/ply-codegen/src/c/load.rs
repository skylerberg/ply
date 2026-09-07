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
        // The object is not deleted: it lives in the cache, and the next run that emits the same
        // source loads it rather than compiling it again. `PLY_C_KEEP` says where it is, which is
        // how the emitted code is read -- this tier's output is a file a disassembler can open,
        // and the other tier's is not. `PLY_C_CACHE` names the directory.
        if std::env::var("PLY_C_KEEP").is_ok() {
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
/// Where compiled units are kept between runs. `PLY_C_CACHE` names another directory; the default
/// is under the system's temporary directory, which is swept by the OS rather than growing without
/// bound.
pub(super) fn cache_dir() -> std::path::PathBuf {
    std::env::var("PLY_C_CACHE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("ply-c-cache"))
}

/// What decides the object: the source, the compiler and the flags it is given.
///
/// The source is the whole of the rest. It carries the prelude's layouts, the runtime table in the
/// order `ply_bind` will fill it, and every body -- so a change to any of them is a change here,
/// and a stale object cannot be loaded against a runtime that moved under it. The compiler's own
/// size and modification time go in because upgrading `cc` in place changes nothing else.
fn key_of(source: &str, level: &str) -> String {
    let cc = compiler();
    let stamp = std::fs::metadata(which(&cc).unwrap_or_default())
        .ok()
        .map(|m| {
            format!(
                "{}:{:?}",
                m.len(),
                m.modified().ok().map(|t| t
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0))
            )
        })
        .unwrap_or_default();
    let mut h = blake3::Hasher::new();
    for part in [cc.as_str(), level, stamp.as_str(), source] {
        h.update(part.as_bytes());
        h.update(&[0]);
    }
    h.finalize().to_hex().to_string()
}

/// The object a key names, if it is already built and loadable.
///
/// The whole-unit cache reaches this without the source: the source is a function of the same
/// inputs the unit key is taken over, so a worker that finds a unit entry has no reason to build
/// twenty-nine megabytes of C to discover the name of an object it already has.
pub(super) fn open_by_key(key: &str) -> Option<Library> {
    let path = cache_dir().join(format!("{key}.{}", ext()));
    path.is_file().then(|| Library::open(&path).ok()).flatten()
}

/// The key an assembled source and the current compiler settle on, so it can be recorded beside
/// the unit that produced it.
pub(super) fn object_key(source: &str) -> String {
    key_of(source, &opt_level())
}

/// The optimisation flag, in one place: three callers ask, and one of them asking differently
/// would have the unit cache record a key the object cache never writes.
fn opt_level() -> String {
    std::env::var("PLY_CC_OPT").unwrap_or_else(|_| "-O2".to_string())
}

fn ext() -> &'static str {
    if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    }
}

/// The compiler's path, so that its stamp can go in the key.
fn which(cc: &str) -> Option<std::path::PathBuf> {
    if cc.contains('/') {
        return Some(std::path::PathBuf::from(cc));
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(cc))
        .find(|p| p.is_file())
}

pub fn compile_and_load(source: &str, stem: &str) -> Result<Library> {
    let level = opt_level();
    let ext = ext();
    // A unit already compiled from this source, by this compiler, on these flags is this object:
    // load it rather than spend the process again. The emitted tier's compile is what keeps it off
    // the loop's path, and for the self-hosted front end it is tens of seconds -- every invocation,
    // because `crates/ply-codegen` persisted nothing across runs.
    let cache = cache_dir();
    let key = key_of(source, &level);
    let cached = cache.join(format!("{key}.{ext}"));
    // A cached object that will not load is not a reason to fail: it is a reason to build one.
    // Anything that could make it unloadable -- a truncated write, an OS upgrade -- is answered by
    // compiling again.
    if cached.is_file()
        && let Ok(library) = Library::open(&cached)
    {
        return Ok(library);
    }
    // A directory of its own per build, not per process. A run compiles the unit once per worker
    // plus a pre-flight, all in one process, so a path keyed on the process id alone had every
    // worker writing the same `unit.c` and loading the same object while its neighbour was still
    // writing it. What that looked like was not a crash: the losers failed to build, declined
    // every call they were offered, and the run went quietly on with the interpreter.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("ply-c-{}-{stem}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let c = dir.join("unit.c");
    let so = dir.join(format!("unit.{ext}"));
    std::fs::write(&c, source)?;
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
    // Into the cache by a rename, which is what makes two workers compiling the same unit at the
    // same time safe: each writes its own file and the last rename wins, and both are the same
    // bytes because the key is the source.
    let _ = std::fs::create_dir_all(&cache);
    let landed = if std::fs::rename(&so, &cached).is_ok() {
        // The C beside it only when somebody asked to read it: for the self-hosted front end the
        // source is a megabyte and the object is what the cache is for.
        if std::env::var("PLY_C_KEEP").is_ok() {
            let _ = std::fs::copy(&c, cache.join(format!("{key}.c")));
        }
        let _ = std::fs::remove_file(&c);
        let _ = std::fs::remove_dir(&dir);
        cached
    } else {
        so
    };
    Library::open(&landed)
}

impl Library {
    /// `dlopen` one object, whether it was compiled just now or last week.
    fn open(so: &std::path::Path) -> Result<Library> {
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
        Ok(Library {
            handle,
            path: so.to_path_buf(),
        })
    }
}
