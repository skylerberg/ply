//! The judgements delta construction cannot make from hashes alone.

use super::{DefKey, Ns, Rehashed};
use ply_eval::{CheckOutput, DefHash, Symbol, Value};
use ply_store::{DefKind, Found, Store};
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

    /// Only a `fn` is compared, as the front end filed both sides: the CLI files a program before
    /// its tests run, so the store holds the current hash's interface beside the baseline's.
    fn interface_stable(&mut self, key: &DefKey, before: DefHash) -> Option<bool> {
        if key.ns == Ns::Decl {
            return Some(false);
        }
        self.check.defs.get(&key.name)?;
        let now = current_hash(self.store, &key.name)?;
        Some(interface(self.store, before, &key.name)? == interface(self.store, now, &key.name)?)
    }

    fn component(&mut self, key: &DefKey) -> Vec<DefKey> {
        self.rehashed.component_of(key)
    }

    fn baseline_image(&mut self) -> BTreeSet<DefHash> {
        self.rehashed.image()
    }
}

/// The one hash the store's fingerprints file `name` under; a name filed under two is not one this
/// can answer for.
fn current_hash(store: &Store, name: &Symbol) -> Option<DefHash> {
    let hashes: BTreeSet<DefHash> = store
        .lookup(name.as_str())
        .into_iter()
        .filter_map(|found| match found {
            Found::Def(def) if def.name == *name && def.kind == DefKind::Fn => Some(def.hash),
            _ => None,
        })
        .collect();
    match hashes.len() {
        1 => hashes.into_iter().next(),
        _ => None,
    }
}

/// A definition's scheme and published footprint as the front end printed and filed them.
fn interface(store: &Store, hash: DefHash, name: &Symbol) -> Option<(Value, Value)> {
    let value = ply_eval::codec::decode(&store.def_of(hash, name)?.value).ok()?;
    let scheme = field(&value, "scheme")?.clone();
    let footprint = field(field(&value, "row")?, "footprint")?.clone();
    Some((scheme, footprint))
}

fn field<'v>(value: &'v Value, name: &str) -> Option<&'v Value> {
    match value {
        Value::Record(fields) => fields
            .iter()
            .find(|(k, _)| k.as_str() == name)
            .map(|(_, v)| v),
        _ => None,
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
