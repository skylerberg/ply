//! Content addressing: a definition's identity is its normalized structure, so
//! a name is never part of it and renaming rebuilds nothing.

pub mod graph;
pub mod normalize;

#[cfg(test)]
mod tests;

use indexmap::IndexMap;
use ply_span::{Diagnostic, Symbol};
use ply_syntax::ast::Module;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeSet;
use std::fmt;

use graph::{ModuleIndex, NodeBody, NodeId};
use normalize::{ComponentIndices, HashTable, Normalizer};

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

    fn of(bytes: &[u8]) -> DefHash {
        DefHash(*blake3::hash(bytes).as_bytes())
    }
}

impl fmt::Display for DefHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.short())
    }
}

/// Hex rather than 32 numbers: the on-disk cache is meant to be readable by
/// hand, and a hash has to work as a JSON object key.
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

/// `deps` and `closure` are keyed by name for every top-level definition — `fn`,
/// `type` and `effect` alike — and additionally by its declared name for every
/// test, which is how a failure is attributed to suspects.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HashOutput {
    pub defs: IndexMap<Symbol, DefHash>,
    pub tests: Vec<DefHash>,
    /// Direct references, definition name -> names it mentions.
    pub deps: IndexMap<Symbol, Vec<Symbol>>,
    /// Transitive closure, including the definition itself.
    pub closure: IndexMap<Symbol, BTreeSet<Symbol>>,
}


pub fn hash_module(
    module: &Module,
    _check: &ply_core::CheckOutput,
) -> Result<HashOutput, Vec<Diagnostic>> {
    hash_ast(module)
}

/// [`hash_module`] without the type-check output, which normalization does not
/// need: a hash is a function of the source structure alone.
pub fn hash_ast(module: &Module) -> Result<HashOutput, Vec<Diagnostic>> {
    let index = ModuleIndex::build(module)?;
    let n = index.nodes.len();
    let no_hashes = HashTable::default();
    let no_component = ComponentIndices::default();

    let mut edges: Vec<Vec<NodeId>> = Vec::with_capacity(n);
    for node in &index.nodes {
        let mut nz = Normalizer::new(&index, &no_hashes, &no_component);
        nz.node(node.body);
        edges.push(nz.finish().1);
    }

    let components = graph::tarjan(n, &edges);
    let mut hashes = HashTable::default();
    for component in &components {
        if graph::is_cyclic(component, &edges) {
            for (v, hash) in hash_component(&index, component, &hashes) {
                hashes.insert(v, hash);
            }
        } else {
            let v = component[0];
            let mut nz = Normalizer::new(&index, &hashes, &no_component);
            nz.node(index.nodes[v].body);
            hashes.insert(v, DefHash::of(&nz.finish().0));
        }
    }

    let mut test_hashes = Vec::with_capacity(index.tests.len());
    let mut test_refs = Vec::with_capacity(index.tests.len());
    for (_, test) in &index.tests {
        let mut nz = Normalizer::new(&index, &hashes, &no_component);
        nz.test_def(test);
        let (bytes, refs) = nz.finish();
        test_hashes.push(DefHash::of(&bytes));
        test_refs.push(refs);
    }

    Ok(assemble(&index, &components, &edges, &hashes, test_hashes, test_refs))
}

/// A cyclic component is hashed as a unit and each member identified by an index
/// within it. Source position cannot supply that index — moving a definition
/// would change its hash — so refinement does: start with every member in one
/// class, re-encode with each intra-component reference written as the
/// referent's current class, and split until nothing splits further.
///
/// Members that never split are the fixed point of that process, which for
/// bodies whose references are positional means their unfoldings agree at every
/// depth: `f(n) = g(n-1)` and `g(n) = f(n-1)` denote the same function and are
/// interchangeable at every call site. They share an index, and so a hash —
/// property 5 reaching inside a cycle, not a tie broken arbitrarily.
fn hash_component(
    index: &ModuleIndex<'_>,
    component: &[usize],
    hashes: &HashTable,
) -> Vec<(usize, DefHash)> {
    let encode = |classes: &ComponentIndices, v: usize| {
        let mut nz = Normalizer::new(index, hashes, classes);
        nz.node(index.nodes[v].body);
        nz.finish().0
    };

    let mut classes: ComponentIndices = component.iter().map(|&v| (v, 0u32)).collect();
    let mut class_count = 1;
    let mut encodings: Vec<Vec<u8>>;
    loop {
        encodings = component.iter().map(|&v| encode(&classes, v)).collect();

        let mut distinct: Vec<&[u8]> = encodings.iter().map(Vec::as_slice).collect();
        distinct.sort_unstable();
        distinct.dedup();
        classes = component
            .iter()
            .zip(&encodings)
            .map(|(&v, e)| {
                (v, distinct.binary_search(&e.as_slice()).unwrap_or(0) as u32)
            })
            .collect();

        // A round that splits nothing never will, and once every member is
        // alone in its class there is nothing left to split.
        if distinct.len() == class_count || distinct.len() == component.len() {
            break;
        }
        class_count = distinct.len();
    }

    let mut sorted: Vec<&[u8]> = encodings.iter().map(Vec::as_slice).collect();
    sorted.sort_unstable();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(sorted.len() as u32).to_le_bytes());
    for member in sorted {
        bytes.extend_from_slice(&(member.len() as u32).to_le_bytes());
        bytes.extend_from_slice(member);
    }
    let component_hash = DefHash::of(&bytes);

    component
        .iter()
        .map(|&v| {
            let mut hasher = blake3::Hasher::new();
            hasher.update(&component_hash.0);
            hasher.update(&classes[&v].to_le_bytes());
            (v, DefHash(*hasher.finalize().as_bytes()))
        })
        .collect()
}

fn assemble(
    index: &ModuleIndex<'_>,
    components: &[Vec<usize>],
    edges: &[Vec<NodeId>],
    hashes: &HashTable,
    test_hashes: Vec<DefHash>,
    test_refs: Vec<Vec<NodeId>>,
) -> HashOutput {
    let mut component_of = vec![usize::MAX; index.nodes.len()];
    for (ci, component) in components.iter().enumerate() {
        for &v in component {
            component_of[v] = ci;
        }
    }

    // Components arrive dependency-first, so every closure a component needs has
    // already been built by the time it is reached.
    let mut component_closure: Vec<BTreeSet<Symbol>> = Vec::with_capacity(components.len());
    for (ci, component) in components.iter().enumerate() {
        let mut closure = BTreeSet::new();
        for &v in component {
            closure.insert(index.nodes[v].name.clone());
        }
        for &v in component {
            for r in &edges[v] {
                if component_of[r.0] != ci
                    && let Some(inner) = component_closure.get(component_of[r.0])
                {
                    closure.extend(inner.iter().cloned());
                }
            }
        }
        component_closure.push(closure);
    }

    let node_of_item: FxHashMap<usize, usize> =
        index.nodes.iter().enumerate().map(|(v, node)| (node.item, v)).collect();
    let test_of_item: FxHashMap<usize, usize> =
        index.tests.iter().enumerate().map(|(t, (item, _))| (*item, t)).collect();

    let mut out = HashOutput { tests: test_hashes, ..HashOutput::default() };
    for item in 0..(index.nodes.len() + index.tests.len()) {
        if let Some(&v) = node_of_item.get(&item) {
            let name = index.nodes[v].name.clone();
            if let (NodeBody::Fn(_), Some(hash)) = (index.nodes[v].body, hashes.get(&v)) {
                out.defs.insert(name.clone(), *hash);
            }
            let deps = edges[v].iter().map(|r| index.nodes[r.0].name.clone()).collect();
            let closure = component_closure.get(component_of[v]).cloned().unwrap_or_default();
            record(&mut out, name, deps, closure);
        } else if let Some(&t) = test_of_item.get(&item) {
            let name = Symbol::new(&index.tests[t].1.name);
            let deps = test_refs[t].iter().map(|r| index.nodes[r.0].name.clone()).collect();
            let mut closure = BTreeSet::new();
            closure.insert(name.clone());
            for r in &test_refs[t] {
                if let Some(inner) = component_closure.get(component_of[r.0]) {
                    closure.extend(inner.iter().cloned());
                }
            }
            record(&mut out, name, deps, closure);
        }
    }
    out
}

/// Names collide across namespaces — a `type` and a `fn` may share one, as may
/// two tests — so entries are merged rather than one silently replacing the
/// other. A merged closure over-approximates the suspects, which is safe;
/// dropping one would not be.
fn record(out: &mut HashOutput, name: Symbol, deps: Vec<Symbol>, closure: BTreeSet<Symbol>) {
    match out.deps.get_mut(&name) {
        Some(existing) => {
            for d in deps {
                if !existing.contains(&d) {
                    existing.push(d);
                }
            }
        }
        None => {
            out.deps.insert(name.clone(), deps);
        }
    }
    out.closure.entry(name).or_default().extend(closure);
}
