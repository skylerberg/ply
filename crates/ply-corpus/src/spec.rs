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
        Ok(())
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
    }
}
