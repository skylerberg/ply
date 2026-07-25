//! Content addressing: a definition's identity is its normalized structure, so
//! a name is never part of it and renaming rebuilds nothing.

pub mod graph;
pub mod normalize;

#[cfg(test)]
mod tests;

use indexmap::IndexMap;
use ply_span::{Diagnostic, Symbol};
use ply_syntax::ast::{Module, Program};
use ply_syntax::resolve::Resolved;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeSet;
use std::fmt;

use graph::{Entry, NodeBody, NodeId, ProgramIndex};
use normalize::{ComponentIndices, EffectIndex, HashTable, Normalizer};

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

/// Every map is keyed by the program-wide name — `store.orders.place`, and
/// `<module>.<label>` for a test. Those keys are namespace metadata: they change
/// when a definition moves and the hashes they point at do not.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HashOutput {
    pub defs: IndexMap<Symbol, DefHash>,
    /// `type` and `effect` declarations, which `defs` deliberately omits — only a
    /// `fn` is a definition a test can be selected on. They are hashed all the
    /// same, because a cached interface has to be keyed by one.
    pub decls: IndexMap<Symbol, DefHash>,
    pub tests: Vec<DefHash>,
    /// Direct references, definition name -> names it mentions.
    pub deps: IndexMap<Symbol, Vec<Symbol>>,
    /// Transitive closure, including the definition itself.
    pub closure: IndexMap<Symbol, BTreeSet<Symbol>>,
}

/// Hashes every module of a program at once. A cross-module reference is
/// normalized to the referent's hash exactly as a same-module one is, so nothing
/// a module system introduces — its name, its imports, its `pub` markers — can
/// reach a hash.
pub fn hash_program(
    program: &Program,
    resolved: &Resolved,
    _check: &ply_core::CheckOutput,
) -> Result<HashOutput, Vec<Diagnostic>> {
    hash_program_ast(program, resolved)
}

/// [`hash_program`] without the type-check output, which normalization does not
/// need: a hash is a function of resolved source structure alone.
pub fn hash_program_ast(
    program: &Program,
    resolved: &Resolved,
) -> Result<HashOutput, Vec<Diagnostic>> {
    hash_index(ProgramIndex::of_program(program, resolved)?)
}

/// One module with nothing imported. Convenience for snippets and tests.
pub fn hash_module(
    module: &Module,
    _check: &ply_core::CheckOutput,
) -> Result<HashOutput, Vec<Diagnostic>> {
    hash_ast(module)
}

/// [`hash_module`] without the type-check output.
pub fn hash_ast(module: &Module) -> Result<HashOutput, Vec<Diagnostic>> {
    hash_index(ProgramIndex::single(module)?)
}

fn hash_index(index: ProgramIndex<'_>) -> Result<HashOutput, Vec<Diagnostic>> {
    let n = index.nodes.len();
    let no_hashes = HashTable::default();
    let no_component = ComponentIndices::default();
    let no_effects = EffectIndex::default();

    // What a definition references does not depend on what any of them hash to,
    // so one pass with nothing known yields the reference graph, plus a sketch
    // that orders a cycle's members without appealing to source position.
    let mut edges: Vec<Vec<NodeId>> = Vec::with_capacity(n);
    let mut sketches: Vec<Vec<u8>> = Vec::with_capacity(n);
    for node in &index.nodes {
        let mut nz = Normalizer::new(&index, node.module, &no_hashes, &no_component, &no_effects);
        nz.node(node.body);
        let (bytes, refs) = nz.finish();
        edges.push(refs);
        sketches.push(bytes);
    }

    let components = graph::tarjan(n, &edges);
    let mut component_of = vec![usize::MAX; n];
    for (ci, component) in components.iter().enumerate() {
        for &v in component {
            component_of[v] = ci;
        }
    }

    // Components arrive dependency-first, so a component's own enumeration can
    // splice in the ones it references.
    let mut orders: Vec<Vec<usize>> = Vec::with_capacity(components.len());
    for (ci, component) in components.iter().enumerate() {
        let mut members = component.clone();
        members.sort_by(|&a, &b| sketches[a].cmp(&sketches[b]).then(a.cmp(&b)));
        let mut order = Vec::new();
        for &v in &members {
            effect_order(&index, &edges[v], &component_of, &orders, Some(ci), &mut order);
        }
        orders.push(order);
    }

    let mut hashes = HashTable::default();
    for (ci, component) in components.iter().enumerate() {
        let effects = slots(&orders[ci]);
        if graph::is_cyclic(component, &edges) {
            for (v, hash) in hash_component(&index, component, &hashes, &effects) {
                hashes.insert(v, hash);
            }
        } else {
            let v = component[0];
            let module = index.nodes[v].module;
            let mut nz = Normalizer::new(&index, module, &hashes, &no_component, &effects);
            nz.node(index.nodes[v].body);
            hashes.insert(v, DefHash::of(&nz.finish().0));
        }
    }

    let mut test_hashes = Vec::with_capacity(index.tests.len());
    let mut test_refs = Vec::with_capacity(index.tests.len());
    for test in &index.tests {
        let mut nz = Normalizer::new(&index, test.module, &no_hashes, &no_component, &no_effects);
        nz.test_def(test.def);
        let refs = nz.finish().1;
        let mut order = Vec::new();
        effect_order(&index, &refs, &component_of, &orders, None, &mut order);
        let effects = slots(&order);

        let mut nz = Normalizer::new(&index, test.module, &hashes, &no_component, &effects);
        nz.test_def(test.def);
        let (bytes, refs) = nz.finish();
        test_hashes.push(DefHash::of(&bytes));
        test_refs.push(refs);
    }

    Ok(assemble(&index, &components, &edges, &hashes, test_hashes, test_refs))
}

/// The effects one component can see, in an order derived only from what it
/// references — never from a name, a module, or a source position.
///
/// A referent contributes its own enumeration before this component's own
/// mentions are added, which is what anchors a slot to the *referent*. Without
/// that anchor two definitions that handle different effects around the same
/// callee would both write slot 0 and collapse onto one hash. A referent's
/// enumeration is a function of its hash, so a slot means the same thing in
/// every program where that hash appears.
fn effect_order(
    index: &ProgramIndex<'_>,
    refs: &[NodeId],
    component_of: &[usize],
    orders: &[Vec<usize>],
    own: Option<usize>,
    out: &mut Vec<usize>,
) {
    let push = |node: usize, out: &mut Vec<usize>| {
        if !out.contains(&node) {
            out.push(node);
        }
    };
    for r in refs {
        if index.is_effect(*r) {
            push(r.0, out);
        }
        let ci = component_of.get(r.0).copied().unwrap_or(usize::MAX);
        if Some(ci) == own {
            continue;
        }
        let Some(inner) = orders.get(ci) else { continue };
        for &node in inner {
            push(node, out);
        }
    }
}

fn slots(order: &[usize]) -> EffectIndex {
    order.iter().enumerate().map(|(i, &node)| (node, i as u32)).collect()
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
    index: &ProgramIndex<'_>,
    component: &[usize],
    hashes: &HashTable,
    effects: &EffectIndex,
) -> Vec<(usize, DefHash)> {
    let encode = |classes: &ComponentIndices, v: usize| {
        let mut nz = Normalizer::new(index, index.nodes[v].module, hashes, classes, effects);
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
    index: &ProgramIndex<'_>,
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

    let mut out = HashOutput { tests: test_hashes, ..HashOutput::default() };
    for entry in &index.order {
        match *entry {
            Entry::Def(NodeId(v)) => {
                let name = index.nodes[v].name.clone();
                if let Some(hash) = hashes.get(&v) {
                    match index.nodes[v].body {
                        NodeBody::Fn(_) => out.defs.insert(name.clone(), *hash),
                        NodeBody::Type(_) | NodeBody::Effect(_) => {
                            out.decls.insert(name.clone(), *hash)
                        }
                    };
                }
                let deps = edges[v].iter().map(|r| index.nodes[r.0].name.clone()).collect();
                let closure = component_closure.get(component_of[v]).cloned().unwrap_or_default();
                record(&mut out, name, deps, closure);
            }
            Entry::Test(t) => {
                let name = index.tests[t].key.clone();
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
