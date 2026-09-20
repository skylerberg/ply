//! Compiling a unit's C and loading it: one object per bucket of bodies, each kept by the digest
//! of its C so an edit recompiles the bucket it reached, and one link into the image.

use super::prelude::RUNTIME_MARK;
use super::tables::BUCKET_MARK;
use anyhow::{Result, anyhow, bail};
use std::ffi::{CString, c_char, c_int, c_void};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

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
/// machine already has. Which one, and on what flag, is the profile's answer -- see `toolchain.rs`.
fn compiler() -> String {
    super::toolchain::Profile::current().compiler()
}

/// Where compiled units are kept between runs. `PLY_C_CACHE` names another directory; the default
/// is under the system's temporary directory. Images sit at its root, bucket objects under
/// `obj/`, emitted bodies under `emit/`.
///
/// **Nothing about that directory bounds it**: an entry is keyed by its content, so a changed
/// definition writes a new one beside the old rather than replacing it, and the system's own
/// sweep runs on a schedule measured in days. `super::sweep` is what bounds it, at the start of
/// a build, and `PLY_C_CACHE_MAX` is the bound.
pub(super) fn cache_dir() -> std::path::PathBuf {
    std::env::var("PLY_C_CACHE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("ply-c-cache"))
}

/// What decides an object: the source, the compiler and the flags it is given.
///
/// The source is the whole of the rest. It carries the prelude's layouts, the runtime table in the
/// order `ply_bind` will fill it, and every body -- so a change to any of them is a change here,
/// and a stale object cannot be loaded against a runtime that moved under it. The compiler's own
/// size and modification time go in because upgrading `cc` in place changes nothing else.
pub(super) fn key_of(cc: &str, source: &str, level: &str) -> String {
    key_over(cc, level, &[source])
}

/// [`key_of`] for a source given in pieces: the key of their concatenation, without making it.
fn key_over(cc: &str, level: &str, pieces: &[&str]) -> String {
    let stamp = std::fs::metadata(which(cc).unwrap_or_default())
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
    for part in [cc, level, stamp.as_str()] {
        h.update(part.as_bytes());
        h.update(&[0]);
    }
    for piece in pieces {
        h.update(piece.as_bytes());
    }
    h.update(&[0]);
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
    key_of(&compiler(), source, &opt_level())
}

/// The optimisation flag, in one place: three callers ask, and one of them asking differently
/// would have the unit cache record a key the object cache never writes.
fn opt_level() -> String {
    super::toolchain::Profile::current().opt_level()
}

pub(super) fn ext() -> &'static str {
    if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    }
}

/// The compiler's path: its stamp goes in the key, and `toolchain::extra_args` reads what sits
/// beside it.
pub(super) fn which(cc: &str) -> Option<std::path::PathBuf> {
    if cc.contains('/') {
        return Some(std::path::PathBuf::from(cc));
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(cc))
        .find(|p| p.is_file())
}

/// A unit's text cut where `build::assemble` marked it. Each bucket, and the tail, compiles as
/// the header followed by itself; the pieces concatenate back to the text byte for byte.
pub struct Parts<'a> {
    /// Up to the first bucket mark: the prelude, the runtime declared, the prototypes.
    pub header: &'a str,
    /// Each bucket's id and its text, from its mark line to the next.
    pub buckets: Vec<(u8, &'a str)>,
    /// From [`RUNTIME_MARK`]: the runtime's definitions and the embedded exports.
    pub tail: &'a str,
}

/// Where `mark` begins a line of `text`, if it does.
fn line_start(text: &str, mark: &str) -> Option<usize> {
    if text.starts_with(mark) {
        return Some(0);
    }
    text.find(&format!("\n{mark}")).map(|at| at + 1)
}

/// A text with no marks is one part, its header; a text with no bucket marks but the runtime's
/// is a header and a tail.
pub fn split(text: &str) -> Result<Parts<'_>> {
    let tail_at = line_start(text, RUNTIME_MARK).unwrap_or(text.len());
    let mut starts: Vec<usize> = Vec::new();
    let mut from = 0;
    while let Some(at) = line_start(&text[from..tail_at], BUCKET_MARK) {
        starts.push(from + at);
        from += at + BUCKET_MARK.len();
    }
    let mut buckets = Vec::with_capacity(starts.len());
    for (i, &start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(tail_at);
        let part = &text[start..end];
        let line = part.lines().next().unwrap_or_default();
        let id = line
            .strip_prefix(BUCKET_MARK)
            .and_then(|rest| rest.strip_suffix(" --- */"))
            .and_then(|digits| u8::from_str_radix(digits, 16).ok())
            .ok_or_else(|| {
                anyhow!("the unit's C has a bucket mark this loader cannot read: `{line}`")
            })?;
        buckets.push((id, part));
    }
    Ok(Parts {
        header: &text[..starts.first().copied().unwrap_or(tail_at)],
        buckets,
        tail: &text[tail_at..],
    })
}

pub fn compile_and_load(source: &str, stem: &str) -> Result<Library> {
    Ok(compile_and_load_timed(source, stem)?.0)
}

/// Compile `source` into a shared object and load it, with the time its objects took to compile,
/// apart from the link and the load; zero when the image was already in the cache.
///
/// One image per unit: `benches/c-floor/` found one link a constant rather than an exponent, and
/// the per-definition image it refused. The objects that link into it are one per bucket, each
/// keyed by its C and kept under `obj/`, so an edit that reached one bucket compiles one bucket.
pub(super) fn compile_and_load_timed(source: &str, stem: &str) -> Result<(Library, Duration)> {
    // The other place the cache is written, and the one that writes the large files. A run that
    // only loads a bootstrap bundle never reaches `build`, and would otherwise add an object per
    // run to a directory nothing swept. `sweep::once` is what makes calling it twice free.
    super::sweep::once();
    let level = opt_level();
    let ext = ext();
    // A unit already compiled from this source, by this compiler, on these flags is this object:
    // load it rather than spend the process again. The emitted tier's compile is what keeps it off
    // the loop's path, and for the self-hosted front end it is tens of seconds -- every invocation,
    // because `crates/ply-codegen` persisted nothing across runs.
    let cache = cache_dir();
    let cc = compiler();
    let key = key_of(&cc, source, &level);
    let cached = cache.join(format!("{key}.{ext}"));
    // A cached object that will not load is not a reason to fail: it is a reason to build one.
    // Anything that could make it unloadable -- a truncated write, an OS upgrade -- is answered by
    // compiling again.
    if cached.is_file()
        && let Ok(library) = Library::open(&cached)
    {
        return Ok((library, Duration::ZERO));
    }
    // A directory of its own per build, not per process. A run compiles the unit once per worker
    // plus a pre-flight, all in one process, so a path keyed on the process id alone had every
    // worker writing the same `unit.c` and loading the same object while its neighbour was still
    // writing it. What that looked like was not a crash: the losers failed to build, declined
    // every call they were offered, and the run went quietly on with the interpreter.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Relaxed);
    let dir = std::env::temp_dir().join(format!("ply-c-{}-{stem}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let support = super::toolchain::extra_args(&cc);
    let toolchain = Toolchain {
        cc: &cc,
        support: &support,
        level: &level,
    };
    let started = Instant::now();
    let objects = objects_of(&split(source)?, &toolchain, &cache.join("obj"), &dir)?;
    let compiling = started.elapsed();
    let so = dir.join(format!("unit.{ext}"));
    let out = std::process::Command::new(&cc)
        .args(&support)
        .arg("-shared")
        .arg("-o")
        .arg(&so)
        .args(&objects)
        .output()
        .map_err(|e| anyhow!("could not run {cc}: {e}"))?;
    if !out.status.success() {
        bail!(
            "the C tier's linker refused the unit's objects ({}):\n{}",
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
            let _ = std::fs::write(cache.join(format!("{key}.c")), source);
        }
        let _ = std::fs::remove_dir_all(&dir);
        cached
    } else {
        so
    };
    // An object this run just built and cannot load is worth a sentence about the compiler that
    // built it. tcc is the one that does this: it finds `libtcc1.a` relative to `-B` and a build
    // that is not installed has no default that finds it, so the *compile succeeds* and `dlopen`
    // refuses with an empty reason. `toolchain::extra_args` supplies the flag when it can see
    // where the library is; when it cannot, this is what says so.
    let library = Library::open(&landed).map_err(|e| match cc.ends_with("tcc") && support.is_empty() {
        true => e.context(format!(
            "`{cc}` compiled the unit but no `libtcc1.a` was found beside it or in `../lib/tcc`,              so the object is missing the support routines every tcc object needs; pass              `-B<directory holding libtcc1.a>` through a wrapper named by `PLY_CC`"
        )),
        false => e,
    })?;
    Ok((library, compiling))
}

struct Toolchain<'a> {
    cc: &'a str,
    /// What this compiler needs on every invocation, the link included: tcc's `-B`.
    support: &'a [String],
    level: &'a str,
}

/// An object for each part, in link order: found under `obj` when this compiler already built one
/// from this C, else compiled now, several at a time, and put there.
fn objects_of(
    parts: &Parts<'_>,
    toolchain: &Toolchain<'_>,
    obj: &Path,
    dir: &Path,
) -> Result<Vec<PathBuf>> {
    let _ = std::fs::create_dir_all(obj);
    let mut texts: Vec<(String, &str)> = parts
        .buckets
        .iter()
        .map(|(id, text)| (format!("{id:02x}"), *text))
        .collect();
    texts.push(("runtime".to_string(), parts.tail));
    let mut objects: Vec<PathBuf> = Vec::with_capacity(texts.len());
    let mut missing: Vec<(usize, &str, &str, PathBuf)> = Vec::new();
    for (label, text) in &texts {
        let target = obj.join(format!(
            "{}.o",
            key_over(toolchain.cc, toolchain.level, &[parts.header, *text])
        ));
        if !target.is_file() {
            missing.push((objects.len(), label.as_str(), *text, target.clone()));
        }
        objects.push(target);
    }
    super::cache::BUCKETS_REUSED.fetch_add(texts.len() - missing.len(), Relaxed);
    let landed: Mutex<Vec<(usize, Result<PathBuf>)>> = Mutex::new(Vec::new());
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..missing.len().min(parallelism()) {
            s.spawn(|| {
                while let Some((at, label, text, target)) = missing.get(next.fetch_add(1, Relaxed))
                {
                    let built = compile_one(toolchain, dir, label, parts.header, text, target);
                    landed
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push((*at, built));
                }
            });
        }
    });
    for (at, built) in landed.into_inner().unwrap_or_else(|e| e.into_inner()) {
        objects[at] = built?;
    }
    super::cache::BUCKETS_COMPILED.fetch_add(missing.len(), Relaxed);
    Ok(objects)
}

/// One part's object: `header` then `text` written as `<label>.c` under `dir`, compiled beside
/// it, and moved to `target`.
fn compile_one(
    toolchain: &Toolchain<'_>,
    dir: &Path,
    label: &str,
    header: &str,
    text: &str,
    target: &Path,
) -> Result<PathBuf> {
    let c = dir.join(format!("{label}.c"));
    let o = dir.join(format!("{label}.o"));
    {
        let mut file = std::fs::File::create(&c)?;
        file.write_all(header.as_bytes())?;
        file.write_all(text.as_bytes())?;
    }
    let out = {
        let _slot = slot();
        std::process::Command::new(toolchain.cc)
            .args(toolchain.support)
            .arg(toolchain.level)
            .arg("-fPIC")
            .arg("-fno-strict-aliasing")
            .arg("-c")
            .arg("-o")
            .arg(&o)
            .arg(&c)
            .output()
            .map_err(|e| anyhow!("could not run {}: {e}", toolchain.cc))?
    };
    if !out.status.success() {
        bail!(
            "the C tier's compiler refused the unit it emitted ({}):\n{}",
            c.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let _ = std::fs::remove_file(&c);
    Ok(land(&o, target))
}

/// Into the cache by a rename, or by a copy and a rename when the cache is on another file
/// system, so a reader never sees half an object; where neither works, the object stays where
/// it was built and links from there.
fn land(built: &Path, target: &Path) -> PathBuf {
    if std::fs::rename(built, target).is_ok() {
        return target.to_path_buf();
    }
    let tmp = target.with_extension(format!("{}.otmp", std::process::id()));
    if std::fs::copy(built, &tmp).is_ok() && std::fs::rename(&tmp, target).is_ok() {
        let _ = std::fs::remove_file(built);
        return target.to_path_buf();
    }
    let _ = std::fs::remove_file(&tmp);
    built.to_path_buf()
}

fn parallelism() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// The compiler processes running at once, bounded across every worker of this process by the
/// machine's parallelism: each worker builds its own copy of the unit, and sixty-four `cc`s per
/// worker is not what the bound is for.
struct Slots {
    free: Mutex<usize>,
    freed: Condvar,
}

static SLOTS: OnceLock<Slots> = OnceLock::new();

struct Slot;

fn slot() -> Slot {
    let slots = SLOTS.get_or_init(|| Slots {
        free: Mutex::new(parallelism()),
        freed: Condvar::new(),
    });
    let mut free = slots.free.lock().unwrap_or_else(|e| e.into_inner());
    while *free == 0 {
        free = slots.freed.wait(free).unwrap_or_else(|e| e.into_inner());
    }
    *free -= 1;
    Slot
}

impl Drop for Slot {
    fn drop(&mut self) {
        let slots = SLOTS.get().expect("a slot was taken from the table");
        *slots.free.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        slots.freed.notify_one();
    }
}

impl Library {
    /// `dlopen` one object, whether it was compiled just now or last week.
    pub(super) fn open(so: &std::path::Path) -> Result<Library> {
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
