//! The judgements delta construction cannot make from hashes alone.

use super::{DefKey, Ns, Rehashed};
use ply_span::Symbol;
use ply_store::{Store, canonicalize_scheme};
use ply_ty::CheckOutput;
use ply_ty::DefHash;
use std::collections::BTreeSet;

pub trait Classify {
    /// `key`'s current body re-normalized against the baseline hash table.
    fn renormalized(&mut self, key: &DefKey) -> Option<DefHash>;

    /// The same for the test's own body.
    fn renormalized_test(&mut self, key: &Symbol) -> Option<DefHash>;

    /// Whether the canonical scheme and footprint match on both sides, which is exactly when a
    /// hybrid swapping this definition alone still typechecks.
    fn interface_stable(&mut self, key: &DefKey, before: DefHash) -> Option<bool>;

    /// The strongly connected component `key` belongs to, when it has more than one member.
    fn component(&mut self, _key: &DefKey) -> Vec<DefKey> {
        Vec::new()
    }

    /// Every hash the whole current program re-normalizes to against the baseline table.
    fn baseline_image(&mut self) -> BTreeSet<DefHash> {
        BTreeSet::new()
    }
}

pub struct StoreClassify<'a> {
    rehashed: Rehashed,
    store: &'a Store,
    check: &'a CheckOutput,
}

impl<'a> StoreClassify<'a> {
    pub fn new(rehashed: Rehashed, store: &'a Store, check: &'a CheckOutput) -> StoreClassify<'a> {
        StoreClassify {
            rehashed,
            store,
            check,
        }
    }
}

impl Classify for StoreClassify<'_> {
    fn renormalized(&mut self, key: &DefKey) -> Option<DefHash> {
        self.rehashed.rehash(key)
    }

    fn renormalized_test(&mut self, key: &Symbol) -> Option<DefHash> {
        self.rehashed.rehash_test(key)
    }

    /// Only a `fn` is compared.
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
        self.rehashed.component_of(key)
    }

    fn baseline_image(&mut self) -> BTreeSet<DefHash> {
        self.rehashed.image()
    }
}

/// A classifier with no evidence: everything is `Edited`, nothing is independent.
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
