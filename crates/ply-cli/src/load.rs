//! Turning a path into the front end's checked answer.

use crate::driver::FrontEnd;
use ply_span::{Diagnostic, SourceId, SourceMap, Span, Symbol, codes};
use ply_ty::HashOutput;
use ply_ty::ModuleName;
use ply_ty::{CheckOutput, DefInfo, Front, ModuleInfo, TestInfo};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Loaded {
    /// What module names are derived relative to, and where the cache lives.
    pub root: PathBuf,
    /// One entry per module, sorted.
    pub files: Vec<PathBuf>,
    pub sources: SourceMap,
    /// Handed to `ply_codegen::Unit::over_front` so one invocation runs one front end.
    pub front: Front,
    /// [`Front::check`].
    pub check: CheckOutput,
    /// [`Front::hashes`].
    pub hashes: HashOutput,
    pub frontend: FrontEnd,
    /// Whether any module declares a `reuse fn`, so the promise check can be skipped.
    pub promised: bool,
}

/// Carries the [`SourceMap`]: a parse error is useless without the text its spans point into.
#[derive(Debug)]
pub struct LoadError {
    pub sources: SourceMap,
    pub diagnostics: Vec<Diagnostic>,
}

impl LoadError {
    pub(crate) fn bare(diagnostics: Vec<Diagnostic>) -> LoadError {
        LoadError {
            sources: SourceMap::new(),
            diagnostics,
        }
    }
}

/// A module and the file it was read from, which the AST does not record.
pub struct ModuleView<'a> {
    pub name: &'a ModuleName,
    pub info: &'a ModuleInfo,
    pub path: &'a Path,
}

impl Loaded {
    /// Reported as a load's diagnostics are, over this program's sources.
    pub fn refused(&self, diagnostic: Diagnostic) -> LoadError {
        LoadError {
            sources: self.sources.clone(),
            diagnostics: vec![diagnostic],
        }
    }

    /// Every module and its text, in the order the port read them.
    pub fn texts(&self) -> Vec<(String, String)> {
        let mut modules: Vec<&ModuleInfo> = self.check.modules.values().collect();
        modules.sort_by_key(|m| m.source.0);
        modules
            .into_iter()
            .map(|m| {
                let text = self.sources.get(m.source).map_or("", |f| &*f.text);
                (m.name.to_string(), text.to_string())
            })
            .collect()
    }

    pub fn hashes(&self) -> Result<HashOutput, Vec<Diagnostic>> {
        Ok(self.hashes.clone())
    }

    pub fn file_names(&self) -> Vec<String> {
        self.files.iter().map(|f| f.display().to_string()).collect()
    }

    pub fn module_count(&self) -> usize {
        self.check.modules.len()
    }

    pub fn modules(&self) -> Vec<ModuleView<'_>> {
        self.check
            .modules
            .values()
            .map(|info| ModuleView {
                name: &info.name,
                info,
                path: self.path_of(info.source),
            })
            .collect()
    }

    pub fn path_of(&self, source: SourceId) -> &Path {
        self.sources
            .get(source)
            .map(|f| f.path.as_path())
            .unwrap_or(Path::new("<unknown>"))
    }

    pub fn defs_of(&self, module: &ModuleName) -> Vec<&DefInfo> {
        self.check
            .defs
            .values()
            .filter(|d| &d.module == module)
            .collect()
    }

    /// Tests declared by one module, with their index in [`CheckOutput::tests`].
    pub fn tests_of(&self, module: &ModuleName) -> Vec<(usize, &TestInfo)> {
        self.check
            .tests
            .iter()
            .enumerate()
            .filter(|(_, t)| &t.module == module)
            .collect()
    }

    /// Every non-std definition named `main`.
    pub fn entry_points(&self) -> Vec<&DefInfo> {
        let main = Symbol::new("main");
        self.check
            .defs
            .values()
            .filter(|d| d.simple_name == main && !crate::shipped::is_shipped(&d.module))
            .collect()
    }
}

/// The directory module names are relative to and the caches live under.
pub fn project_root(path: &Path) -> PathBuf {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf(),
        _ => tidy(path),
    }
}

/// The from-scratch path: no cache is read and none is written.
pub fn load(path: &Path) -> Result<Loaded, LoadError> {
    crate::driver::load_full(path)
}

pub(crate) struct Discovered {
    pub(crate) path: PathBuf,
    /// Relative to the project root; names the module.
    pub(crate) relative: PathBuf,
}

pub(crate) fn discover(path: &Path) -> Result<(PathBuf, Vec<Discovered>), Vec<Diagnostic>> {
    let meta = std::fs::metadata(path).map_err(|e| vec![unreadable(path, &e)])?;

    if meta.is_file() {
        let root = project_root(path);
        let path = tidy(path);
        let relative = path
            .file_name()
            .map(PathBuf::from)
            .unwrap_or_else(|| path.clone());
        return Ok((root, vec![Discovered { path, relative }]));
    }

    let mut files = Vec::new();
    collect(path, &mut files).map_err(|e| vec![unreadable(path, &e)])?;
    files.sort();

    if files.is_empty() {
        return Err(vec![
            Diagnostic::error(
                codes::RUNTIME_ERROR,
                format!("no `.ply` files under `{}`", path.display()),
            )
            .primary(Span::DUMMY, "nothing to compile")
            .note("name a `.ply` file, or a directory that contains one")
            .note("directories whose name starts with `.` are not searched"),
        ]);
    }

    let root = tidy(path);
    let discovered = files
        .into_iter()
        .map(|path| {
            let relative = path.strip_prefix(&root).unwrap_or(&path).to_path_buf();
            Discovered { path, relative }
        })
        .collect();
    Ok((root, discovered))
}

/// Every `.ply` file under `root`, sorted.
pub(crate) fn ply_files(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect(root, &mut files)?;
    files.sort();
    Ok(files)
}

/// Skips hidden directories, which keeps `.ply-cache` and VCS metadata out.
fn collect(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = tidy(&entry.path());
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            let hidden = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'));
            if !hidden {
                collect(&path, out)?;
            }
        } else if file_type.is_file() && path.extension().is_some_and(|e| e == "ply") {
            out.push(path);
        }
    }
    Ok(())
}

/// Strips `./`, which would otherwise show up in every rendered span. `.` and `./` strip to
/// nothing, and the empty path names no directory: it is the working directory, so it stays `.`.
/// Every caller reads the answer as a directory — the root a `--fs` binding resolves once before
/// anything runs is one of them, and an empty one is `E0454`.
pub fn tidy(path: &Path) -> PathBuf {
    let stripped = path.strip_prefix("./").unwrap_or(path);
    if stripped.as_os_str().is_empty() {
        return PathBuf::from(".");
    }
    stripped.to_path_buf()
}

/// [`ModuleName::from_relative_path`] has no source to point at.
pub(crate) fn anchor(
    mut diagnostic: Diagnostic,
    sources: &SourceMap,
    source: SourceId,
) -> Diagnostic {
    let end = sources
        .get(source)
        .map(|f| f.text.find('\n').unwrap_or(f.text.len()) as u32)
        .unwrap_or(0);
    let span = Span::new(source, 0, end);
    for label in &mut diagnostic.labels {
        if label.span.is_dummy() {
            label.span = span;
        }
    }
    diagnostic
}

pub(crate) fn unreadable(path: &Path, e: &std::io::Error) -> Diagnostic {
    let mut diag = Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("could not read `{}`: {e}", path.display()),
    )
    .primary(Span::DUMMY, "this path could not be loaded");

    if e.kind() == std::io::ErrorKind::NotFound {
        diag = diag.note("pass a `.ply` file or a directory containing one; the default is `.`");
    }
    diag
}
