//! The filesystem, as operations confined to roots the run names.

use crate::pool::{Done, FS_FIRST_TOKEN, Pool};
use ply_eval::host::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRegistry, HostRequest, HostResource,
    HostRuntime, Linearity,
};
use ply_eval::{Pending, Value};
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_ty::Resource;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// Must match the effect `std.fs` declares.
pub const EFFECT: &str = "fs";

pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// How long `fs.lock` waits for a holder to release before answering `false`.
pub const LOCK_WAIT: Duration = Duration::from_secs(2);

const LOCK_POLL: Duration = Duration::from_millis(2);

/// Far longer than a read-merge-write takes, so only a lock left by a killed process is broken.
pub const LOCK_STALE_AGE: Duration = Duration::from_secs(30);

/// In the order `std.fs` declares them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    ReadFile,
    ListDir,
    Kind,
    Exists,
    FileSize,
    ModifiedMs,
    WriteFile,
    CreateDir,
    Remove,
    Rename,
    Lock,
    Unlock,
}

impl Op {
    pub const ALL: [Op; 12] = [
        Op::ReadFile,
        Op::ListDir,
        Op::Kind,
        Op::Exists,
        Op::FileSize,
        Op::ModifiedMs,
        Op::WriteFile,
        Op::CreateDir,
        Op::Remove,
        Op::Rename,
        Op::Lock,
        Op::Unlock,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Op::ReadFile => "read_file",
            Op::ListDir => "list_dir",
            Op::Kind => "kind",
            Op::Exists => "exists",
            Op::FileSize => "file_size",
            Op::ModifiedMs => "modified_ms",
            Op::WriteFile => "write_file",
            Op::CreateDir => "create_dir",
            Op::Remove => "remove",
            Op::Rename => "rename",
            Op::Lock => "lock",
            Op::Unlock => "unlock",
        }
    }

    pub fn what(self) -> &'static str {
        match self {
            Op::ReadFile => "`fs.read_file`",
            Op::ListDir => "`fs.list_dir`",
            Op::Kind => "`fs.kind`",
            Op::Exists => "`fs.exists`",
            Op::FileSize => "`fs.file_size`",
            Op::ModifiedMs => "`fs.modified_ms`",
            Op::WriteFile => "`fs.write_file`",
            Op::CreateDir => "`fs.create_dir`",
            Op::Remove => "`fs.remove`",
            Op::Rename => "`fs.rename`",
            Op::Lock => "`fs.lock`",
            Op::Unlock => "`fs.unlock`",
        }
    }

    fn arity(self) -> usize {
        match self {
            Op::WriteFile | Op::Rename => 2,
            _ => 1,
        }
    }

    /// The thread name a job runs under.
    fn label(self) -> &'static str {
        match self {
            Op::ReadFile => "fs-read",
            Op::ListDir => "fs-list",
            Op::Kind => "fs-kind",
            Op::Exists => "fs-exists",
            Op::FileSize => "fs-size",
            Op::ModifiedMs => "fs-modified",
            Op::WriteFile => "fs-write",
            Op::CreateDir => "fs-mkdir",
            Op::Remove => "fs-remove",
            Op::Rename => "fs-rename",
            Op::Lock => "fs-lock",
            Op::Unlock => "fs-unlock",
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
            Resource::Var(_) | Resource::Singleton => None,
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

    pub fn block_on(&self, pending: Pending) -> Result<Value, Diagnostic> {
        self.pool.block_on(pending)
    }

    fn path(op: Op) -> &'static str {
        match op {
            Op::ReadFile => "ply_host::fs::read_file",
            Op::ListDir => "ply_host::fs::list_dir",
            Op::Kind => "ply_host::fs::kind",
            Op::Exists => "ply_host::fs::exists",
            Op::FileSize => "ply_host::fs::file_size",
            Op::ModifiedMs => "ply_host::fs::modified_ms",
            Op::WriteFile => "ply_host::fs::write_file",
            Op::CreateDir => "ply_host::fs::create_dir",
            Op::Remove => "ply_host::fs::remove",
            Op::Rename => "ply_host::fs::rename",
            Op::Lock => "ply_host::fs::lock",
            Op::Unlock => "ply_host::fs::unlock",
        }
    }
}

pub fn register(registry: &mut HostRegistry, fs: Arc<FsHost>) {
    for op in Op::ALL {
        registry.register(
            op.declaration(FsHost::path(op)),
            Arc::new(Operation {
                op,
                fs: Arc::clone(&fs),
            }),
        );
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
            Op::WriteFile => Second::Body(Arc::clone(req.args[1].as_bytes(span, "a body")?)),
            Op::Rename => Second::Path(req.args[1].as_str(span, "a path")?.to_string()),
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
            Ok(meta) if meta.len() > MAX_FILE_BYTES => Done::Refused(too_large(&meta, path, span)),
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
        Op::Lock => Done::Bool(take_lock(&target, held)),
        Op::Unlock => Done::Bool(drop_lock(&target, held)),
    }
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
fn too_large(meta: &std::fs::Metadata, path: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::FS_FILE_TOO_LARGE,
        format!("`{path}` is {} bytes", meta.len()),
    )
    .primary(span, "this file is larger than a whole-file read allows")
    .note(format!(
        "`fs.read_file` answers with the whole file as one value, and the bound is {MAX_FILE_BYTES} bytes"
    ))
    .note("there are no file handles and no streaming in v1, so there is no way to read part of it")
}

#[cold]
pub fn unbound(op: Op, at: &Resource, span: Span) -> Diagnostic {
    let label = match at {
        Resource::Named(name) => name.as_str().to_string(),
        Resource::Var(v) => ply_ty::label_var_name(*v),
        Resource::Singleton => "the singleton resource".to_string(),
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
