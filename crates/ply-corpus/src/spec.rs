use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

/// Everything the shape of a corpus depends on. Two runs with equal specs and
/// equal seeds produce byte-identical files.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CorpusSpec {
    pub seed: u64,
    pub modules: usize,
    pub defs_per_module: usize,
    pub tests: usize,
    /// Layers in the module DAG. A module only imports from a lower layer, so
    /// this is the longest import chain the corpus can have.
    pub depth: usize,
    /// Distinct `db` resource labels, shared across every module — this is what
    /// makes the conflict graph non-trivial rather than one clique per file.
    pub tables: usize,
    /// Distinct `cache` resource labels.
    pub regions: usize,
    pub effect_fraction: f64,
    pub nondet_fraction: f64,
    pub hub_modules: usize,
    /// Cap on `1 + sum(callee weights)`. Without it a random call graph is
    /// exponential to interpret and the corpus stops being realistic.
    pub max_weight: u32,
    /// `simulate` tests, on top of `tests`. Zero leaves a corpus that never
    /// reaches a region, which is what every measurement before M7 wants — and
    /// is what a manifest written before M7 deserializes to.
    #[serde(default)]
    pub concurrent_tests: usize,
    /// Tasks spawned by each of them. Fully contended, a test of `t` tasks and
    /// `s` steps has `(t·s)! / (s!)^t` schedules, so 4×3 is already 369,600 and
    /// well past any sensible `--sim-budget`. 3×2 is 90, which a search can
    /// exhaust.
    #[serde(default)]
    pub tasks_per_test: usize,
    /// `counter.bump` calls per task, separated by a `task.yield()`. Steps are
    /// what an interleaving is a sequence of, so this is the exponent the naive
    /// schedule count grows in.
    #[serde(default)]
    pub steps_per_task: usize,
    /// How much the tasks contend, from 0.0 — one shard each, so no two steps
    /// conflict and the search collapses to a single interleaving — to 1.0, one
    /// shard for everybody, where every pair of steps is dependent and the
    /// reduction has nothing to prune.
    #[serde(default)]
    pub conflict_density: f64,
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
        Ok(())
    }

    /// Shards a test of this shape spreads its tasks over: every task its own at
    /// density 0, one between all of them at density 1. The count is what the
    /// dependence relation reads, so it is derived once here and both the
    /// generator and the manifest use it.
    pub fn shards_per_test(&self) -> usize {
        let tasks = self.tasks_per_test.max(1);
        let spread = (1.0 - self.conflict_density) * (tasks - 1) as f64;
        1 + spread.round() as usize
    }

    /// Generated `fn`s only: the two hand-written core modules and the per-module
    /// `stage` helpers are counted separately by the manifest.
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
    }
}
