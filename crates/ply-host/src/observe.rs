//! What a test read of the world while it ran: the files and directories its handlers read, what
//! they wrote, the shipped modules it asked for, and what any `ply` it started read in turn, keyed
//! by the machine that asked. A test's pass is filed with what this comes to, and stands only while
//! every read still answers as it did.
//!
//! A machine is observed when the tester begins it; one run on its behalf (a nested machine a
//! command drives) is adopted into the same record. What the test writes is its own: a read under a
//! path it wrote is not an input, and a directory it wrote into is read without what it wrote there.
//! Nor is a read under a directory a run keeps for the next one an input, since what is there is a
//! product of its key. A `ply` a test starts is handed a file in [`TRACE_VAR`] and reports there what its whole
//! process read; a program that is not `ply` reports nothing and is the environment, as the clock
//! and the network are. A `ply` that ended without finishing its report leaves the record
//! incomplete, which files no pass.

use ply_eval::host::MachineId;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// Where a parent asks a `ply` it starts to report what it read.
pub const TRACE_VAR: &str = "PLY_TRACE";

/// The last line of a report a `ply` finished.
const END: &str = "end";

/// The digest of a line two answers disagreed on, which nothing answers again.
const SPLIT: &str = "split";

/// Between the paths a listing leaves out, which no name holds.
const OWN: char = '\u{1f}';

/// How a path was read, which is how it is read again to see whether it moved.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Read {
    /// The bytes, or that there are none.
    File,
    /// The names in it, or that it is not a directory.
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

#[derive(Default)]
struct Observed {
    reads: BTreeSet<(Read, PathBuf)>,
    writes: BTreeSet<PathBuf>,
    shipped: BTreeSet<String>,
    /// Whether the record asked what `ply` program ran.
    program: bool,
    /// Lines a `ply` this record started digested against its own binary, which may not be ours.
    digested: BTreeMap<String, String>,
    /// Report files handed to the programs this record started.
    children: Vec<PathBuf>,
    /// Why no record can stand in for the run, when something made it so.
    opaque: Vec<String>,
}

/// One test's record, shared by every machine run on its behalf.
#[derive(Default)]
pub struct Recorder {
    observed: Mutex<Observed>,
}

fn recorders() -> &'static Mutex<HashMap<MachineId, Arc<Recorder>>> {
    static RECORDERS: OnceLock<Mutex<HashMap<MachineId, Arc<Recorder>>>> = OnceLock::new();
    RECORDERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A `ply` started with [`TRACE_VAR`] records every machine it runs.
static PROCESS: OnceLock<Arc<Recorder>> = OnceLock::new();

/// Where the running `ply` lays its shipped modules out as files: a read there is of a module, or
/// of the program beside them.
static SHIPPED_DIR: OnceLock<PathBuf> = OnceLock::new();

static KEPT: OnceLock<Vec<PathBuf>> = OnceLock::new();

/// Where this `ply` lays its shipped modules out as files, and the directories it keeps what one
/// run leaves for the next in.
pub fn laid_out(shipped_dir: &Path, kept: Vec<PathBuf>) {
    let _ = SHIPPED_DIR.set(shipped_dir.to_path_buf());
    let _ = KEPT.set(kept);
}

fn is_kept(path: &Path) -> bool {
    KEPT.get()
        .is_some_and(|dirs| dirs.iter().any(|dir| path.starts_with(dir)))
}

fn recorder_of(machine: MachineId) -> Option<Arc<Recorder>> {
    let held = recorders().lock().unwrap_or_else(|e| e.into_inner());
    held.get(&machine)
        .cloned()
        .or_else(|| PROCESS.get().cloned())
}

fn with(machine: MachineId, f: impl FnOnce(&mut Observed)) {
    if let Some(recorder) = recorder_of(machine) {
        f(&mut recorder.observed.lock().unwrap_or_else(|e| e.into_inner()));
    }
}

/// Observes `machine` until [`end`].
pub fn begin(machine: MachineId) -> Arc<Recorder> {
    let recorder = Arc::new(Recorder::default());
    let mut held = recorders().lock().unwrap_or_else(|e| e.into_inner());
    held.insert(machine, Arc::clone(&recorder));
    recorder
}

/// `child`, run on `parent`'s behalf, is observed into `parent`'s record, if it has one.
pub fn adopt(child: MachineId, parent: MachineId) {
    let mut held = recorders().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(recorder) = held.get(&parent).cloned() {
        held.insert(child, recorder);
    }
}

/// Stops observing `recorder` under every machine it was observing.
pub fn end(recorder: &Arc<Recorder>) {
    let mut held = recorders().lock().unwrap_or_else(|e| e.into_inner());
    held.retain(|_, r| !Arc::ptr_eq(r, recorder));
}

/// The whole process is observed: what a `ply` started with [`TRACE_VAR`] does first.
pub fn begin_process() -> Arc<Recorder> {
    Arc::clone(PROCESS.get_or_init(|| Arc::new(Recorder::default())))
}

pub fn read(machine: MachineId, how: Read, path: &Path) {
    if let Some(rest) = SHIPPED_DIR
        .get()
        .and_then(|dir| path.strip_prefix(dir).ok())
    {
        return match rest.to_str().and_then(|n| n.strip_suffix(".ply")) {
            Some(name) if how == Read::File => shipped(machine, name),
            _ => program(machine),
        };
    }
    if is_kept(path) {
        return;
    }
    with(machine, |o| {
        o.reads.insert((how, path.to_path_buf()));
    });
}

pub fn wrote(machine: MachineId, path: &Path) {
    with(machine, |o| {
        o.writes.insert(path.to_path_buf());
    });
}

pub fn shipped(machine: MachineId, name: &str) {
    with(machine, |o| {
        o.shipped.insert(name.to_string());
    });
}

/// What the running `ply` program is entered into the record.
pub fn program(machine: MachineId) {
    with(machine, |o| o.program = true);
}

impl Recorder {
    /// The `ply` program that ran is part of what a run of this process came to.
    pub fn ran_program(&self) {
        self.observed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .program = true;
    }
}

/// A fresh file a program started for `machine` reports into, when a test is observing it.
pub fn child_trace(machine: MachineId) -> Option<PathBuf> {
    let recorder = recorder_of(machine)?;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let file = std::env::temp_dir().join(format!(
        "ply-trace-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&file);
    recorder
        .observed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .children
        .push(file.clone());
    Some(file)
}

/// What `inner`, a test a test ran, read: the outer test read it too.
pub fn absorb(machine: MachineId, inner: &Recorder) {
    let taken = inner.observed.lock().unwrap_or_else(|e| e.into_inner());
    with(machine, |o| merge(o, &taken));
}

fn merge(into: &mut Observed, from: &Observed) {
    into.reads.extend(from.reads.iter().cloned());
    into.writes.extend(from.writes.iter().cloned());
    into.shipped.extend(from.shipped.iter().cloned());
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

/// What a binary answers about itself, which a trace's `shipped` and `program` lines hold: each
/// shipped module's digest by name, or the list's for the empty name, and its program's digest.
pub struct Binary<'a> {
    pub shipped: &'a dyn Fn(&str) -> Option<String>,
    pub program: String,
}

/// Each `shipped` and `program` line of `o`, digested against `binary` unless a `ply` it started
/// already digested it against its own.
fn digested(o: &Observed, binary: &Binary<'_>) -> BTreeMap<String, String> {
    let mut lines = o.digested.clone();
    for name in &o.shipped {
        let digest = (binary.shipped)(name).unwrap_or_else(|| "none".to_string());
        settle(&mut lines, format!("shipped\t{name}"), digest);
    }
    if o.program {
        settle(&mut lines, "program".to_string(), binary.program.clone());
    }
    lines
}

/// What a `ply` reports to the parent that started it: every read and write, raw, what it asked of
/// its own binary digested, each child's report folded in, then [`END`].
pub fn report(recorder: &Recorder, binary: &Binary<'_>) -> String {
    let mut o = recorder.observed.lock().unwrap_or_else(|e| e.into_inner());
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
    for why in &o.opaque {
        out.push_str(&format!("opaque\t{}\n", why.replace('\n', " ")));
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
                ["opaque", why] => o.opaque.push(why.to_string()),
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

/// What `how` answers of `path` now, as the digest a trace holds, leaving out what lies at `own`
/// below it: the paths the test wrote there.
fn answered(how: Read, path: &Path, own: &[PathBuf]) -> String {
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
        Read::Dir => match std::fs::read_dir(path) {
            Ok(entries) => {
                let mut names: Vec<String> = entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|name| !own.iter().any(|o| o == Path::new(name)))
                    .collect();
                names.sort();
                hasher.update(b"names\0");
                for name in names {
                    hasher.update(name.as_bytes());
                    hasher.update(&[0]);
                }
            }
            Err(_) => {
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
        if own.iter().any(|o| rel.starts_with(o)) {
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

/// What a trace is read against: the run's roots, `None` where no file may be read; the digest of
/// what the run is configured to bind; and the binary running it.
pub struct World<'a> {
    pub roots: Option<&'a [(String, PathBuf)]>,
    pub binding: String,
    pub binary: Binary<'a>,
}

/// A trace: one line a read, sorted — `<how>\t<root>\t<path>\t<own>\t<digest>`, the root empty for
/// a path no root of the run holds and `own` the paths below it the read leaves out;
/// `shipped\t<module>\t<digest>`; `program\t<digest>`; and `binding\t<digest>` for a run that
/// reached a host handler, whose verdict is the binding's.
pub type Trace = String;

/// What `recorder` came to, read against `world`; `None` when something it did no trace can stand
/// in for. `hosted` is whether the run reached a host handler.
pub fn finished(recorder: &Recorder, world: &World<'_>, hosted: bool) -> Option<Trace> {
    let mut observed = recorder.observed.lock().unwrap_or_else(|e| e.into_inner());
    fold_children(&mut observed);
    if !observed.opaque.is_empty() {
        return None;
    }
    let mut lines = digested(&observed, &world.binary);
    if hosted {
        lines.insert("binding".to_string(), world.binding.clone());
    }
    let roots = world.roots.unwrap_or_default();
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
            answered(*how, path, &own),
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

/// Whether every read `trace` holds still answers as it did, read against `world` now. Each read
/// is entered into whatever observes `machine`, since what it decides rests on them.
pub fn unchanged(trace: &str, world: &World<'_>, machine: MachineId) -> bool {
    trace.lines().all(|line| {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.as_slice() {
            ["shipped", name, digest] => {
                shipped(machine, name);
                (world.binary.shipped)(name).unwrap_or_else(|| "none".to_string()) == *digest
            }
            ["program", digest] => {
                program(machine);
                world.binary.program == *digest
            }
            ["binding", digest] => world.binding == *digest,
            [how, root, rest, own, digest] => {
                let (Some(how), Some(roots)) = (Read::of_word(how), world.roots) else {
                    return false;
                };
                let Some(path) = located(root, rest, roots) else {
                    return false;
                };
                let own: Vec<PathBuf> = own
                    .split(OWN)
                    .filter(|o| !o.is_empty())
                    .map(PathBuf::from)
                    .collect();
                read(machine, how, &path);
                answered(how, &path, &own) == *digest
            }
            _ => false,
        }
    })
}
