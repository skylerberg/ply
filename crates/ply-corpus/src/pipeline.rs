//! One pass of the compiler over a directory, with a stopwatch between phases.

use anyhow::{Context, Result, bail};
use ply_hash::HashOutput;
use ply_span::{Diagnostic, SourceMap};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::{Resolved, resolve};
use ply_ty::CheckOutput;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Discover,
    Read,
    Parse,
    Resolve,
    CacheOpen,
    Select,
    /// Building the run's compiled unit, which is nothing at all without a backend. Its own phase
    /// because it is the one an edit does not shrink: the unit is closed over every function the
    /// fragment compiles, whatever changed.
    Compile,
    Execute,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Discover => "discover",
            Phase::Read => "read",
            Phase::Parse => "parse",
            Phase::Resolve => "resolve",
            Phase::Compile => "compile",
            Phase::CacheOpen => "cache open",
            Phase::Select => "select",
            Phase::Execute => "execute",
        }
    }

    pub fn all() -> [Phase; 8] {
        [
            Phase::Discover,
            Phase::Read,
            Phase::Parse,
            Phase::Resolve,
            Phase::CacheOpen,
            Phase::Select,
            Phase::Compile,
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

/// Everything a run produced, so a caller can time the front end once and then select and execute
/// against it several times.
#[derive(Debug)]
pub struct Front {
    pub root: PathBuf,
    pub files: Vec<PathBuf>,
    pub sources: SourceMap,
    pub program: Program,
    pub resolved: Resolved,
    pub check: CheckOutput,
    pub hashes: HashOutput,
    /// The port's whole answer, for the tier this program is run on. `check` and `hashes` above
    /// are taken from it: the Rust chain no longer runs here, so there are no chain phases left to
    /// time and no second answer to disagree with (ADR 0052 §2).
    pub port: ply_ty::Front,
    pub timings: Timings,
    /// This program's region kinds.
    region_kinds: ply_eval::region_kind::Kinds,
}

impl Front {
    /// A machine over the program with the default tier attached.
    pub fn machine(&self) -> ply_eval::Machine<'_> {
        let mut machine =
            crate::tier_machine(&self.program, &self.resolved, &self.port, &self.sources);
        machine.share_region_kinds(self.shared_region_kinds());
        machine
    }

    pub fn shared_region_kinds(&self) -> ply_eval::region_kind::Kinds {
        ply_eval::region_kind::Kinds::clone(&self.region_kinds)
    }
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
    let mut program = ply_syntax::parse_program(inputs).map_err(|d| report(&d))?;
    timings.record(Phase::Parse, started.elapsed());

    let started = Instant::now();
    let resolved = resolve(&mut program).map_err(|d| report(&d))?;
    timings.record(Phase::Resolve, started.elapsed());

    // The port answers the check and the hashes, as it does for every command (ADR 0052 §1).
    // Outside the clock: what this harness times is the phases it still runs itself.
    let ordered: Vec<(String, String)> = ids
        .iter()
        .zip(&names)
        .map(|(&id, name)| {
            (
                name.to_string(),
                sources
                    .get(id)
                    .map_or(String::new(), |f| f.text.to_string()),
            )
        })
        .collect();
    let port = ply_codegen::c::producer::checked_front(&ordered, &ids)?;

    Ok(Front {
        root: root.to_path_buf(),
        files,
        sources,
        program,
        resolved,
        check: port.check.clone(),
        hashes: port.hashes.clone(),
        port,
        timings,
        region_kinds: ply_eval::region_kind::Kinds::default(),
    })
}

fn timed<T>(f: impl FnOnce() -> Result<T>) -> Result<(T, Duration)> {
    let started = Instant::now();
    let value = f()?;
    Ok((value, started.elapsed()))
}

/// Diagnostics collapse to one error here on purpose: this crate compiles a corpus it generated, so
/// a diagnostic is a defect in the generator and the first one is enough to go and look.
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
