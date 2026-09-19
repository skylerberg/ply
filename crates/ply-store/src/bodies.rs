//! Stored body bytes, refusing bytes their key does not name.

use ply_hash::body::{BodySet, StoredBody};
use ply_ty::DefHash;

use crate::{BODY_ENCODING, DefBody, Store};

impl DefBody {
    pub fn of(body: StoredBody) -> DefBody {
        DefBody::new(BODY_ENCODING, body.into_bytes())
    }

    /// `None` for an encoding this build does not speak, or bytes that are not a body envelope.
    pub fn stored(&self) -> Option<StoredBody> {
        if self.encoding() != BODY_ENCODING {
            return None;
        }
        StoredBody::from_bytes(self.as_bytes().to_vec())
    }

    pub fn key(&self) -> Option<DefHash> {
        self.stored()?.key()
    }

    pub fn verifies_as(&self, hash: DefHash) -> bool {
        self.key() == Some(hash)
    }
}

impl Store {
    /// `hashes` must already be closed: finding what a body reaches means decoding it.
    pub fn body_set(&self, hashes: impl IntoIterator<Item = DefHash>) -> (BodySet, Vec<DefHash>) {
        let mut set = BodySet::default();
        let mut missing = Vec::new();
        for hash in hashes {
            match self.body(hash).and_then(|b| b.stored()) {
                Some(body) if body.verify(hash) => set.insert(hash, body),
                _ => missing.push(hash),
            }
        }
        (set, missing)
    }
}
