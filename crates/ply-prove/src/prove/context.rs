//! The facts about a program that the prover reads, indexed once per run.

use super::claims::{Claims, Code, Definition};
use ply_span::Symbol;
use ply_ty::{CheckOutput, CtorInfo, TyVar, Type};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// The constructors of one sum type, in declaration order.
pub struct Variants<'a> {
    pub type_name: Symbol,
    pub ctors: Vec<&'a CtorInfo>,
}

pub struct Context<'a> {
    claims: Claims,
    check: &'a CheckOutput,
    recursive: BTreeSet<Symbol>,
    by_type: BTreeMap<Symbol, Vec<Symbol>>,
    inhabited_types: BTreeSet<Symbol>,
    /// Nominal types whose declaration reaches a `Float`.
    float_types: BTreeSet<Symbol>,
    sort_names: BTreeMap<TyVar, Symbol>,
}

impl<'a> Context<'a> {
    pub fn new(claims: Claims, check: &'a CheckOutput) -> Context<'a> {
        let mut by_type: BTreeMap<Symbol, Vec<(usize, Symbol)>> = BTreeMap::new();
        for (name, info) in &check.ctors {
            by_type
                .entry(info.type_name.clone())
                .or_default()
                .push((info.index, name.clone()));
        }
        // Aliases and builtins contribute no `CtorInfo`, so they are never split on.
        let mut sums: BTreeMap<Symbol, Vec<Symbol>> = BTreeMap::new();
        for (ty, mut ctors) in by_type {
            ctors.sort();
            sums.insert(ty, ctors.into_iter().map(|(_, name)| name).collect());
        }
        drop_incomplete(&claims, &mut sums);

        let recursive = recursive_definitions(&claims.defs);
        let inhabited_types = inhabited_sum_types(check, &sums);
        let float_types = float_reaching_types(check);

        Context {
            claims,
            check,
            recursive,
            by_type: sums,
            inhabited_types,
            float_types,
            sort_names: BTreeMap::new(),
        }
    }

    pub fn claims(&self) -> &Claims {
        &self.claims
    }

    /// Through its arguments, its fields, or its own declaration.
    pub fn reaches_float(&self, ty: &Type) -> bool {
        reaches_float(ty, &self.float_types)
    }

    pub fn with_sort_names(mut self, names: BTreeMap<TyVar, Symbol>) -> Context<'a> {
        self.sort_names = names;
        self
    }

    pub fn sort_name(&self, v: TyVar) -> Symbol {
        self.sort_names
            .get(&v)
            .cloned()
            .unwrap_or_else(|| Symbol::new(Type::Var(v).to_string()))
    }

    pub fn ctor(&self, name: &Symbol) -> Option<&'a CtorInfo> {
        self.check.ctors.get(name)
    }

    /// `None` unless every constructor is in hand: a split over a partial list is unsound.
    pub fn variants(&self, type_name: &Symbol) -> Option<Variants<'a>> {
        let names = self.by_type.get(type_name)?;
        let mut ctors = Vec::with_capacity(names.len());
        for name in names {
            ctors.push(self.check.ctors.get(name)?);
        }
        Some(Variants {
            type_name: type_name.clone(),
            ctors,
        })
    }

    pub fn scheme(&self, name: &Symbol) -> Option<&'a ply_ty::Scheme> {
        self.check.defs.get(name).map(|d| &d.scheme)
    }

    pub fn inhabited(&self, ty: &Type) -> bool {
        match ty {
            Type::Var(_) => true,
            Type::Fn { ret, .. } => self.inhabited(ret),
            Type::Record(fields) => fields.values().all(|t| self.inhabited(t)),
            Type::Con(name, _) => match self.by_type.get(name) {
                None => true,
                Some(_) => self.inhabited_types.contains(name),
            },
        }
    }

    /// Whether equal calls must answer equally, so they may share one term.
    pub fn is_pure(&self, name: &Symbol) -> bool {
        let Some(def) = self.check.defs.get(name) else {
            return false;
        };
        def.footprint.is_empty()
            && match &def.scheme.ty {
                Type::Fn { effects, .. } => effects.is_pure(),
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
        if !self.check.defs.get(name)?.footprint.is_empty() {
            return None;
        }
        self.claims
            .defs
            .get(name)
            .filter(|def| !matches!(def.body, Code::Unreached))
    }
}

fn drop_incomplete(claims: &Claims, sums: &mut BTreeMap<Symbol, Vec<Symbol>>) {
    // The prelude's ADTs have no `type` item, and would otherwise be dropped.
    let mut declared: BTreeMap<Symbol, usize> = ply_ty::prelude::ADTS
        .iter()
        .map(|adt| (Symbol::new(adt.name), adt.variants.len()))
        .collect();
    declared.extend(claims.sums.iter().map(|(ty, n)| (ty.clone(), *n)));
    sums.retain(|ty, ctors| declared.get(ty) == Some(&ctors.len()) && !ctors.is_empty());
}

fn reaches_float(ty: &Type, declared: &BTreeSet<Symbol>) -> bool {
    match ty {
        Type::Con(name, args) => {
            (name.as_str() == "Float" && args.is_empty())
                || declared.contains(name)
                || args.iter().any(|a| reaches_float(a, declared))
        }
        Type::Fn { params, ret, .. } => {
            params.iter().any(|p| reaches_float(p, declared)) || reaches_float(ret, declared)
        }
        Type::Record(fields) => fields.values().any(|f| reaches_float(f, declared)),
        Type::Var(_) => false,
    }
}

/// A least fixed point, so chains of declarations and recursive ones both settle.
fn float_reaching_types(check: &CheckOutput) -> BTreeSet<Symbol> {
    let mut fields: BTreeMap<Symbol, Vec<&Type>> = BTreeMap::new();
    for ctor in check.ctors.values() {
        fields
            .entry(ctor.type_name.clone())
            .or_default()
            .extend(ctor.fields.iter());
    }
    let mut found: BTreeSet<Symbol> = BTreeSet::new();
    loop {
        let mut grew = false;
        for (type_name, fields) in &fields {
            if found.contains(type_name) {
                continue;
            }
            if fields.iter().any(|f| reaches_float(f, &found)) {
                found.insert(type_name.clone());
                grew = true;
            }
        }
        if !grew {
            return found;
        }
    }
}

/// Least fixed point: a type is inhabited once every field of some constructor is.
fn inhabited_sum_types(
    check: &CheckOutput,
    sums: &BTreeMap<Symbol, Vec<Symbol>>,
) -> BTreeSet<Symbol> {
    let mut inhabited: BTreeSet<Symbol> = BTreeSet::new();
    loop {
        let mut grew = false;
        for (type_name, ctors) in sums {
            if inhabited.contains(type_name) {
                continue;
            }
            let any = ctors.iter().any(|name| {
                check.ctors.get(name).is_some_and(|ctor| {
                    ctor.fields
                        .iter()
                        .all(|field| field_inhabited(field, sums, &inhabited))
                })
            });
            if any {
                inhabited.insert(type_name.clone());
                grew = true;
            }
        }
        if !grew {
            return inhabited;
        }
    }
}

fn field_inhabited(
    ty: &Type,
    sums: &BTreeMap<Symbol, Vec<Symbol>>,
    inhabited: &BTreeSet<Symbol>,
) -> bool {
    match ty {
        Type::Var(_) => true,
        Type::Fn { ret, .. } => field_inhabited(ret, sums, inhabited),
        Type::Record(fields) => fields.values().all(|t| field_inhabited(t, sums, inhabited)),
        Type::Con(name, _) if sums.contains_key(name) => inhabited.contains(name),
        Type::Con(..) => true,
    }
}

/// Definitions in a call-graph cycle; Tarjan run iteratively so deep programs cannot overflow.
fn recursive_definitions(defs: &HashMap<Symbol, Definition>) -> BTreeSet<Symbol> {
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
    for component in tarjan(&edges) {
        let cyclic =
            component.len() > 1 || component.first().is_some_and(|&v| edges[v].contains(&v));
        if cyclic {
            for v in component {
                recursive.insert(names[v].clone());
            }
        }
    }
    recursive
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
