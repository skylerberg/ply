//! Putting a corpus on disk, and the manifest a benchmark reads back.

use crate::emit::{self, Emitted};
use crate::model::{Corpus, DefId};
use crate::spec::CorpusSpec;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub const MANIFEST: &str = "corpus.json";

/// A source edit a benchmark can apply and undo with a textual substitution.
/// `find` is a whole definition, so it is unique within `path`; `replace`
/// changes that definition's hash without changing what it returns.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EditSite {
    pub path: String,
    pub find: String,
    pub replace: String,
    /// Generated definitions whose hash the edit changes, this one included.
    pub dependents: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RenameSite {
    /// The definition's simple name, unique corpus-wide, so a benchmark can
    /// rename it with a plain textual substitution across every file.
    pub symbol: String,
    pub replacement: String,
}

/// What the concurrent half of a corpus looks like, so a measurement can plot
/// exploration against contention without re-deriving it from the source.
/// `contention` is measured from the corpus that was written rather than copied
/// from the spec, because the two differ whenever the shard count rounds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ConcurrencyProfile {
    pub tests: usize,
    pub tasks_per_test: usize,
    pub steps_per_task: usize,
    pub shards_per_test: usize,
    pub conflict_density: f64,
    pub contention: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub spec: CorpusSpec,
    pub files: usize,
    pub modules: usize,
    pub definitions: usize,
    pub effectful_definitions: usize,
    pub tests: usize,
    pub nondet_tests: usize,
    #[serde(default)]
    pub concurrency: ConcurrencyProfile,
    pub bytes: usize,
    pub distinct_resources: usize,
    pub mean_out_degree: f64,
    pub max_call_weight: u32,
    /// A widely depended upon definition: editing it invalidates most of the
    /// corpus.
    pub hub_edit: EditSite,
    /// A definition nothing else calls: editing it invalidates almost nothing.
    pub leaf_edit: EditSite,
    pub rename: RenameSite,
}

#[derive(Debug)]
pub struct Written {
    pub root: PathBuf,
    pub manifest: Manifest,
}

pub fn write(root: &Path, spec: &CorpusSpec, corpus: &Corpus) -> Result<Written> {
    let files = emit::emit(corpus);
    prepare(root)?;

    let mut bytes = 0usize;
    for file in &files {
        let path = root.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating `{}`", parent.display()))?;
        }
        std::fs::write(&path, &file.text)
            .with_context(|| format!("writing `{}`", path.display()))?;
        bytes += file.text.len();
    }

    let manifest = manifest_for(spec, corpus, &files, bytes)?;
    let json = serde_json::to_string_pretty(&manifest)?;
    std::fs::write(root.join(MANIFEST), format!("{json}\n"))?;

    Ok(Written {
        root: root.to_path_buf(),
        manifest,
    })
}

/// Refuses a directory that holds anything other than a corpus this tool wrote.
/// The generator deletes what it finds, and deleting a user's source tree
/// because they typed the wrong `--out` is not a recoverable mistake.
fn prepare(root: &Path) -> Result<()> {
    if root.exists() {
        if !root.is_dir() {
            bail!("`{}` exists and is not a directory", root.display());
        }
        let ours = root.join(MANIFEST).exists();
        let empty = root.read_dir()?.next().is_none();
        if !ours && !empty {
            bail!(
                "`{}` is not empty and holds no `{MANIFEST}`; refusing to overwrite it",
                root.display()
            );
        }
        std::fs::remove_dir_all(root)?;
    }
    std::fs::create_dir_all(root)?;
    Ok(())
}

pub fn read_manifest(root: &Path) -> Result<Manifest> {
    let path = root.join(MANIFEST);
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("`{}` is not a generated corpus", root.display()))?;
    Ok(serde_json::from_str(&text)?)
}

fn manifest_for(
    spec: &CorpusSpec,
    corpus: &Corpus,
    files: &[Emitted],
    bytes: usize,
) -> Result<Manifest> {
    let callers = reverse_edges(corpus);
    let tested: BTreeSet<DefId> = corpus.tests.iter().map(|t| t.root).collect();
    let hub = pick(corpus, &callers, &tested, true)
        .context("corpus holds no one-line definition to use as an edit site")?;
    let leaf = pick(corpus, &callers, &tested, false)
        .context("corpus holds no one-line definition to use as an edit site")?;

    let edges: usize = corpus
        .defs
        .iter()
        .map(|d| d.shape.calls().len() + d.extras.len())
        .sum();

    Ok(Manifest {
        spec: spec.clone(),
        files: files.len(),
        modules: corpus.modules.len(),
        definitions: corpus.defs.len(),
        effectful_definitions: corpus.effectful_defs(),
        tests: corpus.tests.len(),
        nondet_tests: corpus.tests.iter().filter(|t| t.nondet).count(),
        concurrency: profile(spec, corpus),
        bytes,
        distinct_resources: corpus.tables.len() + corpus.regions.len(),
        mean_out_degree: edges as f64 / corpus.defs.len().max(1) as f64,
        max_call_weight: corpus.defs.iter().map(|d| d.weight).max().unwrap_or(0),
        hub_edit: edit_site(corpus, &callers, hub)?,
        leaf_edit: edit_site(corpus, &callers, leaf)?,
        rename: RenameSite {
            symbol: corpus.defs[leaf].name.clone(),
            replacement: format!("{}_renamed", corpus.defs[leaf].name),
        },
    })
}

fn profile(spec: &CorpusSpec, corpus: &Corpus) -> ConcurrencyProfile {
    let tests = corpus.concurrent.len();
    let mean = |v: f64| if tests == 0 { 0.0 } else { v / tests as f64 };
    ConcurrencyProfile {
        tests,
        tasks_per_test: spec.tasks_per_test,
        steps_per_task: spec.steps_per_task,
        shards_per_test: spec.shards_per_test(),
        conflict_density: spec.conflict_density,
        contention: mean(corpus.concurrent.iter().map(|t| t.contention()).sum()),
    }
}

fn reverse_edges(corpus: &Corpus) -> Vec<Vec<DefId>> {
    let mut callers = vec![Vec::new(); corpus.defs.len()];
    for def in &corpus.defs {
        for call in def
            .shape
            .calls()
            .into_iter()
            .chain(def.extras.iter().copied())
        {
            callers[call.target].push(def.id);
        }
    }
    callers
}

/// The most, or least, directly depended upon one-line definition that some
/// test roots at — editing one nothing tests selects nothing, which measures
/// only that the run happened. Direct callers stand in for transitive ones so
/// this stays linear at ten thousand definitions; the transitive count is then
/// measured once, for the winner. Ties break on the lowest id, so the choice
/// follows the seed rather than iteration order.
fn pick(
    corpus: &Corpus,
    callers: &[Vec<DefId>],
    tested: &BTreeSet<DefId>,
    most: bool,
) -> Option<DefId> {
    let mut best: Option<(usize, DefId)> = None;
    for def in corpus
        .defs
        .iter()
        .filter(|d| d.shape.is_one_liner() && tested.contains(&d.id))
    {
        let reach = callers[def.id].len();
        let better = match best {
            None => true,
            Some((count, _)) if most => reach > count,
            Some((count, _)) => reach < count,
        };
        if better {
            best = Some((reach, def.id));
        }
    }
    best.map(|(_, id)| id)
}

fn transitive_dependents(callers: &[Vec<DefId>], from: DefId) -> BTreeSet<DefId> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![from];
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        stack.extend(callers[id].iter().copied());
    }
    seen
}

fn edit_site(corpus: &Corpus, callers: &[Vec<DefId>], target: DefId) -> Result<EditSite> {
    let def = &corpus.defs[target];
    let find = emit::emit_def(corpus, def);
    let replace = emit::wrap_body(&find)
        .with_context(|| format!("`{}` is not a one-line definition after all", def.name))?;
    Ok(EditSite {
        path: corpus.modules[def.module].path.clone(),
        find,
        replace,
        dependents: transitive_dependents(callers, target).len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::generate;

    fn spec() -> CorpusSpec {
        CorpusSpec {
            seed: 2,
            modules: 6,
            defs_per_module: 8,
            tests: 12,
            depth: 3,
            ..CorpusSpec::default()
        }
    }

    #[test]
    fn a_written_corpus_round_trips_through_its_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        let corpus = generate(&spec());
        let written = write(&root, &spec(), &corpus).unwrap();

        let read = read_manifest(&root).unwrap();
        assert_eq!(read.spec, spec());
        assert_eq!(read.definitions, written.manifest.definitions);
        assert_eq!(read.files, corpus.modules.len() + 2);
        assert!(root.join("core/prim.ply").exists());
    }

    #[test]
    fn regenerating_over_a_corpus_is_allowed_and_over_anything_else_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        let corpus = generate(&spec());
        write(&root, &spec(), &corpus).unwrap();
        write(&root, &spec(), &corpus).expect("a corpus directory may be regenerated");

        let precious = dir.path().join("precious");
        std::fs::create_dir_all(&precious).unwrap();
        std::fs::write(precious.join("thesis.txt"), "years of work").unwrap();
        let err = write(&precious, &spec(), &corpus).unwrap_err();
        assert!(err.to_string().contains("refusing to overwrite"));
        assert!(precious.join("thesis.txt").exists());
    }

    #[test]
    fn the_hub_edit_reaches_more_definitions_than_the_leaf_edit() {
        let corpus = generate(&spec());
        let callers = reverse_edges(&corpus);
        let tested: BTreeSet<DefId> = corpus.tests.iter().map(|t| t.root).collect();
        let hub = pick(&corpus, &callers, &tested, true).unwrap();
        let leaf = pick(&corpus, &callers, &tested, false).unwrap();
        assert!(
            transitive_dependents(&callers, hub).len()
                > transitive_dependents(&callers, leaf).len()
        );
    }

    #[test]
    fn every_edit_sites_needle_occurs_exactly_once_in_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        let corpus = generate(&spec());
        let written = write(&root, &spec(), &corpus).unwrap();

        for site in [&written.manifest.hub_edit, &written.manifest.leaf_edit] {
            let text = std::fs::read_to_string(root.join(&site.path)).unwrap();
            assert_eq!(
                text.matches(&site.find).count(),
                1,
                "`{}` is not unique",
                site.find
            );
        }
    }

    #[test]
    fn the_rename_target_is_a_corpus_wide_unique_symbol() {
        let corpus = generate(&spec());
        let names: BTreeSet<&str> = corpus.defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names.len(), corpus.defs.len());
    }
}
