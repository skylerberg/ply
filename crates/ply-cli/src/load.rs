use ply_core::{CheckOutput, check_module};
use ply_hash::{HashOutput, hash_module};
use ply_span::{Diagnostic, SourceMap, Span, codes};
use ply_syntax::ast::Module;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Loaded {
    /// The directory the result cache is rooted at.
    pub root: PathBuf,
    pub files: Vec<PathBuf>,
    pub sources: SourceMap,
    pub module: Module,
    pub check: CheckOutput,
}

/// Carries the [`SourceMap`] even on failure: a parse error is useless without
/// the text its spans point into.
#[derive(Debug)]
pub struct LoadError {
    pub sources: SourceMap,
    pub diagnostics: Vec<Diagnostic>,
}

impl LoadError {
    fn bare(diagnostic: Diagnostic) -> LoadError {
        LoadError { sources: SourceMap::new(), diagnostics: vec![diagnostic] }
    }
}

impl Loaded {
    pub fn hashes(&self) -> Result<HashOutput, Vec<Diagnostic>> {
        hash_module(&self.module, &self.check)
    }

    pub fn file_names(&self) -> Vec<String> {
        self.files.iter().map(|f| f.display().to_string()).collect()
    }
}

pub fn load(path: &Path) -> Result<Loaded, LoadError> {
    let (root, files) = discover(path).map_err(LoadError::bare)?;

    let mut sources = SourceMap::new();
    let mut diagnostics = Vec::new();
    for file in &files {
        match std::fs::read_to_string(file) {
            Ok(text) => {
                sources.add(file, text);
            }
            Err(e) => diagnostics.push(unreadable(file, &e)),
        }
    }
    if !diagnostics.is_empty() {
        return Err(LoadError { sources, diagnostics });
    }

    let parsed = {
        let inputs: Vec<_> = sources.files().iter().map(|f| (f.id, &*f.text)).collect();
        ply_syntax::parse_many(inputs)
    };
    let module = match parsed {
        Ok(module) => module,
        Err(diagnostics) => return Err(LoadError { sources, diagnostics }),
    };

    match check_module(&module) {
        Ok(check) => Ok(Loaded { root, files, sources, module, check }),
        Err(diagnostics) => Err(LoadError { sources, diagnostics }),
    }
}

fn discover(path: &Path) -> Result<(PathBuf, Vec<PathBuf>), Diagnostic> {
    let meta = std::fs::metadata(path).map_err(|e| unreadable(path, &e))?;

    if meta.is_file() {
        let root = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf();
        return Ok((root, vec![tidy(path)]));
    }

    let mut files = Vec::new();
    collect(path, &mut files).map_err(|e| unreadable(path, &e))?;
    files.sort();

    if files.is_empty() {
        return Err(Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("no `.ply` files under `{}`", path.display()),
        )
        .primary(Span::DUMMY, "nothing to compile")
        .note("name a `.ply` file, or a directory that contains one")
        .note("directories whose name starts with `.` are not searched"));
    }

    Ok((path.to_path_buf(), files))
}

/// Hidden directories are excluded, which is also what keeps `.ply-cache` and
/// the VCS metadata out of a module. Directory symlinks are not followed, so a
/// cycle cannot hang the walk.
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

/// `Path::new(".").join("a.ply")` is `./a.ply`, and that prefix would show up in
/// every span this file ever renders.
fn tidy(path: &Path) -> PathBuf {
    path.strip_prefix("./").unwrap_or(path).to_path_buf()
}

fn unreadable(path: &Path, e: &std::io::Error) -> Diagnostic {
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

    #[test]
    fn a_directory_becomes_one_module_sorted_by_path() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "b.ply", "fn b() -> Int = a()\n");
        write(dir.path(), "a.ply", "fn a() -> Int = 1\n");
        write(dir.path(), "nested/c.ply", "fn c() -> Int = b()\n");
        write(dir.path(), "notes.txt", "ignored");

        let (root, files) = discover(dir.path()).unwrap();
        assert_eq!(root, dir.path());
        let names: Vec<_> =
            files.iter().map(|f| f.file_name().unwrap().to_str().unwrap()).collect();
        assert_eq!(names, ["a.ply", "b.ply", "c.ply"]);

        let loaded = load(dir.path()).unwrap();
        // `c` calls `b` calls `a` across three files, so they were checked as one
        // module rather than three.
        assert!(loaded.check.defs.contains_key(&ply_span::Symbol::new("c")));
        assert_eq!(loaded.check.defs.len(), 3);
    }

    #[test]
    fn hidden_directories_are_not_part_of_a_module() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "ok.ply", "fn ok() -> Int = 1\n");
        write(dir.path(), ".ply-cache/stale.ply", "this is not even valid ply\n");
        write(dir.path(), ".git/x.ply", "nor is this\n");

        let (_, files) = discover(dir.path()).unwrap();
        assert_eq!(files.len(), 1);
        assert!(load(dir.path()).is_ok());
    }

    #[test]
    fn a_file_argument_roots_the_cache_at_its_directory() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/one.ply", "fn one() -> Int = 1\n");
        write(dir.path(), "src/two.ply", "fn two() -> Int = 2\n");

        let file = dir.path().join("src/one.ply");
        let (root, files) = discover(&file).unwrap();
        assert_eq!(root, dir.path().join("src"));
        assert_eq!(files, vec![file.clone()]);

        // Only the named file, so `two` is not in scope.
        let loaded = load(&file).unwrap();
        assert_eq!(loaded.check.defs.len(), 1);
    }

    #[test]
    fn a_missing_path_is_a_diagnostic_rather_than_a_panic() {
        let err = load(Path::new("definitely/not/here.ply")).unwrap_err();
        assert_eq!(err.diagnostics.len(), 1);
        assert_eq!(err.diagnostics[0].code, codes::RUNTIME_ERROR);
        assert!(err.diagnostics[0].message.contains("definitely/not/here.ply"));
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
        assert!(err.diagnostics.iter().any(|d| d.code == codes::TYPE_MISMATCH));
    }

    #[test]
    fn a_leading_dot_slash_never_reaches_a_rendered_span() {
        assert_eq!(tidy(Path::new("./src/a.ply")), PathBuf::from("src/a.ply"));
        assert_eq!(tidy(Path::new("src/a.ply")), PathBuf::from("src/a.ply"));
    }
}
