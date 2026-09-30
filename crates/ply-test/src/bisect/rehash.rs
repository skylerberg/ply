//! `Edited` versus `Derived`, exactly: today's bodies hashed as the baseline wrote references.

use super::{Baseline, DefKey, Ns};
use ply_eval::decode::{self, At};
use ply_span::Symbol;
use ply_ty::DefHash;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default)]
pub struct Rehashed {
    fresh: BTreeMap<DefKey, DefHash>,
    tests: BTreeMap<Symbol, DefHash>,
    image: BTreeSet<DefHash>,
    components: BTreeMap<DefKey, usize>,
}

impl Rehashed {
    /// The packages are the ones the run was analysed with: a rehash that resolved without them
    /// would read a dependency's own modules as root ones.
    pub fn under(
        sources: &[(String, String)],
        baseline: &Baseline,
        packages: &[(String, Vec<String>)],
        mod_pkg: &[usize],
    ) -> Result<Rehashed, String> {
        let pins: Vec<(String, bool, DefHash)> = baseline
            .keys()
            .filter_map(|key| Some((key.name.to_string(), key.is_decl(), baseline.hash_of(&key)?)))
            .collect();
        let answer = ply_codegen::c::producer::rehash(sources, &pins, packages, mod_pkg)
            .map_err(|e| format!("{e:#}"))?;
        read(At::new("`front.rehash`'s answer", &answer)).map_err(|e| e.to_string())
    }

    pub fn rehash(&self, key: &DefKey) -> Option<DefHash> {
        self.fresh.get(key).copied()
    }

    pub fn rehash_test(&self, key: &Symbol) -> Option<DefHash> {
        self.tests.get(key).copied()
    }

    pub fn image(&self) -> BTreeSet<DefHash> {
        self.image.clone()
    }

    pub fn component_of(&self, key: &DefKey) -> Vec<DefKey> {
        let Some(id) = self.components.get(key) else {
            return Vec::new();
        };
        self.components
            .iter()
            .filter(|(_, c)| *c == id)
            .map(|(k, _)| k.clone())
            .collect()
    }
}

/// A `hash.Rehashed`. A node's component is `-1` when it is in none.
fn read(answer: At<'_>) -> Result<Rehashed, decode::Error> {
    let hash = |at: At<'_>| at.byte_array().map(DefHash);
    let mut out = Rehashed::default();
    for node in answer.field("nodes")?.list()? {
        let key = DefKey {
            name: Symbol::new(node.field("name")?.utf8()?),
            ns: if node.field("decl")?.bool()? {
                Ns::Decl
            } else {
                Ns::Value
            },
        };
        out.fresh.insert(key.clone(), hash(node.field("fresh")?)?);
        out.image.insert(hash(node.field("table")?)?);
        let component = node.field("component")?;
        if component.int()? >= 0 {
            out.components.insert(key, component.number()?);
        }
    }
    for test in answer.field("tests")?.list()? {
        out.tests.insert(
            Symbol::new(test.field("key")?.utf8()?),
            hash(test.field("hash")?)?,
        );
    }
    Ok(out)
}
