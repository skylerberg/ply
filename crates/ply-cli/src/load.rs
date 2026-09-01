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
fn tidy(path: &Path) -> PathBuf {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, rel: &str, text: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, text).unwrap();
    }

    fn names(loaded: &Loaded) -> Vec<String> {
        loaded
            .modules()
            .iter()
            .map(|m| m.name.to_string())
            .collect()
    }

    #[test]
    fn a_directory_becomes_one_module_per_file_named_after_its_path() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "b.ply", "pub fn b() -> Int = 2\n");
        write(dir.path(), "a.ply", "pub fn a() -> Int = 1\n");
        write(dir.path(), "store/orders.ply", "fn c() -> Int = 3\n");
        write(dir.path(), "notes.txt", "ignored");

        let loaded = load(dir.path()).unwrap();
        assert_eq!(names(&loaded), ["a", "b", "store.orders"]);
        assert_eq!(loaded.module_count(), 3);
        assert!(
            loaded
                .check
                .defs
                .contains_key(&Symbol::new("store.orders.c"))
        );
    }

    #[test]
    fn a_name_in_one_file_is_invisible_in_another_until_it_is_imported() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.ply", "pub fn a() -> Int = 1\n");
        write(dir.path(), "b.ply", "fn b() -> Int = a()\n");

        let err = load(dir.path()).unwrap_err();
        assert!(
            err.diagnostics
                .iter()
                .any(|d| d.code == codes::UNKNOWN_NAME),
            "a directory must no longer be concatenated: {:?}",
            err.diagnostics.iter().map(|d| d.code).collect::<Vec<_>>()
        );

        write(dir.path(), "b.ply", "import a\nfn b() -> Int = a::a()\n");
        let loaded = load(dir.path()).unwrap();
        assert!(loaded.check.defs.contains_key(&Symbol::new("b.b")));
    }

    #[test]
    fn a_file_argument_roots_the_project_at_its_directory() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/one.ply", "fn one() -> Int = 1\n");
        write(dir.path(), "src/two.ply", "fn two() -> Int = 2\n");

        let file = dir.path().join("src/one.ply");
        let loaded = load(&file).unwrap();
        assert_eq!(loaded.root, dir.path().join("src"));
        assert_eq!(names(&loaded), ["one"]);
        assert_eq!(loaded.check.defs.len(), 1);
    }

    #[test]
    fn hidden_directories_are_not_part_of_the_program() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "ok.ply", "fn ok() -> Int = 1\n");
        write(
            dir.path(),
            ".ply-cache/stale.ply",
            "this is not even valid ply\n",
        );
        write(dir.path(), ".git/x.ply", "nor is this\n");

        let loaded = load(dir.path()).unwrap();
        assert_eq!(names(&loaded), ["ok"]);
    }

    #[test]
    fn a_path_that_cannot_name_a_module_is_e0111_against_the_file() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "my-notes.ply", "fn f() -> Int = 1\n");

        let err = load(dir.path()).unwrap_err();
        assert_eq!(err.diagnostics.len(), 1);
        assert_eq!(err.diagnostics[0].code, codes::INVALID_MODULE_PATH);
        let span = err.diagnostics[0].primary_span().unwrap();
        assert!(!span.is_dummy(), "E0111 must point at the file it is about");
        assert!(err.sources.get(span.source).is_some());
    }

    #[test]
    fn a_directory_segment_that_is_not_an_identifier_is_also_e0111() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "not-a-module/f.ply", "fn f() -> Int = 1\n");
        let err = load(dir.path()).unwrap_err();
        assert_eq!(err.diagnostics[0].code, codes::INVALID_MODULE_PATH);
        assert!(err.diagnostics[0].message.contains("not-a-module"));
    }

    #[test]
    fn a_missing_path_is_a_diagnostic_rather_than_a_panic() {
        let err = load(Path::new("definitely/not/here.ply")).unwrap_err();
        assert_eq!(err.diagnostics.len(), 1);
        assert_eq!(err.diagnostics[0].code, codes::RUNTIME_ERROR);
        assert!(
            err.diagnostics[0]
                .message
                .contains("definitely/not/here.ply")
        );
    }

    #[test]
    fn an_empty_directory_says_what_to_do_about_it() {
        let dir = tempfile::tempdir().unwrap();
        let err = load(dir.path()).unwrap_err();
        assert!(err.diagnostics[0].message.contains("no `.ply` files"));
        assert!(!err.diagnostics[0].notes.is_empty());
    }

    #[test]
    fn a_syntax_error_still_hands_back_the_sources_its_spans_point_into() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "bad.ply", "fn broken( = 1\n");
        let err = load(dir.path()).unwrap_err();
        assert!(!err.diagnostics.is_empty());
        let span = err.diagnostics[0].primary_span().unwrap();
        assert!(err.sources.get(span.source).is_some());
    }

    #[test]
    fn a_type_error_is_reported_after_a_clean_parse() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "bad.ply", "fn f() -> Int = 1 + true\n");
        let err = load(dir.path()).unwrap_err();
        assert!(
            err.diagnostics
                .iter()
                .any(|d| d.code == codes::TYPE_MISMATCH)
        );
    }

    #[test]
    fn a_module_cycle_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "a.ply",
            "import b\npub fn a() -> Int = b::b()\n",
        );
        write(
            dir.path(),
            "b.ply",
            "import a\npub fn b() -> Int = a::a()\n",
        );
        let err = load(dir.path()).unwrap_err();
        assert!(
            err.diagnostics
                .iter()
                .any(|d| d.code == codes::MODULE_CYCLE)
        );
    }

    #[test]
    fn entry_points_finds_main_in_whatever_module_declares_it() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "lib.ply", "pub fn one() -> Int = 1\n");
        write(
            dir.path(),
            "app.ply",
            "import lib\nfn main() -> Int = lib::one()\n",
        );

        let loaded = load(dir.path()).unwrap();
        let mains = loaded.entry_points();
        assert_eq!(mains.len(), 1);
        assert_eq!(mains[0].name.as_str(), "app.main");
        assert_eq!(mains[0].module.as_str(), "app");
    }

    #[test]
    fn two_modules_may_each_declare_main() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "one.ply", "fn main() -> Int = 1\n");
        write(dir.path(), "two.ply", "fn main() -> Int = 2\n");

        let loaded = load(dir.path()).unwrap();
        let mains: Vec<&str> = loaded
            .entry_points()
            .iter()
            .map(|d| d.name.as_str())
            .collect();
        assert_eq!(mains, ["one.main", "two.main"]);
    }

    #[test]
    fn defs_and_tests_can_be_read_back_per_module() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "a.ply",
            "fn a() -> Int = 1\ntest \"a\" { assert_eq(a(), 1) }\n",
        );
        write(dir.path(), "b.ply", "fn b() -> Int = 2\n");

        let loaded = load(dir.path()).unwrap();
        let a = ModuleName::from_dotted("a");
        assert_eq!(loaded.defs_of(&a).len(), 1);
        assert_eq!(loaded.tests_of(&a).len(), 1);
        assert_eq!(loaded.tests_of(&ModuleName::from_dotted("b")).len(), 0);
    }

    #[test]
    fn a_leading_dot_slash_never_reaches_a_rendered_span() {
        assert_eq!(tidy(Path::new("./src/a.ply")), PathBuf::from("src/a.ply"));
        assert_eq!(tidy(Path::new("src/a.ply")), PathBuf::from("src/a.ply"));
    }
}
