use crate::jobs::{Failure, FallibleJob};
use crate::native::{found, int, parts, read_result_or_none};
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

pub const MAX_READ_BYTES: u64 = 64 * 1024 * 1024;

pub const LOCK_WAIT: Duration = Duration::from_secs(2);

const LOCK_POLL: Duration = Duration::from_millis(2);

pub const LOCK_STALE_AGE: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    ReadFile,
    ReadAt,
    ListDir,
    Kind,
    ResolvePath,
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
        Op::ResolvePath,
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

    pub fn what(self) -> &'static str {
        match self {
            Op::ReadFile => "`fs.read_file`",
            Op::ReadAt => "`fs.read_at`",
            Op::ListDir => "`fs.list_dir`",
            Op::Kind => "`fs.kind`",
            Op::ResolvePath => "`fs.resolved`",
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
            | Op::Link => Last::NoFollow,
            _ => Last::FollowLinks,
        }
    }

    fn nothing(self) -> Vec<u8> {
        match self {
            Op::Kind | Op::ResolvePath => vec![Kind::Missing.number()],
            Op::Open | Op::Link => tried(Err(FileError::NotFound)),
            _ => vec![0],
        }
    }
}

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

fn malformed(text: &str, reason: &str) -> String {
    format!("`{text}` is not a filesystem root: {reason}; write `--fs NAME=PATH`")
}

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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Opening {
    Read,
    Write,
    Append,
    Create,
}

impl Opening {
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

pub enum Second {
    None,
    Opening(Opening),
    Body(Vec<u8>),
    Sealed(Zeroizing<Vec<u8>>),
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

pub enum OpenFileAction {
    Read(u64),
    Write(Vec<u8>),
    Close,
}

struct OpenFile {
    root: PathBuf,
    file: File,
    how: Opening,
}

#[derive(Default)]
struct Descriptors {
    last: i64,
    open: BTreeMap<i64, OpenFile>,
}

pub struct FsHost {
    roots: Roots,
    locked_paths: Arc<Mutex<BTreeSet<PathBuf>>>,
    descriptors: Arc<Mutex<Descriptors>>,
}

impl FsHost {
    pub fn new(roots: Roots) -> FsHost {
        FsHost {
            roots,
            locked_paths: Arc::new(Mutex::new(BTreeSet::new())),
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

    pub fn run_operation(
        &self,
        op: Op,
        label: &str,
        path: String,
        second: Second,
        span: Span,
    ) -> Result<FallibleJob, Diagnostic> {
        let root = self.root(op, label, span)?;
        if let Second::Range { len, .. } = second
            && len as u64 > MAX_READ_BYTES
        {
            return Err(too_large(len as u64, &path, span));
        }
        let held = Arc::clone(&self.locked_paths);
        let descriptors = Arc::clone(&self.descriptors);
        Ok(Box::new(move || {
            run(op, &root, &path, second, &held, &descriptors, span)
        }))
    }

    pub fn on_file(
        &self,
        label: &str,
        descriptor: i64,
        chunk: OpenFileAction,
        span: Span,
    ) -> Result<FallibleJob, Diagnostic> {
        let op = match chunk {
            OpenFileAction::Read(_) => Op::ReadChunk,
            OpenFileAction::Write(_) => Op::WriteChunk,
            OpenFileAction::Close => Op::Close,
        };
        let root = self.root(op, label, span)?;
        if let OpenFileAction::Read(max) = chunk
            && max > MAX_READ_BYTES
        {
            return Err(chunk_too_large(max, span));
        }
        let descriptors = Arc::clone(&self.descriptors);
        Ok(Box::new(move || {
            Ok(operate_on_open_file(&descriptors, &root, descriptor, chunk))
        }))
    }
}

fn operate_on_open_file(
    descriptors: &Mutex<Descriptors>,
    root: &Path,
    id: i64,
    work: OpenFileAction,
) -> Vec<u8> {
    let mut held = descriptors
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let open = held.open.get_mut(&id).filter(|open| open.root == root);
    match work {
        OpenFileAction::Read(max) => match open.filter(|open| open.how == Opening::Read) {
            Some(open) => read_result_or_none(0, |out| (&open.file).take(max).read_to_end(out)),
            None => found(None::<Vec<u8>>),
        },
        OpenFileAction::Write(body) => done(
            open.filter(|open| open.how != Opening::Read)
                .is_some_and(|open| open.file.write_all(&body).is_ok()),
        ),
        OpenFileAction::Close => {
            let there = open.is_some();
            if there {
                held.open.remove(&id);
            }
            done(there)
        }
    }
}

fn open_file(
    descriptors: &Mutex<Descriptors>,
    root: &Path,
    target: &Path,
    how: Opening,
) -> Result<i64, FileError> {
    use std::os::unix::fs::OpenOptionsExt;
    if how != Opening::Create && std::fs::symlink_metadata(target).is_ok_and(|meta| !meta.is_file())
    {
        return Err(FileError::NotAFile);
    }
    let mut options = OpenOptions::new();
    match how {
        Opening::Read => options.read(true),
        Opening::Write => options.write(true).create(true).truncate(true),
        Opening::Append => options.append(true).create(true),
        Opening::Create => options.write(true).create_new(true).mode(0o600),
    };
    let file = open_without_following_links(&mut options, target).map_err(|e| rejection(&e))?;
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

fn done(done: bool) -> Vec<u8> {
    vec![u8::from(done)]
}

fn run(
    op: Op,
    root: &Path,
    path: &str,
    second: Second,
    locked_paths: &Mutex<BTreeSet<PathBuf>>,
    descriptors: &Mutex<Descriptors>,
    span: Span,
) -> Result<Vec<u8>, Failure> {
    let last = match second {
        Second::Opening(Opening::Create) => Last::NoFollow,
        _ => op.last(),
    };
    let Some(target) = walked(root, path, last, span).map_err(Failure::Rejected)? else {
        return Ok(op.nothing());
    };
    let other = |to: &str, last: Last| -> Result<Option<PathBuf>, Failure> {
        walked(root, to, last, span).map_err(Failure::Rejected)
    };
    let unhanded = |what: &str| {
        Err(Failure::Message(format!(
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
                return Err(Failure::Rejected(too_large(meta.len(), path, span)));
            }
            Ok(meta) if meta.is_file() => read_result_or_none(meta.len() as usize, |out| {
                open_without_following_links(OpenOptions::new().read(true), &target)
                    .and_then(|mut file| file.read_to_end(out))
            }),
            _ => found(None::<Vec<u8>>),
        },
        (Op::ListDir, _) => found(listed(&target).map(|entries| encode_entries(&entries))),
        (Op::Kind, _) => vec![Kind::of(std::fs::symlink_metadata(&target).ok().as_ref()).number()],
        (Op::ResolvePath, _) => vec![
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
            open_without_following_links(
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
                return Err(Failure::Rejected(too_large(meta.len(), path, span)));
            }
            Ok(meta) if meta.is_file() => {
                let read = read_private(&target, meta.len())
                    .and_then(|held| framing.decode_framed(&held, &before, &after));
                found(read.and_then(|secret| Some(int(keeper.upgrade()?.store_secret(secret)))))
            }
            _ => found(None::<Vec<u8>>),
        },
        (Op::CreateDir, _) => done(std::fs::create_dir_all(&target).is_ok()),
        (Op::Remove, _) => done(match std::fs::symlink_metadata(&target) {
            Err(_) => false,
            Ok(meta) if meta.is_dir() => std::fs::remove_dir(&target).is_ok(),
            Ok(_) => std::fs::remove_file(&target).is_ok(),
        }),
        (Op::Rename, Second::Path(to)) => match other(&to, Last::NoFollow)? {
            Some(destination) => done(std::fs::rename(&target, &destination).is_ok()),
            None => op.nothing(),
        },
        (Op::ReadAt, Second::Range { offset, len }) => found(read_range(&target, offset, len)),
        (Op::Append, Second::Body(body)) => found(append_to(&target, &body).map(int)),
        (Op::Sync, _) => done(sync_path(&target)),
        (Op::Lock, _) => done(take_lock(&target, locked_paths)),
        (Op::Unlock, _) => done(drop_lock(&target, locked_paths)),
        (Op::Copy, Second::Path(to)) => match other(&to, Last::FollowLinks)? {
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
            link_stays(root, &target, &to, span).map_err(Failure::Rejected)?;
            done(std::os::unix::fs::symlink(&to, &target).is_ok())
        }
        (Op::ReadLink, _) => found(
            std::fs::read_link(&target)
                .ok()
                .and_then(|to| to.to_str().map(str::to_string)),
        ),
        (Op::Walk, _) => match walk(path, &target) {
            Ok(entries) => found(entries.map(|entries| encode_entries(&entries))),
            Err(bytes) => return Err(Failure::Rejected(walk_too_large(bytes, path, span))),
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
            Err(bytes) => return Err(Failure::Rejected(walk_too_large(bytes, path, span))),
        },
        (Op::Space, _) => found(space_of(&target).map(Space::flat)),
        (Op::Link, Second::Path(to)) => match other(&to, Last::NoFollow)? {
            Some(existing) => tried(hard_link(&existing, &target).map(|()| Vec::new())),
            None => op.nothing(),
        },
        (Op::ReadChunk | Op::WriteChunk | Op::Close, _) => {
            return unhanded("as an operation on a path");
        }
        _ => return unhanded("without what it is handed beside its path"),
    })
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    File,
    Dir,
    Symlink,
    Missing,
}

impl Kind {
    fn of(meta: Option<&std::fs::Metadata>) -> Kind {
        Kind::of_type(meta.map(std::fs::Metadata::file_type))
    }

    fn of_type(file_type: Option<std::fs::FileType>) -> Kind {
        match file_type {
            Some(t) if t.is_symlink() => Kind::Symlink,
            Some(t) if t.is_dir() => Kind::Dir,
            Some(t) if t.is_file() => Kind::File,
            _ => Kind::Missing,
        }
    }

    pub fn number(self) -> u8 {
        match self {
            Kind::File => 0,
            Kind::Dir => 1,
            Kind::Symlink => 2,
            Kind::Missing => 3,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum FileError {
    NotFound,
    Denied,
    NotAFile,
    NotADirectory,
    Exists,
    NoSpace,
    Other(i64, String),
}

impl FileError {
    pub fn flat(self) -> Vec<u8> {
        match self {
            FileError::NotFound => vec![0],
            FileError::Denied => vec![1],
            FileError::NotAFile => vec![2],
            FileError::NotADirectory => vec![3],
            FileError::Exists => vec![4],
            FileError::NoSpace => vec![5],
            FileError::Other(code, text) => {
                [&[6u8][..], &parts(&[&int(code)[..], text.as_bytes()])].concat()
            }
        }
    }
}

fn tried(made: Result<Vec<u8>, FileError>) -> Vec<u8> {
    match made {
        Ok(made) => [&[1u8][..], &made].concat(),
        Err(reason) => [&[0u8][..], &reason.flat()].concat(),
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Stat {
    pub kind: Kind,
    pub size: i64,
    pub modified: i64,
    pub mode: u32,
    pub links: i64,
    pub device: i64,
    pub id: i64,
}

impl Stat {
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Space {
    pub total: i64,
    pub free: i64,
    pub available: i64,
    pub inodes: i64,
    pub inodes_free: i64,
}

impl Space {
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

fn rejection(e: &std::io::Error) -> FileError {
    match e.kind() {
        ErrorKind::NotFound => FileError::NotFound,
        ErrorKind::PermissionDenied => FileError::Denied,
        ErrorKind::AlreadyExists => FileError::Exists,
        ErrorKind::NotADirectory => FileError::NotADirectory,
        ErrorKind::IsADirectory => FileError::NotAFile,
        ErrorKind::StorageFull | ErrorKind::QuotaExceeded => FileError::NoSpace,
        _ => FileError::Other(e.raw_os_error().map_or(0, i64::from), e.to_string()),
    }
}

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
        device: meta.dev() as i64,
        id: meta.ino() as i64,
    })
}

fn stat_of(target: &Path) -> Option<Stat> {
    stat_from(&std::fs::symlink_metadata(target).ok()?)
}

#[allow(clippy::unnecessary_cast)]
fn space_of(target: &Path) -> Option<Space> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(target.as_os_str().as_bytes()).ok()?;
    let mut held = std::mem::MaybeUninit::<libc::statvfs>::uninit();
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

fn hard_link(existing: &Path, to: &Path) -> Result<(), FileError> {
    match std::fs::symlink_metadata(existing) {
        Err(e) => Err(rejection(&e)),
        Ok(meta) if !meta.is_file() => Err(FileError::NotAFile),
        Ok(_) => std::fs::hard_link(existing, to).map_err(|e| rejection(&e)),
    }
}

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

fn names_below_root(path: &str) -> bool {
    Path::new(path)
        .components()
        .any(|c| matches!(c, Component::Normal(_)))
}

fn remove_tree(target: &Path) -> bool {
    match std::fs::symlink_metadata(target) {
        Err(_) => false,
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(target).is_ok(),
        Ok(_) => std::fs::remove_file(target).is_ok(),
    }
}

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
    let Some(entries) = listed(at) else {
        return Ok(());
    };
    for (name, kind) in entries {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        *bytes += path.len() as u64;
        if *bytes > MAX_READ_BYTES {
            return Err(*bytes);
        }
        if kind == Kind::Missing {
            continue;
        }
        out.push((path.clone(), kind));
        if kind == Kind::Dir {
            walk_into(&path, &at.join(&name), out, bytes)?;
        }
    }
    Ok(())
}

pub(crate) fn listed(at: &Path) -> Option<Vec<(String, Kind)>> {
    let mut entries: Vec<(String, Kind)> = std::fs::read_dir(at)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            (name, Kind::of_type(e.file_type().ok()))
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Some(entries)
}

fn encode_entries(entries: &[(String, Kind)]) -> Vec<u8> {
    let entries: Vec<Vec<u8>> = entries
        .iter()
        .map(|(path, kind)| parts(&[path.as_bytes(), &[kind.number()][..]]))
        .collect();
    let entries: Vec<&[u8]> = entries.iter().map(Vec::as_slice).collect();
    parts(&entries)
}

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
    let Some(entries) = listed(at) else {
        return Ok(());
    };
    for (name, _) in entries {
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

fn set_modified(target: &Path, ms: i64) -> bool {
    let Ok(ms) = u64::try_from(ms) else {
        return false;
    };
    open_without_following_links(OpenOptions::new().read(true), target)
        .and_then(|file| file.set_modified(UNIX_EPOCH + Duration::from_millis(ms)))
        .is_ok()
}

fn append_to(target: &Path, body: &[u8]) -> Option<i64> {
    let mut file =
        open_without_following_links(OpenOptions::new().append(true).create(true), target).ok()?;
    if body.is_empty() {
        return i64::try_from(file.metadata().ok()?.len()).ok();
    }
    file.write_all(body).ok()?;
    let end = file.stream_position().ok()?;
    i64::try_from(end.checked_sub(body.len() as u64)?).ok()
}

fn write_private(target: &Path, body: &[u8]) -> bool {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let Ok(mut file) = open_without_following_links(
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

fn read_private(target: &Path, size: u64) -> Option<Zeroizing<Vec<u8>>> {
    let size = usize::try_from(size).ok()?;
    let mut bytes = Zeroizing::new(vec![0u8; size]);
    let mut file = open_without_following_links(OpenOptions::new().read(true), target).ok()?;
    file.read_exact(&mut bytes).ok()?;
    let mut past = [0u8; 1];
    (file.read(&mut past).ok()? == 0).then_some(bytes)
}

fn read_range(target: &Path, offset: i64, len: i64) -> Option<Vec<u8>> {
    let (offset, len) = (u64::try_from(offset).ok()?, u64::try_from(len).ok()?);
    let mut file = open_without_following_links(OpenOptions::new().read(true), target).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut out = Vec::new();
    file.take(len).read_to_end(&mut out).ok()?;
    Some(out)
}

fn sync_path(target: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(target) else {
        return false;
    };
    let mut how = OpenOptions::new();
    if meta.is_dir() {
        how.read(true);
    } else {
        how.write(true);
    }
    open_without_following_links(&mut how, target)
        .and_then(|file| file.sync_all())
        .is_ok()
}

pub fn take_lock(target: &Path, held: &Mutex<BTreeSet<PathBuf>>) -> bool {
    take_lock_within(target, held, LOCK_WAIT)
}

pub fn take_lock_within(target: &Path, held: &Mutex<BTreeSet<PathBuf>>, wait: Duration) -> bool {
    let deadline = Instant::now() + wait;
    loop {
        match OpenOptions::new().write(true).create_new(true).open(target) {
            Ok(_) => {
                lock(held).insert(target.to_path_buf());
                return true;
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
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

fn lock(held: &Mutex<BTreeSet<PathBuf>>) -> MutexGuard<'_, BTreeSet<PathBuf>> {
    held.lock().unwrap_or_else(|e| e.into_inner())
}

const LINK_HOPS: usize = 40;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Last {
    FollowLinks,
    NoFollow,
}

pub fn confine(root: &Path, path: &str, span: Span) -> Result<Option<PathBuf>, Diagnostic> {
    walked(root, path, Last::FollowLinks, span)
}

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
    let mut ahead: Vec<OsString> = steps(relative).rev().collect();
    let mut hops = 0;
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
        if missing || (last == Last::NoFollow && ahead.is_empty()) {
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

fn steps(path: &Path) -> impl DoubleEndedIterator<Item = OsString> + '_ {
    path.components().filter_map(|c| match c {
        Component::Normal(name) => Some(name.to_os_string()),
        Component::ParentDir => Some(OsString::from("..")),
        _ => None,
    })
}

fn open_without_following_links(options: &mut OpenOptions, target: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    options.custom_flags(libc::O_NOFOLLOW).open(target)
}

#[cold]
fn escapes(root: &Path, path: &str, reason: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::FS_PATH_ESCAPES_ROOT,
        format!("`{path}` leaves the root it was given"),
    )
    .primary(span, reason.to_string())
    .note(format!("the root is `{}`", root.display()))
    .note("a resource label names a root and an operation reaches only what is under it; a path that leaves one is rejected before the operation runs")
}

#[cold]
fn too_large(bytes: u64, path: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::FS_FILE_TOO_LARGE,
        format!("a read of `{path}` would return {bytes} bytes"),
    )
    .primary(span, "this is more than one read returns")
    .note(format!(
        "a read returns one whole value, and the bound is {MAX_READ_BYTES} bytes"
    ))
    .note("read it a range at a time with `fs.read_at(path, offset, len)`, which is also what keeps the cost of reading a large file proportional to the part that is wanted")
}

#[cold]
fn walk_too_large(bytes: u64, path: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::FS_FILE_TOO_LARGE,
        format!("a walk of `{path}` would return more than {bytes} bytes of paths"),
    )
    .primary(span, "this is more than one walk returns")
    .note(format!(
        "a walk returns one whole value, and the bound is {MAX_READ_BYTES} bytes"
    ))
    .note("walk the tree a directory at a time with `fs.list_dir`")
}

#[cold]
fn chunk_too_large(max: u64, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::FS_FILE_TOO_LARGE,
        format!("`fs.read_chunk` was asked for {max} bytes"),
    )
    .primary(span, "this is more than one read returns")
    .note(format!(
        "a read returns one whole value, and the bound is {MAX_READ_BYTES} bytes"
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
