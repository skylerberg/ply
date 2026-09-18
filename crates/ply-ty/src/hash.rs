//! What content addressing publishes: the definition hash, the keys derived from it, and the
//! table a hashed program answers. The hashing itself — normalizing a definition — is `ply-hash`'s;
//! this is the vocabulary the store, the test runner and the prover key on, kept where they can
//! read it without the hasher.

use crate::decl::SpecKind;
use crate::ty::Mode;
use indexmap::IndexMap;
use ply_span::Symbol;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeSet;
use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct DefHash(pub [u8; 32]);

impl DefHash {
    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(64);
        for b in self.0 {
            s.push(char::from_digit((b >> 4) as u32, 16).unwrap_or('0'));
            s.push(char::from_digit((b & 0xf) as u32, 16).unwrap_or('0'));
        }
        s
    }

    pub fn short(&self) -> String {
        self.to_hex()[..12].to_string()
    }

    pub fn from_hex(s: &str) -> Option<DefHash> {
        if s.len() != 64 {
            return None;
        }
        let bytes = s.as_bytes();
        let mut out = [0u8; 32];
        for (i, byte) in out.iter_mut().enumerate() {
            let hi = (bytes[2 * i] as char).to_digit(16)?;
            let lo = (bytes[2 * i + 1] as char).to_digit(16)?;
            *byte = ((hi << 4) | lo) as u8;
        }
        Some(DefHash(out))
    }

    pub fn of(bytes: &[u8]) -> DefHash {
        DefHash(*blake3::hash(bytes).as_bytes())
    }
}

impl fmt::Display for DefHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.short())
    }
}

/// Hex rather than 32 numbers: the on-disk cache is meant to be readable by hand, and a hash has to
/// work as a JSON object key.
impl Serialize for DefHash {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for DefHash {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        DefHash::from_hex(&s)
            .ok_or_else(|| serde::de::Error::custom(format!("malformed definition hash `{s}`")))
    }
}

/// Every map is keyed by the program-wide name — `store.orders.place`, and `<module>.<label>` for a
/// test.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HashOutput {
    pub defs: IndexMap<Symbol, DefHash>,
    /// `defs`, with each reference written as the referent's name rather than
    /// its hash, so this moves with a definition's own text and not with a
    /// callee's.
    ///
    /// Never an identity: two definitions calling different functions of the
    /// same shape share one.
    pub own: IndexMap<Symbol, DefHash>,
    /// `type` and `effect` declarations, which `defs` deliberately omits — only a `fn` is a
    /// definition a test can be selected on.
    pub decls: IndexMap<Symbol, DefHash>,
    pub tests: Vec<DefHash>,
    /// Parallel to `CheckOutput::laws`.
    pub laws: Vec<DefHash>,
    /// Definition program-wide name -> one hash per `requires` / `ensures` clause, in source order.
    pub specs: IndexMap<Symbol, Vec<DefHash>>,
    /// The same clauses as `specs`, identified as **sentences** rather than as obligations:
    /// references by name, and no owner hash in the stream.
    pub spec_texts: IndexMap<Symbol, Vec<DefHash>>,
    /// Parallel to `laws`, and a sentence identity for the same reason [`HashOutput::spec_texts`]
    /// is one.
    pub law_texts: Vec<DefHash>,
    /// Direct references, definition name -> names it mentions.
    pub deps: IndexMap<Symbol, Vec<Symbol>>,
    /// Transitive closure, including the definition itself.
    pub closure: IndexMap<Symbol, BTreeSet<Symbol>>,
}

/// Domain tag, so a spec's key cannot collide with a definition's own hash, which is `blake3` over
/// normalized bytes carrying no tag.
const SPEC_DOMAIN: &[u8] = b"ply.spec.1";

/// Domain tag for a claim's *sentence* identity, kept apart from [`SPEC_DOMAIN`] so that a review
/// baseline can never be mistaken for an obligation key.
const SPEC_TEXT_DOMAIN: &[u8] = b"ply.spec.text.1";

/// Domain tag for [`HashOutput::own`]. A definition's own-form key is one hash
/// of one definition's bytes and so is its `DefHash`; tagging is what stops the
/// two being interchangeable at a call site that takes `DefHash` and means
/// identity.
const OWN_DOMAIN: &[u8] = b"ply.own.1";

/// The identity of a claim as written, for [`HashOutput::spec_texts`] and
/// [`HashOutput::law_texts`].
pub fn spec_text_hash(kind: Option<SpecKind>, index: u32, normalized: &[u8]) -> DefHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(SPEC_TEXT_DOMAIN);
    hasher.update(&[kind.map_or(0, |k| k.tag())]);
    hasher.update(&index.to_le_bytes());
    hasher.update(normalized);
    DefHash(*hasher.finalize().as_bytes())
}

/// The key for [`HashOutput::own`], over a definition's by-name encoding.
pub fn own_hash(normalized: &[u8]) -> DefHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(OWN_DOMAIN);
    hasher.update(normalized);
    DefHash(*hasher.finalize().as_bytes())
}

/// The key an obligation attached to a definition is discharged under.
pub fn spec_hash(owner: DefHash, kind: SpecKind, index: u32, normalized: &[u8]) -> DefHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(SPEC_DOMAIN);
    hasher.update(&owner.0);
    hasher.update(&[kind.tag()]);
    hasher.update(&index.to_le_bytes());
    hasher.update(normalized);
    DefHash(*hasher.finalize().as_bytes())
}

/// How an access mode is written in a hashed stream.
pub fn mode_byte(mode: Mode) -> u8 {
    match mode {
        Mode::Read => 0,
        Mode::Write => 1,
    }
}
