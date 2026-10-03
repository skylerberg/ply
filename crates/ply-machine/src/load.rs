//! A program as a machine holds it: the front end's checked answer over the sources it names.

use crate::driver::FrontEnd;
use ply_eval::{
    Analysis, CheckOutput, DefInfo, Diagnostic, HashOutput, ModuleInfo, ModuleName, SourceId,
    SourceMap, Symbol, TestInfo, codes,
};
use std::path::{Component, Path, PathBuf};

/// A file this load read.
#[derive(Clone, Debug)]
pub struct Found {
    pub path: PathBuf,
}

#[derive(Debug)]
pub struct Loaded {
    /// What module names are derived relative to, and where the cache lives.
    pub root: PathBuf,
    /// One entry per module, sorted.
    pub files: Vec<Found>,
    pub sources: SourceMap,
    pub front: std::sync::Arc<Analysis>,
    /// [`Analysis::check`].
    pub check: CheckOutput,
    /// [`Analysis::hashes`].
    pub hashes: HashOutput,
    pub frontend: FrontEnd,
}

/// Carries the [`SourceMap`]: a parse error is useless without the text its spans point into.
#[derive(Debug)]
pub struct LoadError {
    pub sources: SourceMap,
    pub diagnostics: Vec<Diagnostic>,
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

    pub fn file_names(&self) -> Vec<String> {
        self.files
            .iter()
            .map(|f| f.path.display().to_string())
            .collect()
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

    /// The one `main` this program declares. Which entry `ply run` takes, and what a program
    /// with none or two of them is told, is `crates/ply-cli/ply/run.ply`'s; this is for the `ply`
    /// program itself, which declares exactly one.
    pub fn sole_entry_point(&self) -> Result<&DefInfo, Diagnostic> {
        let mut candidates = self.entry_points();
        match candidates.len() {
            1 => Ok(candidates.remove(0)),
            n => Err(Diagnostic::error(
                codes::AMBIGUOUS_ENTRY_POINT,
                format!("{n} definitions are named `main`, and exactly one was wanted"),
            )),
        }
    }

    /// Every root-package definition named `main`. A dependency's `main` is its own business:
    /// only the package being loaded offers an entry point.
    pub fn entry_points(&self) -> Vec<&DefInfo> {
        let main = Symbol::new("main");
        let root = self.root_package();
        self.check
            .defs
            .values()
            .filter(|d| d.simple_name == main && root.contains(&d.module))
            .collect()
    }

    /// Which modules are the package being loaded, rather than a dependency or the shelf: what a
    /// run offers as an entry point and what a test run tests. A project without packages is every
    /// module the toolchain does not ship.
    pub fn root_package(&self) -> RootPackage {
        let packaged = !self.front.packages.is_empty();
        RootPackage {
            modules: packaged.then(|| {
                self.front
                    .ordinals
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| self.front.module_packages.get(*i) == Some(&0))
                    .map(|(_, (module, _))| module.to_string())
                    .collect()
            }),
        }
    }
}

/// The modules of the package being loaded.
pub struct RootPackage {
    /// `None` for a project without packages.
    modules: Option<std::collections::HashSet<String>>,
}

impl RootPackage {
    pub fn contains(&self, module: &ModuleName) -> bool {
        match &self.modules {
            Some(modules) => modules.contains(module.as_str()),
            None => !crate::shelf::is_shipped(module),
        }
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

/// A path with every `.` component dropped, which is how it is recorded, rendered in a span and
/// keyed in the cache: `./m.ply` and `m.ply` are one file, and only one of them is a spelling a
/// reader can compare. A path that is nothing but `.` keeps it — the empty path names no
/// directory, and the root a `--fs` binding resolves before anything runs is `E0454` when it is
/// one. This is the store package's `source_key` rule, on the argument side of the same boundary.
pub fn tidy(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        if component != Component::CurDir {
            out.push(component);
        }
    }
    if out.as_os_str().is_empty() {
        return PathBuf::from(".");
    }
    out
}
