//! The facts about a program that the prover reads, indexed once per run.

use super::claims::{Claims, Code, Definition};
use crate::sort::Sort;
use crate::world::{Ctor, Decl, Signature, World};
use ply_span::Symbol;
use std::collections::{BTreeSet, HashMap};

pub struct Context<'a> {
    claims: Claims,
    world: &'a World,
    recursive: BTreeSet<Symbol>,
    /// A component of one definition calling itself: the shape induction unrolls.
    self_recursive: BTreeSet<Symbol>,
    /// The sum types a case split may range over.
    sums: BTreeSet<Symbol>,
    inhabited_types: BTreeSet<Symbol>,
    /// Nominal types whose declaration reaches a `Float`.
    float_types: BTreeSet<Symbol>,
}

impl<'a> Context<'a> {
    pub fn new(claims: Claims, world: &'a World) -> Context<'a> {
        // A split over a partial list is unsound, so a type the lowered claims count otherwise is
        // never split on; aliases and builtins have no constructors, so they are not sums at all.
        let sums: BTreeSet<Symbol> = world
            .decls()
            .filter(|d| {
                !d.variants.is_empty()
                    && claims
                        .sums
                        .get(&d.name)
                        .is_none_or(|n| *n == d.variants.len())
            })
            .map(|d| d.name.clone())
            .collect();
        let (recursive, self_recursive) = recursive_definitions(&claims.defs);
        let inhabited_types = inhabited_sum_types(world, &sums);
        let float_types = float_reaching_types(world);

        Context {
            claims,
            world,
            recursive,
            self_recursive,
            sums,
            inhabited_types,
            float_types,
        }
    }

    pub fn claims(&self) -> &Claims {
        &self.claims
    }

    /// Through its arguments, its fields, or its own declaration.
    pub fn reaches_float(&self, sort: &Sort) -> bool {
        reaches_float(sort, &self.float_types)
    }

    pub fn ctor(&self, name: &Symbol) -> Option<Ctor<'a>> {
        self.world.ctor(name)
    }

    /// `None` unless every constructor is in hand: a split over a partial list is unsound.
    pub fn variants(&self, type_name: &Symbol) -> Option<&'a Decl> {
        self.world
            .decl(type_name)
            .filter(|_| self.sums.contains(type_name))
    }

    pub fn signature(&self, name: &Symbol) -> Option<&'a Signature> {
        self.world.signature(name)
    }

    pub fn inhabited(&self, sort: &Sort) -> bool {
        match sort {
            Sort::Var(_) => true,
            Sort::Fn { ret, .. } => self.inhabited(ret),
            Sort::Record(fields) => fields.iter().all(|(_, t)| self.inhabited(t)),
            Sort::Con(name, _) => !self.sums.contains(name) || self.inhabited_types.contains(name),
        }
    }

    /// Whether equal calls must answer equally, so they may share one term.
    pub fn is_pure(&self, name: &Symbol) -> bool {
        let Some(signature) = self.world.signature(name) else {
            return false;
        };
        signature.pure
            && match &signature.sort {
                Sort::Fn { pure, .. } => *pure,
                _ => true,
            }
    }

    pub fn is_recursive(&self, name: &Symbol) -> bool {
        self.recursive.contains(name)
    }

    /// Not in a recursive component, with an empty footprint, and with a body the lowering reached.
    pub fn unfoldable(&self, name: &Symbol) -> Option<&Definition> {
        if self.recursive.contains(name) {
            return None;
        }
        self.reached_pure(name)
    }

    /// Calling only itself, with an empty footprint, and with a body the lowering reached: what
    /// induction may unroll once it has shown the recursion decreases.
    pub fn self_recursive(&self, name: &Symbol) -> Option<&Definition> {
        if !self.self_recursive.contains(name) {
            return None;
        }
        self.reached_pure(name)
    }

    fn reached_pure(&self, name: &Symbol) -> Option<&Definition> {
        if !self.world.signature(name)?.pure {
            return None;
        }
        self.claims
            .defs
            .get(name)
            .filter(|def| !matches!(def.body, Code::Unreached))
    }
}

fn reaches_float(sort: &Sort, declared: &BTreeSet<Symbol>) -> bool {
    match sort {
        Sort::Con(name, args) => {
            (name.as_str() == "Float" && args.is_empty())
                || declared.contains(name)
                || args.iter().any(|a| reaches_float(a, declared))
        }
        Sort::Fn { params, ret, .. } => {
            params.iter().any(|p| reaches_float(p, declared)) || reaches_float(ret, declared)
        }
        Sort::Record(fields) => fields.iter().any(|(_, f)| reaches_float(f, declared)),
        Sort::Var(_) => false,
    }
}

/// A least fixed point, so chains of declarations and recursive ones both settle.
fn float_reaching_types(world: &World) -> BTreeSet<Symbol> {
    let mut found: BTreeSet<Symbol> = BTreeSet::new();
    loop {
        let mut grew = false;
        for decl in world.decls() {
            if found.contains(&decl.name) {
                continue;
            }
            if decl
                .variants
                .iter()
                .any(|v| v.fields.iter().any(|f| reaches_float(f, &found)))
            {
                found.insert(decl.name.clone());
                grew = true;
            }
        }
        if !grew {
            return found;
        }
    }
}

/// Least fixed point: a type is inhabited once every field of some constructor is.
fn inhabited_sum_types(world: &World, sums: &BTreeSet<Symbol>) -> BTreeSet<Symbol> {
    let mut inhabited: BTreeSet<Symbol> = BTreeSet::new();
    loop {
        let mut grew = false;
        for decl in world.decls().filter(|d| sums.contains(&d.name)) {
            if inhabited.contains(&decl.name) {
                continue;
            }
            if decl.variants.iter().any(|v| {
                v.fields
                    .iter()
                    .all(|field| field_inhabited(field, sums, &inhabited))
            }) {
                inhabited.insert(decl.name.clone());
                grew = true;
            }
        }
        if !grew {
            return inhabited;
        }
    }
}

fn field_inhabited(sort: &Sort, sums: &BTreeSet<Symbol>, inhabited: &BTreeSet<Symbol>) -> bool {
    match sort {
        Sort::Var(_) => true,
        Sort::Fn { ret, .. } => field_inhabited(ret, sums, inhabited),
        Sort::Record(fields) => fields
            .iter()
            .all(|(_, t)| field_inhabited(t, sums, inhabited)),
        Sort::Con(name, _) if sums.contains(name) => inhabited.contains(name),
        Sort::Con(..) => true,
    }
}

/// Definitions in a call-graph cycle, and among them the ones whose cycle is only themselves;
/// Tarjan run iteratively so deep programs cannot overflow.
fn recursive_definitions(
    defs: &HashMap<Symbol, Definition>,
) -> (BTreeSet<Symbol>, BTreeSet<Symbol>) {
    let mut names: Vec<Symbol> = defs.keys().cloned().collect();
    names.sort();
    let index: HashMap<&Symbol, usize> = names.iter().enumerate().map(|(i, n)| (n, i)).collect();

    let edges: Vec<Vec<usize>> = names
        .iter()
        .map(|name| {
            defs[name]
                .refs
                .iter()
                .filter_map(|r| index.get(r).copied())
                .collect()
        })
        .collect();

    let mut recursive = BTreeSet::new();
    let mut alone = BTreeSet::new();
    for component in tarjan(&edges) {
        let self_loop = component.first().is_some_and(|&v| edges[v].contains(&v));
        if component.len() == 1 && self_loop {
            alone.insert(names[component[0]].clone());
        }
        if component.len() > 1 || self_loop {
            for v in component {
                recursive.insert(names[v].clone());
            }
        }
    }
    (recursive, alone)
}

fn tarjan(edges: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let n = edges.len();
    let mut index = vec![usize::MAX; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut next = 0usize;
    let mut components = Vec::new();

    for root in 0..n {
        if index[root] != usize::MAX {
            continue;
        }
        let mut work: Vec<(usize, usize)> = vec![(root, 0)];
        while let Some((v, child)) = work.pop() {
            if child == 0 {
                index[v] = next;
                low[v] = next;
                next += 1;
                stack.push(v);
                on_stack[v] = true;
            }
            let mut recursed = false;
            for (i, &w) in edges[v].iter().enumerate().skip(child) {
                if index[w] == usize::MAX {
                    work.push((v, i + 1));
                    work.push((w, 0));
                    recursed = true;
                    break;
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
            }
            if recursed {
                continue;
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
                components.push(component);
            }
            if let Some(&(parent, _)) = work.last() {
                low[parent] = low[parent].min(low[v]);
            }
        }
    }
    components
}
