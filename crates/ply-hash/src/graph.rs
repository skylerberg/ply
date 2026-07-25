//! The reference graph over a module's top-level definitions, and the strongly
//! connected components that decide which definitions must be hashed together.

use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::{EffectDef, FnDef, Ident, Item, Module, TestDef, TypeDef, TypeDefBody};
use rustc_hash::FxHashMap;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct NodeId(pub usize);

#[derive(Clone, Copy, Debug)]
pub enum NodeBody<'a> {
    Fn(&'a FnDef),
    Type(&'a TypeDef),
    Effect(&'a EffectDef),
}

#[derive(Clone, Copy, Debug)]
pub struct Node<'a> {
    pub name: &'a Symbol,
    /// Position in `Module::items`, which fixes the order of the output but
    /// never enters a hash.
    pub item: usize,
    pub body: NodeBody<'a>,
}

pub struct ModuleIndex<'a> {
    pub nodes: Vec<Node<'a>>,
    pub fns: FxHashMap<Symbol, NodeId>,
    pub types: FxHashMap<Symbol, NodeId>,
    pub effects: FxHashMap<Symbol, NodeId>,
    /// Variant name -> the type definition that declares it.
    pub ctors: FxHashMap<Symbol, NodeId>,
    pub tests: Vec<(usize, &'a TestDef)>,
    /// What separates an effect from the other effects declaring exactly the
    /// same operations, since structure alone cannot.
    pub effect_ids: FxHashMap<usize, u32>,
}

impl<'a> ModuleIndex<'a> {
    pub fn build(module: &'a Module) -> Result<ModuleIndex<'a>, Vec<Diagnostic>> {
        let mut idx = ModuleIndex {
            nodes: Vec::new(),
            fns: FxHashMap::default(),
            types: FxHashMap::default(),
            effects: FxHashMap::default(),
            ctors: FxHashMap::default(),
            tests: Vec::new(),
            effect_ids: FxHashMap::default(),
        };
        let mut diags = Vec::new();
        let mut fn_spans: FxHashMap<Symbol, Span> = FxHashMap::default();
        let mut type_spans: FxHashMap<Symbol, Span> = FxHashMap::default();
        let mut effect_spans: FxHashMap<Symbol, Span> = FxHashMap::default();
        let mut ctor_spans: FxHashMap<Symbol, Span> = FxHashMap::default();

        for (item_ix, item) in module.items.iter().enumerate() {
            match item {
                Item::Fn(d) => {
                    let id = NodeId(idx.nodes.len());
                    idx.nodes.push(Node { name: &d.name.name, item: item_ix, body: NodeBody::Fn(d) });
                    declare(&mut idx.fns, &mut fn_spans, &d.name, id, "definition", &mut diags);
                }
                Item::Type(d) => {
                    let id = NodeId(idx.nodes.len());
                    idx.nodes.push(Node { name: &d.name.name, item: item_ix, body: NodeBody::Type(d) });
                    declare(&mut idx.types, &mut type_spans, &d.name, id, "type", &mut diags);
                    if let TypeDefBody::Sum(variants) = &d.body {
                        for v in variants {
                            declare(&mut idx.ctors, &mut ctor_spans, &v.name, id, "variant", &mut diags);
                        }
                    }
                }
                Item::Effect(d) => {
                    let id = NodeId(idx.nodes.len());
                    idx.nodes.push(Node { name: &d.name.name, item: item_ix, body: NodeBody::Effect(d) });
                    declare(&mut idx.effects, &mut effect_spans, &d.name, id, "effect", &mut diags);
                }
                Item::Test(d) => idx.tests.push((item_ix, d)),
            }
        }

        if !diags.is_empty() {
            return Err(diags);
        }
        idx.effect_ids = crate::normalize::effect_disambiguators(&idx);
        Ok(idx)
    }
}

fn declare(
    map: &mut FxHashMap<Symbol, NodeId>,
    spans: &mut FxHashMap<Symbol, Span>,
    name: &Ident,
    id: NodeId,
    what: &str,
    diags: &mut Vec<Diagnostic>,
) {
    if let Some(&first) = spans.get(&name.name) {
        diags.push(
            Diagnostic::error(
                codes::DUPLICATE_DEFINITION,
                format!("duplicate {what} `{}`", name.name),
            )
            .primary(name.span, "redefined here")
            .secondary(first, "first defined here")
            .note(format!("rename one of them; every {what} in a module needs a distinct name")),
        );
        return;
    }
    spans.insert(name.name.clone(), name.span);
    map.insert(name.name.clone(), id);
}

/// Tarjan's algorithm, iterative so that a deep dependency chain cannot blow the
/// stack. Components come out in reverse topological order: a component is
/// emitted only after every component it references, which is exactly the order
/// definitions have to be hashed in.
pub fn tarjan(n: usize, edges: &[Vec<NodeId>]) -> Vec<Vec<usize>> {
    const UNVISITED: usize = usize::MAX;
    let mut index = vec![UNVISITED; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut next = 0usize;
    let mut out: Vec<Vec<usize>> = Vec::new();
    let mut call: Vec<(usize, usize)> = Vec::new();

    for root in 0..n {
        if index[root] != UNVISITED {
            continue;
        }
        index[root] = next;
        low[root] = next;
        next += 1;
        stack.push(root);
        on_stack[root] = true;
        call.push((root, 0));

        while let Some(&mut (v, ref mut edge_ix)) = call.last_mut() {
            let outgoing = edges.get(v).map(|e| e.as_slice()).unwrap_or(&[]);
            if *edge_ix < outgoing.len() {
                let w = outgoing[*edge_ix].0;
                *edge_ix += 1;
                if w >= n {
                    continue;
                }
                if index[w] == UNVISITED {
                    index[w] = next;
                    low[w] = next;
                    next += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    call.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
            } else {
                call.pop();
                if let Some(&(parent, _)) = call.last() {
                    low[parent] = low[parent].min(low[v]);
                }
                if low[v] == index[v] {
                    let mut component = Vec::new();
                    while let Some(w) = stack.pop() {
                        on_stack[w] = false;
                        component.push(w);
                        if w == v {
                            break;
                        }
                    }
                    out.push(component);
                }
            }
        }
    }
    out
}

pub fn is_cyclic(component: &[usize], edges: &[Vec<NodeId>]) -> bool {
    match component {
        [only] => edges.get(*only).is_some_and(|e| e.iter().any(|r| r.0 == *only)),
        _ => true,
    }
}
