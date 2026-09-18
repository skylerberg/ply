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
    /// Simple name, unique corpus-wide, so a plain textual substitution renames it.
    pub symbol: String,
    pub replacement: String,
}

/// The concurrent half of a corpus, so a measurement need not re-derive it from source.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ConcurrencyProfile {
    pub tests: usize,
    pub tasks_per_test: usize,
    pub steps_per_task: usize,
    pub shards_per_test: usize,
    pub conflict_density: f64,
    pub contention: f64,
}

/// The specified half of a corpus, so a measurement need not re-derive it from source.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SpecProfile {
    /// Generated definitions carrying an `ensures`, specimens included.
    pub specified_definitions: usize,
    /// Definitions carrying none.
    pub unspecified_definitions: usize,
    /// One per `ensures` clause and one per law.
    pub obligations: usize,
    pub laws: usize,
    pub specimens: usize,
    pub decided: usize,
    pub sampled: usize,
    pub gaps: usize,
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
    #[serde(default)]
    pub specs: SpecProfile,
    pub bytes: usize,
    pub distinct_resources: usize,
    pub mean_out_degree: f64,
    pub max_call_weight: u32,
    /// A widely depended upon definition: editing it invalidates most of the corpus.
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
        specs: spec_profile(corpus),
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

fn spec_profile(corpus: &Corpus) -> SpecProfile {
    let [decided, sampled, gaps] = corpus.obligations_by_intent();
    let specified = corpus.specified_defs() + corpus.specimens.len();
    let definitions = corpus.defs.len() + corpus.specimens.len();
    SpecProfile {
        specified_definitions: specified,
        unspecified_definitions: definitions - specified,
        obligations: decided + sampled + gaps,
        laws: corpus.laws.len(),
        specimens: corpus.specimens.len(),
        decided,
        sampled,
        gaps,
    }
}

pub fn reverse_edges(corpus: &Corpus) -> Vec<Vec<DefId>> {
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

/// The most, or least, depended upon one-line definition that some test roots at.
pub fn pick(
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

pub fn transitive_dependents(callers: &[Vec<DefId>], from: DefId) -> BTreeSet<DefId> {
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
