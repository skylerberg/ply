//! Turns stored body bytes back into definitions, refusing bytes their key does not name.

use ply_hash::body::{BodySet, Reconstruction, StoredBody, reconstruct};
use ply_span::Diagnostic;
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

    /// Definitions get synthesized names, so a historical set rebuilds without today's names.
    pub fn reconstruct(
        &self,
        hashes: impl IntoIterator<Item = DefHash>,
    ) -> Result<Reconstruction, Vec<Diagnostic>> {
        let (set, missing) = self.body_set(hashes);
        if !missing.is_empty() {
            let named: Vec<String> = missing.iter().take(8).map(|h| h.short()).collect();
            return Err(vec![
                Diagnostic::warning(
                    crate::codes::CACHE_UNREADABLE,
                    format!(
                        "{} of the definitions asked for have no stored body",
                        missing.len()
                    ),
                )
                .note(format!("missing: {}", named.join(", ")))
                .note("a definition gets a body only after a run that checked it"),
            ]);
        }
        reconstruct(&set)
    }
}
