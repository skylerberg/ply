use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

/// Everything the shape of a corpus depends on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CorpusSpec {
    pub seed: u64,
    pub modules: usize,
    pub defs_per_module: usize,
    pub tests: usize,
    /// Layers in the module DAG.
    pub depth: usize,
    /// Distinct `db` resource labels, shared across every module — this is what makes the conflict
    /// graph non-trivial rather than one clique per file.
    pub tables: usize,
    /// Distinct `cache` resource labels.
    pub regions: usize,
    pub effect_fraction: f64,
    pub nondet_fraction: f64,
    pub hub_modules: usize,
    /// Cap on `1 + sum(callee weights)`.
    pub max_weight: u32,
    /// `simulate` tests, on top of `tests`.
    #[serde(default)]
    pub concurrent_tests: usize,
    /// Tasks spawned by each of them.
    #[serde(default)]
    pub tasks_per_test: usize,
    /// `counter.bump` calls per task, separated by a `task.yield()`.
    #[serde(default)]
    pub steps_per_task: usize,
    /// How much the tasks contend, from 0.0 — one shard each, so no two steps conflict and the
    /// search collapses to a single interleaving — to 1.0, one shard for everybody, where every
    /// pair of steps is dependent and the reduction has nothing to prune.
    #[serde(default)]
    pub conflict_density: f64,
    /// Fraction of generated definitions carrying a `requires`/`ensures` pair.
    #[serde(default)]
    pub spec_fraction: f64,
    /// Definitions per module written for their obligation rather than for their call graph, so
    /// that the tier distribution spans the table instead of landing in one bucket.
    #[serde(default)]
    pub specimens_per_module: usize,
}

impl Default for CorpusSpec {
    fn default() -> Self {
        CorpusSpec {
            seed: 1,
            modules: 20,
            defs_per_module: 25,
            tests: 200,
            depth: 4,
            tables: 12,
            regions: 6,
            effect_fraction: 0.35,
            nondet_fraction: 0.03,
            hub_modules: 3,
            max_weight: 192,
            concurrent_tests: 0,
            tasks_per_test: 3,
            steps_per_task: 2,
            conflict_density: 0.5,
            spec_fraction: 0.0,
            specimens_per_module: 0,
        }
    }
}

impl CorpusSpec {
    pub fn validate(&self) -> Result<()> {
        if self.modules == 0 {
            bail!("a corpus needs at least one module");
        }
        if self.defs_per_module == 0 {
            bail!("a module needs at least one definition");
        }
        if self.depth == 0 {
            bail!("`--depth` must be at least 1");
        }
        if self.depth > self.modules {
            bail!(
                "`--depth {}` exceeds `--modules {}`: every layer must hold at least one module",
                self.depth,
                self.modules
            );
        }
        if self.tables == 0 {
            bail!("`--tables` must be at least 1");
        }
        if self.regions == 0 {
            bail!("`--regions` must be at least 1");
        }
        if !(0.0..=1.0).contains(&self.effect_fraction) {
            bail!("`--effect-fraction` must be between 0 and 1");
        }
        if !(0.0..=1.0).contains(&self.nondet_fraction) {
            bail!("`--nondet-fraction` must be between 0 and 1");
        }
        if self.max_weight < 8 {
            bail!("`--max-weight` below 8 leaves no room for a call graph");
        }
        if self.concurrent_tests > 0 && self.tasks_per_test < 2 {
            bail!("`--tasks-per-test` below 2 has nothing to interleave");
        }
        if self.concurrent_tests > 0 && self.steps_per_task == 0 {
            bail!("`--steps-per-task` must be at least 1");
        }
        if !(0.0..=1.0).contains(&self.conflict_density) {
            bail!("`--conflict-density` must be between 0 and 1");
        }
        if !(0.0..=1.0).contains(&self.spec_fraction) {
            bail!("`--spec-fraction` must be between 0 and 1");
        }
        Ok(())
    }

    /// Shards a test of this shape spreads its tasks over: every task its own at density 0, one
    /// between all of them at density 1.
    pub fn shards_per_test(&self) -> usize {
        let tasks = self.tasks_per_test.max(1);
        let spread = (1.0 - self.conflict_density) * (tasks - 1) as f64;
        1 + spread.round() as usize
    }

    /// Generated `fn`s only: the two hand-written core modules and the per-module `stage` helpers
    /// are counted separately by the manifest.
    pub fn generated_defs(&self) -> usize {
        self.modules * self.defs_per_module
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_spec_is_valid() {
        CorpusSpec::default().validate().unwrap();
    }

    #[test]
    fn a_depth_deeper_than_the_module_count_is_rejected() {
        let spec = CorpusSpec {
            modules: 3,
            depth: 8,
            ..CorpusSpec::default()
        };
        assert!(
            spec.validate()
                .unwrap_err()
                .to_string()
                .contains("--depth 8")
        );
    }

    #[test]
    fn density_walks_the_shard_count_from_one_each_to_one_between_all() {
        let spec = |d: f64| CorpusSpec {
            tasks_per_test: 4,
            conflict_density: d,
            ..CorpusSpec::default()
        };
        assert_eq!(spec(0.0).shards_per_test(), 4);
        assert_eq!(spec(1.0).shards_per_test(), 1);
        assert_eq!(spec(0.5).shards_per_test(), 3);
        assert_eq!(
            CorpusSpec {
                tasks_per_test: 2,
                conflict_density: 0.4,
                ..CorpusSpec::default()
            }
            .shards_per_test(),
            2
        );
    }

    #[test]
    fn a_concurrent_test_needs_two_tasks_and_a_step() {
        let spec = CorpusSpec {
            concurrent_tests: 4,
            tasks_per_test: 1,
            ..CorpusSpec::default()
        };
        assert!(
            spec.validate()
                .unwrap_err()
                .to_string()
                .contains("nothing to interleave")
        );
        let spec = CorpusSpec {
            concurrent_tests: 4,
            steps_per_task: 0,
            ..CorpusSpec::default()
        };
        assert!(spec.validate().is_err());
        assert!(
            CorpusSpec {
                concurrent_tests: 0,
                tasks_per_test: 1,
                ..CorpusSpec::default()
            }
            .validate()
            .is_ok(),
            "a corpus with no concurrent tests is not constrained by their shape"
        );
    }

    /// A manifest written before M8 has no `spec_fraction`, and deserializing one must produce a
    /// corpus with no obligations rather than a default density nobody asked for.
    #[test]
    fn a_spec_written_before_m8_deserializes_to_a_corpus_with_no_obligations() {
        let spec: CorpusSpec = serde_json::from_str(
            r#"{"seed":1,"modules":2,"defs_per_module":3,"tests":1,"depth":1,
                "tables":2,"regions":1,"effect_fraction":0.3,"nondet_fraction":0.0,
                "hub_modules":1,"max_weight":64}"#,
        )
        .unwrap();
        assert_eq!(spec.spec_fraction, 0.0);
        assert_eq!(spec.specimens_per_module, 0);
        spec.validate().unwrap();
    }

    #[test]
    fn out_of_range_fractions_are_rejected() {
        let spec = CorpusSpec {
            effect_fraction: 1.5,
            ..CorpusSpec::default()
        };
        assert!(spec.validate().is_err());
        let spec = CorpusSpec {
            nondet_fraction: -0.1,
            ..CorpusSpec::default()
        };
        assert!(spec.validate().is_err());
        let spec = CorpusSpec {
            conflict_density: 1.5,
            ..CorpusSpec::default()
        };
        assert!(spec.validate().is_err());
        let spec = CorpusSpec {
            spec_fraction: 1.2,
            ..CorpusSpec::default()
        };
        assert!(
            spec.validate()
                .unwrap_err()
                .to_string()
                .contains("--spec-fraction")
        );
    }
}
