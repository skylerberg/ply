//! What `std.fs`'s real handler asks the host through its natives: the files under the roots the
//! run names, each operation's path confined to its root by a walk that follows its links here.
//! Each operation is a job that answers flat: a byte 1 or 0 for whether it did what it was asked,
//! a kind as [`Kind::number`] numbers it, a refusal as [`Refusal::flat`] writes it, and anything
//! that may be absent found.

use crate::jobs::{Failure, Refusing};
use crate::native::{found, int, parts};
use crate::pool::Run;
use ply_core::crypto::Framing;
use ply_core::{Diagnostic, Resource, Span, Symbol, codes};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use zeroize::Zeroizing;

/// The most one read answers. A larger file is read a range at a time with `fs.read_at`, so this
/// bounds a single call rather than a file.
pub const MAX_READ_BYTES: u64 = 64 * 1024 * 1024;

/// How long `fs.lock` waits for a holder to release before answering `false`.
pub const LOCK_WAIT: Duration = Duration::from_secs(2);

const LOCK_POLL: Duration = Duration::from_millis(2);

/// Far longer than a read-merge-write takes, so only a lock left by a killed process is broken.
pub const LOCK_STALE_AGE: Duration = Duration::from_secs(30);

/// An operation of `std.fs`'s effect.
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
    Open,
    ReadChunk,
    WriteChunk,
    Close,
    Stat,
    Scan,
    Space,
    Link,
    WriteSecret,
    ReadSecret,
}

impl Op {
    pub const ALL: [Op; 36] = [
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
        Op::Open,
        Op::ReadChunk,
        Op::WriteChunk,
        Op::Close,
        Op::Stat,
        Op::Scan,
        Op::Space,
        Op::Link,
        Op::WriteSecret,
        Op::ReadSecret,
    ];

    /// How a diagnostic names the operation.
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
            Op::Open => "`fs.open`",
            Op::ReadChunk => "`fs.read_chunk`",
            Op::WriteChunk => "`fs.write_chunk`",
            Op::Close => "`fs.close`",
            Op::Stat => "`fs.stat`",
            Op::Scan => "`fs.scan`",
            Op::Space => "`fs.space`",
            Op::Link => "`fs.link`",
            Op::WriteSecret => "`fs.write_secret`",
            Op::ReadSecret => "`fs.read_secret`",
        }
    }

    /// Whether the operation follows a link that is the last name of its path. `open` keeps one
    /// when it is asked to create, which `run` knows.
    fn last(self) -> Last {
        match self {
            Op::Kind
            | Op::Exists
            | Op::Remove
            | Op::Rename
            | Op::Lock
            | Op::Unlock
            | Op::RemoveTree
            | Op::Symlink
            | Op::ReadLink
            | Op::Stat
            | Op::Link => Last::Kept,
            _ => Last::Followed,
        }
    }

    /// What the operation answers of a path that leads nowhere: what it answers where nothing is.
    fn nothing(self) -> Vec<u8> {
        match self {
            Op::Kind | Op::Resolved => vec![Kind::Missing.number()],
            Op::Open | Op::Link => tried(Err(Refusal::NotFound)),
            // Absent, or not done.
            _ => vec![0],
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

/// How `fs.open` opens a file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Opening {
    Read,
    Write,
    Append,
    Create,
}

impl Opening {
    /// The opening `number` numbers: `ToRead` 0, `ToWrite` 1, `ToAppend` 2 and `ToCreate` 3.
    pub fn of(number: i64) -> Option<Opening> {
        Some(match number {
            0 => Opening::Read,
            1 => Opening::Write,
            2 => Opening::Append,
            3 => Opening::Create,
            _ => return None,
        })
    }
}

/// What an operation is handed beside its path.
pub enum Second {
    None,
    Opening(Opening),
    Body(Vec<u8>),
    /// A credential in its frame, this job's own copy, wiped when the job is done with it.
    Sealed(Zeroizing<Vec<u8>>),
    /// What a credential is read back from between, and the run that keeps it once it is read.
    Frame {
        before: Vec<u8>,
        framing: Framing,
        after: Vec<u8>,
        keeper: Weak<Run>,
    },
    Path(String),
    Target(String),
    Name(String),
    Mode(u32),
    Millis(i64),
    Range {
        offset: i64,
        len: i64,
    },
    Deep(bool),
}

/// What is asked of an open file.
pub enum Chunk {
    Read(u64),
    Write(Vec<u8>),
    Close,
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
    /// The lock files this run took and has not released; a lock it did not take it cannot release.
    held: Arc<Mutex<BTreeSet<PathBuf>>>,
    descriptors: Arc<Mutex<Descriptors>>,
}

impl FsHost {
    pub fn new(roots: Roots) -> FsHost {
        FsHost {
            roots,
            held: Arc::new(Mutex::new(BTreeSet::new())),
            descriptors: Arc::new(Mutex::new(Descriptors::default())),
        }
    }

    pub fn roots(&self) -> &Roots {
        &self.roots
    }

    fn root(&self, op: Op, label: &str, span: Span) -> Result<PathBuf, Diagnostic> {
        self.roots
            .get(&Resource::Named(Symbol::new(label)))
            .map(Path::to_path_buf)
            .ok_or_else(|| unbound(op, label, span))
    }

    /// The job of `op` on `path` under the root `label` names, handed `second` beside it.
    ///
    /// # Errors
    /// No root is bound to `label`, or a range asks for more than one read answers.
    pub fn asked(
        &self,
        op: Op,
        label: &str,
        path: String,
        second: Second,
        span: Span,
    ) -> Result<Refusing, Diagnostic> {
        let root = self.root(op, label, span)?;
        if let Second::Range { len, .. } = second
            && len as u64 > MAX_READ_BYTES
        {
            return Err(too_large(len as u64, &path, span));
        }
        let held = Arc::clone(&self.held);
        let descriptors = Arc::clone(&self.descriptors);
        Ok(Box::new(move || {
            run(op, &root, &path, second, &held, &descriptors, span)
        }))
    }

    /// The job of `chunk` asked of the file `descriptor` names under the root `label` names: a
    /// read's bytes found, else whether it was done. A descriptor another root opened is not open
    /// under this one, and neither is one opened the other way.
    ///
    /// # Errors
    /// No root is bound to `label`, or a read asks for more than one read answers.
    pub fn on_file(
        &self,
        label: &str,
        descriptor: i64,
        chunk: Chunk,
        span: Span,
    ) -> Result<Refusing, Diagnostic> {
        let op = match chunk {
            Chunk::Read(_) => Op::ReadChunk,
            Chunk::Write(_) => Op::WriteChunk,
            Chunk::Close => Op::Close,
        };
        let root = self.root(op, label, span)?;
        if let Chunk::Read(max) = chunk
            && max > MAX_READ_BYTES
        {
            return Err(chunk_too_large(max, span));
        }
        let descriptors = Arc::clone(&self.descriptors);
        Ok(Box::new(move || {
            Ok(on_open_file(&descriptors, &root, descriptor, chunk))
        }))
    }
}

fn on_open_file(descriptors: &Mutex<Descriptors>, root: &Path, id: i64, work: Chunk) -> Vec<u8> {
    let mut held = descriptors
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let open = held.open.get_mut(&id).filter(|open| open.root == root);
    match work {
        Chunk::Read(max) => found(
            open.filter(|open| open.how == Opening::Read)
                .and_then(|open| {
                    let mut out = Vec::new();
                    (&open.file).take(max).read_to_end(&mut out).ok()?;
                    Some(out)
                }),
        ),
        Chunk::Write(body) => done(
            open.filter(|open| open.how != Opening::Read)
                .is_some_and(|open| open.file.write_all(&body).is_ok()),
        ),
        Chunk::Close => {
            let there = open.is_some();
            if there {
                held.open.remove(&id);
            }
            done(there)
        }
    }
}

/// The file at `target` opened as `how` asks, under the next descriptor, or why not.
fn open_file(
    descriptors: &Mutex<Descriptors>,
    root: &Path,
    target: &Path,
    how: Opening,
) -> Result<i64, Refusal> {
    use std::os::unix::fs::OpenOptionsExt;
    // A directory opens for reading, and only its first read says it is one.
    if how != Opening::Create && std::fs::symlink_metadata(target).is_ok_and(|meta| !meta.is_file())
    {
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
    let file = unfollowed(&mut options, target).map_err(|e| refusal(&e))?;
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

/// Whether it was done, as one byte.
fn done(done: bool) -> Vec<u8> {
    vec![u8::from(done)]
}

fn run(
    op: Op,
    root: &Path,
    path: &str,
    second: Second,
    held: &Mutex<BTreeSet<PathBuf>>,
    descriptors: &Mutex<Descriptors>,
    span: Span,
) -> Result<Vec<u8>, Failure> {
    let last = match second {
        Second::Opening(Opening::Create) => Last::Kept,
        _ => op.last(),
    };
    let Some(target) = walked(root, path, last, span).map_err(Failure::Refused)? else {
        return Ok(op.nothing());
    };
    // A second path of the root, where it leads; what the operation answers where it leads nowhere.
    let other = |to: &str, last: Last| -> Result<Option<PathBuf>, Failure> {
        walked(root, to, last, span).map_err(Failure::Refused)
    };
    let unhanded = |what: &str| {
        Err(Failure::Said(format!(
            "{} reached the pool {what}",
            op.what()
        )))
    };
    Ok(match (op, second) {
        (Op::Open, Second::Opening(how)) => {
            tried(open_file(descriptors, root, &target, how).map(|id| int(id).to_vec()))
        }
        (Op::ReadFile, _) => match std::fs::symlink_metadata(&target) {
            Ok(meta) if meta.is_file() && meta.len() > MAX_READ_BYTES => {
                return Err(Failure::Refused(too_large(meta.len(), path, span)));
            }
            Ok(meta) if meta.is_file() => {
                let mut bytes = Vec::new();
                let read = unfollowed(OpenOptions::new().read(true), &target)
                    .and_then(|mut file| file.read_to_end(&mut bytes));
                found(read.ok().map(|_| bytes))
            }
            _ => found(None::<Vec<u8>>),
        },
        (Op::ListDir, _) => found(std::fs::read_dir(&target).ok().map(|entries| {
            let mut names: Vec<String> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            // Read order is a fact about the filesystem, not the directory's contents.
            names.sort();
            let names: Vec<&[u8]> = names.iter().map(|n| n.as_bytes()).collect();
            parts(&names)
        })),
        // `symlink_metadata` does not follow, so a symlink is reported as one rather than as its target.
        (Op::Kind, _) => vec![Kind::of(std::fs::symlink_metadata(&target).ok().as_ref()).number()],
        // The walk followed every link to here, so what is at the path is what it resolves to.
        (Op::Resolved, _) => vec![
            match Kind::of(std::fs::symlink_metadata(&target).ok().as_ref()) {
                Kind::Symlink => Kind::Missing,
                kind => kind,
            }
            .number(),
        ],
        (Op::Exists, _) => done(std::fs::symlink_metadata(&target).is_ok()),
        (Op::FileSize, _) => found(
            std::fs::symlink_metadata(&target)
                .ok()
                .filter(|m| m.is_file())
                .and_then(|m| i64::try_from(m.len()).ok())
                .map(int),
        ),
        (Op::ModifiedMs, _) => found(
            std::fs::symlink_metadata(&target)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .and_then(|d| i64::try_from(d.as_millis()).ok())
                .map(int),
        ),
        (Op::WriteFile, Second::Body(body)) => done(
            unfollowed(
                OpenOptions::new().write(true).create(true).truncate(true),
                &target,
            )
            .and_then(|mut file| file.write_all(&body))
            .is_ok(),
        ),
        (Op::WriteSecret, Second::Sealed(body)) => done(write_private(&target, &body)),
        (
            Op::ReadSecret,
            Second::Frame {
                before,
                framing,
                after,
                keeper,
            },
        ) => match std::fs::symlink_metadata(&target) {
            Ok(meta) if meta.is_file() && meta.len() > MAX_READ_BYTES => {
                return Err(Failure::Refused(too_large(meta.len(), path, span)));
            }
            Ok(meta) if meta.is_file() => {
                let read = read_private(&target, meta.len())
                    .and_then(|held| framing.unframed(&held, &before, &after));
                found(read.and_then(|secret| Some(int(keeper.upgrade()?.keep_secret(secret)))))
            }
            _ => found(None::<Vec<u8>>),
        },
        // Idempotent, so a cache writer need not check first and race with itself.
        (Op::CreateDir, _) => done(std::fs::create_dir_all(&target).is_ok()),
        // One file, or one empty directory.
        (Op::Remove, _) => done(match std::fs::symlink_metadata(&target) {
            Err(_) => false,
            Ok(meta) if meta.is_dir() => std::fs::remove_dir(&target).is_ok(),
            Ok(_) => std::fs::remove_file(&target).is_ok(),
        }),
        // Both paths are under one label, which makes this the atomic cache write.
        (Op::Rename, Second::Path(to)) => match other(&to, Last::Kept)? {
            Some(destination) => done(std::fs::rename(&target, &destination).is_ok()),
            None => op.nothing(),
        },
        (Op::ReadAt, Second::Range { offset, len }) => found(read_range(&target, offset, len)),
        (Op::Append, Second::Body(body)) => found(append_to(&target, &body).map(int)),
        (Op::Sync, _) => done(sync_path(&target)),
        (Op::Lock, _) => done(take_lock(&target, held)),
        (Op::Unlock, _) => done(drop_lock(&target, held)),
        (Op::Copy, Second::Path(to)) => match other(&to, Last::Followed)? {
            Some(destination) => done(copy_file(&target, &destination)),
            None => op.nothing(),
        },
        (Op::RemoveTree, _) => done(names_below_root(path) && remove_tree(&target)),
        (Op::TempDir, Second::Name(prefix)) => found(temp_dir(path, &target, &prefix)),
        (Op::Canonical, _) => found(
            std::fs::canonicalize(&target)
                .ok()
                .filter(|real| real.starts_with(root))
                .and_then(|real| real.to_str().map(str::to_string)),
        ),
        (Op::Mode, _) => found(mode_of(&target).map(|bits| int(i64::from(bits)))),
        (Op::SetMode, Second::Mode(bits)) => done(set_mode(&target, bits)),
        (Op::Symlink, Second::Target(to)) => {
            link_stays(root, &target, &to, span).map_err(Failure::Refused)?;
            done(std::os::unix::fs::symlink(&to, &target).is_ok())
        }
        (Op::ReadLink, _) => found(
            std::fs::read_link(&target)
                .ok()
                .and_then(|to| to.to_str().map(str::to_string)),
        ),
        (Op::Walk, _) => match walk(path, &target) {
            Ok(entries) => found(entries.map(|entries| {
                let entries: Vec<Vec<u8>> = entries
                    .iter()
                    .map(|(path, kind)| parts(&[path.as_bytes(), &[kind.number()][..]]))
                    .collect();
                let entries: Vec<&[u8]> = entries.iter().map(Vec::as_slice).collect();
                parts(&entries)
            })),
            Err(bytes) => return Err(Failure::Refused(walk_too_large(bytes, path, span))),
        },
        (Op::SetModified, Second::Millis(ms)) => done(set_modified(&target, ms)),
        (Op::Stat, _) => found(stat_of(&target).map(Stat::flat)),
        (Op::Scan, Second::Deep(deep)) => match scan(path, &target, deep) {
            Ok(entries) => found(entries.map(|entries| {
                let entries: Vec<Vec<u8>> = entries
                    .into_iter()
                    .map(|(path, seen)| parts(&[path.as_bytes(), &seen.flat()[..]]))
                    .collect();
                let entries: Vec<&[u8]> = entries.iter().map(Vec::as_slice).collect();
                parts(&entries)
            })),
            Err(bytes) => return Err(Failure::Refused(walk_too_large(bytes, path, span))),
        },
        (Op::Space, _) => found(space_of(&target).map(Space::flat)),
        (Op::Link, Second::Path(to)) => match other(&to, Last::Kept)? {
            Some(existing) => tried(hard_link(&existing, &target).map(|()| Vec::new())),
            None => op.nothing(),
        },
        (Op::ReadChunk | Op::WriteChunk | Op::Close, _) => {
            return unhanded("as an operation on a path");
        }
        _ => return unhanded("without what it is handed beside its path"),
    })
}

/// What a path names, read without following a link.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    File,
    Dir,
    Symlink,
    Missing,
}

impl Kind {
    fn of(meta: Option<&std::fs::Metadata>) -> Kind {
        match meta {
            Some(meta) if meta.is_symlink() => Kind::Symlink,
            Some(meta) if meta.is_dir() => Kind::Dir,
            Some(meta) if meta.is_file() => Kind::File,
            _ => Kind::Missing,
        }
    }

    /// `File` 0, `Dir` 1, `Symlink` 2 and `Missing` 3.
    pub fn number(self) -> u8 {
        match self {
            Kind::File => 0,
            Kind::Dir => 1,
            Kind::Symlink => 2,
            Kind::Missing => 3,
        }
    }
}

/// Why the file system would not do what an operation asked.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Refusal {
    NotFound,
    Denied,
    NotAFile,
    NotADirectory,
    Exists,
    NoSpace,
    /// The host's error number and its words for it.
    Other(i64, String),
}

impl Refusal {
    /// One byte, `NotFound` 0 to `NoSpace` 5 as declared, or 6 for `Other` and then its number
    /// and its words as parts.
    pub fn flat(self) -> Vec<u8> {
        match self {
            Refusal::NotFound => vec![0],
            Refusal::Denied => vec![1],
            Refusal::NotAFile => vec![2],
            Refusal::NotADirectory => vec![3],
            Refusal::Exists => vec![4],
            Refusal::NoSpace => vec![5],
            Refusal::Other(code, text) => {
                [&[6u8][..], &parts(&[&int(code)[..], text.as_bytes()])].concat()
            }
        }
    }
}

/// What an operation the file system may refuse answers: byte 1 and what it made, or byte 0 and
/// why it made nothing.
fn tried(made: Result<Vec<u8>, Refusal>) -> Vec<u8> {
    match made {
        Ok(made) => [&[1u8][..], &made].concat(),
        Err(why) => [&[0u8][..], &why.flat()].concat(),
    }
}

/// What one look at a path reads of it, a link as itself.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Stat {
    pub kind: Kind,
    pub size: i64,
    /// Nanoseconds since the Unix epoch.
    pub modified: i64,
    pub mode: u32,
    pub links: i64,
    pub device: i64,
    pub id: i64,
}

impl Stat {
    /// As parts: the kind's byte, then its size, time, mode, links, device and id each eight bytes.
    pub fn flat(self) -> Vec<u8> {
        parts(&[
            &[self.kind.number()][..],
            &int(self.size)[..],
            &int(self.modified)[..],
            &int(i64::from(self.mode))[..],
            &int(self.links)[..],
            &int(self.device)[..],
            &int(self.id)[..],
        ])
    }
}

/// The room in the file system that holds a path.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Space {
    pub total: i64,
    pub free: i64,
    pub available: i64,
    pub inodes: i64,
    pub inodes_free: i64,
}

impl Space {
    /// As parts, each eight bytes in the order the fields are declared.
    pub fn flat(self) -> Vec<u8> {
        parts(&[
            &int(self.total)[..],
            &int(self.free)[..],
            &int(self.available)[..],
            &int(self.inodes)[..],
            &int(self.inodes_free)[..],
        ])
    }
}

/// Why the file system refused.
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
fn stat_from(meta: &std::fs::Metadata) -> Option<Stat> {
    use std::os::unix::fs::MetadataExt;
    let kind = match Kind::of(Some(meta)) {
        Kind::Missing => return None,
        kind => kind,
    };
    let file = kind == Kind::File;
    Some(Stat {
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
        mode: if kind == Kind::Symlink {
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
fn stat_of(target: &Path) -> Option<Stat> {
    stat_from(&std::fs::symlink_metadata(target).ok()?)
}

/// The room in the file system that holds `target`, in bytes and in inodes.
// Each field's width is the platform's, so a cast one target needs is a no-op on another.
#[allow(clippy::unnecessary_cast)]
fn space_of(target: &Path) -> Option<Space> {
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
    Some(Space {
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
    std::fs::symlink_metadata(target)
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

/// Read from the directory the link is made in, which is where `link` stands once every link
/// above it is followed, a target must stay under the root: one that names another root or climbs
/// out of this one would hand whoever follows the link a path outside it.
fn link_stays(root: &Path, link: &Path, target: &str, span: Span) -> Result<(), Diagnostic> {
    let to = Path::new(target);
    if to.is_absolute() {
        return Err(escapes(
            root,
            target,
            "a link's target names its own root",
            span,
        ));
    }
    let mut depth = link
        .parent()
        .and_then(|dir| dir.strip_prefix(root).ok())
        .map_or(0, |below| below.components().count());
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

/// Every entry under `dir`, depth first and each directory's names in byte order, a symlink listed
/// and never followed. `Err` carries the bytes of paths gathered once they pass what one answer
/// holds.
fn walk(dir: &str, target: &Path) -> Result<Option<Vec<(String, Kind)>>, u64> {
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
    out: &mut Vec<(String, Kind)>,
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
        let kind = match Kind::of(std::fs::symlink_metadata(&full).ok().as_ref()) {
            Kind::Missing => continue,
            kind => kind,
        };
        out.push((path.clone(), kind));
        if kind == Kind::Dir {
            walk_into(&path, &full, out, bytes)?;
        }
    }
    Ok(())
}

/// Every entry under `dir` with what one look reads of it, in a walk's order: its own entries
/// alone unless `deep`. `Err` as a walk's is.
fn scan(dir: &str, target: &Path, deep: bool) -> Result<Option<Vec<(String, Stat)>>, u64> {
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
    out: &mut Vec<(String, Stat)>,
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
        let below = deep && found.kind == Kind::Dir;
        out.push((path.clone(), found));
        if below {
            scan_into(&path, &full, deep, out, bytes)?;
        }
    }
    Ok(())
}

/// A directory is opened as a file to stamp it, which `futimens` allows its owner.
fn set_modified(target: &Path, ms: i64) -> bool {
    let Ok(ms) = u64::try_from(ms) else {
        return false;
    };
    unfollowed(OpenOptions::new().read(true), target)
        .and_then(|file| file.set_modified(UNIX_EPOCH + Duration::from_millis(ms)))
        .is_ok()
}

/// Costs the size of what is new rather than the size of the file, which is the point of it. The
/// answer is the offset the bytes landed at: `O_APPEND` puts them at the end atomically, and the
/// descriptor's position afterwards is the end of what *this* write produced, so a second appender
/// racing this one moves neither the bytes nor the answer.
fn append_to(target: &Path, body: &[u8]) -> Option<i64> {
    let mut file = unfollowed(OpenOptions::new().append(true).create(true), target).ok()?;
    // A write of nothing writes nothing, leaving the descriptor at 0 rather than at the end.
    if body.is_empty() {
        return i64::try_from(file.metadata().ok()?.len()).ok();
    }
    file.write_all(body).ok()?;
    let end = file.stream_position().ok()?;
    i64::try_from(end.checked_sub(body.len() as u64)?).ok()
}

/// Writes `body` as the whole of a file its owner alone may read or write (`rw-------`): a new file
/// is made so, and one already there is made so before it is emptied and written, so no byte of
/// the body is ever readable by anyone else.
fn write_private(target: &Path, body: &[u8]) -> bool {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let Ok(mut file) = unfollowed(
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600),
        target,
    ) else {
        return false;
    };
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .is_ok()
        && file.set_len(0).is_ok()
        && file.write_all(body).is_ok()
}

/// A file's bytes read into memory that is wiped when they are dropped, in one buffer of the size
/// the file had, so no copy is left behind as a buffer grows; `None` for a file that changed size
/// under the read or could not be read.
fn read_private(target: &Path, size: u64) -> Option<Zeroizing<Vec<u8>>> {
    let size = usize::try_from(size).ok()?;
    let mut bytes = Zeroizing::new(vec![0u8; size]);
    let mut file = unfollowed(OpenOptions::new().read(true), target).ok()?;
    file.read_exact(&mut bytes).ok()?;
    let mut past = [0u8; 1];
    (file.read(&mut past).ok()? == 0).then_some(bytes)
}

/// What is there, which may be less than was asked for: a file can end before the range does, and
/// a reader that recorded an offset cannot be told its cache is short by a diagnostic it cannot
/// catch. `None` keeps the meaning it has for `read_file`: this path names nothing to read.
fn read_range(target: &Path, offset: i64, len: i64) -> Option<Vec<u8>> {
    let (offset, len) = (u64::try_from(offset).ok()?, u64::try_from(len).ok()?);
    let mut file = unfollowed(OpenOptions::new().read(true), target).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut out = Vec::new();
    file.take(len).read_to_end(&mut out).ok()?;
    Some(out)
}

/// A file's own bytes, or — for a directory — the names in it, which is what makes a rename
/// durable. Both are what an append-only store fsyncs before an index is allowed to name them.
fn sync_path(target: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(target) else {
        return false;
    };
    // A directory cannot be opened for writing, and its entries are what fsync flushes either way.
    let mut how = OpenOptions::new();
    if meta.is_dir() {
        how.read(true);
    } else {
        how.write(true);
    }
    unfollowed(&mut how, target)
        .and_then(|file| file.sync_all())
        .is_ok()
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
    std::fs::symlink_metadata(path)
        .and_then(|m| m.modified())
        .is_ok_and(|m| m.elapsed().is_ok_and(|elapsed| elapsed >= age))
}

/// The guarded set has no invariant a panicking job can break, so recovering is correct.
fn lock(held: &Mutex<BTreeSet<PathBuf>>) -> MutexGuard<'_, BTreeSet<PathBuf>> {
    held.lock().unwrap_or_else(|e| e.into_inner())
}

/// As many links as one path may pass through before it is a loop, where `ELOOP` stops.
const LINK_HOPS: usize = 40;

/// Whether an operation acts on what a link that is the last name of its path leads to, or on the
/// link.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Last {
    Followed,
    Kept,
}

/// Where `path` leads under `root` with every link along it followed, the last included: the path
/// to hand a system call, which had no link in it when this looked. `E0452` where it leads out of
/// the root, whether or not anything is where it leads, and `None` where it leads nowhere.
pub fn confine(root: &Path, path: &str, span: Span) -> Result<Option<PathBuf>, Diagnostic> {
    walked(root, path, Last::Followed, span)
}

/// The walk behind [`confine`]: each name looked at in turn and each link followed here rather
/// than by the system, so that a link to where nothing is has somewhere it leads. Under
/// `Last::Kept` the last name is not looked at, and a link there is the answer's last name.
/// `None` for more links than a path may pass through, and for a link that steps back out of a
/// name that is not there, which the system resolves to nothing.
fn walked(root: &Path, path: &str, last: Last, span: Span) -> Result<Option<PathBuf>, Diagnostic> {
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

    let mut at = root.to_path_buf();
    // The next name is the last one; a link's target goes on above what came after the link.
    let mut ahead: Vec<OsString> = steps(relative).rev().collect();
    let mut hops = 0;
    // Nothing is below a name that is not there, so from one on the walk only spells the path.
    let mut missing = false;
    while let Some(name) = ahead.pop() {
        if name == ".." {
            if missing {
                return Ok(None);
            }
            at.pop();
            continue;
        }
        at.push(&name);
        if missing || (last == Last::Kept && ahead.is_empty()) {
            continue;
        }
        match std::fs::symlink_metadata(&at) {
            Ok(meta) if meta.is_symlink() => {
                hops += 1;
                let target = match std::fs::read_link(&at) {
                    Ok(target) if hops <= LINK_HOPS && !target.as_os_str().is_empty() => target,
                    _ => return Ok(None),
                };
                if target.is_absolute() {
                    at = PathBuf::from("/");
                } else {
                    at.pop();
                }
                ahead.extend(steps(&target).rev());
            }
            Ok(_) => {}
            Err(_) => missing = true,
        }
    }
    if at.starts_with(root) {
        Ok(Some(at))
    } else {
        Err(escapes(
            root,
            path,
            &format!("it leads to `{}`", at.display()),
            span,
        ))
    }
}

/// A path's names in order, a step back to the directory above as `..`.
fn steps(path: &Path) -> impl DoubleEndedIterator<Item = OsString> + '_ {
    path.components().filter_map(|c| match c {
        Component::Normal(name) => Some(name.to_os_string()),
        Component::ParentDir => Some(OsString::from("..")),
        _ => None,
    })
}

/// The file at `target` opened as `options` ask, and refused where its name is a link: the path a
/// walk answered had none there, so one there now was put there since.
fn unfollowed(options: &mut OpenOptions, target: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    options.custom_flags(libc::O_NOFOLLOW).open(target)
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
pub fn unbound(op: Op, label: &str, span: Span) -> Diagnostic {
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
