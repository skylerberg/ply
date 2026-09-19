//! The stored form of a definition body: DESIGN.md §3's `Definition`, the one element of `Hash ->
//! (Definition, Type, Footprint)` the store never held.

use indexmap::IndexMap;
use ply_span::Symbol;

use crate::DefHash;

pub const BODY_ENCODING: u32 = 7;

/// A definition that is its own strongly connected component: the payload is its normalized bytes
/// and `blake3(payload)` is the key.
const KIND_SOLO: u8 = 0;
/// A member of a mutually recursive component: the payload is the *component's* bytes — every
/// member, so the cycle can be rebuilt — and the key is `blake3(blake3(payload) ‖ class_le_u32)`.
const KIND_MEMBER: u8 = 1;

/// A member of a component, given the component's own hash.
pub(crate) fn member_hash(component: DefHash, class: u32) -> DefHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&component.0);
    hasher.update(&class.to_le_bytes());
    DefHash(*hasher.finalize().as_bytes())
}

/// One definition's canonical body bytes, in the envelope that makes them self-checking against the
/// [`DefHash`] they are filed under.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StoredBody(Vec<u8>);

enum Shape<'a> {
    Solo(&'a [u8]),
    Member { class: u32, payload: &'a [u8] },
}

impl StoredBody {
    pub(crate) fn solo(encoding: &[u8]) -> StoredBody {
        let mut out = Vec::with_capacity(encoding.len() + 1);
        out.push(KIND_SOLO);
        out.extend_from_slice(encoding);
        StoredBody(out)
    }

    pub(crate) fn member(component: &[u8], class: u32) -> StoredBody {
        let mut out = Vec::with_capacity(component.len() + 5);
        out.push(KIND_MEMBER);
        out.extend_from_slice(&class.to_le_bytes());
        out.extend_from_slice(component);
        StoredBody(out)
    }

    /// Bytes read back from a store.
    pub fn from_bytes(bytes: Vec<u8>) -> Option<StoredBody> {
        let body = StoredBody(bytes);
        body.shape()?;
        Some(body)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn shape(&self) -> Option<Shape<'_>> {
        let (kind, rest) = self.0.split_first()?;
        match *kind {
            KIND_SOLO => Some(Shape::Solo(rest)),
            KIND_MEMBER => {
                let index = rest.get(..4)?;
                let class = u32::from_le_bytes(index.try_into().ok()?);
                Some(Shape::Member {
                    class,
                    payload: &rest[4..],
                })
            }
            _ => None,
        }
    }

    /// The one `DefHash` these bytes may be filed under.
    pub fn key(&self) -> Option<DefHash> {
        match self.shape()? {
            Shape::Solo(bytes) => Some(DefHash::of(bytes)),
            Shape::Member { class, payload } => Some(member_hash(DefHash::of(payload), class)),
        }
    }

    pub fn verify(&self, hash: DefHash) -> bool {
        self.key() == Some(hash)
    }

    pub fn component(&self) -> Option<(DefHash, Vec<DefHash>)> {
        match self.shape()? {
            Shape::Solo(bytes) => {
                let id = DefHash::of(bytes);
                Some((id, vec![id]))
            }
            Shape::Member { payload, .. } => {
                let id = DefHash::of(payload);
                let count = u32::from_le_bytes(payload.get(..4)?.try_into().ok()?);
                if count as usize > payload.len() / 4 {
                    return None;
                }
                Some((id, (0..count).map(|class| member_hash(id, class)).collect()))
            }
        }
    }
}

/// The bodies out of a front end's answer, keyed the way the store files them.
///
/// The inverse of `ply_codegen::source`'s `fill_bodies`, which writes each body as
/// [`StoredBody::as_bytes`]; `from_bytes` reads that envelope back, so `key()` re-derives the
/// hash the definition is filed under rather than being told it. A name declared in two
/// namespaces has two bodies and one entry per hash, which is the case `verify` settles.
pub fn of_front(front: &ply_ty::Front) -> BodySet {
    let hashes = &front.hashes;
    let mut by_name: std::collections::BTreeMap<&Symbol, Vec<StoredBody>> = Default::default();
    for (name, bytes) in &front.bodies {
        if let Some(body) = StoredBody::from_bytes(bytes.clone()) {
            by_name.entry(name).or_default().push(body);
        }
    }
    let mut out = BodySet::default();
    for (name, stored) in by_name {
        for hash in [hashes.defs.get(name), hashes.decls.get(name)]
            .into_iter()
            .flatten()
        {
            let found = match stored.as_slice() {
                [only] => Some(only),
                many => many.iter().find(|b| b.verify(*hash)),
            };
            if let Some(body) = found {
                out.insert(*hash, body.clone());
            }
        }
    }
    // Parallel to `CheckOutput::tests`, which the protocol checks when it decodes the frames.
    for bytes in &front.test_bodies {
        if let Some(body) = StoredBody::from_bytes(bytes.clone()) {
            out.push_test(body);
        }
    }
    out
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BodySet {
    defs: IndexMap<DefHash, StoredBody>,
    /// Parallel to [`crate::HashOutput::tests`].
    tests: Vec<StoredBody>,
}

impl BodySet {
    pub fn insert(&mut self, hash: DefHash, body: StoredBody) {
        self.defs.insert(hash, body);
    }

    pub fn push_test(&mut self, body: StoredBody) {
        self.tests.push(body);
    }

    pub fn get(&self, hash: DefHash) -> Option<&StoredBody> {
        self.defs.get(&hash)
    }

    pub fn contains(&self, hash: DefHash) -> bool {
        self.defs.contains_key(&hash)
    }

    pub fn defs(&self) -> impl Iterator<Item = (DefHash, &StoredBody)> {
        self.defs.iter().map(|(h, b)| (*h, b))
    }

    pub fn tests(&self) -> &[StoredBody] {
        &self.tests
    }

    pub fn len(&self) -> usize {
        self.defs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty() && self.tests.is_empty()
    }
}
