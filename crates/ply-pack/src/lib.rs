//! What a `ply` binary carries besides its Rust: the shipped modules, the builder, and the `ply`
//! program's sources and runnable, each a file keyed by its path in the repository. `ply-pack`
//! appends them to the binary the Rust build made, so a change to them never builds Rust again;
//! the binary maps them back at startup.
//!
//! A pack is the files' bytes, then a table of `(path, offset, length, digest)`, then a trailer of
//! the pack's and the table's lengths and [`MAGIC`].

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// The shipped modules: each `.ply` file at or below it is one, `hash/legacy.ply` being
/// `std.hash.legacy`. Every other file at or below it is data those modules embed.
pub const STD: &str = "crates/ply-std/ply";

const MODULE: &str = ".ply";

/// The compiler's own modules, shipped as `compiler.<stem>`, and the builtins it embeds.
pub const COMPILER: &str = "crates/ply-compiler/ply";
pub const PRELUDE: &str = "crates/ply-compiler/prelude.ply";

/// Where `ply bootstrap` commits the builder and the `ply` program, and what it writes there.
pub const BUILDER_BUILT: &str = "crates/ply-compiler/bootstrap";
pub const PROGRAM_BUILT: &str = "crates/ply-cli/bootstrap";
const BUILDER_FILES: [&str; 3] = ["build.run", "build.digest", "build.key"];
const PROGRAM_FILES: [&str; 3] = ["ply.run", "ply.digest", "ply.key"];

/// The `ply` program's package; it and every package it depends on by path ship.
pub const PROGRAM: &str = "crates/ply-cli/ply";

pub const MANIFEST: &str = "ply.pkg";

/// What a trace names a directory's data by, after the directory.
const DATA_BELOW: &str = "/**";

/// What a trace names the modules at or below a directory by, after the directory.
const MODULES_BELOW: &str = "/**.ply";

const MAGIC: &[u8; 8] = b"PLYPACK1";
const TRAILER: usize = 8 + 8 + MAGIC.len();

pub type Digest = [u8; 32];

pub struct Pack {
    /// Ascending by path.
    entries: Vec<Entry>,
    mapped: Option<memmap2::Mmap>,
}

struct Entry {
    path: String,
    content: Content,
    digest: OnceLock<Digest>,
}

enum Content {
    Mapped {
        at: usize,
        len: usize,
    },
    File {
        at: PathBuf,
        read: OnceLock<Vec<u8>>,
    },
}

impl Pack {
    /// What a binary packed from the checkout at `repo` would carry. Listed now and read on first
    /// use, so a process that asks for one module reads one file.
    pub fn of_checkout(repo: &Path) -> Result<Pack, String> {
        let mut paths: Vec<String> = Vec::new();
        paths.extend(below(repo, STD)?);
        paths.extend(files_in(repo, COMPILER, |name| name.ends_with(MODULE))?);
        paths.push(PRELUDE.to_string());
        paths.extend(bootstrap(repo, BUILDER_BUILT, &BUILDER_FILES)?);
        for package in packages(PROGRAM, |manifest| {
            std::fs::read_to_string(repo.join(manifest)).ok()
        })? {
            paths.extend(files_in(repo, &package, |name| {
                name.ends_with(MODULE) || name == MANIFEST
            })?);
        }
        paths.extend(bootstrap(repo, PROGRAM_BUILT, &PROGRAM_FILES)?);
        for required in [
            PRELUDE.to_string(),
            format!("{BUILDER_BUILT}/build.run"),
            format!("{BUILDER_BUILT}/build.digest"),
        ] {
            if !paths.contains(&required) || !repo.join(&required).is_file() {
                return Err(format!(
                    "`{required}` is not in the checkout at {}",
                    repo.display()
                ));
            }
        }
        paths.sort();
        paths.dedup();
        Ok(Pack {
            entries: paths
                .into_iter()
                .map(|path| Entry {
                    content: Content::File {
                        at: repo.join(&path),
                        read: OnceLock::new(),
                    },
                    path,
                    digest: OnceLock::new(),
                })
                .collect(),
            mapped: None,
        })
    }

    /// The pack appended to the binary at `binary`, or `None` when it carries none.
    pub fn of_binary(binary: &Path) -> Result<Option<Pack>, String> {
        let file = std::fs::File::open(binary)
            .map_err(|e| format!("`{}` could not be opened: {e}", binary.display()))?;
        // SAFETY: a binary is not written while it runs; `ply-pack` lands a new one by a rename.
        let mapped = unsafe { memmap2::Mmap::map(&file) }
            .map_err(|e| format!("`{}` could not be mapped: {e}", binary.display()))?;
        let Some((start, table)) = located(&mapped)? else {
            return Ok(None);
        };
        let entries = read_table(&mapped[table..mapped.len() - TRAILER], start, table)?;
        Ok(Some(Pack {
            entries,
            mapped: Some(mapped),
        }))
    }

    pub fn paths(&self) -> impl Iterator<Item = &str> {
        asked(|a| a.every = true);
        self.listed()
    }

    fn listed(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|entry| entry.path.as_str())
    }

    /// The paths of the files directly in `dir`, ascending.
    pub fn files_in<'a>(&'a self, dir: &str) -> impl Iterator<Item = &'a str> + 'a {
        asked(|a| {
            a.dirs.insert(dir.to_string());
        });
        self.directly_in(dir)
    }

    fn directly_in<'a>(&'a self, dir: &str) -> impl Iterator<Item = &'a str> + 'a {
        let prefix = format!("{dir}/");
        self.listed().filter(move |path| {
            path.strip_prefix(prefix.as_str())
                .is_some_and(|name| !name.contains('/'))
        })
    }

    /// The paths of the data files at or below `dir`, ascending: every file there that is no
    /// module.
    pub fn data_below<'a>(&'a self, dir: &str) -> impl Iterator<Item = &'a str> + 'a {
        asked(|a| {
            a.dirs.insert(format!("{dir}{DATA_BELOW}"));
        });
        self.below_in(dir, false)
    }

    /// The paths of the modules at or below `dir`, ascending.
    pub fn modules_below<'a>(&'a self, dir: &str) -> impl Iterator<Item = &'a str> + 'a {
        asked(|a| {
            a.dirs.insert(format!("{dir}{MODULES_BELOW}"));
        });
        self.below_in(dir, true)
    }

    fn below_in<'a>(&'a self, dir: &str, modules: bool) -> impl Iterator<Item = &'a str> + 'a {
        let prefix = format!("{dir}/");
        self.listed().filter(move |path| {
            path.starts_with(prefix.as_str()) && path.ends_with(MODULE) == modules
        })
    }

    /// What this process asked of the pack since [`record`], as trace lines: `pack\t<path>\t<digest>`
    /// a file read, `packed\t<dir>\t<digest>` a directory listed, `<dir>/**` for the data below one,
    /// `<dir>/**.ply` for the modules below one and `*` for every path.
    pub fn asked_lines(&self) -> Vec<String> {
        let Some(asked) = ASKED.get() else {
            return Vec::new();
        };
        let asked = asked.lock().unwrap_or_else(|e| e.into_inner());
        let mut lines: Vec<String> = asked
            .paths
            .iter()
            .map(|path| format!("pack\t{path}\t{}", self.answer_of(path)))
            .collect();
        for dir in &asked.dirs {
            lines.push(format!("packed\t{dir}\t{}", self.listing_of(dir)));
        }
        if asked.every {
            lines.push(format!("packed\t*\t{}", self.listing_of("*")));
        }
        lines
    }

    /// Whether a trace line this pack answers still answers as it did; `None` for a line it does not.
    pub fn stands(&self, line: &str) -> Option<bool> {
        match line.split('\t').collect::<Vec<_>>().as_slice() {
            ["pack", path, digest] => Some(self.answer_of(path) == *digest),
            ["packed", dir, digest] => Some(self.listing_of(dir) == *digest),
            _ => None,
        }
    }

    fn answer_of(&self, path: &str) -> String {
        match self.entry_quietly(path) {
            Some(entry) => blake3::Hash::from(self.entry_digest(entry))
                .to_hex()
                .to_string(),
            None => "none".to_string(),
        }
    }

    fn listing_of(&self, dir: &str) -> String {
        let mut hasher = blake3::Hasher::new();
        let names: Vec<&str> = if dir == "*" {
            self.listed().collect()
        } else if let Some(dir) = dir.strip_suffix(MODULES_BELOW) {
            self.below_in(dir, true).collect()
        } else if let Some(dir) = dir.strip_suffix(DATA_BELOW) {
            self.below_in(dir, false).collect()
        } else {
            self.directly_in(dir).collect()
        };
        for name in names {
            hasher.update(name.as_bytes());
            hasher.update(&[0]);
        }
        hasher.finalize().to_hex().to_string()
    }

    pub fn bytes(&self, path: &str) -> Option<&[u8]> {
        self.entry(path).map(|entry| self.content(entry))
    }

    /// A text the pack carries. The sources are UTF-8, as the front end reads them.
    pub fn text(&self, path: &str) -> Option<&str> {
        self.bytes(path).map(|bytes| {
            std::str::from_utf8(bytes)
                .unwrap_or_else(|e| panic!("`{path}` in the pack is not UTF-8: {e}"))
        })
    }

    pub fn digest_of(&self, path: &str) -> Option<Digest> {
        self.entry(path).map(|entry| self.entry_digest(entry))
    }

    /// The whole pack: every path and what its bytes digest to.
    pub fn digest(&self) -> Digest {
        let mut hasher = blake3::Hasher::new();
        for entry in &self.entries {
            hasher.update(&(entry.path.len() as u64).to_le_bytes());
            hasher.update(entry.path.as_bytes());
            hasher.update(&self.entry_digest(entry));
        }
        *hasher.finalize().as_bytes()
    }

    /// The packages the `ply` program is built from: [`PROGRAM`], then each it depends on by path.
    pub fn program_packages(&self) -> Vec<String> {
        packages(PROGRAM, |manifest| self.text(manifest).map(str::to_string))
            .unwrap_or_else(|why| panic!("the pack's own manifests do not resolve: {why}"))
    }

    /// The bytes [`append`] writes after a binary: every file, the table, the trailer.
    pub fn encoded(&self) -> Vec<u8> {
        let mut data = Vec::new();
        let mut table = Vec::new();
        table.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for entry in &self.entries {
            let bytes = self.content(entry);
            table.extend_from_slice(&(entry.path.len() as u32).to_le_bytes());
            table.extend_from_slice(entry.path.as_bytes());
            table.extend_from_slice(&(data.len() as u64).to_le_bytes());
            table.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            table.extend_from_slice(&self.entry_digest(entry));
            data.extend_from_slice(bytes);
        }
        let pack_len = (data.len() + table.len()) as u64;
        let table_len = table.len() as u64;
        data.extend_from_slice(&table);
        data.extend_from_slice(&pack_len.to_le_bytes());
        data.extend_from_slice(&table_len.to_le_bytes());
        data.extend_from_slice(MAGIC);
        data
    }

    fn entry(&self, path: &str) -> Option<&Entry> {
        asked(|a| {
            a.paths.insert(path.to_string());
        });
        self.entry_quietly(path)
    }

    fn entry_quietly(&self, path: &str) -> Option<&Entry> {
        self.entries
            .binary_search_by(|entry| entry.path.as_str().cmp(path))
            .ok()
            .map(|at| &self.entries[at])
    }

    fn content<'a>(&'a self, entry: &'a Entry) -> &'a [u8] {
        match &entry.content {
            Content::Mapped { at, len } => {
                &self.mapped.as_ref().expect("a mapped entry has its map")[*at..*at + *len]
            }
            Content::File { at, read } => read.get_or_init(|| {
                std::fs::read(at).unwrap_or_else(|e| {
                    panic!("`{}` was listed for the pack and reads: {e}", at.display())
                })
            }),
        }
    }

    fn entry_digest(&self, entry: &Entry) -> Digest {
        *entry
            .digest
            .get_or_init(|| *blake3::hash(self.content(entry)).as_bytes())
    }
}

/// Where a pack starts and where its table does, when `binary` ends in one.
fn located(binary: &[u8]) -> Result<Option<(usize, usize)>, String> {
    if binary.len() < TRAILER || &binary[binary.len() - MAGIC.len()..] != MAGIC {
        return Ok(None);
    }
    let trailer = binary.len() - TRAILER;
    let word = |at: usize| u64::from_le_bytes(binary[at..at + 8].try_into().expect("eight bytes"));
    let (pack_len, table_len) = (word(trailer) as usize, word(trailer + 8) as usize);
    if pack_len > trailer || table_len > pack_len {
        return Err("the pack's trailer names more bytes than the binary holds".to_string());
    }
    Ok(Some((trailer - pack_len, trailer - table_len)))
}

fn read_table(table: &[u8], start: usize, end: usize) -> Result<Vec<Entry>, String> {
    let torn = || "the pack's table is torn".to_string();
    let mut at = 0;
    let mut take = |n: usize| -> Result<&[u8], String> {
        let got = table.get(at..at + n).ok_or_else(torn)?;
        at += n;
        Ok(got)
    };
    let count = u32::from_le_bytes(take(4)?.try_into().expect("four bytes")) as usize;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let path_len = u32::from_le_bytes(take(4)?.try_into().expect("four bytes")) as usize;
        let path = String::from_utf8(take(path_len)?.to_vec()).map_err(|_| torn())?;
        let offset = u64::from_le_bytes(take(8)?.try_into().expect("eight bytes")) as usize;
        let len = u64::from_le_bytes(take(8)?.try_into().expect("eight bytes")) as usize;
        let digest: Digest = take(32)?.try_into().expect("thirty-two bytes");
        if start + offset + len > end {
            return Err(torn());
        }
        entries.push(Entry {
            path,
            content: Content::Mapped {
                at: start + offset,
                len,
            },
            digest: OnceLock::from(digest),
        });
    }
    if entries.windows(2).any(|pair| pair[0].path >= pair[1].path) {
        return Err("the pack's table is not in ascending order".to_string());
    }
    Ok(entries)
}

/// `binary` with `pack` after it in place of any it carried, landed by a rename so a process
/// running the old one keeps its file.
pub fn append(binary: &Path, pack: &Pack) -> Result<(), String> {
    let bytes = std::fs::read(binary)
        .map_err(|e| format!("`{}` could not be read: {e}", binary.display()))?;
    let runtime = match located(&bytes)? {
        Some((start, _)) => &bytes[..start],
        None => &bytes[..],
    };
    let permissions = std::fs::metadata(binary)
        .map_err(|e| format!("`{}` has no metadata: {e}", binary.display()))?
        .permissions();
    let aside = binary.with_extension(format!("pack.{}", std::process::id()));
    let written = std::fs::write(&aside, [runtime, &pack.encoded()].concat())
        .and_then(|()| std::fs::set_permissions(&aside, permissions))
        .and_then(|()| std::fs::rename(&aside, binary));
    written.map_err(|e| {
        let _ = std::fs::remove_file(&aside);
        format!("`{}` could not be packed: {e}", binary.display())
    })
}

/// What `binary` carries, against `wanted`.
pub enum Checked {
    Same,
    /// The first path, in order, the two packs disagree on.
    Differs(String),
    Absent,
}

/// `check` reads every byte the binary carries rather than trusting its table, so a binary torn
/// or edited after it was packed is refused.
pub fn check(binary: &Path, wanted: &Pack) -> Result<Checked, String> {
    let Some(carried) = Pack::of_binary(binary)? else {
        return Ok(Checked::Absent);
    };
    for entry in &carried.entries {
        if *blake3::hash(carried.content(entry)).as_bytes() != carried.entry_digest(entry) {
            return Err(format!(
                "`{}` in the pack `{}` carries is not what its table says",
                entry.path,
                binary.display()
            ));
        }
    }
    if carried.digest() == wanted.digest() {
        return Ok(Checked::Same);
    }
    let mut paths: Vec<&str> = carried.paths().chain(wanted.paths()).collect();
    paths.sort_unstable();
    paths.dedup();
    let differs = paths
        .into_iter()
        .find(|path| carried.digest_of(path) != wanted.digest_of(path))
        .expect("two packs whose digests differ differ on a path");
    Ok(Checked::Differs(differs.to_string()))
}

/// The checkout `dir` is in: the nearest directory holding the shipped modules.
pub fn checkout_around(dir: &Path) -> Result<PathBuf, String> {
    dir.ancestors()
        .find(|at| at.join(STD).is_dir())
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("`{}` is in no Ply checkout", dir.display()))
}

static INSTALLED: OnceLock<Pack> = OnceLock::new();

#[derive(Default)]
struct Asked {
    paths: BTreeSet<String>,
    dirs: BTreeSet<String>,
    every: bool,
}

static ASKED: OnceLock<Mutex<Asked>> = OnceLock::new();

/// From now on, what this process asks of any pack is noted, for [`Pack::asked_lines`].
pub fn record() {
    ASKED.get_or_init(Mutex::default);
}

thread_local! {
    static QUIET: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// `f` run without what it asks of the pack being noted: for what only keys a cache, whose
/// contents are a product of their key.
pub fn unrecorded<T>(f: impl FnOnce() -> T) -> T {
    let was = QUIET.with(|q| q.replace(true));
    let out = f();
    QUIET.with(|q| q.set(was));
    out
}

fn asked(f: impl FnOnce(&mut Asked)) {
    if QUIET.with(std::cell::Cell::get) {
        return;
    }
    if let Some(asked) = ASKED.get() {
        f(&mut asked.lock().unwrap_or_else(|e| e.into_inner()));
    }
}

/// The pack this process reads: the `ply` binary installs its own before anything runs, and a test
/// binary the checkout's.
pub fn install(pack: Pack) {
    if INSTALLED.set(pack).is_err() {
        panic!("a process installs one pack");
    }
}

pub fn installed() -> &'static Pack {
    INSTALLED
        .get()
        .expect("no pack is installed: `ply` installs its own, and a test binary the checkout's")
}

/// The checkout this test binary was built in, installed as its pack.
pub fn install_checkout(repo: &Path) {
    match Pack::of_checkout(repo) {
        Ok(pack) => install(pack),
        Err(why) => panic!("the checkout's pack: {why}"),
    }
}

/// The paths of the files in `dir` whose names `keep`, ascending.
fn files_in(repo: &Path, dir: &str, keep: impl Fn(&str) -> bool) -> Result<Vec<String>, String> {
    let entries = std::fs::read_dir(repo.join(dir))
        .map_err(|e| format!("`{dir}` could not be listed: {e}"))?;
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("`{dir}` could not be listed: {e}"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with('.') && entry.path().is_file() && keep(&name) {
            out.push(format!("{dir}/{name}"));
        }
    }
    out.sort();
    Ok(out)
}

/// The paths of the files at or below `dir`, ascending: its modules and the data they embed.
/// Nothing under a name starting with `.` is either, as no walk of a project reads there.
fn below(repo: &Path, dir: &str) -> Result<Vec<String>, String> {
    let unlisted = |e: std::io::Error| format!("`{dir}` could not be listed: {e}");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(repo.join(dir)).map_err(unlisted)? {
        let entry = entry.map_err(unlisted)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = format!("{dir}/{name}");
        if entry.path().is_dir() {
            out.extend(below(repo, &path)?);
        } else if entry.path().is_file() {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

/// What `ply bootstrap` wrote into `dir`. Anything else there is a file nothing reads.
fn bootstrap(repo: &Path, dir: &str, written: &[&str]) -> Result<Vec<String>, String> {
    let found = files_in(repo, dir, |_| true)?;
    if let Some(stray) = found.iter().find(|path| {
        !written
            .iter()
            .any(|name| path.ends_with(&format!("/{name}")))
    }) {
        return Err(format!(
            "`{stray}` is something `ply bootstrap` does not write and nothing reads; it writes {written:?}"
        ));
    }
    Ok(found)
}

/// `root` and, depth first, every package its manifests reach by `Path("...")`, each once: the
/// manifest's text is read through `manifest`, and each dependency resolved against its own
/// package's directory.
fn packages(root: &str, manifest: impl Fn(&str) -> Option<String>) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    let mut queue = vec![root.to_string()];
    while let Some(dir) = queue.pop() {
        if out.contains(&dir) {
            continue;
        }
        let at = format!("{dir}/{MANIFEST}");
        let text = manifest(&at).ok_or_else(|| format!("`{at}` is not there"))?;
        queue.extend(
            path_dependencies(&text)
                .into_iter()
                .map(|dep| normalized(&format!("{dir}/{dep}"))),
        );
        out.push(dir);
    }
    Ok(out)
}

/// Every `Path("...")` a manifest's text names. The text alone, and only that: what a manifest
/// means is the front end's (`crates/ply-compiler/ply/pkg.ply`).
pub fn path_dependencies(manifest: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = manifest;
    while let Some(at) = rest.find("Path(\"") {
        rest = &rest[at + "Path(\"".len()..];
        match rest.find('"') {
            Some(end) => {
                out.push(rest[..end].to_string());
                rest = &rest[end + 1..];
            }
            None => break,
        }
    }
    out
}

/// A `/`-separated path with its `.` and `..` segments taken out, as the repository spells it.
pub fn normalized(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out.join("/")
}
