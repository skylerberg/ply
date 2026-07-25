//! One pass of the compiler over a directory, with a stopwatch between phases.
//!
//! The compiler's own front end is not reused here: it parses, resolves and
//! checks behind one call, and one number is what this crate exists not to
//! report.

use anyhow::{Context, Result, bail};
use ply_core::CheckOutput;
use ply_hash::HashOutput;
use ply_span::{Diagnostic, SourceMap};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::{Resolved, resolve};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Discover,
    Read,
    Parse,
    Resolve,
    Typecheck,
    Hash,
    CacheOpen,
    Select,
    Execute,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Discover => "discover",
            Phase::Read => "read",
            Phase::Parse => "parse",
            Phase::Resolve => "resolve",
            Phase::Typecheck => "typecheck",
            Phase::Hash => "hash",
            Phase::CacheOpen => "cache open",
            Phase::Select => "select",
            Phase::Execute => "execute",
        }
    }

    pub fn all() -> [Phase; 9] {
        [
            Phase::Discover,
            Phase::Read,
            Phase::Parse,
            Phase::Resolve,
            Phase::Typecheck,
            Phase::Hash,
            Phase::CacheOpen,
            Phase::Select,
            Phase::Execute,
        ]
    }
}

#[derive(Clone, Debug, Default)]
pub struct Timings {
    entries: Vec<(Phase, Duration)>,
}

impl Timings {
    pub fn record(&mut self, phase: Phase, taken: Duration) {
        match self.entries.iter_mut().find(|(p, _)| *p == phase) {
            Some(slot) => slot.1 += taken,
            None => self.entries.push((phase, taken)),
        }
    }

    pub fn get(&self, phase: Phase) -> Duration {
        self.entries
            .iter()
            .find(|(p, _)| *p == phase)
            .map(|(_, d)| *d)
            .unwrap_or_default()
    }

    pub fn total(&self) -> Duration {
        self.entries.iter().map(|(_, d)| *d).sum()
    }

    pub fn entries(&self) -> &[(Phase, Duration)] {
        &self.entries
    }
}

/// Everything a run produced, so a caller can time the front end once and then
/// select and execute against it several times.
#[derive(Debug)]
pub struct Front {
    pub root: PathBuf,
    pub files: Vec<PathBuf>,
    pub sources: SourceMap,
    pub program: Program,
    pub resolved: Resolved,
    pub check: CheckOutput,
    pub hashes: HashOutput,
    pub timings: Timings,
}

pub fn front(root: &Path) -> Result<Front> {
    let mut timings = Timings::default();

    let (files, taken) = timed(|| discover(root))?;
    timings.record(Phase::Discover, taken);
    if files.is_empty() {
        bail!("no `.ply` files under `{}`", root.display());
    }

    let started = Instant::now();
    let mut sources = SourceMap::new();
    let mut names = Vec::with_capacity(files.len());
    let mut ids = Vec::with_capacity(files.len());
    for path in &files {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading `{}`", path.display()))?;
        let relative = path.strip_prefix(root).unwrap_or(path);
        let name = ModuleName::from_relative_path(relative).map_err(|d| report(&[d]))?;
        ids.push(sources.add(path, text));
        names.push(name);
    }
    timings.record(Phase::Read, started.elapsed());

    let started = Instant::now();
    let inputs: Vec<_> = ids
        .iter()
        .zip(&names)
        .map(|(&id, name)| (id, name.clone(), sources.get(id).map_or("", |f| &*f.text)))
        .collect();
    let program = ply_syntax::parse_program(inputs).map_err(|d| report(&d))?;
    timings.record(Phase::Parse, started.elapsed());

    let started = Instant::now();
    let resolved = resolve(&program).map_err(|d| report(&d))?;
    timings.record(Phase::Resolve, started.elapsed());

    let started = Instant::now();
    let check = ply_core::check_program(&program, &resolved).map_err(|d| report(&d))?;
    timings.record(Phase::Typecheck, started.elapsed());

    let started = Instant::now();
    let hashes = ply_hash::hash_program(&program, &resolved, &check).map_err(|d| report(&d))?;
    timings.record(Phase::Hash, started.elapsed());

    Ok(Front {
        root: root.to_path_buf(),
        files,
        sources,
        program,
        resolved,
        check,
        hashes,
        timings,
    })
}

fn timed<T>(f: impl FnOnce() -> Result<T>) -> Result<(T, Duration)> {
    let started = Instant::now();
    let value = f()?;
    Ok((value, started.elapsed()))
}

/// Diagnostics collapse to one error here on purpose: this crate compiles a
/// corpus it generated, so a diagnostic is a defect in the generator and the
/// first one is enough to go and look.
fn report(diagnostics: &[Diagnostic]) -> anyhow::Error {
    let shown: Vec<String> = diagnostics.iter().take(5).map(|d| d.to_string()).collect();
    anyhow::anyhow!(
        "the corpus does not compile ({} diagnostic(s)):\n  {}",
        diagnostics.len(),
        shown.join("\n  ")
    )
}

pub fn discover(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    walk(root, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading `{}`", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            let hidden = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'));
            if !hidden {
                walk(&path, out)?;
            }
        } else if file_type.is_file() && path.extension().is_some_and(|e| e == "ply") {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timings_accumulate_rather_than_replace() {
        let mut t = Timings::default();
        t.record(Phase::Parse, Duration::from_millis(3));
        t.record(Phase::Parse, Duration::from_millis(4));
        t.record(Phase::Hash, Duration::from_millis(1));
        assert_eq!(t.get(Phase::Parse), Duration::from_millis(7));
        assert_eq!(t.total(), Duration::from_millis(8));
        assert_eq!(t.get(Phase::Execute), Duration::ZERO);
    }

    #[test]
    fn discovery_skips_hidden_directories_and_non_ply_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".ply-cache")).unwrap();
        std::fs::create_dir_all(dir.path().join("pkg")).unwrap();
        std::fs::write(dir.path().join("a.ply"), "").unwrap();
        std::fs::write(dir.path().join("pkg/b.ply"), "").unwrap();
        std::fs::write(dir.path().join("corpus.json"), "{}").unwrap();
        std::fs::write(dir.path().join(".ply-cache/c.ply"), "").unwrap();

        let found = discover(dir.path()).unwrap();
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|p| p.extension().unwrap() == "ply"));
    }

    #[test]
    fn an_empty_root_is_an_error_rather_than_an_empty_program() {
        let dir = tempfile::tempdir().unwrap();
        let err = front(dir.path()).unwrap_err();
        assert!(err.to_string().contains("no `.ply` files"));
    }
}
