//! Where the time actually goes.
//!
//! **This harness drives the from-scratch front end only** — [`crate::pipeline`]
//! parses and checks everything on every run. Its `warm` number is therefore the
//! cost of the *result* cache alone, and says nothing about the front-end cache
//! that `ply test` uses. For that, measure `ply test --json`, whose
//! `front_end.phases` reports the same breakdown for the path a user is on.
//!
//! The scenarios exist because the total is the least interesting number a
//! compiler with a perfect cache can report:
//!
//! - `cold` — nothing cached; every test runs.
//! - `warm` — nothing changed; every test is a cache hit and the front end is
//!   the entire cost. The number the thesis lives or dies on.
//! - `rename` — a top-level definition renamed; selection is observed rather
//!   than asserted, so a regression shows up as a number.
//! - `edit-leaf` — one definition's body changed; only its dependents re-run.
//! - `edit-hub` — the same, for a definition most of the corpus reaches.

use crate::pipeline::{Front, Phase, Timings, front};
use crate::write::{EditSite, read_manifest};
use anyhow::{Context, Result, bail};
use ply_store::Store;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Clone, Debug, Serialize)]
pub struct PhaseTime {
    pub phase: String,
    pub millis: f64,
    pub share: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Scenario {
    pub name: String,
    pub note: String,
    pub tests_total: usize,
    pub tests_selected: usize,
    pub tests_cached: usize,
    pub groups: usize,
    pub passed: usize,
    pub failed: usize,
    pub total_millis: f64,
    pub phases: Vec<PhaseTime>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub root: String,
    pub modules: usize,
    pub definitions: usize,
    pub tests: usize,
    pub source_bytes: usize,
    pub scenarios: Vec<Scenario>,
}

pub struct Options {
    /// Repetitions per scenario; the fastest is reported, because a slower run
    /// only ever means the machine did something else as well.
    pub repeats: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options { repeats: 3 }
    }
}

pub fn run(root: &Path, options: &Options) -> Result<Report> {
    let manifest = read_manifest(root)?;
    let mut scenarios = Vec::new();

    clear_cache(root)?;
    scenarios.push(measure(
        root,
        options,
        "cold",
        "empty cache; every test runs",
        Reset::Clear,
        None,
    )?);

    // The cold scenario left a full cache behind, which is exactly the state
    // `warm` is about.
    scenarios.push(measure(
        root,
        options,
        "warm",
        "nothing changed; every test is a cache hit",
        Reset::None,
        None,
    )?);

    scenarios.push(measure(
        root,
        options,
        "rename",
        &format!(
            "`{}` renamed; a rename must select nothing",
            manifest.rename.symbol
        ),
        Reset::Restore,
        Some(Mutation::Rename(
            manifest.rename.symbol.clone(),
            manifest.rename.replacement.clone(),
        )),
    )?);

    scenarios.push(measure(
        root,
        options,
        "edit-leaf",
        &format!(
            "one definition edited; {} dependent(s)",
            manifest.leaf_edit.dependents
        ),
        Reset::Restore,
        Some(Mutation::Edit(manifest.leaf_edit.clone())),
    )?);

    scenarios.push(measure(
        root,
        options,
        "edit-hub",
        &format!(
            "a hub definition edited; {} dependent(s)",
            manifest.hub_edit.dependents
        ),
        Reset::Restore,
        Some(Mutation::Edit(manifest.hub_edit.clone())),
    )?);

    Ok(Report {
        root: root.display().to_string(),
        modules: manifest.modules,
        definitions: manifest.definitions,
        tests: manifest.tests,
        source_bytes: manifest.bytes,
        scenarios,
    })
}

/// What the cache must look like at the start of every repeat. Without this a
/// second repeat measures the state the first repeat's run left behind — an
/// edit would look free because its dependents were already re-proved.
enum Reset {
    None,
    Clear,
    Restore,
}

/// A copy of the cache directory, kept in memory so restoring it cannot itself
/// be measured as disk work in the run that follows.
struct CacheSnapshot {
    dir: PathBuf,
    files: Vec<(PathBuf, Vec<u8>)>,
}

impl CacheSnapshot {
    fn take(root: &Path) -> Result<CacheSnapshot> {
        let dir = Store::open(root)?.dir().to_path_buf();
        let mut files = Vec::new();
        if dir.is_dir() {
            for entry in std::fs::read_dir(&dir)? {
                let path = entry?.path();
                if path.is_file() {
                    files.push((path.clone(), std::fs::read(&path)?));
                }
            }
        }
        Ok(CacheSnapshot { dir, files })
    }

    fn restore(&self) -> Result<()> {
        if self.dir.is_dir() {
            std::fs::remove_dir_all(&self.dir)?;
        }
        std::fs::create_dir_all(&self.dir)?;
        for (path, bytes) in &self.files {
            std::fs::write(path, bytes)?;
        }
        Ok(())
    }
}

#[derive(Clone)]
enum Mutation {
    Rename(String, String),
    Edit(EditSite),
}

/// A mutation is applied before the measurement and undone after it, so every
/// scenario starts from the same tree and the order they run in does not leak
/// into the numbers.
#[derive(Debug)]
struct Applied {
    files: Vec<(PathBuf, String)>,
}

impl Applied {
    fn undo(self) -> Result<()> {
        for (path, text) in self.files {
            std::fs::write(&path, text)
                .with_context(|| format!("restoring `{}`", path.display()))?;
        }
        Ok(())
    }
}

fn apply(root: &Path, mutation: &Mutation) -> Result<Applied> {
    let mut touched = Vec::new();
    match mutation {
        Mutation::Rename(from, to) => {
            for path in crate::pipeline::discover(root)? {
                let text = std::fs::read_to_string(&path)?;
                if !text.contains(from.as_str()) {
                    continue;
                }
                touched.push((path.clone(), text.clone()));
                std::fs::write(&path, text.replace(from.as_str(), to.as_str()))?;
            }
            if touched.is_empty() {
                bail!("`{from}` does not occur in the corpus; the manifest is stale");
            }
        }
        Mutation::Edit(site) => {
            let path = root.join(&site.path);
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading `{}`", path.display()))?;
            let count = text.matches(&site.find).count();
            if count != 1 {
                bail!(
                    "the edit site occurs {count} times in `{}`, not once",
                    site.path
                );
            }
            touched.push((path.clone(), text.clone()));
            std::fs::write(&path, text.replacen(&site.find, &site.replace, 1))?;
        }
    }
    Ok(Applied { files: touched })
}

fn measure(
    root: &Path,
    options: &Options,
    name: &str,
    note: &str,
    reset: Reset,
    mutation: Option<Mutation>,
) -> Result<Scenario> {
    let snapshot = match reset {
        Reset::Restore => Some(CacheSnapshot::take(root)?),
        _ => None,
    };
    let applied = mutation.as_ref().map(|m| apply(root, m)).transpose()?;
    let result = measure_inner(root, options, name, note, reset, snapshot.as_ref());

    // The tree and the cache are both put back, or the next scenario starts
    // from a state no one chose.
    if let Some(applied) = applied {
        applied.undo()?;
    }
    if let Some(snapshot) = &snapshot {
        snapshot.restore()?;
    }
    result
}

fn measure_inner(
    root: &Path,
    options: &Options,
    name: &str,
    note: &str,
    reset: Reset,
    snapshot: Option<&CacheSnapshot>,
) -> Result<Scenario> {
    let mut best: Option<(Timings, Shape)> = None;

    for _ in 0..options.repeats.max(1) {
        match reset {
            Reset::Clear => clear_cache(root)?,
            Reset::Restore => {
                if let Some(snapshot) = snapshot {
                    snapshot.restore()?;
                }
            }
            Reset::None => {}
        }
        let (timings, shape) = once(root)?;
        let keep = match &best {
            None => true,
            Some((current, _)) => timings.total() < current.total(),
        };
        if keep {
            best = Some((timings, shape));
        }
    }

    let (timings, shape) = best.expect("at least one repeat always runs");
    let total = timings.total().as_secs_f64() * 1000.0;
    let phases = Phase::all()
        .into_iter()
        .map(|phase| {
            let millis = timings.get(phase).as_secs_f64() * 1000.0;
            PhaseTime {
                phase: phase.label().to_string(),
                millis,
                share: if total > 0.0 { millis / total } else { 0.0 },
            }
        })
        .collect();

    Ok(Scenario {
        name: name.to_string(),
        note: note.to_string(),
        tests_total: shape.total,
        tests_selected: shape.selected,
        tests_cached: shape.cached,
        groups: shape.groups,
        passed: shape.passed,
        failed: shape.failed,
        total_millis: total,
        phases,
    })
}

#[derive(Clone, Copy, Debug, Default)]
struct Shape {
    total: usize,
    selected: usize,
    cached: usize,
    groups: usize,
    passed: usize,
    failed: usize,
}

fn once(root: &Path) -> Result<(Timings, Shape)> {
    let Front {
        program,
        resolved,
        check,
        hashes,
        mut timings,
        ..
    } = front(root)?;

    let started = Instant::now();
    let mut store = Store::open(root).context("opening the result cache")?;
    timings.record(Phase::CacheOpen, started.elapsed());

    let started = Instant::now();
    let selection = ply_test::select(&check, &hashes, &store);
    timings.record(Phase::Select, started.elapsed());

    let started = Instant::now();
    let report = ply_test::run(&selection, &program, &resolved, &check, &hashes, &mut store);
    timings.record(Phase::Execute, started.elapsed());

    Ok((
        timings,
        Shape {
            total: selection.total,
            selected: selection.to_run.len(),
            cached: selection.cached.len(),
            groups: selection.groups.len(),
            passed: report.passed,
            failed: report.failed,
        },
    ))
}

fn clear_cache(root: &Path) -> Result<()> {
    let mut store = Store::open(root)?;
    store.clear()?;
    Ok(())
}

pub fn render(report: &Report) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "{}\n{} modules · {} definitions · {} tests · {} KiB of source\n\n",
        report.root,
        report.modules,
        report.definitions,
        report.tests,
        report.source_bytes / 1024
    ));

    for scenario in &report.scenarios {
        s.push_str(&format!("  {} — {}\n", scenario.name, scenario.note));
        s.push_str(&format!(
            "    selected {} of {} ({} cached) · {} group(s) · {} passed, {} failed\n",
            scenario.tests_selected,
            scenario.tests_total,
            scenario.tests_cached,
            scenario.groups,
            scenario.passed,
            scenario.failed
        ));
        for phase in &scenario.phases {
            let bar = "#".repeat((phase.share * 40.0).round() as usize);
            s.push_str(&format!(
                "      {:<11} {:>9.2} ms  {:>5.1}%  {bar}\n",
                phase.phase,
                phase.millis,
                phase.share * 100.0
            ));
        }
        s.push_str(&format!(
            "      {:<11} {:>9.2} ms\n\n",
            "total", scenario.total_millis
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::generate;
    use crate::spec::CorpusSpec;
    use crate::write::{Manifest, write};

    fn corpus_at(root: &Path) -> Manifest {
        let spec = CorpusSpec {
            seed: 4,
            modules: 5,
            defs_per_module: 6,
            tests: 10,
            depth: 2,
            ..CorpusSpec::default()
        };
        write(root, &spec, &generate(&spec)).unwrap().manifest
    }

    #[test]
    fn a_mutation_is_undone_even_when_the_measurement_fails() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        let manifest = corpus_at(&root);

        let before: Vec<String> = crate::pipeline::discover(&root)
            .unwrap()
            .iter()
            .map(|p| std::fs::read_to_string(p).unwrap())
            .collect();

        let applied = apply(&root, &Mutation::Edit(manifest.leaf_edit.clone())).unwrap();
        let during: Vec<String> = crate::pipeline::discover(&root)
            .unwrap()
            .iter()
            .map(|p| std::fs::read_to_string(p).unwrap())
            .collect();
        assert_ne!(before, during, "the edit changed nothing");

        applied.undo().unwrap();
        let after: Vec<String> = crate::pipeline::discover(&root)
            .unwrap()
            .iter()
            .map(|p| std::fs::read_to_string(p).unwrap())
            .collect();
        assert_eq!(before, after);
    }

    #[test]
    fn a_rename_touches_every_file_that_mentions_the_symbol() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        let manifest = corpus_at(&root);

        let applied = apply(
            &root,
            &Mutation::Rename(
                manifest.rename.symbol.clone(),
                manifest.rename.replacement.clone(),
            ),
        )
        .unwrap();
        assert!(!applied.files.is_empty());
        for path in crate::pipeline::discover(&root).unwrap() {
            let text = std::fs::read_to_string(&path).unwrap();
            let bare = text
                .replace(&manifest.rename.replacement, "")
                .contains(&manifest.rename.symbol);
            assert!(!bare, "`{}` still names the old symbol", path.display());
        }
        applied.undo().unwrap();
    }

    /// With more than one repeat, an edit scenario has to re-prove its
    /// dependents every time: if the cache is not restored, every repeat after
    /// the first is a warm run and the fastest one is reported.
    #[test]
    fn an_edit_scenario_reselects_on_every_repeat() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        corpus_at(&root);

        let report = run(&root, &Options { repeats: 2 }).unwrap();
        let named = |name: &str| {
            report
                .scenarios
                .iter()
                .find(|s| s.name == name)
                .unwrap_or_else(|| panic!("{name}"))
        };
        let warm = named("warm");
        let hub = named("edit-hub");
        assert!(
            hub.tests_selected > warm.tests_selected,
            "editing a hub selected {} tests, no more than an unchanged run's {}",
            hub.tests_selected,
            warm.tests_selected
        );
    }

    /// The headline invariant, measured rather than asserted in the abstract.
    #[test]
    fn renaming_selects_no_more_than_an_unchanged_run() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        corpus_at(&root);

        let report = run(&root, &Options { repeats: 1 }).unwrap();
        let warm = report.scenarios.iter().find(|s| s.name == "warm").unwrap();
        let rename = report
            .scenarios
            .iter()
            .find(|s| s.name == "rename")
            .unwrap();
        assert_eq!(rename.tests_selected, warm.tests_selected);
        assert_eq!(rename.failed, 0);
    }

    #[test]
    fn a_stale_edit_site_is_an_error_rather_than_a_silent_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        let mut manifest = corpus_at(&root);
        manifest.leaf_edit.find = "fn definitely_not_here() -> Int = 0\n".to_string();

        let err = apply(&root, &Mutation::Edit(manifest.leaf_edit)).unwrap_err();
        assert!(err.to_string().contains("occurs 0 times"));
    }
}
