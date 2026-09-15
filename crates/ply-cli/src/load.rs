//! Turning a path into a checked [`Program`].

use crate::driver::FrontEnd;
use ply_core::{CheckOutput, DefInfo, ModuleInfo, TestInfo};
use ply_hash::HashOutput;
use ply_span::{Diagnostic, SourceId, SourceMap, Span, Symbol, codes};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::Resolved;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Loaded {
    /// The project root: what module names are derived relative to, and where the cache lives.
    pub root: PathBuf,
    /// One entry per module, in load order — paths sorted.
    pub files: Vec<PathBuf>,
    pub sources: SourceMap,
    /// Only the modules this run actually parsed.
    pub program: Program,
    pub resolved: Resolved,
    /// The program to *run*, when it differs: `program` plus the modules a selected test needs the
    /// bodies of and which nothing asked to re-derive.
    ///
    /// Separate from `program` rather than replacing it, because `program` is what this run
    /// checked, and every command that reports on what it checked — `prove`'s obligations, the
    /// cost report — must go on seeing that and not the larger set. Only the runner and the
    /// backend it installs use this one, and they must use the same one as each other: a backend
    /// answers only for the program it was built over.
    pub run: Option<(Program, Resolved)>,
    /// Every module, whether it was checked or restored from the cache.
    pub check: CheckOutput,
    pub hashes: HashOutput,
    pub complete: bool,
    pub frontend: FrontEnd,
    /// Whether any module — parsed this run or restored from the cache — declares a `reuse fn`,
    /// so a command knows whether the promise check has anything to check before it parses
    /// everything the check needs.
    pub promised: bool,
}

impl Loaded {
    /// The program the runner and its backend work over: what was checked, plus the modules a
    /// selected test needs the bodies of.
    pub fn to_run(&self) -> (&Program, &Resolved) {
        match &self.run {
            Some((program, resolved)) => (program, resolved),
            None => (&self.program, &self.resolved),
        }
    }
}

/// Carries the [`SourceMap`] even on failure: a parse error is useless without the text its spans
/// point into.
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

/// A module and the file it was read from, which the AST does not record — a
/// [`ply_syntax::ast::Module`] knows its [`SourceId`], not its path.
pub struct ModuleView<'a> {
    pub name: &'a ModuleName,
    pub info: &'a ModuleInfo,
    pub path: &'a Path,
}

impl Loaded {
    pub fn hashes(&self) -> Result<HashOutput, Vec<Diagnostic>> {
        Ok(self.hashes.clone())
    }

    pub fn file_names(&self) -> Vec<String> {
        self.files.iter().map(|f| f.display().to_string()).collect()
    }

    pub fn module_count(&self) -> usize {
        self.check.modules.len()
    }

    /// Whether this module was parsed.
    pub fn has_ast(&self, module: &ModuleName) -> bool {
        self.program.modules.iter().any(|m| &m.name == module)
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

    /// Tests declared by one module, paired with their index in [`CheckOutput::tests`] — the index
    /// everything else is keyed by.
    pub fn tests_of(&self, module: &ModuleName) -> Vec<(usize, &TestInfo)> {
        self.check
            .tests
            .iter()
            .enumerate()
            .filter(|(_, t)| &t.module == module)
            .collect()
    }

    /// Every definition named `main`, whatever module declares it.
    pub fn entry_points(&self) -> Vec<&DefInfo> {
        let main = Symbol::new("main");
        self.check
            .defs
            .values()
            .filter(|d| d.simple_name == main && !ply_std::is_std(&d.module))
            .collect()
    }
}

/// The directory module names are derived relative to, and the directory the caches live under.
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
    /// Relative to the project root, which is what names the module.
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
    // A project rooted at `.` keys its cache under the empty path, which names the cache directory
    // correctly and reads as no directory at all.
    let root = if root.as_os_str().is_empty() {
        Path::new(".")
    } else {
        root
    };
    let mut files = Vec::new();
    collect(root, &mut files)?;
    files.sort();
    Ok(files)
}

/// Hidden directories are excluded, which is also what keeps `.ply-cache` and the VCS metadata out
/// of the program.
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

/// `Path::new(".").join("a.ply")` is `./a.ply`, and that prefix would show up in every span this
/// file ever renders.
pub fn tidy(path: &Path) -> PathBuf {
    path.strip_prefix("./").unwrap_or(path).to_path_buf()
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
