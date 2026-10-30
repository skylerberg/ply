//! What a test read of the world while it ran: the files and directories the host handlers that
//! handled it read, what they wrote, the configuration keys it read, the shipped modules it asked
//! for, and what any `ply` it started read in turn, but for that `ply`'s configuration, which is what
//! the test handed it. No trace holds anything of a secret, so a run that read a secret
//! configuration value records no pass. A pass is recorded with what this comes to, and stands only while every read
//! still returns as before. A load that read a shipped module for only what its program reaches says
//! so in a view: the module's form and each function reached, with its hash, stand in for that read
//! of the whole module.
//!
//! A test's record is its run's, which the handler that handles a read enters it into; a handler of
//! the test's own touches no host and records nothing. A run made on a test's behalf (a nested
//! machine a command drives) shares its caller's record. What the test writes is its own: a read
//! under a path it wrote is not an input, and a directory it wrote into is read without what it wrote
//! there. Nor is a read under a cache directory an input, a front end's store
//! included wherever it sits, since what is there is a product of its key; nor one of a checkout's
//! version control, whose commit moves with every change, as the clock does. A `ply` a test starts
//! is handed a file in [`TRACE_VAR`] and reports there what its whole process read; a program that is
//! not `ply` reports nothing and is the environment, as the clock and the network are. A `ply` that
//! ended without finishing its report leaves the record incomplete, which files no pass.

use ply_core::HostRuntime;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

/// Where a parent asks a `ply` it starts to report what it read.
pub const TRACE_VAR: &str = "PLY_TRACE";

/// The last line of a report a `ply` finished.
const END: &str = "end";

/// The digest of a line two results disagreed on, which nothing returns again.
const SPLIT: &str = "split";

/// Between the paths a listing leaves out, which no name holds.
const OWN: char = '\u{1f}';

/// How a path was read, which is how it is read again to see whether it moved.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Read {
    /// The bytes, or that there are none.
    File,
    /// The names in it, each with what it is, or that it is not a directory.
    Dir,
    /// What is there: a file, a directory, a link, or nothing.
    Kind,
    /// Every path below, each with what it is and a file's bytes.
    Tree,
}

impl Read {
    fn word(self) -> &'static str {
        match self {
            Read::File => "file",
            Read::Dir => "dir",
            Read::Kind => "kind",
            Read::Tree => "tree",
        }
    }

    fn of_word(word: &str) -> Option<Read> {
        match word {
            "file" => Some(Read::File),
            "dir" => Some(Read::Dir),
            "kind" => Some(Read::Kind),
            "tree" => Some(Read::Tree),
            _ => None,
        }
    }
}

/// How often a record read a shipped module whole, and how many of those reads a view stands for.
#[derive(Default)]
struct Reads {
    read: usize,
    viewed: usize,
}

#[derive(Default)]
struct Observed {
    reads: BTreeSet<(Read, PathBuf)>,
    writes: BTreeSet<PathBuf>,
    shipped: BTreeMap<String, Reads>,
    /// The `shipform` and `shipdef` lines of the views that stand for reads of shipped modules.
    viewed: BTreeMap<String, String>,
    /// Whether the record asked what `ply` program ran.
    program: bool,
    /// Lines a `ply` this record started digested against its own binary, which may not be ours.
    digested: BTreeMap<String, String>,
    /// Report files handed to the programs this record started.
    children: Vec<PathBuf>,
    /// Each configuration key read, with the digest of what it returned. A run made on the record's
    /// behalf, a nested one's or a child's, is configured by what this one handed it, so what it
    /// read of its own configuration is none of this.
    config: BTreeMap<String, String>,
    /// The secret configuration keys read, of which a trace holds nothing.
    secrets: BTreeSet<String>,
    /// Why no record can stand in for the run, when something made it so.
    opaque: Vec<String>,
}

/// One test's record, shared by every run made on its behalf.
#[derive(Default)]
pub struct Recorder {
    observed: Mutex<Observed>,
}

/// A `ply` started with [`TRACE_VAR`] records every run that has no record of its own.
static PROCESS: OnceLock<Arc<Recorder>> = OnceLock::new();

/// The record `runtime`'s run is entered into: its own, else the process's where a `ply` that
/// started this one asked for it, else none.
pub fn of(runtime: &dyn HostRuntime) -> Option<Arc<Recorder>> {
    crate::registry::facilities_of(runtime)
        .and_then(|facilities| facilities.run().recorder())
        .or_else(|| PROCESS.get().cloned())
}

/// `runtime`'s run is entered into `recorder` from now on.
pub fn records_into(runtime: &dyn HostRuntime, recorder: &Arc<Recorder>) {
    if let Some(facilities) = crate::registry::facilities_of(runtime) {
        facilities.run().records_into(Arc::clone(recorder));
    }
}

/// Where the running `ply` lays its shipped modules out as files: a read there is of a module, of
/// a data file one embeds, or of the program beside them.
static SHIPPED_DIR: OnceLock<PathBuf> = OnceLock::new();

static KEPT: OnceLock<Vec<PathBuf>> = OnceLock::new();

/// The directory a front end keeps its store in, beside what it loads.
const STORE: &str = ".ply-cache";

/// A checkout's version control.
const VCS: &str = ".git";

/// Where this `ply` lays its shipped modules out as files, and the directories it caches what one
/// run leaves for the next in.
pub fn laid_out(shipped_dir: &Path, kept: Vec<PathBuf>) {
    let _ = SHIPPED_DIR.set(resolved(shipped_dir));
    keeps(kept);
}

/// The directories this process caches what one run leaves for the next in.
pub fn keeps(kept: Vec<PathBuf>) {
    let _ = KEPT.set(
        kept.into_iter()
            .flat_map(|dir| [resolved(&dir), dir])
            .collect(),
    );
}

/// `dir` as a root names what is below it, its links resolved, though its tail need not exist yet.
fn resolved(dir: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut at = dir;
    loop {
        if let Ok(real) = at.canonicalize() {
            return rest.iter().rev().fold(real, |path, name| path.join(name));
        }
        match (at.parent(), at.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                at = parent;
            }
            _ => return dir.to_path_buf(),
        }
    }
}

/// `f` runs as the process exits normally, after its `main` returns.
pub fn at_exit(f: extern "C" fn()) {
    // SAFETY: `atexit` only records the function, which takes and returns nothing.
    unsafe {
        libc::atexit(f);
    }
}

fn is_kept(path: &Path) -> bool {
    path.components()
        .any(|c| c.as_os_str() == STORE || c.as_os_str() == VCS)
        || KEPT
            .get()
            .is_some_and(|dirs| dirs.iter().any(|dir| path.starts_with(dir)))
}

/// The whole process is observed: what a `ply` started with [`TRACE_VAR`] does first.
pub fn begin_process() -> Arc<Recorder> {
    Arc::clone(PROCESS.get_or_init(|| Arc::new(Recorder::default())))
}

type Reporter = Box<dyn Fn() + Send + Sync>;

static REPORTER: OnceLock<Reporter> = OnceLock::new();

/// How the process's report is written, which a process ending outside its entry calls first.
pub fn reports_with(reporter: Reporter) {
    let _ = REPORTER.set(reporter);
}

/// The process is ending wherever it stands: its report is written now.
pub fn exiting() {
    if let Some(report) = REPORTER.get() {
        report();
    }
}

/// What this `ply` wrote outside any handler, when a `ply` that started it is observing it.
pub fn process_wrote(path: &Path) {
    if let Some(recorder) = PROCESS.get() {
        recorder.wrote(path);
    }
}

impl Recorder {
    fn observed(&self) -> MutexGuard<'_, Observed> {
        self.observed.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `path` was read as `how` says; a read of the shipped modules' files is one of a module or
    /// of the program, and one under a cache directory is no input.
    pub fn read(&self, how: Read, path: &Path) {
        if let Some(rest) = SHIPPED_DIR
            .get()
            .and_then(|dir| path.strip_prefix(dir).ok())
        {
            // A module lies there as `<name>.ply`, and a data file one embeds below it, under its
            // name.
            let shipped_name = rest.to_str().and_then(|file| {
                file.strip_suffix(".ply")
                    .or_else(|| file.contains('/').then_some(file))
            });
            return match shipped_name {
                Some(name) if how == Read::File => self.shipped(name),
                _ => self.ran_program(),
            };
        }
        if is_kept(path) {
            return;
        }
        self.observed().reads.insert((how, path.to_path_buf()));
    }

    /// The configuration key `key` was read, and returned `value`.
    pub fn configured(&self, key: &str, value: Option<&str>) {
        settle(
            &mut self.observed().config,
            key.to_string(),
            configured_digest(value),
        );
    }

    /// A secret configuration value was read: no trace holds anything of a secret, so none stands
    /// in for the run.
    pub fn read_secret(&self, key: &str) {
        self.observed().secrets.insert(key.to_string());
    }

    pub fn wrote(&self, path: &Path) {
        self.observed().writes.insert(path.to_path_buf());
    }

    /// The shipped module `name` was read whole; the empty name is their list.
    pub fn shipped(&self, name: &str) {
        self.observed()
            .shipped
            .entry(name.to_string())
            .or_default()
            .read += 1;
    }

    /// What a load said it read of the shipped modules it pulled: per module, a
    /// `shipform\t<module>\t<text digest>\t<form>` line and a `shipdef\t<module>\t<name>\t<hash>`
    /// line for each function its program reached. A view `pulled` its modules stands for one whole
    /// read of each that the record has not yet had a view for; one made again of a load already
    /// read, as a splice re-analyses it, adds to what such a view holds. A module the record never
    /// read of the binary, one a caller handed the load in its place, is left out.
    pub fn reached(&self, view: &str, pulled: bool) {
        let mut modules: BTreeMap<&str, Vec<(String, String)>> = BTreeMap::new();
        for line in view.lines() {
            let fields: Vec<&str> = line.split('\t').collect();
            if let [kind @ ("shipform" | "shipdef"), module, at, digest] = fields.as_slice() {
                modules
                    .entry(module)
                    .or_default()
                    .push((format!("{kind}\t{module}\t{at}"), digest.to_string()));
            }
        }
        let mut o = self.observed();
        for (module, lines) in modules {
            let Some(reads) = o.shipped.get_mut(module) else {
                continue;
            };
            let stands_for = if pulled {
                reads.read > reads.viewed
            } else {
                reads.read > 0
            };
            if !stands_for {
                continue;
            }
            if pulled {
                reads.viewed += 1;
            }
            for (line, digest) in lines {
                settle(&mut o.viewed, line, digest);
            }
        }
    }

    /// The `ply` program that ran is part of what the record came to.
    pub fn ran_program(&self) {
        self.observed().program = true;
    }

    /// A fresh file a program the run starts reports into.
    pub fn child_trace(&self) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let file = std::env::temp_dir().join(format!(
            "ply-trace-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_file(&file);
        self.observed().children.push(file.clone());
        file
    }

    /// What `inner`, a test this record's test ran, read: this one read it too.
    pub fn absorb(&self, inner: &Recorder) {
        if std::ptr::eq(self, inner) {
            return;
        }
        let taken = inner.observed();
        merge(&mut self.observed(), &taken);
    }
}

fn merge(into: &mut Observed, from: &Observed) {
    into.reads.extend(from.reads.iter().cloned());
    into.writes.extend(from.writes.iter().cloned());
    for (name, reads) in &from.shipped {
        let into = into.shipped.entry(name.clone()).or_default();
        into.read += reads.read;
        into.viewed += reads.viewed;
    }
    for (line, digest) in &from.viewed {
        settle(&mut into.viewed, line.clone(), digest.clone());
    }
    into.program |= from.program;
    for (line, digest) in &from.digested {
        settle(&mut into.digested, line.clone(), digest.clone());
    }
    into.children.extend(from.children.iter().cloned());
    into.opaque.extend(from.opaque.iter().cloned());
}

fn settle(lines: &mut BTreeMap<String, String>, line: String, digest: String) {
    lines
        .entry(line)
        .and_modify(|held| {
            if *held != digest {
                *held = SPLIT.to_string();
            }
        })
        .or_insert(digest);
}

/// What a binary reports about itself, which a trace's `shipped` and `program` lines hold: each
/// shipped module's digest by name, or the list's for the empty name, and its program's digest.
pub struct Binary<'a> {
    pub shipped: &'a dyn Fn(&str) -> Option<String>,
    pub program: String,
}

/// Each `shipped` and `program` line of `o`, digested against `binary` unless a `ply` it started
/// already digested it against its own, and the views that stand for the rest of its reads.
fn digested(o: &Observed, binary: &Binary<'_>) -> BTreeMap<String, String> {
    let mut lines = o.digested.clone();
    for (name, reads) in &o.shipped {
        if reads.read > reads.viewed {
            let digest = (binary.shipped)(name).unwrap_or_else(|| "none".to_string());
            settle(&mut lines, format!("shipped\t{name}"), digest);
        }
    }
    for (line, digest) in &o.viewed {
        settle(&mut lines, line.clone(), digest.clone());
    }
    if o.program {
        settle(&mut lines, "program".to_string(), binary.program.clone());
    }
    lines
}

/// What a `ply` reports to the parent that started it: every read and write, raw, what it asked of
/// its own binary digested, each child's report folded in, then [`END`].
pub fn report(recorder: &Recorder, binary: &Binary<'_>) -> String {
    let mut o = recorder.observed();
    fold_children(&mut o);
    let mut out = String::new();
    for (how, path) in &o.reads {
        out.push_str(&format!("read\t{}\t{}\n", how.word(), path.display()));
    }
    for path in &o.writes {
        out.push_str(&format!("wrote\t{}\n", path.display()));
    }
    for (line, digest) in digested(&o, binary) {
        out.push_str(&format!("{line}\t{digest}\n"));
    }
    for reason in &o.opaque {
        out.push_str(&format!("opaque\t{}\n", reason.replace('\n', " ")));
    }
    out.push_str(END);
    out.push('\n');
    out
}

/// Each child's report read into `o` and its file removed. A report never begun is a program that
/// is not `ply`; one begun and never finished is a `ply` whose reads are unknown.
fn fold_children(o: &mut Observed) {
    for file in std::mem::take(&mut o.children) {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let _ = std::fs::remove_file(&file);
        if text.lines().last() != Some(END) {
            o.opaque
                .push("a `ply` it started ended before reporting what it read".to_string());
            continue;
        }
        for line in text.lines() {
            let fields: Vec<&str> = line.split('\t').collect();
            match fields.as_slice() {
                ["read", how, path] => {
                    if let Some(how) = Read::of_word(how) {
                        o.reads.insert((how, PathBuf::from(path)));
                    }
                }
                ["wrote", path] => {
                    o.writes.insert(PathBuf::from(path));
                }
                ["shipped", name, digest] => settle(
                    &mut o.digested,
                    format!("shipped\t{name}"),
                    digest.to_string(),
                ),
                ["program", digest] => {
                    settle(&mut o.digested, "program".to_string(), digest.to_string())
                }
                [kind @ ("shipform" | "shipdef"), module, at, digest] => settle(
                    &mut o.viewed,
                    format!("{kind}\t{module}\t{at}"),
                    digest.to_string(),
                ),
                ["opaque", reason] => o.opaque.push(reason.to_string()),
                _ => {}
            }
        }
    }
}

/// Where a recorded path is written down: under the run's own root of that name when one holds it,
/// so a trace reads the same from another checkout or scratch directory, else whole.
fn rooted(path: &Path, roots: &[(String, PathBuf)]) -> (String, String) {
    let found = roots
        .iter()
        .filter_map(|(name, dir)| path.strip_prefix(dir).ok().map(|rest| (name, dir, rest)))
        .max_by(|a, b| {
            a.1.components()
                .count()
                .cmp(&b.1.components().count())
                .then_with(|| b.0.cmp(a.0))
        });
    match found {
        Some((name, _, rest)) => (name.clone(), rest.to_string_lossy().into_owned()),
        None => (String::new(), path.to_string_lossy().into_owned()),
    }
}

fn located(root: &str, rest: &str, roots: &[(String, PathBuf)]) -> Option<PathBuf> {
    if root.is_empty() {
        return Some(PathBuf::from(rest));
    }
    let (_, dir) = roots.iter().find(|(name, _)| name == root)?;
    Some(if rest.is_empty() {
        dir.clone()
    } else {
        dir.join(rest)
    })
}

/// What `how` returns of `path` now, as the digest a trace holds, leaving out what lies at `own`
/// below it: the paths the test wrote there.
fn returned(how: Read, path: &Path, own: &[PathBuf]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(how.word().as_bytes());
    hasher.update(&[0]);
    match how {
        Read::File => match std::fs::read(path) {
            Ok(bytes) => {
                hasher.update(b"bytes\0");
                hasher.update(&bytes);
            }
            Err(_) => {
                hasher.update(b"none");
            }
        },
        Read::Dir => match crate::fs::listed(path) {
            Some(entries) => {
                hasher.update(b"entries\0");
                for (name, kind) in entries {
                    if name == STORE || own.iter().any(|o| o == Path::new(&name)) {
                        continue;
                    }
                    hasher.update(name.as_bytes());
                    hasher.update(&[0, kind.number()]);
                }
            }
            None => {
                hasher.update(b"none");
            }
        },
        Read::Kind => {
            let kind = match std::fs::symlink_metadata(path) {
                Ok(m) if m.file_type().is_symlink() => "link",
                Ok(m) if m.is_dir() => "dir",
                Ok(_) => "file",
                Err(_) => "none",
            };
            hasher.update(kind.as_bytes());
        }
        Read::Tree => tree_into(&mut hasher, path, Path::new(""), own),
    }
    hasher.finalize().to_hex().to_string()
}

fn tree_into(hasher: &mut blake3::Hasher, root: &Path, below: &Path, own: &[PathBuf]) {
    let Ok(entries) = std::fs::read_dir(root.join(below)) else {
        hasher.update(b"none");
        return;
    };
    let mut names: Vec<std::ffi::OsString> = entries.flatten().map(|e| e.file_name()).collect();
    names.sort();
    for name in names {
        let rel = below.join(&name);
        if name == STORE || own.iter().any(|o| rel.starts_with(o)) {
            continue;
        }
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update(&[0]);
        match std::fs::symlink_metadata(root.join(&rel)) {
            Ok(m) if m.file_type().is_symlink() => {
                hasher.update(b"link\0");
            }
            Ok(m) if m.is_dir() => {
                hasher.update(b"dir\0");
                tree_into(hasher, root, &rel, own);
            }
            Ok(_) => {
                hasher.update(b"file\0");
                let bytes = std::fs::read(root.join(&rel)).unwrap_or_default();
                hasher.update(blake3::hash(&bytes).as_bytes());
            }
            Err(_) => {
                hasher.update(b"none\0");
            }
        }
    }
}

/// What a trace is read against: the run's roots, the digest of what the run is configured to bind,
/// the plain value of each configuration key the run is given, and the binary running it.
pub struct World<'a> {
    pub roots: &'a [(String, PathBuf)],
    pub binding: String,
    pub config: &'a BTreeMap<String, String>,
    pub binary: Binary<'a>,
}

/// A trace: one line a read, sorted — `<how>\t<root>\t<path>\t<own>\t<digest>`, the root empty for
/// a path no root of the run holds and `own` the paths below it the read leaves out;
/// `config\t<key>\t<digest>` of what a configuration key returned; `shipped\t<module>\t<digest>`; a
/// view's `shipform` and `shipdef` lines ([`reached`]); `program\t<digest>`; and `binding\t<digest>`
/// for a run that reached a host handler, whose verdict is the binding's.
pub type Trace = String;

/// What a configuration key returning `value` is held at in a trace.
fn configured_digest(value: Option<&str>) -> String {
    let mut hasher = blake3::Hasher::new();
    match value {
        Some(value) => {
            hasher.update(b"value\0");
            hasher.update(value.as_bytes());
        }
        None => {
            hasher.update(b"none");
        }
    }
    hasher.finalize().to_hex().to_string()
}

/// What `recorder` came to, read against `world`; `None` when something it did no trace can stand
/// in for. `hosted` is whether the run reached a host handler.
pub fn finished(recorder: &Recorder, world: &World<'_>, hosted: bool) -> Option<Trace> {
    let mut observed = recorder.observed();
    fold_children(&mut observed);
    if !observed.opaque.is_empty() || !observed.secrets.is_empty() {
        return None;
    }
    let mut lines = digested(&observed, &world.binary);
    for (key, digest) in &observed.config {
        lines.insert(format!("config\t{key}"), digest.clone());
    }
    if hosted {
        lines.insert("binding".to_string(), world.binding.clone());
    }
    let roots = world.roots;
    for (how, path) in &observed.reads {
        if observed.writes.iter().any(|w| path.starts_with(w)) {
            continue;
        }
        let own = own_below(*how, path, &observed.writes);
        let (root, rest) = rooted(path, roots);
        let listed: Vec<String> = own
            .iter()
            .map(|o| o.to_string_lossy().into_owned())
            .collect();
        lines.insert(
            format!(
                "{}\t{root}\t{rest}\t{}",
                how.word(),
                listed.join(&OWN.to_string())
            ),
            returned(*how, path, &own),
        );
    }
    Some(
        lines
            .into_iter()
            .map(|(at, digest)| format!("{at}\t{digest}\n"))
            .collect(),
    )
}

/// What a listing of `path` leaves out of `writes`: for a directory, each name the test wrote under
/// it; for a tree, each path. A file or a kind has nothing below it to leave out.
fn own_below(how: Read, path: &Path, writes: &BTreeSet<PathBuf>) -> Vec<PathBuf> {
    let below = writes.iter().filter_map(|w| w.strip_prefix(path).ok());
    let own: BTreeSet<PathBuf> = match how {
        Read::Dir => below
            .filter_map(|rest| rest.components().next())
            .map(|first| PathBuf::from(first.as_os_str()))
            .collect(),
        Read::Tree => below
            .filter(|rest| !rest.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .collect(),
        Read::File | Read::Kind => BTreeSet::new(),
    };
    own.into_iter().collect()
}

/// The first read `trace` holds that no longer returns what it did, read against `world` now, as a
/// person reads it; `None` when every one still does. Each read checked is entered into `into`, the
/// asking run's record, since what it decides rests on them. A view stands here while its module's
/// text does; what can hash the module again holds its form and functions instead.
pub fn moved(trace: &str, world: &World<'_>, into: Option<&Recorder>) -> Option<String> {
    let noted = |note: &dyn Fn(&Recorder)| {
        if let Some(recorder) = into {
            note(recorder);
        }
    };
    trace.lines().find_map(|line| {
        let fields: Vec<&str> = line.split('\t').collect();
        let stands = match fields.as_slice() {
            ["shipped", name, digest] => {
                noted(&|r| r.shipped(name));
                (world.binary.shipped)(name).unwrap_or_else(|| "none".to_string()) == *digest
            }
            ["shipform", name, text, _] => {
                noted(&|r| r.shipped(name));
                (world.binary.shipped)(name).as_deref() == Some(*text)
            }
            ["shipdef", ..] => true,
            ["program", digest] => {
                noted(&|r| r.ran_program());
                world.binary.program == *digest
            }
            ["binding", digest] => world.binding == *digest,
            ["config", key, digest] => {
                let now = world.config.get(*key).map(String::as_str);
                noted(&|r| r.configured(key, now));
                configured_digest(now) == *digest
            }
            [how, root, rest, own, digest] => match Read::of_word(how) {
                Some(how) => match located(root, rest, world.roots) {
                    Some(path) => {
                        let own: Vec<PathBuf> = own
                            .split(OWN)
                            .filter(|o| !o.is_empty())
                            .map(PathBuf::from)
                            .collect();
                        noted(&|r| r.read(how, &path));
                        returned(how, &path, &own) == *digest
                    }
                    None => false,
                },
                None => false,
            },
            _ => false,
        };
        (!stands).then(|| said(&fields))
    })
}

/// A trace line as a person reads it.
fn said(fields: &[&str]) -> String {
    match fields {
        ["shipped", "", _] => "the list of shipped modules".to_string(),
        ["shipped", name, _] | ["shipform", name, ..] => format!("the shipped module `{name}`"),
        ["shipdef", _, name, _] => format!("the shipped definition `{name}`"),
        ["program", _] => "the `ply` program".to_string(),
        ["binding", _] => "the binding".to_string(),
        ["config", key, _] => format!("the configuration key `{key}`"),
        [how, "", rest, ..] => format!("the {how} `{rest}`"),
        [how, root, rest, ..] => format!("the {how} `{rest}` under `{root}`"),
        _ => format!("an unreadable trace line `{}`", fields.join(" ")),
    }
}
