//! The judgements delta construction cannot make from hashes alone.

use super::{Baseline, DefKey, EraTable, Ns, Renormalizer};
use ply_core::CheckOutput;
use ply_hash::DefHash;
use ply_span::Symbol;
use ply_store::{Store, canonicalize_scheme};
use std::collections::BTreeSet;

pub trait Classify {
    /// `key`'s current body re-normalized against the baseline hash table.
    /// Equality with the baseline hash is what proves nobody edited it, so
    /// `None` — "cannot say" — must be read as *edited*: a false `Derived` drops
    /// a real candidate and yields a confidently wrong culprit.
    fn renormalized(&mut self, key: &DefKey) -> Option<DefHash>;

    /// The same for the test's own body. `None` withholds `TestChanged` rather
    /// than accusing a test nobody touched.
    fn renormalized_test(&mut self, key: &Symbol) -> Option<DefHash>;

    /// Whether the published interface — canonical scheme and footprint — is the
    /// same on both sides, which is exactly the condition under which a hybrid
    /// that swaps this definition alone still typechecks. `None` is read as *not*
    /// independent, which costs a larger cluster and never a wrong flip.
    fn interface_stable(&mut self, key: &DefKey, before: DefHash) -> Option<bool>;

    /// The strongly connected component `key` belongs to, when it has more than
    /// one member. Those members share a component hash and one stored body, so
    /// no hybrid can flip one of them alone and the search must never be offered
    /// the choice.
    fn component(&mut self, _key: &DefKey) -> Vec<DefKey> {
        Vec::new()
    }

    /// Every hash the *whole current program* re-normalizes to against the
    /// baseline table — the identities the current definitions would have had
    /// back then. A definition whose baseline hash is in here still exists; it
    /// was renamed, not removed.
    fn baseline_image(&mut self) -> BTreeSet<DefHash> {
        BTreeSet::new()
    }
}

pub struct StoreClassify<'a> {
    renormalizer: &'a Renormalizer<'a>,
    /// The baseline era's hash for every node, resolved once. Building it walks
    /// the program, and a classifier is asked about every changed definition.
    table: EraTable,
    store: &'a Store,
    check: &'a CheckOutput,
}

impl<'a> StoreClassify<'a> {
    pub fn new(
        renormalizer: &'a Renormalizer<'a>,
        baseline: &'a Baseline,
        store: &'a Store,
        check: &'a CheckOutput,
    ) -> StoreClassify<'a> {
        StoreClassify {
            table: renormalizer.era_table(&|key: &DefKey| baseline.hash_of(key)),
            renormalizer,
            store,
            check,
        }
    }

    pub fn table(&self) -> &EraTable {
        &self.table
    }
}

impl Classify for StoreClassify<'_> {
    fn renormalized(&mut self, key: &DefKey) -> Option<DefHash> {
        self.renormalizer.rehash(key, &self.table)
    }

    fn renormalized_test(&mut self, key: &Symbol) -> Option<DefHash> {
        self.renormalizer.rehash_test(key, &self.table)
    }

    /// Only a `fn` is compared. A `type` or `effect` whose hash moved has by
    /// definition changed something every mention of it can see, so treating it
    /// as interface-breaking is not a concession — it is the answer.
    fn interface_stable(&mut self, key: &DefKey, before: DefHash) -> Option<bool> {
        if key.ns == Ns::Decl {
            return Some(false);
        }
        let now = self.check.defs.get(&key.name)?;
        let then = self.store.def_of(before, &key.name)?;
        Some(
            canonicalize_scheme(&now.scheme) == canonicalize_scheme(&then.scheme)
                && now.footprint == then.footprint,
        )
    }

    fn component(&mut self, key: &DefKey) -> Vec<DefKey> {
        self.renormalizer.component_of(key)
    }

    fn baseline_image(&mut self) -> BTreeSet<DefHash> {
        self.table.image()
    }
}

/// A classifier with no evidence: everything is `Edited`, nothing is
/// independent. What a run gets when the front-end cache has been pruned, and
/// the shape every degraded answer has to take — wider, never wrong.
#[derive(Clone, Copy, Debug, Default)]
pub struct Unknown;

impl Classify for Unknown {
    fn renormalized(&mut self, _: &DefKey) -> Option<DefHash> {
        None
    }
    fn renormalized_test(&mut self, _: &Symbol) -> Option<DefHash> {
        None
    }
    fn interface_stable(&mut self, _: &DefKey, _: DefHash) -> Option<bool> {
        None
    }
}
