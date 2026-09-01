//! The filesystem, as nine operations over roots the run names.

use crate::pool::{Done, FS_FIRST_TOKEN, Pool};
use ply_core::ty::Resource;
use ply_eval::host::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRegistry, HostRequest, HostResource,
    HostRuntime, Linearity,
};
use ply_eval::{Pending, Value};
use ply_span::{Diagnostic, Span, Symbol, codes};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

/// The effect these operations serve, spelled as `std.fs` declares it.
pub const EFFECT: &str = "fs";

/// The largest file `fs.read_file` will answer with.
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// The nine operations, in the order `std.fs` declares them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    ReadFile,
    ListDir,
    Exists,
    FileSize,
    ModifiedMs,
    WriteFile,
    CreateDir,
    Remove,
    Rename,
}

impl Op {
    pub const ALL: [Op; 9] = [
        Op::ReadFile,
        Op::ListDir,
        Op::Exists,
        Op::FileSize,
        Op::ModifiedMs,
        Op::WriteFile,
        Op::CreateDir,
        Op::Remove,
        Op::Rename,
    ];

    /// The name in `std.fs`.
    pub fn name(self) -> &'static str {
        match self {
            Op::ReadFile => "read_file",
            Op::ListDir => "list_dir",
            Op::Exists => "exists",
            Op::FileSize => "file_size",
            Op::ModifiedMs => "modified_ms",
            Op::WriteFile => "write_file",
            Op::CreateDir => "create_dir",
            Op::Remove => "remove",
            Op::Rename => "rename",
        }
    }

    /// How a diagnostic names it.
    pub fn what(self) -> &'static str {
        match self {
            Op::ReadFile => "`fs.read_file`",
            Op::ListDir => "`fs.list_dir`",
            Op::Exists => "`fs.exists`",
            Op::FileSize => "`fs.file_size`",
            Op::ModifiedMs => "`fs.modified_ms`",
            Op::WriteFile => "`fs.write_file`",
            Op::CreateDir => "`fs.create_dir`",
            Op::Remove => "`fs.remove`",
            Op::Rename => "`fs.rename`",
        }
    }

    fn arity(self) -> usize {
        match self {
            Op::WriteFile | Op::Rename => 2,
            _ => 1,
        }
    }

    /// The thread name a job runs under, so a stack from a wedged run says which operation is
    /// holding the thread.
    fn label(self) -> &'static str {
        match self {
            Op::ReadFile => "fs-read",
            Op::ListDir => "fs-list",
            Op::Exists => "fs-exists",
            Op::FileSize => "fs-size",
            Op::ModifiedMs => "fs-modified",
            Op::WriteFile => "fs-write",
            Op::CreateDir => "fs-mkdir",
            Op::Remove => "fs-remove",
            Op::Rename => "fs-rename",
        }
    }

    fn declaration(self, path: &'static str) -> HostOp {
        HostOp {
            effect: Symbol::new(EFFECT),
            op: Symbol::new(self.name()),
            // Whichever roots the program names.
            resource: HostResource::Any,
            determinism: Determinism::Nondeterministic,
            // At most once.
            linearity: Linearity::AtMostOnce,
            // Every one of them waits on a disk, so every one of them leaves the machine's thread.
            blocking: true,
            // A path is a `String` and a body is `Bytes`, and no expression turns a `Secret` into
            // either.
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
        // A label is a resource label in a Ply program, and those are ordinary identifiers.
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

    /// Every root the run named, resolved.
    pub fn load(specs: &[RootSpec], span: Span) -> Result<Roots, Diagnostic> {
        let mut roots = Roots::new();
        for spec in specs {
            roots.bind(&spec.name, &spec.path, span)?;
        }
        Ok(roots)
    }

    /// Resolve `path` and bind it to `name`.
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

    /// The directory bound to the label an operation named, if any.
    pub fn get(&self, at: &Resource) -> Option<&Path> {
        match at {
            Resource::Named(name) => self.bound.get(name.as_str()).map(PathBuf::as_path),
            // `std.fs` declares every operation `[r]`, so a singleton resource cannot arise from a
            // well-typed program; answering `None` rather than panicking keeps a malformed
            // registration a diagnostic.
            Resource::Singleton => None,
        }
    }

    /// Every root, ascending, for the listing `ply hosts` prints.
    pub fn listing(&self) -> impl Iterator<Item = (&str, &Path)> {
        self.bound
            .iter()
            .map(|(name, path)| (name.as_str(), path.as_path()))
    }

    pub fn is_empty(&self) -> bool {
        self.bound.is_empty()
    }
}

/// The filesystem, and the pool its operations wait on.
pub struct FsHost {
    roots: Roots,
    pool: Pool,
}

impl FsHost {
    pub fn new(roots: Roots) -> FsHost {
        FsHost {
            roots,
            pool: Pool::new(FS_FIRST_TOKEN),
        }
    }

    pub fn roots(&self) -> &Roots {
        &self.roots
    }

    /// Whether this pool minted the token. What a composed runtime routes on.
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

    /// Wait for one operation and answer it.
    pub fn block_on(&self, pending: Pending) -> Result<Value, Diagnostic> {
        self.pool.block_on(pending)
    }

    /// The Rust path `ply hosts` prints, which must identify the implementation rather than the
    /// effect.
    fn path(op: Op) -> &'static str {
        match op {
            Op::ReadFile => "ply_host::fs::read_file",
            Op::ListDir => "ply_host::fs::list_dir",
            Op::Exists => "ply_host::fs::exists",
            Op::FileSize => "ply_host::fs::file_size",
            Op::ModifiedMs => "ply_host::fs::modified_ms",
            Op::WriteFile => "ply_host::fs::write_file",
            Op::CreateDir => "ply_host::fs::create_dir",
            Op::Remove => "ply_host::fs::remove",
            Op::Rename => "ply_host::fs::rename",
        }
    }
}

/// Register every operation of `fs` against `fs`'s implementation.
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
        let pending = self.fs.pool.submit(
            span,
            op.label(),
            op.what(),
            Box::new(move || run(op, &root, &first, second, span)),
        )?;
        Ok(HostAnswer::Pending(pending))
    }
}

/// The second argument of the two operations that take one.
enum Second {
    None,
    Body(Arc<[u8]>),
    Path(String),
}

/// One operation, on a pool thread, with every syscall it makes.
fn run(op: Op, root: &Path, path: &str, second: Second, span: Span) -> Done {
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
                // Ascending, because the order a directory is read in is a fact about the
                // filesystem rather than about the directory, and a program whose output depended
                // on it would answer differently on two machines holding the same bytes.
                names.sort();
                Done::MaybeStrings(Some(names))
            }
        },
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
        // Every missing ancestor, and idempotent, because a cache writer that had to check first
        // would race with itself.
        Op::CreateDir => Done::Bool(std::fs::create_dir_all(&target).is_ok()),
        // One file, or one empty directory.
        Op::Remove => match std::fs::symlink_metadata(&target) {
            Err(_) => Done::Bool(false),
            Ok(meta) if meta.is_dir() => Done::Bool(std::fs::remove_dir(&target).is_ok()),
            Ok(_) => Done::Bool(std::fs::remove_file(&target).is_ok()),
        },
        Op::Rename => match second {
            Second::Path(to) => match confine(root, &to, span) {
                // Both paths are under the one label the operation named, which is what makes this
                // Both paths are under one label, which is what makes this the atomic cache write.
                Err(refusal) => Done::Refused(refusal),
                Ok(destination) => Done::Bool(std::fs::rename(&target, &destination).is_ok()),
            },
            _ => Done::Failed("a rename with no destination reached the pool".into()),
        },
    }
}

/// The path `root` and `path` name together, or `E0452`.
fn confine(root: &Path, path: &str, span: Span) -> Result<PathBuf, Diagnostic> {
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
            // Not there yet, so check the nearest ancestor that is — the one a symlink would
            // have to be on for the write to land outside.
            Err(_) => match existing.parent() {
                Some(parent) if parent.starts_with(root) => existing = parent,
                // Nothing above it exists inside the root either, so there is no link to have been
                // followed and the lexical checks above are the whole answer.
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
fn unbound(op: Op, at: &Resource, span: Span) -> Diagnostic {
    let label = match at {
        Resource::Named(name) => name.as_str().to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> tempfile::TempDir {
        tempfile::tempdir().expect("a temporary directory")
    }

    fn span() -> Span {
        Span::DUMMY
    }

    #[test]
    fn a_path_under_the_root_resolves() {
        let dir = root();
        let real = dir.path().canonicalize().unwrap();
        std::fs::write(real.join("a.ply"), b"x").unwrap();
        assert_eq!(confine(&real, "a.ply", span()).unwrap(), real.join("a.ply"));
    }

    #[test]
    fn an_absolute_path_and_a_parent_component_are_refused() {
        let dir = root();
        let real = dir.path().canonicalize().unwrap();
        for path in ["/etc/passwd", "../secrets", "src/../../secrets"] {
            let refusal = confine(&real, path, span()).expect_err("it should be refused");
            assert_eq!(refusal.code, codes::FS_PATH_ESCAPES_ROOT, "for `{path}`");
        }
    }

    /// The half a lexical check cannot do.
    #[test]
    fn a_symlink_out_of_the_root_is_refused() {
        let dir = root();
        let real = dir.path().canonicalize().unwrap();
        let outside = root();
        let outside_real = outside.path().canonicalize().unwrap();
        std::fs::write(outside_real.join("secrets"), b"s").unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside_real, real.join("link")).unwrap();
        #[cfg(not(unix))]
        return;

        let refusal = confine(&real, "link/secrets", span()).expect_err("it should be refused");
        assert_eq!(refusal.code, codes::FS_PATH_ESCAPES_ROOT);
    }

    /// A write to a path that does not exist yet still traverses the link its parent is, so the
    /// check has to look at the nearest existing ancestor rather than give up when the target is
    /// absent.
    #[test]
    fn a_write_through_a_symlinked_directory_is_refused() {
        let dir = root();
        let real = dir.path().canonicalize().unwrap();
        let outside = root();
        let outside_real = outside.path().canonicalize().unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside_real, real.join("out")).unwrap();
        #[cfg(not(unix))]
        return;

        let refusal =
            confine(&real, "out/artifact.plyx", span()).expect_err("it should be refused");
        assert_eq!(refusal.code, codes::FS_PATH_ESCAPES_ROOT);
    }

    #[test]
    fn a_root_that_is_not_a_directory_does_not_bind() {
        let dir = root();
        let file = dir.path().join("a.ply");
        std::fs::write(&file, b"x").unwrap();
        let mut roots = Roots::new();
        let refused = roots
            .bind("src", &file, span())
            .expect_err("a file is not a root");
        assert_eq!(refused.code, codes::FS_ROOT_INVALID);
        assert!(roots.is_empty());
    }

    #[test]
    fn an_unbound_label_names_the_flag_that_would_bind_it() {
        let d = unbound(Op::ReadFile, &Resource::Named(Symbol::new("src")), span());
        assert_eq!(d.code, codes::FS_ROOT_UNBOUND);
        assert!(
            d.notes.iter().any(|n| n.contains("--fs src=")),
            "the diagnostic should name the flag: {:?}",
            d.notes
        );
    }
}
