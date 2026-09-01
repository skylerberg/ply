//! The store's side of definition-body storage: turning the opaque bytes it keeps into the
//! definitions M5 has to check and evaluate, and refusing bytes that are not the ones their key
//! names.

use ply_hash::DefHash;
use ply_hash::body::{BodySet, Reconstruction, StoredBody, reconstruct};
use ply_span::Diagnostic;

use crate::{BODY_ENCODING, DefBody, Store};

impl DefBody {
    pub fn of(body: StoredBody) -> DefBody {
        DefBody::new(BODY_ENCODING, body.into_bytes())
    }

    /// `None` when this build does not speak the encoding, or when the bytes are not a body
    /// envelope at all.
    pub fn stored(&self) -> Option<StoredBody> {
        if self.encoding() != BODY_ENCODING {
            return None;
        }
        StoredBody::from_bytes(self.as_bytes().to_vec())
    }

    /// The one [`DefHash`] these bytes may be filed under.
    pub fn key(&self) -> Option<DefHash> {
        self.stored()?.key()
    }

    /// Whether these bytes are the body of the definition `hash` names.
    pub fn verifies_as(&self, hash: DefHash) -> bool {
        self.key() == Some(hash)
    }
}

impl Store {
    /// The caller supplies the closure rather than this walking one, because a body names its
    /// referents by hash and by nothing else — working out what a body reaches means decoding it,
    /// and a caller that wants a definition set already knows which one it wants.
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

    /// Its definitions carry synthesized names, which is the point: a historical definition set has
    /// to be rebuildable without knowing what anything is called now, because the names moved and
    /// the hashes did not.
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
