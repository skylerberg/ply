use super::descriptor::{self, Descriptor};
use anyhow::{Context, Result, bail};
use std::collections::HashSet;
use std::ffi::{CString, c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

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
    Opened { handle: *mut c_void, image: Image },
    Linked(&'static Descriptor),
}

enum Image {
    Cached(PathBuf),
    Private,
}

impl Library {
    pub fn descriptor(&self) -> Option<&Descriptor> {
        match self.handle {
            Handle::Linked(unit) => Some(unit),
            Handle::Opened { handle, .. } => {
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
        let Handle::Opened { handle, image } = &self.handle else {
            return;
        };
        let mut held = held_images();
        unsafe { dlclose(*handle) };
        if let Image::Cached(identity) = image {
            held.remove(identity);
        }
    }
}

unsafe impl Send for Library {}

fn held_images() -> MutexGuard<'static, HashSet<PathBuf>> {
    static HELD: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    (HELD.get_or_init(|| Mutex::new(HashSet::new())).lock()).unwrap_or_else(|e| e.into_inner())
}

impl Library {
    pub fn open(so: &Path) -> Result<Library> {
        let identity = std::fs::canonicalize(so)
            .with_context(|| format!("the unit at {} cannot be found", so.display()))?;
        let (handle, image) = if held_images().insert(identity.clone()) {
            match opened(&identity) {
                Ok(handle) => (handle, Image::Cached(identity)),
                Err(reason) => {
                    held_images().remove(&identity);
                    return Err(reason);
                }
            }
        } else {
            (opened_copy(&identity)?, Image::Private)
        };
        Ok(Library {
            handle: Handle::Opened { handle, image },
            path: so.to_path_buf(),
        })
    }
}

fn opened_copy(identity: &Path) -> Result<*mut c_void> {
    static COPIES: AtomicU64 = AtomicU64::new(0);
    let copy = std::env::temp_dir().join(format!(
        "ply-image-{}-{}.so",
        std::process::id(),
        COPIES.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::copy(identity, &copy)
        .with_context(|| format!("the unit at {} cannot be copied", identity.display()))?;
    let handle = opened(&copy);
    let removed = std::fs::remove_file(&copy)
        .with_context(|| format!("the copy at {} cannot be removed", copy.display()));
    match (handle, removed) {
        (Ok(handle), Ok(())) => Ok(handle),
        (Ok(handle), Err(reason)) => {
            unsafe { dlclose(handle) };
            Err(reason)
        }
        (Err(reason), Ok(())) => Err(reason),
        (Err(reason), Err(left)) => bail!("{reason:#}; {left:#}"),
    }
}

fn opened(so: &Path) -> Result<*mut c_void> {
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
    Ok(handle)
}
