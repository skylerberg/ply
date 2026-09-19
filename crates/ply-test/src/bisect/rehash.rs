//! Deciding `Edited` versus `Derived`, exactly: today's bodies hashed as the baseline wrote their
//! references.

use super::{Baseline, DefKey, Ns};
use ply_hash::DefHash;
use ply_span::Symbol;
use ply_span::frames::Cursor;
use std::collections::{BTreeMap, BTreeSet};

/// Every definition and test of the current program, hashed against the baseline era's table.
#[derive(Clone, Debug, Default)]
pub struct Rehashed {
    fresh: BTreeMap<DefKey, DefHash>,
    tests: BTreeMap<Symbol, DefHash>,
    /// Every identity the era's table assigns, which is what a rename has to be recognized against.
    image: BTreeSet<DefHash>,
    /// The mutually recursive definitions, by component.
    components: BTreeMap<DefKey, usize>,
}

impl Rehashed {
    /// `sources` are the current program's modules as the front end read them.
    pub fn under(sources: &[(String, String)], baseline: &Baseline) -> Result<Rehashed, String> {
        let pins: Vec<(String, bool, DefHash)> = baseline
            .keys()
            .filter_map(|key| Some((key.name.to_string(), key.is_decl(), baseline.hash_of(&key)?)))
            .collect();
        let dump =
            ply_codegen::c::producer::rehash_dump(sources, &pins).map_err(|e| format!("{e:#}"))?;
        read(&dump)
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

    /// `key`'s strongly connected component, `key` included, when it is mutually recursive.
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

fn read(dump: &str) -> Result<Rehashed, String> {
    let mut out = Rehashed::default();
    let mut frames = Cursor::new(dump.as_bytes(), "frame");
    while !frames.done() {
        let (words, payload) = frames.unit()?;
        let mut fields: BTreeMap<&str, &str> = BTreeMap::new();
        let mut cursor = Cursor::new(payload, "field");
        while !cursor.done() {
            let (key, body) = cursor.unit()?;
            let [key] = key[..] else {
                return Err(format!("a field headed `{}`", key.join(" ")));
            };
            fields.insert(key, std::str::from_utf8(body).map_err(|e| e.to_string())?);
        }
        let hash = |field: &str| {
            fields
                .get(field)
                .copied()
                .and_then(DefHash::from_hex)
                .ok_or_else(|| format!("a `{}` frame with no `{field}` hash", words.join(" ")))
        };
        match words[..] {
            ["node", name] => {
                let ns = match fields.get("ns") {
                    Some(&"decl") => Ns::Decl,
                    _ => Ns::Value,
                };
                let key = DefKey {
                    name: Symbol::new(name),
                    ns,
                };
                out.fresh.insert(key.clone(), hash("fresh")?);
                out.image.insert(hash("table")?);
                if let Some(component) = fields.get("component") {
                    let id = component
                        .parse()
                        .map_err(|_| format!("component `{component}`"))?;
                    out.components.insert(key, id);
                }
            }
            ["test", _] => {
                let key = fields.get("key").ok_or("a test frame with no key")?;
                out.tests.insert(Symbol::new(key), hash("hash")?);
            }
            _ => return Err(format!("a `{}` frame", words.join(" "))),
        }
    }
    Ok(out)
}
