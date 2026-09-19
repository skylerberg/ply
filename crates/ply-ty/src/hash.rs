//! What content addressing publishes: the definition hash, the keys derived from it, and the
//! table a hashed program answers. The hashing itself is `ply-hash`'s.

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

/// Hex, so a hash can be a JSON object key.
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

/// Every map is keyed by the program-wide name; a test's is `<module>.<label>`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HashOutput {
    pub defs: IndexMap<Symbol, DefHash>,
    /// `defs` with references by name, so it moves only with a definition's own text.
    pub own: IndexMap<Symbol, DefHash>,
    /// `type` and `effect` declarations; `defs` holds only `fn`s, which a test can be selected on.
    pub decls: IndexMap<Symbol, DefHash>,
    pub tests: Vec<DefHash>,
    /// Parallel to `CheckOutput::laws`.
    pub laws: Vec<DefHash>,
    /// Definition program-wide name -> one hash per `requires` / `ensures` clause, in source order.
    pub specs: IndexMap<Symbol, Vec<DefHash>>,
    /// The same clauses as `specs`, as sentences: references by name, and no owner hash.
    pub spec_texts: IndexMap<Symbol, Vec<DefHash>>,
    /// Parallel to `laws`; a sentence identity like [`HashOutput::spec_texts`].
    pub law_texts: Vec<DefHash>,
    /// Direct references, definition name -> names it mentions.
    pub deps: IndexMap<Symbol, Vec<Symbol>>,
    /// Transitive closure, including the definition itself.
    pub closure: IndexMap<Symbol, BTreeSet<Symbol>>,
}

// Domain tags keep keys hashed over the same bytes from being interchangeable.
const SPEC_DOMAIN: &[u8] = b"ply.spec.1";
const SPEC_TEXT_DOMAIN: &[u8] = b"ply.spec.text.1";
const OWN_DOMAIN: &[u8] = b"ply.own.1";
const HASHES_DOMAIN: &[u8] = b"ply.hashes.1";

impl HashOutput {
    /// Every published hash in order: what a compiled unit and the machine entering it agree on.
    pub fn digest(&self) -> DefHash {
        let mut hasher = blake3::Hasher::new();
        hasher.update(HASHES_DOMAIN);
        let count = |hasher: &mut blake3::Hasher, n: usize| {
            hasher.update(&(n as u64).to_le_bytes());
        };
        for named in [&self.defs, &self.decls] {
            count(&mut hasher, named.len());
            for (name, hash) in named {
                count(&mut hasher, name.as_str().len());
                hasher.update(name.as_str().as_bytes());
                hasher.update(&hash.0);
            }
        }
        count(&mut hasher, self.specs.len());
        for (name, clauses) in &self.specs {
            count(&mut hasher, name.as_str().len());
            hasher.update(name.as_str().as_bytes());
            count(&mut hasher, clauses.len());
            for hash in clauses {
                hasher.update(&hash.0);
            }
        }
        for listed in [&self.tests, &self.laws] {
            count(&mut hasher, listed.len());
            for hash in listed {
                hasher.update(&hash.0);
            }
        }
        DefHash(*hasher.finalize().as_bytes())
    }
}

/// The identity of a claim as written.
pub fn spec_text_hash(kind: Option<SpecKind>, index: u32, normalized: &[u8]) -> DefHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(SPEC_TEXT_DOMAIN);
    hasher.update(&[kind.map_or(0, |k| k.tag())]);
    hasher.update(&index.to_le_bytes());
    hasher.update(normalized);
    DefHash(*hasher.finalize().as_bytes())
}

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
