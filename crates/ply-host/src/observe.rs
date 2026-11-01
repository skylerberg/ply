use ply_core::HostRuntime;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

pub const TRACE_VAR: &str = "PLY_TRACE";

const END: &str = "end";

const SPLIT_EVENT: &str = "split";

const OWN: char = '\u{1f}';

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Read {
    File,
    Dir,
    Kind,
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

    fn from_name(word: &str) -> Option<Read> {
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
struct Reads {
    read: usize,
    viewed: usize,
}

#[derive(Default)]
struct Observations {
    reads: BTreeSet<(Read, PathBuf)>,
    writes: BTreeSet<PathBuf>,
    shipped: BTreeMap<String, Reads>,
    viewed: BTreeMap<String, String>,
    program: bool,
    digested: BTreeMap<String, String>,
    children: Vec<PathBuf>,
    config: BTreeMap<String, String>,
    secrets: BTreeSet<String>,
    opaque: Vec<String>,
}

#[derive(Default)]
pub struct Recorder {
    observed: Mutex<Observations>,
}

static PROCESS: OnceLock<Arc<Recorder>> = OnceLock::new();

pub fn of(runtime: &dyn HostRuntime) -> Option<Arc<Recorder>> {
    crate::registry::facilities_of(runtime)
        .and_then(|facilities| facilities.run().recorder())
        .or_else(|| PROCESS.get().cloned())
}

pub fn records_into(runtime: &dyn HostRuntime, recorder: &Arc<Recorder>) {
    if let Some(facilities) = crate::registry::facilities_of(runtime) {
        facilities.run().records_into(Arc::clone(recorder));
    }
}

static SHIPPED_DIR: OnceLock<PathBuf> = OnceLock::new();

static CACHE_DIRS: OnceLock<Vec<PathBuf>> = OnceLock::new();

const CACHE_DIR_NAME: &str = ".ply-cache";

const VCS: &str = ".git";

pub fn set_shipped_and_cache_dirs(shipped_dir: &Path, cache_dirs: Vec<PathBuf>) {
    let _ = SHIPPED_DIR.set(resolved(shipped_dir));
    set_cache_dirs(cache_dirs);
}

pub fn set_cache_dirs(cache_dirs: Vec<PathBuf>) {
    let _ = CACHE_DIRS.set(
        cache_dirs
            .into_iter()
            .flat_map(|dir| [resolved(&dir), dir])
            .collect(),
    );
}

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

pub fn at_exit(f: extern "C" fn()) {
    unsafe {
        libc::atexit(f);
    }
}

fn is_ignored(path: &Path) -> bool {
    path.components()
        .any(|c| c.as_os_str() == CACHE_DIR_NAME || c.as_os_str() == VCS)
        || CACHE_DIRS
            .get()
            .is_some_and(|dirs| dirs.iter().any(|dir| path.starts_with(dir)))
}

pub fn begin_process() -> Arc<Recorder> {
    Arc::clone(PROCESS.get_or_init(|| Arc::new(Recorder::default())))
}

type Reporter = Box<dyn Fn() + Send + Sync>;

static REPORTER: OnceLock<Reporter> = OnceLock::new();

pub fn reports_with(reporter: Reporter) {
    let _ = REPORTER.set(reporter);
}

pub fn report_at_exit() {
    if let Some(report) = REPORTER.get() {
        report();
    }
}

impl Recorder {
    fn observed(&self) -> MutexGuard<'_, Observations> {
        self.observed.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn read(&self, how: Read, path: &Path) {
        if let Some(rest) = SHIPPED_DIR
            .get()
            .and_then(|dir| path.strip_prefix(dir).ok())
        {
            let shipped_name = rest.to_str().and_then(|file| {
                file.strip_suffix(".ply")
                    .or_else(|| file.contains('/').then_some(file))
            });
            return match shipped_name {
                Some(name) if how == Read::File => self.shipped(name),
                _ => self.ran_program(),
            };
        }
        if is_ignored(path) {
            return;
        }
        self.observed().reads.insert((how, path.to_path_buf()));
    }

    pub fn configured(&self, key: &str, value: Option<&str>) {
        record_digest(
            &mut self.observed().config,
            key.to_string(),
            configured_digest(value),
        );
    }

    pub fn read_secret(&self, key: &str) {
        self.observed().secrets.insert(key.to_string());
    }

    pub fn wrote(&self, path: &Path) {
        self.observed().writes.insert(path.to_path_buf());
    }

    pub fn shipped(&self, name: &str) {
        self.observed()
            .shipped
            .entry(name.to_string())
            .or_default()
            .read += 1;
    }

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
                record_digest(&mut o.viewed, line, digest);
            }
        }
    }

    pub fn ran_program(&self) {
        self.observed().program = true;
    }

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

    pub fn absorb(&self, inner: &Recorder) {
        if std::ptr::eq(self, inner) {
            return;
        }
        let taken = inner.observed();
        merge(&mut self.observed(), &taken);
    }
}

fn merge(into: &mut Observations, from: &Observations) {
    into.reads.extend(from.reads.iter().cloned());
    into.writes.extend(from.writes.iter().cloned());
    for (name, reads) in &from.shipped {
        let into = into.shipped.entry(name.clone()).or_default();
        into.read += reads.read;
        into.viewed += reads.viewed;
    }
    for (line, digest) in &from.viewed {
        record_digest(&mut into.viewed, line.clone(), digest.clone());
    }
    into.program |= from.program;
    for (line, digest) in &from.digested {
        record_digest(&mut into.digested, line.clone(), digest.clone());
    }
    into.children.extend(from.children.iter().cloned());
    into.opaque.extend(from.opaque.iter().cloned());
}

fn record_digest(lines: &mut BTreeMap<String, String>, line: String, digest: String) {
    lines
        .entry(line)
        .and_modify(|held| {
            if *held != digest {
                *held = SPLIT_EVENT.to_string();
            }
        })
        .or_insert(digest);
}

pub struct Binary<'a> {
    pub shipped: &'a dyn Fn(&str) -> Option<String>,
    pub program: String,
}

fn digested(o: &Observations, binary: &Binary<'_>) -> BTreeMap<String, String> {
    let mut lines = o.digested.clone();
    for (name, reads) in &o.shipped {
        if reads.read > reads.viewed {
            let digest = (binary.shipped)(name).unwrap_or_else(|| "none".to_string());
            record_digest(&mut lines, format!("shipped\t{name}"), digest);
        }
    }
    for (line, digest) in &o.viewed {
        record_digest(&mut lines, line.clone(), digest.clone());
    }
    if o.program {
        record_digest(&mut lines, "program".to_string(), binary.program.clone());
    }
    lines
}

pub fn report(recorder: &Recorder, binary: &Binary<'_>) -> String {
    let mut o = recorder.observed();
    merge_child_observations(&mut o);
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

fn merge_child_observations(o: &mut Observations) {
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
                    if let Some(how) = Read::from_name(how) {
                        o.reads.insert((how, PathBuf::from(path)));
                    }
                }
                ["wrote", path] => {
                    o.writes.insert(PathBuf::from(path));
                }
                ["shipped", name, digest] => record_digest(
                    &mut o.digested,
                    format!("shipped\t{name}"),
                    digest.to_string(),
                ),
                ["program", digest] => {
                    record_digest(&mut o.digested, "program".to_string(), digest.to_string())
                }
                [kind @ ("shipform" | "shipdef"), module, at, digest] => record_digest(
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
                    if name == CACHE_DIR_NAME || own.iter().any(|o| o == Path::new(&name)) {
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
        Read::Tree => hash_tree(&mut hasher, path, Path::new(""), own),
    }
    hasher.finalize().to_hex().to_string()
}

fn hash_tree(hasher: &mut blake3::Hasher, root: &Path, below: &Path, own: &[PathBuf]) {
    let Ok(entries) = std::fs::read_dir(root.join(below)) else {
        hasher.update(b"none");
        return;
    };
    let mut names: Vec<std::ffi::OsString> = entries.flatten().map(|e| e.file_name()).collect();
    names.sort();
    for name in names {
        let rel = below.join(&name);
        if name == CACHE_DIR_NAME || own.iter().any(|o| rel.starts_with(o)) {
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
                hash_tree(hasher, root, &rel, own);
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

pub struct World<'a> {
    pub roots: &'a [(String, PathBuf)],
    pub binding: String,
    pub config: &'a BTreeMap<String, String>,
    pub binary: Binary<'a>,
}

pub type Trace = String;

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

pub fn finished(recorder: &Recorder, world: &World<'_>, hosted: bool) -> Option<Trace> {
    let mut observed = recorder.observed();
    merge_child_observations(&mut observed);
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
        let own = paths_written_under(*how, path, &observed.writes);
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

fn paths_written_under(how: Read, path: &Path, writes: &BTreeSet<PathBuf>) -> Vec<PathBuf> {
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
            [how, root, rest, own, digest] => match Read::from_name(how) {
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
