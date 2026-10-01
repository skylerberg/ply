//! The filesystem, as operations confined to roots the run names.

use crate::pool::{Bell, Done, FS_FIRST_TOKEN, Inbox, Pool};
use ply_eval::host::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRegistry, HostRequest, HostResource,
    HostRuntime, Linearity,
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

/// In the order `std.fs` declares them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    ReadFile,
    ReadAt,
    ListDir,
    Kind,
    Resolved,
    Exists,
    FileSize,
    ModifiedMs,
    WriteFile,
    Append,
    CreateDir,
    Remove,
    Rename,
    Sync,
    Lock,
    Unlock,
    Copy,
    RemoveTree,
    TempDir,
    Canonical,
    Mode,
    SetMode,
    Symlink,
    ReadLink,
    Walk,
    SetModified,
}

impl Op {
    pub const ALL: [Op; 26] = [
        Op::ReadFile,
        Op::ReadAt,
        Op::ListDir,
        Op::Kind,
        Op::Resolved,
        Op::Exists,
        Op::FileSize,
        Op::ModifiedMs,
        Op::WriteFile,
        Op::Append,
        Op::CreateDir,
        Op::Remove,
        Op::Rename,
        Op::Sync,
        Op::Lock,
        Op::Unlock,
        Op::Copy,
        Op::RemoveTree,
        Op::TempDir,
        Op::Canonical,
        Op::Mode,
        Op::SetMode,
        Op::Symlink,
        Op::ReadLink,
        Op::Walk,
        Op::SetModified,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Op::ReadFile => "read_file",
            Op::ReadAt => "read_at",
            Op::ListDir => "list_dir",
            Op::Kind => "kind",
            Op::Resolved => "resolved",
            Op::Exists => "exists",
            Op::FileSize => "file_size",
            Op::ModifiedMs => "modified_ms",
            Op::WriteFile => "write_file",
            Op::Append => "append",
            Op::CreateDir => "create_dir",
            Op::Remove => "remove",
            Op::Rename => "rename",
            Op::Sync => "sync",
            Op::Lock => "lock",
            Op::Unlock => "unlock",
            Op::Copy => "copy",
            Op::RemoveTree => "remove_tree",
            Op::TempDir => "temp_dir",
            Op::Canonical => "canonical",
            Op::Mode => "mode",
            Op::SetMode => "set_mode",
            Op::Symlink => "symlink",
            Op::ReadLink => "read_link",
            Op::Walk => "walk",
            Op::SetModified => "set_modified",
        }
    }

    pub fn what(self) -> &'static str {
        match self {
            Op::ReadFile => "`fs.read_file`",
            Op::ReadAt => "`fs.read_at`",
            Op::ListDir => "`fs.list_dir`",
            Op::Kind => "`fs.kind`",
            Op::Resolved => "`fs.resolved`",
            Op::Exists => "`fs.exists`",
            Op::FileSize => "`fs.file_size`",
            Op::ModifiedMs => "`fs.modified_ms`",
            Op::WriteFile => "`fs.write_file`",
            Op::Append => "`fs.append`",
            Op::CreateDir => "`fs.create_dir`",
            Op::Remove => "`fs.remove`",
            Op::Rename => "`fs.rename`",
            Op::Sync => "`fs.sync`",
            Op::Lock => "`fs.lock`",
            Op::Unlock => "`fs.unlock`",
            Op::Copy => "`fs.copy`",
            Op::RemoveTree => "`fs.remove_tree`",
            Op::TempDir => "`fs.temp_dir`",
            Op::Canonical => "`fs.canonical`",
            Op::Mode => "`fs.mode`",
            Op::SetMode => "`fs.set_mode`",
            Op::Symlink => "`fs.symlink`",
            Op::ReadLink => "`fs.read_link`",
            Op::Walk => "`fs.walk`",
            Op::SetModified => "`fs.set_modified`",
        }
    }

    fn arity(self) -> usize {
        match self {
            Op::ReadAt => 3,
            Op::WriteFile
            | Op::Append
            | Op::Rename
            | Op::Copy
            | Op::TempDir
            | Op::SetMode
            | Op::Symlink
            | Op::SetModified => 2,
            _ => 1,
        }
    }

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
        }
    }

    pub fn declaration(self, path: &'static str) -> HostOp {
        HostOp {
            effect: Symbol::new(EFFECT),
            op: Symbol::new(self.name()),
            resource: HostResource::Any,
            determinism: Determinism::Nondeterministic,
            linearity: Linearity::AtMostOnce,
            blocking: true,
            // No expression turns a `Secret` into a path `String` or a body `Bytes`.
            secrets: false,
            path,
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

pub struct FsHost {
    roots: Roots,
    pool: Pool,
    /// The lock files this run took and has not released; a lock it did not take it cannot release.
    held: Arc<Mutex<BTreeSet<PathBuf>>>,
}

impl FsHost {
    pub fn new(roots: Roots) -> FsHost {
        FsHost {
            roots,
            pool: Pool::new(FS_FIRST_TOKEN),
            held: Arc::new(Mutex::new(BTreeSet::new())),
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

    fn path(op: Op) -> &'static str {
        match op {
            Op::ReadFile => "ply_host::fs::read_file",
            Op::ReadAt => "ply_host::fs::read_at",
            Op::ListDir => "ply_host::fs::list_dir",
            Op::Kind => "ply_host::fs::kind",
            Op::Resolved => "ply_host::fs::resolved",
            Op::Exists => "ply_host::fs::exists",
            Op::FileSize => "ply_host::fs::file_size",
            Op::ModifiedMs => "ply_host::fs::modified_ms",
            Op::WriteFile => "ply_host::fs::write_file",
            Op::Append => "ply_host::fs::append",
            Op::CreateDir => "ply_host::fs::create_dir",
            Op::Remove => "ply_host::fs::remove",
            Op::Rename => "ply_host::fs::rename",
            Op::Sync => "ply_host::fs::sync",
            Op::Lock => "ply_host::fs::lock",
            Op::Unlock => "ply_host::fs::unlock",
            Op::Copy => "ply_host::fs::copy",
            Op::RemoveTree => "ply_host::fs::remove_tree",
            Op::TempDir => "ply_host::fs::temp_dir",
            Op::Canonical => "ply_host::fs::canonical",
            Op::Mode => "ply_host::fs::mode",
            Op::SetMode => "ply_host::fs::set_mode",
            Op::Symlink => "ply_host::fs::symlink",
            Op::ReadLink => "ply_host::fs::read_link",
            Op::Walk => "ply_host::fs::walk",
            Op::SetModified => "ply_host::fs::set_modified",
        }
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
            (op.declaration(FsHost::path(*op)), handler)
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

        let first = req.args[0].as_str(span, "a path")?.to_string();
        let second = match self.op {
            Op::WriteFile | Op::Append => {
                Second::Body(Arc::clone(req.args[1].as_bytes(span, "a body")?))
            }
            Op::Rename | Op::Copy => Second::Path(req.args[1].as_str(span, "a path")?.to_string()),
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
        let pending = self.fs.pool.submit(
            span,
            op.label(),
            op.what(),
            Box::new(move || run(op, &root, &first, second, &held, span)),
        )?;
        Ok(HostAnswer::Pending(pending))
    }
}

enum Second {
    None,
    Body(Arc<[u8]>),
    Path(String),
    Target(String),
    Name(String),
    Mode(u32),
    Millis(i64),
    Range { offset: i64, len: i64 },
}

fn run(
    op: Op,
    root: &Path,
    path: &str,
    second: Second,
    held: &Mutex<BTreeSet<PathBuf>>,
    span: Span,
) -> Done {
    let target = match confine(root, path, span) {
        Ok(target) => target,
        Err(refusal) => return Done::Refused(refusal),
    };
    match op {
        Op::ReadFile => match std::fs::metadata(&target) {
            Err(_) => Done::MaybeBytes(None),
            Ok(meta) if !meta.is_file() => Done::MaybeBytes(None),
            Ok(meta) if meta.len() > MAX_READ_BYTES => {
                Done::Refused(too_large(meta.len(), path, span))
            }
            Ok(_) => match std::fs::read(&target) {
                Ok(bytes) => Done::MaybeBytes(Some(bytes)),
                Err(_) => Done::MaybeBytes(None),
            },
        },
        Op::ListDir => match std::fs::read_dir(&target) {
            Err(_) => Done::MaybeStrings(None),
            Ok(entries) => {
                let mut names: Vec<String> = entries
                    .filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect();
                // Read order is a fact about the filesystem, not the directory's contents.
                names.sort();
                Done::MaybeStrings(Some(names))
            }
        },
        // `symlink_metadata` does not follow, so a symlink is reported as one rather than as its target.
        Op::Kind => Done::Ctor(match std::fs::symlink_metadata(&target) {
            Ok(meta) if meta.is_symlink() => "std.fs.Symlink",
            Ok(meta) if meta.is_dir() => "std.fs.Dir",
            Ok(meta) if meta.is_file() => "std.fs.File",
            _ => "std.fs.Missing",
        }),
        // `metadata` follows, so this answers what the path resolves to. `confine` has already
        // refused a target that resolves outside the root, so following one cannot leave it.
        Op::Resolved => Done::Ctor(match std::fs::metadata(&target) {
            Ok(meta) if meta.is_dir() => "std.fs.Dir",
            Ok(meta) if meta.is_file() => "std.fs.File",
            _ => "std.fs.Missing",
        }),
        Op::Exists => Done::Bool(std::fs::symlink_metadata(&target).is_ok()),
        Op::FileSize => Done::MaybeInt(
            std::fs::metadata(&target)
                .ok()
                .filter(|m| m.is_file())
                .and_then(|m| i64::try_from(m.len()).ok()),
        ),
        Op::ModifiedMs => Done::MaybeInt(
            std::fs::metadata(&target)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .and_then(|d| i64::try_from(d.as_millis()).ok()),
        ),
        Op::WriteFile => match second {
            Second::Body(body) => Done::Bool(std::fs::write(&target, &body[..]).is_ok()),
            _ => Done::Failed("a write with no body reached the pool".into()),
        },
        // Idempotent, so a cache writer need not check first and race with itself.
        Op::CreateDir => Done::Bool(std::fs::create_dir_all(&target).is_ok()),
        // One file, or one empty directory.
        Op::Remove => match std::fs::symlink_metadata(&target) {
            Err(_) => Done::Bool(false),
            Ok(meta) if meta.is_dir() => Done::Bool(std::fs::remove_dir(&target).is_ok()),
            Ok(_) => Done::Bool(std::fs::remove_file(&target).is_ok()),
        },
        Op::Rename => match second {
            Second::Path(to) => match confine(root, &to, span) {
                // Both paths are under one label, which makes this the atomic cache write.
                Err(refusal) => Done::Refused(refusal),
                Ok(destination) => Done::Bool(std::fs::rename(&target, &destination).is_ok()),
            },
            _ => Done::Failed("a rename with no destination reached the pool".into()),
        },
        Op::ReadAt => match second {
            Second::Range { offset, len } => Done::MaybeBytes(read_range(&target, offset, len)),
            _ => Done::Failed("a ranged read with no range reached the pool".into()),
        },
        Op::Append => match second {
            Second::Body(body) => Done::MaybeInt(append_to(&target, &body)),
            _ => Done::Failed("an append with no body reached the pool".into()),
        },
        Op::Sync => Done::Bool(sync_path(&target)),
        Op::Lock => Done::Bool(take_lock(&target, held)),
        Op::Unlock => Done::Bool(drop_lock(&target, held)),
        Op::Copy => match second {
            Second::Path(to) => match confine(root, &to, span) {
                Err(refusal) => Done::Refused(refusal),
                Ok(destination) => Done::Bool(copy_file(&target, &destination)),
            },
            _ => Done::Failed("a copy with no destination reached the pool".into()),
        },
        Op::RemoveTree => Done::Bool(names_below_root(path) && remove_tree(&target)),
        Op::TempDir => match second {
            Second::Name(prefix) => Done::MaybeString(temp_dir(path, &target, &prefix)),
            _ => Done::Failed("a temporary directory with no prefix reached the pool".into()),
        },
        Op::Canonical => Done::MaybeString(
            std::fs::canonicalize(&target)
                .ok()
                .and_then(|real| real.to_str().map(str::to_string)),
        ),
        Op::Mode => Done::MaybeMode(mode_of(&target)),
        Op::SetMode => match second {
            Second::Mode(bits) => Done::Bool(set_mode(&target, bits)),
            _ => Done::Failed("a mode change with no mode reached the pool".into()),
        },
        Op::Symlink => match second {
            Second::Target(to) => match link_stays(root, path, &to, span) {
                Err(refusal) => Done::Refused(refusal),
                Ok(()) => Done::Bool(std::os::unix::fs::symlink(&to, &target).is_ok()),
            },
            _ => Done::Failed("a link with no target reached the pool".into()),
        },
        Op::ReadLink => Done::MaybeString(
            std::fs::read_link(&target)
                .ok()
                .and_then(|to| to.to_str().map(str::to_string)),
        ),
        Op::Walk => match walk(path, &target) {
            Ok(entries) => Done::MaybeEntries(entries),
            Err(bytes) => Done::Refused(walk_too_large(bytes, path, span)),
        },
        Op::SetModified => match second {
            Second::Millis(ms) => Done::Bool(set_modified(&target, ms)),
            _ => Done::Failed("a stamp with no time reached the pool".into()),
        },
    }
}

/// A file's bytes and permission bits, over whatever the destination held; never a directory.
fn copy_file(from: &Path, to: &Path) -> bool {
    std::fs::metadata(from).is_ok_and(|m| m.is_file()) && std::fs::copy(from, to).is_ok()
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
