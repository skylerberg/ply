//! Deciding `Edited` versus `Derived`, exactly.

use super::{DefKey, Ns};
use ply_hash::graph::{Entry, NodeBody, NodeId, ProgramIndex};
use ply_hash::normalize::{ComponentIndices, EffectIndex, HashTable, Normalizer};
use ply_hash::{DefHash, HashOutput};
use ply_span::{Diagnostic, Symbol};
use ply_syntax::ast::Program;
use ply_syntax::resolve::Resolved;
use std::collections::{BTreeMap, BTreeSet};

/// What every definition in the program hashed to in one era, node by node.
#[derive(Clone, Debug, Default)]
pub struct EraTable {
    table: HashTable,
}

impl EraTable {
    /// Every identity this era assigns, which is what a rename has to be recognized against.
    pub fn image(&self) -> BTreeSet<DefHash> {
        self.table.values().copied().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}

pub struct Renormalizer<'a> {
    index: ProgramIndex<'a>,
    components: Vec<Vec<usize>>,
    component_of: Vec<usize>,
    orders: Vec<Vec<usize>>,
    cyclic: Vec<bool>,
    nodes_by_name: BTreeMap<DefKey, usize>,
    tests_by_key: BTreeMap<Symbol, usize>,
    /// Effect slots for a test, which belongs to no component.
    test_orders: Vec<Vec<usize>>,
    witnessed: Vec<bool>,
    witnessed_tests: Vec<bool>,
}

impl<'a> Renormalizer<'a> {
    /// `test_keys` is what `hashes.tests` is parallel to — the run's `CheckOutput::tests`.
    pub fn new(
        program: &'a Program,
        resolved: &Resolved,
        hashes: &HashOutput,
        test_keys: &[Symbol],
    ) -> Result<Renormalizer<'a>, Vec<Diagnostic>> {
        let index = ProgramIndex::of_program(program, resolved)?;
        let n = index.nodes.len();

        let empty_hashes = HashTable::default();
        let empty_component = ComponentIndices::default();
        let empty_effects = EffectIndex::default();

        let mut edges: Vec<Vec<NodeId>> = Vec::with_capacity(n);
        let mut sketches: Vec<Vec<u8>> = Vec::with_capacity(n);
        for node in &index.nodes {
            let mut nz = Normalizer::new(
                &index,
                node.module,
                &empty_hashes,
                &empty_component,
                &empty_effects,
            );
            nz.node(node.body);
            let (bytes, refs) = nz.finish();
            edges.push(refs);
            sketches.push(bytes);
        }

        let components = ply_hash::graph::tarjan(n, &edges);
        let mut component_of = vec![usize::MAX; n];
        for (ci, component) in components.iter().enumerate() {
            for &v in component {
                component_of[v] = ci;
            }
        }

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

        let mut cyclic = vec![false; n];
        for component in &components {
            let is_cyclic = ply_hash::graph::is_cyclic(component, &edges);
            for &v in component {
                cyclic[v] = is_cyclic;
            }
        }

        let mut test_orders = Vec::with_capacity(index.tests.len());
        for test in &index.tests {
            let mut nz = Normalizer::new(
                &index,
                test.module,
                &empty_hashes,
                &empty_component,
                &empty_effects,
            );
            nz.test_def(test.def);
            let refs = nz.finish().1;
            let mut order = Vec::new();
            effect_order(&index, &refs, &component_of, &orders, None, &mut order);
            test_orders.push(order);
        }

        let mut nodes_by_name = BTreeMap::new();
        for (v, node) in index.nodes.iter().enumerate() {
            nodes_by_name.insert(key_of(node.name.clone(), node.body), v);
        }
        let tests_by_key = index
            .tests
            .iter()
            .enumerate()
            .map(|(t, test)| (test.key.clone(), t))
            .collect();

        let mut me = Renormalizer {
            index,
            components,
            component_of,
            orders,
            cyclic,
            nodes_by_name,
            tests_by_key,
            test_orders,
            witnessed: vec![false; n],
            witnessed_tests: Vec::new(),
        };
        me.witness(hashes, test_keys);
        Ok(me)
    }

    /// Whether this module reproduces `ply-hash`'s answer for `name`.
    pub fn witnessed(&self, name: &Symbol) -> bool {
        self.node_of(&DefKey::value(name.clone()))
            .or_else(|| self.node_of(&DefKey::decl(name.clone())))
            .is_some_and(|v| self.witnessed.get(v).copied().unwrap_or(false))
    }

    pub fn witnessed_test(&self, key: &Symbol) -> bool {
        self.tests_by_key
            .get(key)
            .is_some_and(|&t| self.witnessed_tests.get(t).copied().unwrap_or(false))
    }

    pub fn unwitnessed(&self) -> usize {
        self.witnessed.iter().filter(|w| !**w).count()
    }

    /// `key`'s current body, re-normalized with every reference written as `table` says rather than
    /// as it hashes now.
    pub fn rehash(&self, key: &DefKey, table: &EraTable) -> Option<DefHash> {
        let v = self.node_of(key)?;
        if !self.witnessed[v] {
            return None;
        }
        self.hash_node(v, &table.table)
    }

    pub fn rehash_test(&self, key: &Symbol, table: &EraTable) -> Option<DefHash> {
        let t = *self.tests_by_key.get(key)?;
        if !self.witnessed_tests.get(t).copied().unwrap_or(false) {
            return None;
        }
        self.hash_test(t, &table.table)
    }

    /// The other members of `key`'s strongly connected component, `key` included, when it is
    /// mutually recursive.
    pub fn component_of(&self, key: &DefKey) -> Vec<DefKey> {
        let Some(v) = self.node_of(key) else {
            return Vec::new();
        };
        if !self.cyclic[v] {
            return Vec::new();
        }
        let Some(component) = self.components.get(self.component_of[v]) else {
            return Vec::new();
        };
        if component.len() < 2 {
            return Vec::new();
        }
        let mut out: Vec<DefKey> = component
            .iter()
            .map(|&w| key_of(self.index.nodes[w].name.clone(), self.index.nodes[w].body))
            .collect();
        out.sort();
        out.dedup();
        out
    }

    fn node_of(&self, key: &DefKey) -> Option<usize> {
        self.nodes_by_name.get(key).copied()
    }

    /// What each node hashed to in the era `resolve` describes.
    pub fn era_table(&self, resolve: &dyn Fn(&DefKey) -> Option<DefHash>) -> EraTable {
        let mut table = HashTable::default();
        for (ci, component) in self.components.iter().enumerate() {
            let named: Vec<Option<DefHash>> = component
                .iter()
                .map(|&v| {
                    resolve(&key_of(
                        self.index.nodes[v].name.clone(),
                        self.index.nodes[v].body,
                    ))
                })
                .collect();
            if named.iter().all(Option::is_some) {
                for (&v, hash) in component.iter().zip(named) {
                    table.insert(v, hash.expect("checked just above"));
                }
                continue;
            }

            let effects = match self.orders.get(ci) {
                Some(order) => slots(order),
                None => EffectIndex::default(),
            };
            let computed: BTreeMap<usize, DefHash> = if component.iter().any(|&v| self.cyclic[v]) {
                ply_hash::component_hashes(&self.index, component, &table, &effects)
                    .into_iter()
                    .collect()
            } else {
                component
                    .iter()
                    .filter_map(|&v| Some((v, self.hash_node(v, &table)?)))
                    .collect()
            };
            for (&v, named) in component.iter().zip(named) {
                if let Some(hash) = named.or_else(|| computed.get(&v).copied()) {
                    table.insert(v, hash);
                }
            }
        }
        EraTable { table }
    }

    fn hash_node(&self, v: usize, table: &HashTable) -> Option<DefHash> {
        let ci = *self.component_of.get(v)?;
        let effects = slots(self.orders.get(ci)?);
        if self.cyclic[v] {
            let component = self.components.get(ci)?;
            return ply_hash::component_hashes(&self.index, component, table, &effects)
                .into_iter()
                .find(|(w, _)| *w == v)
                .map(|(_, hash)| hash);
        }
        let node = self.index.nodes.get(v)?;
        let alone = ComponentIndices::default();
        let mut nz = Normalizer::new(&self.index, node.module, table, &alone, &effects);
        nz.node(node.body);
        Some(digest(&nz.finish().0))
    }

    fn hash_test(&self, t: usize, table: &HashTable) -> Option<DefHash> {
        let test = self.index.tests.get(t)?;
        let effects = slots(self.test_orders.get(t)?);
        let alone = ComponentIndices::default();
        let mut nz = Normalizer::new(&self.index, test.module, table, &alone, &effects);
        nz.test_def(test.def);
        Some(digest(&nz.finish().0))
    }

    /// Re-normalizing against the hashes the program actually has must reproduce those hashes.
    fn witness(&mut self, hashes: &HashOutput, test_keys: &[Symbol]) {
        let published = |key: &DefKey| -> Option<DefHash> {
            match key.ns {
                Ns::Value => hashes.defs.get(&key.name).copied(),
                Ns::Decl => hashes.decls.get(&key.name).copied(),
            }
        };
        let table = self.era_table(&published).table;

        for entry in &self.index.order {
            if let Entry::Def(NodeId(v)) = *entry {
                let expected = match self.index.nodes[v].body {
                    NodeBody::Fn(_) => hashes.defs.get(&self.index.nodes[v].name),
                    NodeBody::Type(_) | NodeBody::Effect(_) => {
                        hashes.decls.get(&self.index.nodes[v].name)
                    }
                };
                self.witnessed[v] =
                    matches!((self.hash_node(v, &table), expected), (Some(a), Some(b)) if a == *b);
            }
        }

        let published_test: BTreeMap<&Symbol, DefHash> =
            test_keys.iter().zip(hashes.tests.iter().copied()).collect();
        let mut witnessed_tests = Vec::with_capacity(self.index.tests.len());
        for t in 0..self.index.tests.len() {
            let expected = published_test.get(&self.index.tests[t].key).copied();
            witnessed_tests.push(
                matches!((self.hash_test(t, &table), expected), (Some(a), Some(b)) if a == b),
            );
        }
        self.witnessed_tests = witnessed_tests;
    }
}

fn key_of(name: Symbol, body: NodeBody<'_>) -> DefKey {
    match body {
        NodeBody::Fn(_) => DefKey {
            name,
            ns: Ns::Value,
        },
        NodeBody::Type(_) | NodeBody::Effect(_) => DefKey { name, ns: Ns::Decl },
    }
}

fn digest(bytes: &[u8]) -> DefHash {
    DefHash(*blake3::hash(bytes).as_bytes())
}

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
