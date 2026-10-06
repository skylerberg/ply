//! The filesystem, as operations confined to roots the run names.

use crate::pool::{self, Bell, FS_FIRST_TOKEN, Inbox, JobOutput, Pool, Refusal};
use ply_eval::host::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRegistry, HostRequest, HostResource,
    HostRuntime, Linearity, MachineId,
};
use ply_eval::{Diagnostic, Pending, Resource, Span, Symbol, Value, codes};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Must match the effect `std.fs` declares.
pub const EFFECT: &str = "fs";

/// The most one read answers. A larger file is read a range at a time with `fs.read_at`, so this
/// bounds a single call rather than a file.
pub const MAX_READ_BYTES: u64 = 64 * 1024 * 1024;

/// How long `fs.lock` waits for a holder to release before answering `false`.
pub const LOCK_WAIT: Duration = Duration::from_secs(2);

const LOCK_POLL: Duration = Duration::from_millis(2);

/// Far longer than a read-merge-write takes, so only a lock left by a killed process is broken.
pub const LOCK_STALE_AGE: Duration = Duration::from_secs(30);

// In the order `std.fs` declares them.
operations! {
    what "fs";
    path "fs";
    ReadFile = "read_file" / 1,
    ReadAt = "read_at" / 3,
    ListDir = "list_dir" / 1,
    Kind = "kind" / 1,
    Resolved = "resolved" / 1,
    Exists = "exists" / 1,
    FileSize = "file_size" / 1,
    ModifiedMs = "modified_ms" / 1,
    WriteFile = "write_file" / 2,
    Append = "append" / 2,
    CreateDir = "create_dir" / 1,
    Remove = "remove" / 1,
    Rename = "rename" / 2,
    Sync = "sync" / 1,
    Lock = "lock" / 1,
    Unlock = "unlock" / 1,
    Copy = "copy" / 2,
    RemoveTree = "remove_tree" / 1,
    TempDir = "temp_dir" / 2,
    Canonical = "canonical" / 1,
    Mode = "mode" / 1,
    SetMode = "set_mode" / 2,
    Symlink = "symlink" / 2,
    ReadLink = "read_link" / 1,
    Walk = "walk" / 1,
    SetModified = "set_modified" / 2,
    Open = "open" / 2,
    ReadChunk = "read_chunk" / 2,
    WriteChunk = "write_chunk" / 2,
    Close = "close" / 1,
    Stat = "stat" / 1,
    Scan = "scan" / 2,
    Space = "space" / 1,
    Link = "link" / 2,
}

impl Op {
    /// The thread name a job runs under.
    fn label(self) -> &'static str {
        match self {
            Op::ReadFile => "fs-read",
            Op::ReadAt => "fs-read-at",
            Op::ListDir => "fs-list",
            Op::Kind => "fs-kind",
            Op::Resolved => "fs-resolved",
            Op::Exists => "fs-exists",
            Op::FileSize => "fs-size",
            Op::ModifiedMs => "fs-modified",
            Op::WriteFile => "fs-write",
            Op::Append => "fs-append",
            Op::CreateDir => "fs-mkdir",
            Op::Remove => "fs-remove",
            Op::Rename => "fs-rename",
            Op::Sync => "fs-sync",
            Op::Lock => "fs-lock",
            Op::Unlock => "fs-unlock",
            Op::Copy => "fs-copy",
            Op::RemoveTree => "fs-remove-tree",
            Op::TempDir => "fs-temp-dir",
            Op::Canonical => "fs-canonical",
            Op::Mode => "fs-mode",
            Op::SetMode => "fs-set-mode",
            Op::Symlink => "fs-symlink",
            Op::ReadLink => "fs-read-link",
            Op::Walk => "fs-walk",
            Op::SetModified => "fs-set-modified",
            Op::Open => "fs-open",
            Op::ReadChunk => "fs-read-chunk",
            Op::WriteChunk => "fs-write-chunk",
            Op::Close => "fs-close",
            Op::Stat => "fs-stat",
            Op::Scan => "fs-scan",
            Op::Space => "fs-space",
            Op::Link => "fs-link",
        }
    }

    pub fn declaration(self) -> HostOp {
        HostOp {
            effect: Symbol::new(EFFECT),
            op: Symbol::new(self.name()),
            resource: HostResource::Any,
            determinism: Determinism::Nondeterministic,
            linearity: Linearity::AtMostOnce,
            blocking: true,
            // No expression turns a `Secret` into a path `String` or a body `Bytes`.
            secrets: false,
            path: self.path(),
        }
    }
}

/// One `--fs NAME=PATH`, as the argument was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootSpec {
    pub name: String,
    pub path: PathBuf,
}

impl RootSpec {
    pub fn parse(text: &str) -> Result<RootSpec, String> {
        let (name, path) = text
            .split_once('=')
            .ok_or_else(|| malformed(text, "there is no `=`"))?;
        if name.is_empty() {
            return Err(malformed(text, "the root has no name"));
        }
        if path.is_empty() {
            return Err(malformed(text, "the path is empty"));
        }
        if !name.chars().all(|c| c.is_alphanumeric() || c == '_')
            || name.chars().next().is_some_and(|c| c.is_numeric())
        {
            return Err(malformed(
                text,
                "a root's name is a resource label, so it is an identifier: letters, digits and `_`, not starting with a digit",
            ));
        }
        Ok(RootSpec {
            name: name.to_string(),
            path: PathBuf::from(path),
        })
    }
}

fn malformed(text: &str, why: &str) -> String {
    format!("`{text}` is not a filesystem root: {why}; write `--fs NAME=PATH`")
}

/// What `--fs NAME=PATH` bound, resolved once.
#[derive(Clone, Debug, Default)]
pub struct Roots {
    bound: BTreeMap<String, PathBuf>,
}

impl Roots {
    pub fn new() -> Roots {
        Roots::default()
    }

    pub fn load(specs: &[RootSpec], span: Span) -> Result<Roots, Diagnostic> {
        let mut roots = Roots::new();
        for spec in specs {
            roots.bind(&spec.name, &spec.path, span)?;
        }
        Ok(roots)
    }

    pub fn bind(&mut self, name: &str, path: &Path, span: Span) -> Result<(), Diagnostic> {
        let resolved = path.canonicalize().map_err(|e| {
            Diagnostic::error(
                codes::FS_ROOT_INVALID,
                format!("`--fs {name}={}` does not resolve: {e}", path.display()),
            )
            .primary(span, "this root does not exist")
            .note("every root is resolved once, before anything runs, and the resolved path is what a confinement check is against")
        })?;
        if !resolved.is_dir() {
            return Err(Diagnostic::error(
                codes::FS_ROOT_INVALID,
                format!("`--fs {name}={}` is not a directory", path.display()),
            )
            .primary(span, "a root is a directory")
            .note("an operation names a path *under* its root, so a root that is a file has nothing under it"));
        }
        self.bound.insert(name.to_string(), resolved);
        Ok(())
    }

    pub fn get(&self, at: &Resource) -> Option<&Path> {
        match at {
            Resource::Named(name) => self.bound.get(name.as_str()).map(PathBuf::as_path),
            // Unreachable when well-typed; `None` keeps a malformed registration a diagnostic.
            Resource::Var(_) | Resource::Singleton | Resource::Every => None,
        }
    }

    pub fn listing(&self) -> impl Iterator<Item = (&str, &Path)> {
        self.bound
            .iter()
            .map(|(name, path)| (name.as_str(), path.as_path()))
    }

    pub fn is_empty(&self) -> bool {
        self.bound.is_empty()
    }
}

/// How `fs.open` opens a file, as `std.fs.Opening` names it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Opening {
    Read,
    Write,
    Append,
    Create,
}

/// A file a run holds open, under the root it was opened under.
struct OpenFile {
    root: PathBuf,
    file: File,
    how: Opening,
}

/// The files a run holds open, each under the descriptor `fs.open` answered. A descriptor is never
/// answered twice, so one that was closed names nothing afterwards.
#[derive(Default)]
struct Descriptors {
    last: i64,
    open: BTreeMap<i64, OpenFile>,
}

pub struct FsHost {
    roots: Roots,
    pool: Pool,
    /// The lock files this run took and has not released; a lock it did not take it cannot release.
    held: Arc<Mutex<BTreeSet<PathBuf>>>,
    descriptors: Arc<Mutex<Descriptors>>,
}

impl FsHost {
    pub fn new(roots: Roots) -> FsHost {
        FsHost {
            roots,
            pool: Pool::new(FS_FIRST_TOKEN),
            held: Arc::new(Mutex::new(BTreeSet::new())),
            descriptors: Arc::new(Mutex::new(Descriptors::default())),
        }
    }

    pub fn roots(&self) -> &Roots {
        &self.roots
    }

    pub fn owns(&self, pending: &Pending) -> bool {
        self.pool.owns(pending)
    }

    pub fn watch_into(&self, pending: &Pending, inbox: &Arc<Inbox>) -> Result<(), Diagnostic> {
        self.pool.watch(pending, inbox)
    }

    pub fn collect(&self, inbox: &Inbox) -> Vec<(u64, Result<Value, Diagnostic>)> {
        self.pool.collect(inbox)
    }

    pub fn poll(&self, pending: &Pending) -> Result<Option<Value>, Diagnostic> {
        self.pool.poll(pending)
    }

    pub fn park(&self) -> Result<(), Diagnostic> {
        self.pool.park()
    }

    pub fn park_until(&self, bound: Duration) -> Result<(), Diagnostic> {
        self.pool.park_until(bound)
    }

    pub fn outstanding(&self) -> usize {
        self.pool.outstanding()
    }

    pub fn ready(&self) -> bool {
        self.pool.ready()
    }

    pub fn ring(&self, bell: &Arc<Bell>) {
        self.pool.ring(bell);
    }

    pub fn block_on(&self, pending: Pending) -> Result<Value, Diagnostic> {
        self.pool.block_on(pending)
    }
}

pub fn registrations(fs: &Arc<FsHost>) -> Vec<(HostOp, Arc<dyn HostHandler>)> {
    Op::ALL
        .iter()
        .map(|op| {
            let handler: Arc<dyn HostHandler> = Arc::new(Operation {
                op: *op,
                fs: Arc::clone(fs),
            });
            (op.declaration(), handler)
        })
        .collect()
}

pub fn register(registry: &mut HostRegistry, fs: Arc<FsHost>) {
    for (op, handler) in registrations(&fs) {
        registry.register(op, handler);
    }
}

struct Operation {
    op: Op,
    fs: Arc<FsHost>,
}

impl HostHandler for Operation {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        if req.args.len() != self.op.arity() {
            return Err(arity(self.op, req.args.len(), span));
        }
        // The resolved atom's resource, never one the handler re-derives.
        let at = &req.atom.resource;
        let root = match self.fs.roots.get(at) {
            Some(root) => root.to_path_buf(),
            None => return Err(unbound(self.op, at, span)),
        };
        if let Some(work) = self.on_descriptor(req)? {
            let descriptors = Arc::clone(&self.fs.descriptors);
            let descriptor = req.args[0].as_int(span, "a file descriptor")?;
            let pending = self.fs.pool.submit(
                span,
                self.op.label(),
                self.op.what(),
                Box::new(move || on_open_file(&descriptors, &root, descriptor, work)),
            )?;
            return Ok(HostAnswer::Pending(pending));
        }

        let first = req.args[0].as_str(span, "a path")?.to_string();
        let second = match self.op {
            Op::Open => Second::Opening(opening(&req.args[1], span)?),
            Op::WriteFile | Op::Append => {
                Second::Body(Arc::clone(req.args[1].as_bytes(span, "a body")?))
            }
            Op::Rename | Op::Copy | Op::Link => {
                Second::Path(req.args[1].as_str(span, "a path")?.to_string())
            }
            Op::Scan => Second::Deep(req.args[1].as_bool(span, "whether to go below")?),
            Op::Symlink => Second::Target(req.args[1].as_str(span, "a link's target")?.to_string()),
            Op::TempDir => Second::Name(req.args[1].as_str(span, "a name's prefix")?.to_string()),
            Op::SetMode => Second::Mode(mode_bits(&req.args[1], span)?),
            Op::SetModified => {
                let ms = req.args[1].as_int(span, "milliseconds since the epoch")?;
                if ms < 0 {
                    return Err(before_epoch(ms, span));
                }
                Second::Millis(ms)
            }
            Op::ReadAt => {
                let offset = req.args[1].as_int(span, "an offset")?;
                let len = req.args[2].as_int(span, "a length")?;
                // A file can be any length, but neither of these can be negative whatever it holds.
                if offset < 0 || len < 0 {
                    return Err(negative_range(offset, len, span));
                }
                if len as u64 > MAX_READ_BYTES {
                    return Err(too_large(len as u64, &first, span));
                }
                Second::Range { offset, len }
            }
            _ => Second::None,
        };

        let op = self.op;
        let held = Arc::clone(&self.fs.held);
        let descriptors = Arc::clone(&self.fs.descriptors);
        let machine = req.machine;
        let pending = self.fs.pool.submit(
            span,
            op.label(),
            op.what(),
            Box::new(move || {
                let done = run(op, &root, &first, second, &held, &descriptors, span);
                observed(op, &root, &first, &done, machine, span);
                done
            }),
        )?;
        Ok(HostAnswer::Pending(pending))
    }
}

impl Operation {
    /// What an operation on an open file asks of it; `None` for an operation on a path.
    fn on_descriptor(&self, req: &HostRequest<'_>) -> Result<Option<Chunk>, Diagnostic> {
        let span = req.span;
        Ok(Some(match self.op {
            Op::ReadChunk => {
                let max = req.args[1].as_int(span, "a length")?;
                if max < 0 {
                    return Err(negative_chunk(max, span));
                }
                if max as u64 > MAX_READ_BYTES {
                    return Err(chunk_too_large(max as u64, span));
                }
                Chunk::Read(max as u64)
            }
            Op::WriteChunk => Chunk::Write(Arc::clone(req.args[1].as_bytes(span, "a body")?)),
            Op::Close => Chunk::Close,
            _ => return Ok(None),
        }))
    }
}

/// What is asked of an open file.
enum Chunk {
    Read(u64),
    Write(Arc<[u8]>),
    Close,
}

/// One operation on an open file. A descriptor another root opened is not open under this one, and
/// neither is one opened the other way: each answers as a descriptor that names nothing does.
fn on_open_file(descriptors: &Mutex<Descriptors>, root: &Path, id: i64, work: Chunk) -> JobOutput {
    let mut held = descriptors
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let open = held.open.get_mut(&id).filter(|open| open.root == root);
    match work {
        Chunk::Read(max) => JobOutput::MaybeBytes(
            open.filter(|open| open.how == Opening::Read)
                .and_then(|open| {
                    let mut out = Vec::new();
                    (&open.file).take(max).read_to_end(&mut out).ok()?;
                    Some(out)
                }),
        ),
        Chunk::Write(body) => JobOutput::Bool(
            open.filter(|open| open.how != Opening::Read)
                .is_some_and(|open| open.file.write_all(&body).is_ok()),
        ),
        Chunk::Close => {
            let there = open.is_some();
            if there {
                held.open.remove(&id);
            }
            JobOutput::Bool(there)
        }
    }
}

/// The file at `target` opened as `how` asks, under the next descriptor, or the `std.fs.Refused`
/// that says why not.
fn open_file(
    descriptors: &Mutex<Descriptors>,
    root: &Path,
    target: &Path,
    how: Opening,
) -> Result<i64, Refusal> {
    use std::os::unix::fs::OpenOptionsExt;
    // A directory opens for reading, and only its first read says it is one.
    if how != Opening::Create && std::fs::metadata(target).is_ok_and(|meta| !meta.is_file()) {
        return Err(Refusal::NotAFile);
    }
    let mut options = OpenOptions::new();
    match how {
        Opening::Read => options.read(true),
        Opening::Write => options.write(true).create(true).truncate(true),
        Opening::Append => options.append(true).create(true),
        // `O_EXCL` refuses whatever is at the name, a link included, so nothing is written through one.
        Opening::Create => options.write(true).create_new(true).mode(0o600),
    };
    let file = options.open(target).map_err(|e| refusal(&e))?;
    let mut held = descriptors
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    held.last += 1;
    let id = held.last;
    held.open.insert(
        id,
        OpenFile {
            root: root.to_path_buf(),
            file,
            how,
        },
    );
    Ok(id)
}

fn opening(how: &Value, span: Span) -> Result<Opening, Diagnostic> {
    match how {
        Value::Ctor { name, .. } => match name.as_str().rsplit('.').next() {
            Some("ToRead") => Ok(Opening::Read),
            Some("ToWrite") => Ok(Opening::Write),
            Some("ToAppend") => Ok(Opening::Append),
            Some("ToCreate") => Ok(Opening::Create),
            _ => Err(not_an_opening(span)),
        },
        _ => Err(not_an_opening(span)),
    }
}

enum Second {
    None,
    Opening(Opening),
    Body(Arc<[u8]>),
    Path(String),
    Target(String),
    Name(String),
    Mode(u32),
    Millis(i64),
    Range { offset: i64, len: i64 },
    Deep(bool),
}

fn run(
    op: Op,
    root: &Path,
    path: &str,
    second: Second,
    held: &Mutex<BTreeSet<PathBuf>>,
    descriptors: &Mutex<Descriptors>,
    span: Span,
) -> JobOutput {
    let target = match confine(root, path, span) {
        Ok(target) => target,
        Err(refusal) => return JobOutput::Refused(refusal),
    };
    match op {
        Op::Open => match second {
            Second::Opening(how) => JobOutput::Opened(
                open_file(descriptors, root, &target, how),
                how != Opening::Read,
            ),
            _ => JobOutput::Failed("an open that does not say how reached the pool".into()),
        },
        Op::ReadChunk | Op::WriteChunk | Op::Close => JobOutput::Failed(
            "an operation on an open file reached the pool as one on a path".into(),
        ),
        Op::ReadFile => match std::fs::metadata(&target) {
            Err(_) => JobOutput::MaybeBytes(None),
            Ok(meta) if !meta.is_file() => JobOutput::MaybeBytes(None),
            Ok(meta) if meta.len() > MAX_READ_BYTES => {
                JobOutput::Refused(too_large(meta.len(), path, span))
            }
            Ok(_) => match std::fs::read(&target) {
                Ok(bytes) => JobOutput::MaybeBytes(Some(bytes)),
                Err(_) => JobOutput::MaybeBytes(None),
            },
        },
        Op::ListDir => match std::fs::read_dir(&target) {
            Err(_) => JobOutput::MaybeStrings(None),
            Ok(entries) => {
                let mut names: Vec<String> = entries
                    .filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect();
                // Read order is a fact about the filesystem, not the directory's contents.
                names.sort();
                JobOutput::MaybeStrings(Some(names))
            }
        },
        // `symlink_metadata` does not follow, so a symlink is reported as one rather than as its target.
        Op::Kind => JobOutput::Ctor(match std::fs::symlink_metadata(&target) {
            Ok(meta) if meta.is_symlink() => "std.fs.Symlink",
            Ok(meta) if meta.is_dir() => "std.fs.Dir",
            Ok(meta) if meta.is_file() => "std.fs.File",
            _ => "std.fs.Missing",
        }),
        // `metadata` follows, so this answers what the path resolves to. `confine` has already
        // refused a target that resolves outside the root, so following one cannot leave it.
        Op::Resolved => JobOutput::Ctor(match std::fs::metadata(&target) {
            Ok(meta) if meta.is_dir() => "std.fs.Dir",
            Ok(meta) if meta.is_file() => "std.fs.File",
            _ => "std.fs.Missing",
        }),
        Op::Exists => JobOutput::Bool(std::fs::symlink_metadata(&target).is_ok()),
        Op::FileSize => JobOutput::MaybeInt(
            std::fs::metadata(&target)
                .ok()
                .filter(|m| m.is_file())
                .and_then(|m| i64::try_from(m.len()).ok()),
        ),
        Op::ModifiedMs => JobOutput::MaybeInt(
            std::fs::metadata(&target)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .and_then(|d| i64::try_from(d.as_millis()).ok()),
        ),
        Op::WriteFile => match second {
            Second::Body(body) => JobOutput::Bool(std::fs::write(&target, &body[..]).is_ok()),
            _ => JobOutput::Failed("a write with no body reached the pool".into()),
        },
        // Idempotent, so a cache writer need not check first and race with itself.
        Op::CreateDir => JobOutput::Bool(std::fs::create_dir_all(&target).is_ok()),
        // One file, or one empty directory.
        Op::Remove => match std::fs::symlink_metadata(&target) {
            Err(_) => JobOutput::Bool(false),
            Ok(meta) if meta.is_dir() => JobOutput::Bool(std::fs::remove_dir(&target).is_ok()),
            Ok(_) => JobOutput::Bool(std::fs::remove_file(&target).is_ok()),
        },
        Op::Rename => match second {
            Second::Path(to) => match confine(root, &to, span) {
                // Both paths are under one label, which makes this the atomic cache write.
                Err(refusal) => JobOutput::Refused(refusal),
                Ok(destination) => JobOutput::Bool(std::fs::rename(&target, &destination).is_ok()),
            },
            _ => JobOutput::Failed("a rename with no destination reached the pool".into()),
        },
        Op::ReadAt => match second {
            Second::Range { offset, len } => {
                JobOutput::MaybeBytes(read_range(&target, offset, len))
            }
            _ => JobOutput::Failed("a ranged read with no range reached the pool".into()),
        },
        Op::Append => match second {
            Second::Body(body) => JobOutput::MaybeInt(append_to(&target, &body)),
            _ => JobOutput::Failed("an append with no body reached the pool".into()),
        },
        Op::Sync => JobOutput::Bool(sync_path(&target)),
        Op::Lock => JobOutput::Bool(take_lock(&target, held)),
        Op::Unlock => JobOutput::Bool(drop_lock(&target, held)),
        Op::Copy => match second {
            Second::Path(to) => match confine(root, &to, span) {
                Err(refusal) => JobOutput::Refused(refusal),
                Ok(destination) => JobOutput::Bool(copy_file(&target, &destination)),
            },
            _ => JobOutput::Failed("a copy with no destination reached the pool".into()),
        },
        Op::RemoveTree => JobOutput::Bool(names_below_root(path) && remove_tree(&target)),
        Op::TempDir => match second {
            Second::Name(prefix) => JobOutput::MaybeString(temp_dir(path, &target, &prefix)),
            _ => JobOutput::Failed("a temporary directory with no prefix reached the pool".into()),
        },
        Op::Canonical => JobOutput::MaybeString(
            std::fs::canonicalize(&target)
                .ok()
                .and_then(|real| real.to_str().map(str::to_string)),
        ),
        Op::Mode => JobOutput::MaybeMode(mode_of(&target)),
        Op::SetMode => match second {
            Second::Mode(bits) => JobOutput::Bool(set_mode(&target, bits)),
            _ => JobOutput::Failed("a mode change with no mode reached the pool".into()),
        },
        Op::Symlink => match second {
            Second::Target(to) => match link_stays(root, path, &to, span) {
                Err(refusal) => JobOutput::Refused(refusal),
                Ok(()) => JobOutput::Bool(std::os::unix::fs::symlink(&to, &target).is_ok()),
            },
            _ => JobOutput::Failed("a link with no target reached the pool".into()),
        },
        Op::ReadLink => JobOutput::MaybeString(
            std::fs::read_link(&target)
                .ok()
                .and_then(|to| to.to_str().map(str::to_string)),
        ),
        Op::Walk => match walk(path, &target) {
            Ok(entries) => JobOutput::MaybeEntries(entries),
            Err(bytes) => JobOutput::Refused(walk_too_large(bytes, path, span)),
        },
        Op::SetModified => match second {
            Second::Millis(ms) => JobOutput::Bool(set_modified(&target, ms)),
            _ => JobOutput::Failed("a stamp with no time reached the pool".into()),
        },
        Op::Stat => JobOutput::MaybeStat(stat_of(&target)),
        Op::Scan => match second {
            Second::Deep(deep) => match scan(path, &target, deep) {
                Ok(entries) => JobOutput::MaybeScan(entries),
                Err(bytes) => JobOutput::Refused(walk_too_large(bytes, path, span)),
            },
            _ => JobOutput::Failed("a scan that does not say how deep reached the pool".into()),
        },
        Op::Space => JobOutput::MaybeSpace(space_of(&target)),
        Op::Link => match second {
            Second::Path(to) => match confine(root, &to, span) {
                Err(refusal) => JobOutput::Refused(refusal),
                Ok(existing) => JobOutput::Done(hard_link(&existing, &target)),
            },
            _ => JobOutput::Failed("a link with no file to name reached the pool".into()),
        },
    }
}

/// What `op` read of the world or wrote to it, for the record of the test whose machine asked. Taken
/// after the operation, so a directory `temp_dir` made is known by the name it got.
fn observed(op: Op, root: &Path, path: &str, done: &JobOutput, machine: MachineId, span: Span) {
    use crate::observe::{Read, read, wrote};
    let Ok(target) = confine(root, path, span) else {
        return;
    };
    match op {
        Op::ReadFile | Op::ReadAt | Op::FileSize | Op::Mode | Op::ReadLink => {
            read(machine, Read::File, &target)
        }
        Op::ListDir => read(machine, Read::Dir, &target),
        Op::Walk | Op::Scan => read(machine, Read::Tree, &target),
        Op::Stat => {
            read(machine, Read::Kind, &target);
            read(machine, Read::File, &target)
        }
        Op::Kind | Op::Resolved | Op::Exists | Op::Canonical => read(machine, Read::Kind, &target),
        // A stamp is when, not what; a sync and a lock change nothing a read answers; and the room a
        // file system has is the machine's, as the clock is.
        Op::ModifiedMs | Op::Sync | Op::Lock | Op::Unlock | Op::Space => {}
        // What an open file is read or written through is recorded when it opens.
        Op::Open => match done {
            JobOutput::Opened(_, true) => wrote(machine, &target),
            _ => read(machine, Read::File, &target),
        },
        Op::ReadChunk | Op::WriteChunk | Op::Close => {}
        Op::WriteFile
        | Op::Append
        | Op::CreateDir
        | Op::Remove
        | Op::RemoveTree
        | Op::SetMode
        | Op::SetModified
        | Op::Symlink
        | Op::Link
        | Op::Rename
        | Op::Copy => wrote(machine, &target),
        Op::TempDir => {
            if let JobOutput::MaybeString(Some(made)) = done
                && let Ok(made) = confine(root, made, span)
            {
                wrote(machine, &made)
            }
        }
    }
}

/// Why the file system refused, as `std.fs.Refused` says it.
fn refusal(e: &std::io::Error) -> Refusal {
    match e.kind() {
        ErrorKind::NotFound => Refusal::NotFound,
        ErrorKind::PermissionDenied => Refusal::Denied,
        ErrorKind::AlreadyExists => Refusal::Exists,
        ErrorKind::NotADirectory => Refusal::NotADirectory,
        ErrorKind::IsADirectory => Refusal::NotAFile,
        ErrorKind::StorageFull | ErrorKind::QuotaExceeded => Refusal::NoSpace,
        _ => Refusal::Other(e.raw_os_error().map_or(0, i64::from), e.to_string()),
    }
}

/// What one look reads of what `meta` describes: a file's length and how many names it has, and
/// for anything else neither, since a directory's are the file system's own bookkeeping.
fn stat_from(meta: &std::fs::Metadata) -> Option<pool::Stat> {
    use std::os::unix::fs::MetadataExt;
    let kind = if meta.is_symlink() {
        KIND_SYMLINK
    } else if meta.is_dir() {
        KIND_DIR
    } else if meta.is_file() {
        KIND_FILE
    } else {
        return None;
    };
    let file = kind == KIND_FILE;
    Some(pool::Stat {
        kind,
        size: if file {
            i64::try_from(meta.len()).ok()?
        } else {
            0
        },
        modified: meta
            .mtime()
            .saturating_mul(1_000_000_000)
            .saturating_add(meta.mtime_nsec()),
        // A link's own bits are the platform's and bar nothing, so every link reads alike.
        mode: if kind == KIND_SYMLINK {
            0o777
        } else {
            meta.mode() & 0o777
        },
        links: if file {
            i64::try_from(meta.nlink()).ok()?
        } else {
            1
        },
        // An identity is compared and never counted with, so a number past `i64` wraps.
        device: meta.dev() as i64,
        id: meta.ino() as i64,
    })
}

/// `symlink_metadata` does not follow, so a link is read as itself.
fn stat_of(target: &Path) -> Option<pool::Stat> {
    stat_from(&std::fs::symlink_metadata(target).ok()?)
}

/// The room in the file system that holds `target`, in bytes and in inodes.
// Each field's width is the platform's, so a cast one target needs is a no-op on another.
#[allow(clippy::unnecessary_cast)]
fn space_of(target: &Path) -> Option<pool::Space> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(target.as_os_str().as_bytes()).ok()?;
    let mut held = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is NUL-terminated and `held` is room for one `statvfs`, which the call fills
    // when it answers zero.
    let room = unsafe {
        if libc::statvfs(path.as_ptr(), held.as_mut_ptr()) != 0 {
            return None;
        }
        held.assume_init()
    };
    let bytes = |blocks: u64| i64::try_from(blocks.saturating_mul(room.f_frsize as u64)).ok();
    Some(pool::Space {
        total: bytes(room.f_blocks as u64)?,
        free: bytes(room.f_bfree as u64)?,
        available: bytes(room.f_bavail as u64)?,
        inodes: i64::try_from(room.f_files as u64).ok()?,
        inodes_free: i64::try_from(room.f_ffree as u64).ok()?,
    })
}

/// `to` made another name for the file at `existing`, which is read without following a link:
/// only a file takes a second name here.
fn hard_link(existing: &Path, to: &Path) -> Result<(), Refusal> {
    match std::fs::symlink_metadata(existing) {
        Err(e) => Err(refusal(&e)),
        Ok(meta) if !meta.is_file() => Err(Refusal::NotAFile),
        Ok(_) => std::fs::hard_link(existing, to).map_err(|e| refusal(&e)),
    }
}

/// A file's bytes and permission bits, over whatever the destination held; never a directory. A
/// file copied onto one of its own names is left as it is: opening it to write would empty it.
fn copy_file(from: &Path, to: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(source) = std::fs::metadata(from) else {
        return false;
    };
    if !source.is_file() {
        return false;
    }
    let itself = std::fs::metadata(to)
        .is_ok_and(|held| held.dev() == source.dev() && held.ino() == source.ino());
    itself || std::fs::copy(from, to).is_ok()
}

/// A path naming the root itself names nothing under it, so no operation removes the root.
fn names_below_root(path: &str) -> bool {
    Path::new(path)
        .components()
        .any(|c| matches!(c, Component::Normal(_)))
}

/// A symlink is removed as itself; a directory with everything under it, its links unfollowed.
fn remove_tree(target: &Path) -> bool {
    match std::fs::symlink_metadata(target) {
        Err(_) => false,
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(target).is_ok(),
        Ok(_) => std::fs::remove_file(target).is_ok(),
    }
}

/// The path as the root names it: `.` and empty segments dropped, so `./a/` and `a` agree.
fn under_root(path: &str) -> String {
    Path::new(path)
        .components()
        .filter_map(|c| match c {
            Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn joined(dir: &str, name: &str) -> String {
    let dir = under_root(dir);
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// `create_dir` rather than `create_dir_all`: a name another run made first is a collision to
/// retry, never a directory to share.
fn temp_dir(dir: &str, target: &Path, prefix: &str) -> Option<String> {
    if prefix.contains('/') || prefix.contains('\0') || !target.is_dir() {
        return None;
    }
    for _ in 0..64 {
        let name = format!("{prefix}{}", unique_suffix());
        match std::fs::create_dir(target.join(&name)) {
            Ok(()) => return Some(joined(dir, &name)),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
    None
}

fn unique_suffix() -> String {
    use std::hash::{BuildHasher, Hasher};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(NEXT.fetch_add(1, Ordering::Relaxed));
    hasher.write_u32(std::process::id());
    hasher.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    format!("{:012x}", hasher.finish() & 0xffff_ffff_ffff)
}

fn mode_of(target: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(target)
        .ok()
        .map(|meta| meta.permissions().mode() & 0o777)
}

/// The nine permission bits; the set-id and sticky bits the file had are kept.
fn set_mode(target: &Path, bits: u32) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = std::fs::metadata(target) else {
        return false;
    };
    let kept = meta.permissions().mode() & !0o777;
    std::fs::set_permissions(
        target,
        std::fs::Permissions::from_mode(kept | (bits & 0o777)),
    )
    .is_ok()
}

/// Read from the link's own directory, a target must stay under the root: one that names another
/// root or climbs out of this one would hand whoever follows the link a path outside it.
fn link_stays(root: &Path, link: &str, target: &str, span: Span) -> Result<(), Diagnostic> {
    let to = Path::new(target);
    if to.is_absolute() {
        return Err(escapes(
            root,
            target,
            "a link's target names its own root",
            span,
        ));
    }
    let mut depth = Path::new(link)
        .components()
        .filter(|c| matches!(c, Component::Normal(_)))
        .count()
        .saturating_sub(1);
    for component in to.components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::ParentDir if depth == 0 => {
                return Err(escapes(
                    root,
                    target,
                    "read from the link's directory, `..` leaves the root",
                    span,
                ));
            }
            Component::ParentDir => depth -= 1,
            _ => {}
        }
    }
    Ok(())
}

pub const KIND_FILE: &str = "std.fs.File";
pub const KIND_DIR: &str = "std.fs.Dir";
pub const KIND_SYMLINK: &str = "std.fs.Symlink";

/// Every entry under `dir`, depth first and each directory's names in byte order, a symlink listed
/// and never followed. `Err` carries the bytes of paths gathered once they pass what one answer
/// holds.
fn walk(dir: &str, target: &Path) -> Result<Option<Vec<(String, &'static str)>>, u64> {
    if !std::fs::metadata(target).is_ok_and(|m| m.is_dir()) {
        return Ok(None);
    }
    let mut out = Vec::new();
    let mut bytes = 0;
    walk_into(&under_root(dir), target, &mut out, &mut bytes)?;
    Ok(Some(out))
}

fn walk_into(
    prefix: &str,
    at: &Path,
    out: &mut Vec<(String, &'static str)>,
    bytes: &mut u64,
) -> Result<(), u64> {
    let Ok(entries) = std::fs::read_dir(at) else {
        return Ok(());
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    for name in names {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        *bytes += path.len() as u64;
        if *bytes > MAX_READ_BYTES {
            return Err(*bytes);
        }
        let full = at.join(&name);
        let kind = match std::fs::symlink_metadata(&full) {
            Ok(meta) if meta.is_symlink() => KIND_SYMLINK,
            Ok(meta) if meta.is_dir() => KIND_DIR,
            Ok(meta) if meta.is_file() => KIND_FILE,
            _ => continue,
        };
        out.push((path.clone(), kind));
        if kind == KIND_DIR {
            walk_into(&path, &full, out, bytes)?;
        }
    }
    Ok(())
}

/// Every entry under `dir` with what one look reads of it, in a walk's order: its own entries
/// alone unless `deep`. `Err` as a walk's is.
fn scan(dir: &str, target: &Path, deep: bool) -> Result<Option<Vec<(String, pool::Stat)>>, u64> {
    if !std::fs::metadata(target).is_ok_and(|m| m.is_dir()) {
        return Ok(None);
    }
    let mut out = Vec::new();
    let mut bytes = 0;
    scan_into(&under_root(dir), target, deep, &mut out, &mut bytes)?;
    Ok(Some(out))
}

fn scan_into(
    prefix: &str,
    at: &Path,
    deep: bool,
    out: &mut Vec<(String, pool::Stat)>,
    bytes: &mut u64,
) -> Result<(), u64> {
    let Ok(entries) = std::fs::read_dir(at) else {
        return Ok(());
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    for name in names {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        *bytes += path.len() as u64;
        if *bytes > MAX_READ_BYTES {
            return Err(*bytes);
        }
        let full = at.join(&name);
        let Some(found) = stat_of(&full) else {
            continue;
        };
        let below = deep && found.kind == KIND_DIR;
        out.push((path.clone(), found));
        if below {
            scan_into(&path, &full, deep, out, bytes)?;
        }
    }
    Ok(())
}

/// A directory is opened as a file to stamp it, which `futimens` allows its owner.
fn set_modified(target: &Path, ms: i64) -> bool {
    File::open(target)
        .and_then(|file| file.set_modified(UNIX_EPOCH + Duration::from_millis(ms as u64)))
        .is_ok()
}

/// A `std.fs.Mode` as its nine permission bits: the owner's, the group's and everyone else's.
fn mode_bits(mode: &Value, span: Span) -> Result<u32, Diagnostic> {
    let access = |who: &str| -> Option<u32> {
        let Value::Record(fields) = mode else {
            return None;
        };
        let Some(Value::Record(access)) = fields.named(who) else {
            return None;
        };
        let bit = |name: &str, value: u32| match access.named(name) {
            Some(Value::Bool(true)) => Some(value),
            Some(Value::Bool(false)) => Some(0),
            _ => None,
        };
        Some(bit("read", 4)? | bit("write", 2)? | bit("execute", 1)?)
    };
    match (access("owner"), access("group"), access("other")) {
        (Some(owner), Some(group), Some(other)) => Ok(owner << 6 | group << 3 | other),
        _ => Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            "`fs.set_mode` was handed something that is not a `std.fs.Mode`".to_string(),
        )
        .primary(span, "a well-typed call hands a mode")),
    }
}

/// Costs the size of what is new rather than the size of the file, which is the point of it. The
/// answer is the offset the bytes landed at: `O_APPEND` puts them at the end atomically, and the
/// descriptor's position afterwards is the end of what *this* write produced, so a second appender
/// racing this one moves neither the bytes nor the answer.
fn append_to(target: &Path, body: &[u8]) -> Option<i64> {
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(target)
        .ok()?;
    // A write of nothing writes nothing, leaving the descriptor at 0 rather than at the end.
    if body.is_empty() {
        return i64::try_from(file.metadata().ok()?.len()).ok();
    }
    file.write_all(body).ok()?;
    let end = file.stream_position().ok()?;
    i64::try_from(end.checked_sub(body.len() as u64)?).ok()
}

/// What is there, which may be less than was asked for: a file can end before the range does, and
/// a reader that recorded an offset cannot be told its cache is short by a diagnostic it cannot
/// catch. `None` keeps the meaning it has for `read_file`: this path names nothing to read.
fn read_range(target: &Path, offset: i64, len: i64) -> Option<Vec<u8>> {
    let mut file = File::open(target).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    file.seek(SeekFrom::Start(offset as u64)).ok()?;
    let mut out = Vec::new();
    file.take(len as u64).read_to_end(&mut out).ok()?;
    Some(out)
}

/// A file's own bytes, or — for a directory — the names in it, which is what makes a rename
/// durable. Both are what an append-only store fsyncs before an index is allowed to name them.
fn sync_path(target: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(target) else {
        return false;
    };
    // A directory cannot be opened for writing, and its entries are what fsync flushes either way.
    let opened = if meta.is_dir() {
        File::open(target)
    } else {
        OpenOptions::new().write(true).open(target)
    };
    opened.and_then(|file| file.sync_all()).is_ok()
}

/// `O_CREAT|O_EXCL` on a lock file, waiting out a holder and breaking one a dead run left behind.
pub fn take_lock(target: &Path, held: &Mutex<BTreeSet<PathBuf>>) -> bool {
    take_lock_within(target, held, LOCK_WAIT)
}

/// [`take_lock`] over a wait it does not choose; `fs.lock` always waits [`LOCK_WAIT`].
pub fn take_lock_within(target: &Path, held: &Mutex<BTreeSet<PathBuf>>, wait: Duration) -> bool {
    let deadline = Instant::now() + wait;
    loop {
        match OpenOptions::new().write(true).create_new(true).open(target) {
            Ok(_) => {
                lock(held).insert(target.to_path_buf());
                return true;
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            // No directory to hold it, or no permission: waiting would not change either.
            Err(_) => return false,
        }
        if Instant::now() >= deadline {
            return false;
        }
        if is_older_than(target, LOCK_STALE_AGE) {
            let _ = std::fs::remove_file(target);
        }
        std::thread::sleep(LOCK_POLL);
    }
}

/// The claim, not the file, is what a run releases: a lock it never took is not its to break.
pub fn drop_lock(target: &Path, held: &Mutex<BTreeSet<PathBuf>>) -> bool {
    if !lock(held).remove(target) {
        return false;
    }
    let _ = std::fs::remove_file(target);
    true
}

fn is_older_than(path: &Path, age: Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .is_ok_and(|m| m.elapsed().is_ok_and(|elapsed| elapsed >= age))
}

/// The guarded set has no invariant a panicking job can break, so recovering is correct.
fn lock(held: &Mutex<BTreeSet<PathBuf>>) -> MutexGuard<'_, BTreeSet<PathBuf>> {
    held.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn confine(root: &Path, path: &str, span: Span) -> Result<PathBuf, Diagnostic> {
    let relative = Path::new(path);
    if relative.is_absolute() {
        return Err(escapes(
            root,
            path,
            "an absolute path names its own root",
            span,
        ));
    }
    if relative
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(escapes(
            root,
            path,
            "`..` leaves the root it starts in",
            span,
        ));
    }

    let target = root.join(relative);
    let mut existing = target.as_path();
    loop {
        match existing.canonicalize() {
            Ok(real) => {
                if !real.starts_with(root) {
                    return Err(escapes(
                        root,
                        path,
                        &format!("it resolves to `{}`", real.display()),
                        span,
                    ));
                }
                return Ok(target);
            }
            // Not there yet: check the nearest existing ancestor, where a symlink could escape.
            Err(_) => match existing.parent() {
                Some(parent) if parent.starts_with(root) => existing = parent,
                // No existing ancestor inside the root, so the lexical checks suffice.
                _ => return Ok(target),
            },
        }
    }
}

#[cold]
fn escapes(root: &Path, path: &str, why: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::FS_PATH_ESCAPES_ROOT,
        format!("`{path}` leaves the root it was given"),
    )
    .primary(span, why.to_string())
    .note(format!("the root is `{}`", root.display()))
    .note("a resource label names a root and an operation reaches only what is under it; a path that leaves one is refused before the operation runs")
}

#[cold]
fn too_large(bytes: u64, path: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::FS_FILE_TOO_LARGE,
        format!("a read of `{path}` would answer {bytes} bytes"),
    )
    .primary(span, "this is more than one read answers")
    .note(format!(
        "a read answers with one whole value, and the bound is {MAX_READ_BYTES} bytes"
    ))
    .note("read it a range at a time with `fs.read_at(path, offset, len)`, which is also what keeps the cost of reading a large file proportional to the part that is wanted")
}

#[cold]
fn walk_too_large(bytes: u64, path: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::FS_FILE_TOO_LARGE,
        format!("a walk of `{path}` would answer more than {bytes} bytes of paths"),
    )
    .primary(span, "this is more than one walk answers")
    .note(format!(
        "a walk answers with one whole value, and the bound is {MAX_READ_BYTES} bytes"
    ))
    .note("walk the tree a directory at a time with `fs.list_dir`")
}

#[cold]
fn before_epoch(ms: i64, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`fs.set_modified` was given {ms} milliseconds"),
    )
    .primary(
        span,
        "a modification time is milliseconds since the Unix epoch, never before it",
    )
}

#[cold]
fn negative_range(offset: i64, len: i64, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`fs.read_at` was given offset {offset} and length {len}"),
    )
    .primary(span, "neither an offset nor a length can be negative")
    .note("a range running past the end of a file is answered short, because a file can shrink between the call that measured it and the call that reads it; a negative one is arithmetic that went wrong, which no file can answer")
}

#[cold]
fn negative_chunk(max: i64, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`fs.read_chunk` was asked for {max} bytes"),
    )
    .primary(span, "a chunk's length cannot be negative")
}

#[cold]
fn chunk_too_large(max: u64, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::FS_FILE_TOO_LARGE,
        format!("`fs.read_chunk` was asked for {max} bytes"),
    )
    .primary(span, "this is more than one read answers")
    .note(format!(
        "a read answers with one whole value, and the bound is {MAX_READ_BYTES} bytes"
    ))
    .note("ask for a smaller chunk: a read costs at most the chunk it asks for, whatever the file holds")
}

#[cold]
fn not_an_opening(span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        "`fs.open` was not told how to open the file",
    )
    .primary(
        span,
        "this is none of `ToRead`, `ToWrite`, `ToAppend` and `ToCreate`",
    )
}

#[cold]
pub fn unbound(op: Op, at: &Resource, span: Span) -> Diagnostic {
    let label = match at {
        Resource::Named(name) => name.as_str().to_string(),
        Resource::Var(v) => ply_eval::label_var_name(*v),
        Resource::Singleton => "the singleton resource".to_string(),
        Resource::Every => "every label".to_string(),
    };
    Diagnostic::error(
        codes::FS_ROOT_UNBOUND,
        format!("{} names `{label}`, and no root is bound to it", op.what()),
    )
    .primary(span, format!("`{label}` has no root"))
    .note(format!(
        "bind one beside the run: `--fs {label}=<directory>`"
    ))
    .note("a resource label is the capability: what a filesystem operation may reach is named where the run is configured, never in the program")
}

#[cold]
fn arity(op: Op, given: usize, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!(
            "{} takes {} argument(s) and was given {given}",
            op.what(),
            op.arity()
        ),
    )
    .primary(span, "this call does not match the declaration")
}
