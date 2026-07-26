use crate::{Outcome, PassRecord, RUNTIME_VERSION};
use anyhow::Context;
use ply_hash::DefHash;
use ply_span::{Diagnostic, Symbol};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub(crate) const RESULTS_FILE: &str = "results.json";
const RESULTS_STEM: &str = "results";
pub(crate) const PASSES_FILE: &str = "passes.json";
const PASSES_STEM: &str = "passes";

/// Independent of [`RUNTIME_VERSION`]: the layout can change without
/// invalidating results, and results can be invalidated without the layout
/// changing.
///
/// Format 1 carried the pass records inline. They are the largest thing the
/// result cache holds — at ten thousand definitions they were seven of its nine
/// megabytes — and only a *failing* test ever reads one, so parsing them at
/// `Store::open` charged every green run for a file it was not going to look at.
/// Format 2 keeps them in [`PASSES_FILE`], read on the first question. A format
/// 1 file is still read, its records are carried over, and the first flush
/// rewrites both files; nothing is lost and no test re-runs.
const FORMAT: u32 = 2;
const FORMAT_INLINE_PASSES: u32 = 1;

const TEMP_SUFFIX: &str = ".tmp";

/// A temp file younger than this may belong to a concurrent writer that has not
/// reached its rename yet; deleting it would make that writer fail.
const STALE_TEMP_AGE: Duration = Duration::from_secs(60);

pub(crate) type Entries = BTreeMap<DefHash, Outcome>;
pub(crate) type Definitions = BTreeSet<DefHash>;
pub(crate) type Passes = BTreeMap<Symbol, PassRecord>;

/// `definitions` records which definitions a run has already seen. No test
/// outcome can stand in for it: a green test vouches for the definitions in
/// *its* closure, which says nothing about whether they are the ones that
/// changed under a red one.
#[derive(Default)]
pub(crate) struct Cache {
    pub(crate) results: Entries,
    pub(crate) definitions: Definitions,
    /// Non-empty only for a format 1 file, and only until the first flush.
    pub(crate) inline_passes: Passes,
    pub(crate) migrating: bool,
}

#[derive(Deserialize)]
struct CacheFile {
    format: u32,
    runtime_version: String,
    results: Entries,
    #[serde(default)]
    definitions: Definitions,
    #[serde(default)]
    passes: PassesRepr,
}

#[derive(Serialize)]
struct CacheFileRef<'a> {
    format: u32,
    runtime_version: &'a str,
    results: &'a Entries,
    definitions: &'a Definitions,
}

#[derive(Deserialize)]
struct PassesFile {
    format: u32,
    runtime_version: String,
    passes: PassesRepr,
}

#[derive(Serialize)]
struct PassesFileRef<'a> {
    format: u32,
    runtime_version: &'a str,
    passes: PassesRepr,
}

/// `Symbol` has no `serde` impl and should not grow one for this: a test key is
/// a string on disk and nothing else reads it back as a name.
type PassesRepr = BTreeMap<String, PassRecord>;

pub(crate) enum LoadError {
    Missing,
    Io(std::io::Error),
    Parse(serde_json::Error),
    Format(u32),
    Version(String),
}

impl LoadError {
    pub(crate) fn into_diagnostic(self, path: &Path) -> Diagnostic {
        let path = path.display();
        let d = match self {
            LoadError::Missing => Diagnostic::warning(
                crate::codes::CACHE_UNREADABLE,
                format!("no result cache at `{path}`"),
            ),
            LoadError::Io(e) => Diagnostic::warning(
                crate::codes::CACHE_UNREADABLE,
                format!("could not read the result cache `{path}`: {e}"),
            ),
            LoadError::Parse(e) => Diagnostic::warning(
                crate::codes::CACHE_CORRUPT,
                format!("the result cache `{path}` is corrupt: {e}"),
            ),
            LoadError::Format(found) => Diagnostic::warning(
                crate::codes::CACHE_CORRUPT,
                format!("the result cache `{path}` is format {found}, this build reads {FORMAT}"),
            ),
            LoadError::Version(found) => Diagnostic::warning(
                crate::codes::CACHE_VERSION_CHANGED,
                format!(
                    "the result cache `{path}` was written by runtime `{found}`, \
                     this build is `{RUNTIME_VERSION}`"
                ),
            ),
        };
        d.note("continuing with an empty cache; every test will re-run and the cache is rewritten")
    }

    /// Separate from [`LoadError::into_diagnostic`] because losing the pass
    /// records costs an attribution rather than a result: nothing re-runs, and a
    /// reader told otherwise would go looking for a cache miss that never
    /// happened.
    pub(crate) fn into_passes_diagnostic(self, path: &Path) -> Diagnostic {
        let display = path.display();
        let d = match self {
            LoadError::Missing => Diagnostic::warning(
                crate::codes::CACHE_UNREADABLE,
                format!("no pass records at `{display}`"),
            ),
            LoadError::Io(e) => Diagnostic::warning(
                crate::codes::CACHE_UNREADABLE,
                format!("could not read the pass records `{display}`: {e}"),
            ),
            LoadError::Parse(e) => Diagnostic::warning(
                crate::codes::CACHE_CORRUPT,
                format!("the pass records `{display}` are corrupt: {e}"),
            ),
            LoadError::Format(found) => Diagnostic::warning(
                crate::codes::CACHE_CORRUPT,
                format!(
                    "the pass records `{display}` are format {found}, this build reads {FORMAT}"
                ),
            ),
            LoadError::Version(found) => Diagnostic::warning(
                crate::codes::CACHE_VERSION_CHANGED,
                format!(
                    "the pass records `{display}` were written by runtime `{found}`, \
                     this build is `{RUNTIME_VERSION}`"
                ),
            ),
        };
        d.note(
            "no test re-runs and no result is lost, but until each test passes again \
             a failure is attributed as if it had never passed",
        )
    }
}

pub(crate) fn load(path: &Path) -> Result<Cache, LoadError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => return Err(LoadError::Missing),
        Err(e) => return Err(LoadError::Io(e)),
    };
    let file: CacheFile = serde_json::from_str(&text).map_err(LoadError::Parse)?;
    if file.format != FORMAT && file.format != FORMAT_INLINE_PASSES {
        return Err(LoadError::Format(file.format));
    }
    if file.runtime_version != RUNTIME_VERSION {
        return Err(LoadError::Version(file.runtime_version));
    }
    Ok(Cache {
        results: file.results,
        definitions: file.definitions,
        inline_passes: intern(file.passes),
        migrating: file.format == FORMAT_INLINE_PASSES,
    })
}

pub(crate) fn save(dir: &Path, path: &Path, cache: &Cache) -> anyhow::Result<()> {
    let file = CacheFileRef {
        format: FORMAT,
        runtime_version: RUNTIME_VERSION,
        results: &cache.results,
        definitions: &cache.definitions,
    };
    let mut bytes =
        serde_json::to_vec_pretty(&file).context("could not serialize the result cache")?;
    bytes.push(b'\n');
    write_atomic(dir, path, RESULTS_STEM, &bytes, "result cache")
}

pub(crate) fn load_passes(path: &Path) -> Result<Passes, LoadError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => return Err(LoadError::Missing),
        Err(e) => return Err(LoadError::Io(e)),
    };
    let file: PassesFile = serde_json::from_str(&text).map_err(LoadError::Parse)?;
    if file.format != FORMAT {
        return Err(LoadError::Format(file.format));
    }
    if file.runtime_version != RUNTIME_VERSION {
        return Err(LoadError::Version(file.runtime_version));
    }
    Ok(intern(file.passes))
}

pub(crate) fn save_passes(dir: &Path, path: &Path, passes: &Passes) -> anyhow::Result<()> {
    let file = PassesFileRef {
        format: FORMAT,
        runtime_version: RUNTIME_VERSION,
        passes: passes
            .iter()
            .map(|(key, record)| (key.to_string(), record.clone()))
            .collect(),
    };
    let mut bytes =
        serde_json::to_vec_pretty(&file).context("could not serialize the pass records")?;
    bytes.push(b'\n');
    write_atomic(dir, path, PASSES_STEM, &bytes, "pass records")
}

fn intern(repr: PassesRepr) -> Passes {
    repr.into_iter()
        .map(|(key, record)| (Symbol::new(key), record))
        .collect()
}

pub(crate) fn write_atomic(
    dir: &Path,
    path: &Path,
    stem: &str,
    bytes: &[u8],
    what: &str,
) -> anyhow::Result<()> {
    let temp = temp_path(dir, stem);
    if let Err(e) = write_new(&temp, bytes) {
        let _ = fs::remove_file(&temp);
        return Err(anyhow::Error::new(e)
            .context(format!("could not write the {what} `{}`", temp.display())));
    }

    if let Err(e) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(anyhow::Error::new(e)
            .context(format!("could not replace the {what} `{}`", path.display())));
    }

    // Without this the rename can be lost by a crash even though the data was
    // synced; the cache would then silently revert to its previous contents.
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut f = OpenOptions::new().write(true).create_new(true).open(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

pub(crate) fn temp_path(dir: &Path, stem: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    dir.join(format!(
        "{stem}.{}.{seq}.{nanos}{TEMP_SUFFIX}",
        std::process::id()
    ))
}

pub(crate) fn sweep_temps(dir: &Path, max_age: Option<Duration>) {
    let Ok(read) = fs::read_dir(dir) else { return };
    for entry in read.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let ours = [RESULTS_STEM, PASSES_STEM, crate::frontend::FRONTEND_STEM]
            .iter()
            .any(|stem| name.starts_with(&format!("{stem}.")));
        if !ours || !name.ends_with(TEMP_SUFFIX) {
            continue;
        }
        if let Some(max_age) = max_age
            && !is_older_than(&entry.path(), max_age)
        {
            continue;
        }
        let _ = fs::remove_file(entry.path());
    }
}

pub(crate) fn sweep_stale_temps(dir: &Path) {
    sweep_temps(dir, Some(STALE_TEMP_AGE));
}

const LOCK_FILE: &str = "lock";
const LOCK_WAIT: Duration = Duration::from_secs(2);
const LOCK_POLL: Duration = Duration::from_millis(2);

/// Far longer than a write takes, so only a lock left by a killed process is
/// ever broken.
const LOCK_STALE_AGE: Duration = Duration::from_secs(30);

/// Serializes the read-merge-write in `Store::flush` across processes, which
/// rename alone cannot do: two writers can otherwise read the same cache and the
/// second rename drops the first one's entries.
///
/// Waiting is bounded and giving up is not an error — a caller that proceeds
/// unlocked risks losing a concurrent writer's entries, which costs a re-run,
/// but still cannot produce a torn file.
pub(crate) struct Lock {
    path: PathBuf,
    pub(crate) held: bool,
}

impl Lock {
    pub(crate) fn acquire(dir: &Path) -> Lock {
        Lock::acquire_within(dir, LOCK_WAIT)
    }

    pub(crate) fn acquire_within(dir: &Path, wait: Duration) -> Lock {
        let path = dir.join(LOCK_FILE);
        let deadline = Instant::now() + wait;
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Lock { path, held: true },
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
                Err(_) => return Lock { path, held: false },
            }
            if Instant::now() >= deadline {
                return Lock { path, held: false };
            }
            if is_older_than(&path, LOCK_STALE_AGE) {
                let _ = fs::remove_file(&path);
            }
            std::thread::sleep(LOCK_POLL);
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        if self.held {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub(crate) fn is_older_than(path: &Path, age: Duration) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|m| m.elapsed().map(|elapsed| elapsed >= age).unwrap_or(false))
        .unwrap_or(false)
}
