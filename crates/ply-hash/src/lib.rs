//! Content addressing: a definition's identity is its normalized structure, so
//! a name is never part of it and renaming rebuilds nothing.

pub mod body;
pub mod graph;
pub mod normalize;

#[cfg(test)]
mod tests;

use indexmap::IndexMap;
use ply_span::{Diagnostic, Symbol};
use ply_syntax::ast::{Module, Program, SpecKind};
use ply_syntax::resolve::Resolved;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeSet;
use std::fmt;

use body::{BodySet, StoredBody};
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
    /// Parallel to `CheckOutput::laws`. A law is an item with a body, so it is
    /// hashed like a test: its binder types, guard and body normalized together
    /// under their own discriminant.
    pub laws: Vec<DefHash>,
    /// Definition program-wide name -> one hash per `requires` / `ensures`
    /// clause, in source order.
    ///
    /// A spec is *erased* by normalization, so it appears in no entry of `defs`
    /// and adding one re-runs no test. It gets its own key instead, through
    /// [`spec_hash`], which covers the owner's hash — and therefore the owner's
    /// whole closure.
    pub specs: IndexMap<Symbol, Vec<DefHash>>,
    /// The same clauses as `specs`, identified as **sentences** rather than as
    /// obligations: references by name, and no owner hash in the stream.
    ///
    /// `specs` is what an obligation is *discharged* under, so it covers the
    /// owner and therefore moves whenever the implementation does — which is
    /// what re-opens a proof when the code it is about is rewritten. That makes
    /// it the wrong question for review, where "did the claim change?" must be
    /// answerable independently of "did the implementation change?". Without
    /// this, every implementation edit would also read as a spec edit and ADR
    /// 0007 §9.2's *implementation changed · spec unchanged* row — the cheapest
    /// review in the system — would be unreachable.
    pub spec_texts: IndexMap<Symbol, Vec<DefHash>>,
    /// Parallel to `laws`, and a sentence identity for the same reason
    /// [`HashOutput::spec_texts`] is one. A law's ordinary hash substitutes the
    /// hash of every definition it names, so re-implementing any of them moves
    /// it; the law as written has not changed.
    pub law_texts: Vec<DefHash>,
    /// Direct references, definition name -> names it mentions.
    pub deps: IndexMap<Symbol, Vec<Symbol>>,
    /// Transitive closure, including the definition itself.
    pub closure: IndexMap<Symbol, BTreeSet<Symbol>>,
}

/// Domain tag, so a spec's key cannot collide with a definition's own hash,
/// which is `blake3` over normalized bytes carrying no tag. The same device as
/// the component/index composition below and as `ply_test::sim_key`.
const SPEC_DOMAIN: &[u8] = b"ply.spec.1";

/// Domain tag for a claim's *sentence* identity, kept apart from [`SPEC_DOMAIN`]
/// so that a review baseline can never be mistaken for an obligation key.
const SPEC_TEXT_DOMAIN: &[u8] = b"ply.spec.text.1";

/// The identity of a claim as written, for [`HashOutput::spec_texts`] and
/// [`HashOutput::law_texts`].
///
/// Deliberately *not* a function of the owner's hash or of any referent's hash:
/// it moves when the sentence is rewritten and stays put when the definitions
/// it mentions are re-implemented.
fn spec_text_hash(kind: Option<SpecKind>, index: u32, normalized: &[u8]) -> DefHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(SPEC_TEXT_DOMAIN);
    hasher.update(&[kind.map_or(0, |k| k.tag())]);
    hasher.update(&index.to_le_bytes());
    hasher.update(normalized);
    DefHash(*hasher.finalize().as_bytes())
}

/// The key an obligation attached to a definition is discharged under.
///
/// `normalized` is the ordinary body encoding of the clause: the owner's
/// parameters and `result` as de Bruijn levels, a free reference contributing
/// the referent's hash, names and spans erased.
///
/// `owner` is first and is not optional. A key that omitted it would leave a
/// discharged `ensures` discharged after its definition was rewritten — a cached
/// proof of something no longer true, which is the permissive-direction failure
/// this milestone must not ship.
pub fn spec_hash(owner: DefHash, kind: SpecKind, index: u32, normalized: &[u8]) -> DefHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(SPEC_DOMAIN);
    hasher.update(&owner.0);
    hasher.update(&[kind.tag()]);
    hasher.update(&index.to_le_bytes());
    hasher.update(normalized);
    DefHash(*hasher.finalize().as_bytes())
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
    hash_index(ProgramIndex::of_program(program, resolved)?, None)
}

/// [`hash_program_ast`], keeping the normalized bytes it would otherwise throw
/// away. They *are* the stored body — a definition's identity and its stored
/// form are one byte stream — so collecting them costs a copy and nothing else.
pub fn hash_program_with_bodies(
    program: &Program,
    resolved: &Resolved,
) -> Result<(HashOutput, BodySet), Vec<Diagnostic>> {
    let mut bodies = BodySet::default();
    let hashes = hash_index(
        ProgramIndex::of_program(program, resolved)?,
        Some(&mut bodies),
    )?;
    Ok((hashes, bodies))
}

/// One module with nothing imported: a snippet, an editor buffer, a test.
///
/// A module that declares an `import` is refused with `E0106` — the referent is
/// not here to be hashed, and writing its name instead would silently alias two
/// different definitions. Use [`hash_program`] for anything that imports.
pub fn hash_module(
    module: &Module,
    _check: &ply_core::CheckOutput,
) -> Result<HashOutput, Vec<Diagnostic>> {
    hash_ast(module)
}

/// [`hash_module`] without the type-check output.
pub fn hash_ast(module: &Module) -> Result<HashOutput, Vec<Diagnostic>> {
    hash_index(ProgramIndex::single(module)?, None)
}

pub fn hash_ast_with_bodies(module: &Module) -> Result<(HashOutput, BodySet), Vec<Diagnostic>> {
    let mut bodies = BodySet::default();
    let hashes = hash_index(ProgramIndex::single(module)?, Some(&mut bodies))?;
    Ok((hashes, bodies))
}

fn hash_index(
    index: ProgramIndex<'_>,
    mut bodies: Option<&mut BodySet>,
) -> Result<HashOutput, Vec<Diagnostic>> {
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
            effect_order(
                &index,
                &edges[v],
                &component_of,
                &orders,
                Some(ci),
                &mut order,
            );
        }
        orders.push(order);
    }

    let mut hashes = HashTable::default();
    for (ci, component) in components.iter().enumerate() {
        let effects = slots(&orders[ci]);
        if graph::is_cyclic(component, &edges) {
            let (members, packed, classes) = hash_component(&index, component, &hashes, &effects);
            for (v, hash) in members {
                hashes.insert(v, hash);
                if let Some(bodies) = bodies.as_deref_mut() {
                    bodies.insert(hash, StoredBody::member(&packed, classes[&v]));
                }
            }
        } else {
            let v = component[0];
            let module = index.nodes[v].module;
            let mut nz = Normalizer::new(&index, module, &hashes, &no_component, &effects);
            nz.node(index.nodes[v].body);
            let encoding = nz.finish().0;
            let hash = DefHash::of(&encoding);
            hashes.insert(v, hash);
            if let Some(bodies) = bodies.as_deref_mut() {
                bodies.insert(hash, StoredBody::solo(&encoding));
            }
        }
    }

    let scc = Components {
        component_of: &component_of,
        orders: &orders,
        no_hashes: &no_hashes,
        no_component: &no_component,
        no_effects: &no_effects,
    };

    let mut test_hashes = Vec::with_capacity(index.tests.len());
    let mut test_refs = Vec::with_capacity(index.tests.len());
    for test in &index.tests {
        let (bytes, refs) = encode_item(&index, test.module, &scc, &hashes, |nz| {
            nz.test_def(test.def)
        });
        test_hashes.push(DefHash::of(&bytes));
        test_refs.push(refs);
        if let Some(bodies) = bodies.as_deref_mut() {
            bodies.push_test(StoredBody::solo(&bytes));
        }
    }

    // A law is an item with a body, hashed exactly as a test is. A spec clause
    // is not: it is a claim *about* a definition, so it is keyed by
    // `spec_hash`, whose first field is the owner's hash — which is what makes
    // editing an implementation re-open its obligations while editing a claim
    // moves no definition hash at all.
    let mut law_hashes = Vec::with_capacity(index.laws.len());
    let mut law_refs = Vec::with_capacity(index.laws.len());
    let mut law_texts = Vec::with_capacity(index.laws.len());
    for law in &index.laws {
        let (bytes, refs) =
            encode_item(&index, law.module, &scc, &hashes, |nz| nz.law_def(law.def));
        law_hashes.push(DefHash::of(&bytes));
        law_refs.push(refs);
        let (text, _) = encode_item_with(&index, law.module, &scc, &hashes, true, |nz| {
            nz.law_def(law.def)
        });
        law_texts.push(spec_text_hash(None, 0, &text));
    }

    let mut specs: IndexMap<Symbol, Vec<DefHash>> = IndexMap::new();
    let mut spec_texts: IndexMap<Symbol, Vec<DefHash>> = IndexMap::new();
    for entry in &index.order {
        let Entry::Def(NodeId(v)) = *entry else {
            continue;
        };
        let NodeBody::Fn(def) = index.nodes[v].body else {
            continue;
        };
        if def.spec.is_empty() {
            continue;
        }
        let Some(&owner) = hashes.get(&v) else {
            continue;
        };
        let module = index.nodes[v].module;
        let clauses = def
            .spec
            .iter()
            .enumerate()
            .map(|(i, clause)| {
                let (bytes, _) = encode_item(&index, module, &scc, &hashes, |nz| {
                    nz.spec_clause(def, clause)
                });
                spec_hash(owner, clause.kind, i as u32, &bytes)
            })
            .collect();
        let texts = def
            .spec
            .iter()
            .enumerate()
            .map(|(i, clause)| {
                let (bytes, _) = encode_item_with(&index, module, &scc, &hashes, true, |nz| {
                    nz.spec_clause(def, clause)
                });
                spec_text_hash(Some(clause.kind), i as u32, &bytes)
            })
            .collect();
        specs.insert(index.nodes[v].name.clone(), clauses);
        spec_texts.insert(index.nodes[v].name.clone(), texts);
    }

    Ok(assemble(
        &index,
        &components,
        &edges,
        &hashes,
        Hashed {
            tests: test_hashes,
            test_refs,
            laws: law_hashes,
            law_refs,
            law_texts,
            specs,
            spec_texts,
        },
    ))
}

/// One item with a body, encoded the way a test is: a first pass with nothing
/// known yields the references, which fix the effect enumeration, and a second
/// pass writes the bytes against the finished hash table.
fn encode_item<'a>(
    index: &'a ProgramIndex<'a>,
    module: usize,
    scc: &Components<'a>,
    hashes: &HashTable,
    encode: impl Fn(&mut Normalizer<'a, '_>),
) -> (Vec<u8>, Vec<NodeId>) {
    encode_item_with(index, module, scc, hashes, false, encode)
}

/// [`encode_item`], with the choice of how a reference is written. `by_name`
/// yields a *sentence* identity — see [`Normalizer::by_name`] — and is used only
/// for the review baselines, never for a hash anything is selected or cached on.
fn encode_item_with<'a>(
    index: &'a ProgramIndex<'a>,
    module: usize,
    scc: &Components<'a>,
    hashes: &HashTable,
    by_name: bool,
    encode: impl Fn(&mut Normalizer<'a, '_>),
) -> (Vec<u8>, Vec<NodeId>) {
    let mut nz = Normalizer::new(
        index,
        module,
        scc.no_hashes,
        scc.no_component,
        scc.no_effects,
    );
    encode(&mut nz);
    let refs = nz.finish().1;
    let mut order = Vec::new();
    effect_order(index, &refs, scc.component_of, scc.orders, None, &mut order);
    let effects = slots(&order);

    let mut nz = Normalizer::new(index, module, hashes, scc.no_component, &effects);
    if by_name {
        nz = nz.by_name();
    }
    encode(&mut nz);
    nz.finish()
}

/// The component data an item's encoding reads, plus the empty tables a first
/// pass runs against. Grouped so [`encode_item`] takes one borrow rather than
/// six.
struct Components<'a> {
    component_of: &'a [usize],
    orders: &'a [Vec<usize>],
    no_hashes: &'a HashTable,
    no_component: &'a ComponentIndices,
    no_effects: &'a EffectIndex,
}

/// What the item passes produced, so [`assemble`] takes one argument per idea
/// rather than five positional vectors.
struct Hashed {
    tests: Vec<DefHash>,
    test_refs: Vec<Vec<NodeId>>,
    laws: Vec<DefHash>,
    law_refs: Vec<Vec<NodeId>>,
    law_texts: Vec<DefHash>,
    specs: IndexMap<Symbol, Vec<DefHash>>,
    spec_texts: IndexMap<Symbol, Vec<DefHash>>,
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
        let Some(inner) = orders.get(ci) else {
            continue;
        };
        for &node in inner {
            push(node, out);
        }
    }
}

fn slots(order: &[usize]) -> EffectIndex {
    order
        .iter()
        .enumerate()
        .map(|(i, &node)| (node, i as u32))
        .collect()
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
) -> (Vec<(usize, DefHash)>, Vec<u8>, ComponentIndices) {
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
            .map(|(&v, e)| (v, distinct.binary_search(&e.as_slice()).unwrap_or(0) as u32))
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

    let members = component
        .iter()
        .map(|&v| (v, body::member_hash(component_hash, classes[&v])))
        .collect();
    (members, bytes, classes)
}

fn assemble(
    index: &ProgramIndex<'_>,
    components: &[Vec<usize>],
    edges: &[Vec<NodeId>],
    hashes: &HashTable,
    hashed: Hashed,
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

    let Hashed {
        tests,
        test_refs,
        laws,
        law_refs,
        law_texts,
        specs,
        spec_texts,
    } = hashed;
    let mut out = HashOutput {
        tests,
        laws,
        law_texts,
        specs,
        spec_texts,
        ..HashOutput::default()
    };
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
                let deps = edges[v]
                    .iter()
                    .map(|r| index.nodes[r.0].name.clone())
                    .collect();
                let closure = component_closure
                    .get(component_of[v])
                    .cloned()
                    .unwrap_or_default();
                record(&mut out, name, deps, closure);
            }
            Entry::Test(t) => {
                let name = index.tests[t].key.clone();
                let (deps, closure) = reached(
                    &test_refs[t],
                    index,
                    &component_of,
                    &component_closure,
                    &name,
                );
                record(&mut out, name, deps, closure);
            }
            // A law's references are what it is a claim *about*: `Laws::of`
            // reads them to decide which definitions one law covers, which is
            // why coverage is what a law names directly rather than what it can
            // reach.
            Entry::Law(l) => {
                let name = index.laws[l].key.clone();
                let (deps, closure) = reached(
                    &law_refs[l],
                    index,
                    &component_of,
                    &component_closure,
                    &name,
                );
                record(&mut out, name, deps, closure);
            }
        }
    }
    out
}

/// What a nameless item — a test or a law — directly references, and everything
/// its references can reach.
fn reached(
    refs: &[NodeId],
    index: &ProgramIndex<'_>,
    component_of: &[usize],
    component_closure: &[BTreeSet<Symbol>],
    own: &Symbol,
) -> (Vec<Symbol>, BTreeSet<Symbol>) {
    let deps = refs.iter().map(|r| index.nodes[r.0].name.clone()).collect();
    let mut closure = BTreeSet::new();
    closure.insert(own.clone());
    for r in refs {
        if let Some(inner) = component_closure.get(component_of[r.0]) {
            closure.extend(inner.iter().cloned());
        }
    }
    (deps, closure)
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
