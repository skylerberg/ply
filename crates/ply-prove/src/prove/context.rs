//! The facts about a program that the prover reads, indexed once per run.
//!
//! Three of them decide whether a rule may fire at all, and each is refused in
//! the conservative direction when it cannot be established:
//!
//! - **which constructors a type has.** A case split is only exhaustive if the
//!   list is complete, so a split happens only for a type this index built from
//!   whole-program check output. Everything else stays uninterpreted.
//! - **whether a definition is recursive.** Unfolding one needs induction to
//!   reach a general statement and M8 has none, so a definition in a cycle is
//!   never unfolded. The call graph over-approximates — a local that shadows a
//!   top-level name still counts as a reference — which can only mark more
//!   definitions recursive than are.
//! - **whether a definition is pure.** A body that performs is not a value the
//!   fragment reasons about, so it is never unfolded either.

use ply_core::{CheckOutput, CtorInfo, TyVar, Type};
use ply_span::Symbol;
use ply_syntax::ast::{Expr, ExprKind, FnDef, Item, Program, QName, Stmt, TypeDefBody};
use ply_syntax::resolve::{Namespace, Resolved};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// A definition the prover may inline.
pub struct Unfoldable<'a> {
    pub name: Symbol,
    pub def: &'a FnDef,
    /// Index into `Program::modules`, which is what the body's bare names
    /// resolve against.
    pub module: usize,
}

/// The constructors of one sum type, in declaration order.
pub struct Variants<'a> {
    pub type_name: Symbol,
    pub ctors: Vec<&'a CtorInfo>,
}

pub struct Context<'a> {
    resolved: &'a Resolved,
    check: &'a CheckOutput,
    defs: HashMap<Symbol, (usize, &'a FnDef)>,
    recursive: BTreeSet<Symbol>,
    by_type: BTreeMap<Symbol, Vec<Symbol>>,
    /// The sum types with at least one value, by least fixed point. A type all
    /// of whose constructors recurse — `type Bad = Wrap(Bad)` — has none.
    inhabited_types: BTreeSet<Symbol>,
    /// Every nominal type whose *declaration* reaches a `Float`, by least fixed
    /// point over the constructors. See [`Context::reaches_float`].
    float_types: BTreeSet<Symbol>,
    sort_names: BTreeMap<TyVar, Symbol>,
}

impl<'a> Context<'a> {
    pub fn new(
        program: &'a Program,
        resolved: &'a Resolved,
        check: &'a CheckOutput,
    ) -> Context<'a> {
        let mut defs: HashMap<Symbol, (usize, &FnDef)> = HashMap::new();
        for (index, module) in program.modules.iter().enumerate() {
            for item in &module.items {
                if let Item::Fn(def) = item {
                    defs.insert(module.name.qualify(&def.name.name), (index, def));
                }
            }
        }

        let mut by_type: BTreeMap<Symbol, Vec<(usize, Symbol)>> = BTreeMap::new();
        for (name, info) in &check.ctors {
            by_type
                .entry(info.type_name.clone())
                .or_default()
                .push((info.index, name.clone()));
        }
        // A type whose declaration the prover cannot see in full is one it must
        // never split on, so an alias and a builtin are both absent by
        // construction — neither contributes a `CtorInfo`.
        let mut sums: BTreeMap<Symbol, Vec<Symbol>> = BTreeMap::new();
        for (ty, mut ctors) in by_type {
            ctors.sort();
            sums.insert(ty, ctors.into_iter().map(|(_, name)| name).collect());
        }
        drop_incomplete(program, &mut sums);

        let recursive = recursive_definitions(&defs, resolved);
        let inhabited_types = inhabited_sum_types(check, &sums);
        let float_types = float_reaching_types(check);

        Context {
            resolved,
            check,
            defs,
            recursive,
            by_type: sums,
            inhabited_types,
            float_types,
            sort_names: BTreeMap::new(),
        }
    }

    /// Whether a type reaches a `Float` — through its arguments, its fields, or
    /// **its own declaration**.
    ///
    /// The declaration is the half a walk over the written type misses, and
    /// missing it is a false `proved`: `type Money = Cents(Float)` is
    /// `Type::Con("Money", [])`, nothing in it says `Float`, and the value it
    /// quantifies over is still `Cents(NaN)` — at which `==` is false and every
    /// rule here assumes it is true. `ply_core::derivable` answers this same
    /// question over the same declarations, which is what refuses
    /// `Map<Money, v>`; the two must not disagree.
    ///
    /// Over-approximates on a parameterised declaration — `type W<a> = W(Float,
    /// a)` marks every `W<_>` — which costs completeness and never soundness.
    pub fn reaches_float(&self, ty: &Type) -> bool {
        reaches_float(ty, &self.float_types)
    }

    /// Names for the type variables a proof leaves as uninterpreted sorts. A
    /// caller that has them makes `Certificate::sorts` read as the law was
    /// written; without them the sorts are rendered as the checker names them.
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

    /// The program-wide name a reference denotes, or `None` when it denotes
    /// nothing this crate can see — in which case the term becomes a fresh
    /// symbol rather than a guess.
    /// The program-wide name a value reference denotes.
    ///
    /// A bare name no module declares falls back to the prelude's constructors,
    /// which no module *can* declare. Without it a `match o { None -> .., Some(v)
    /// -> .. }` reads `None` as a binder rather than a nullary constructor, the
    /// arm becomes undecidable, and an obligation the fragment can decide comes
    /// back `Unknown` — a weaker tier for a resolution failure, which is a cost
    /// with no argument behind it.
    pub fn resolve_value(&self, module: usize, q: &QName) -> Option<Symbol> {
        if let Ok(binding) = self.resolved.lookup(module, Namespace::Value, q) {
            return Some(binding.qualified.clone());
        }
        let bare = q.is_bare().then(|| q.symbol().clone())?;
        self.check.ctors.contains_key(&bare).then_some(bare)
    }

    pub fn ctor(&self, name: &Symbol) -> Option<&'a CtorInfo> {
        self.check.ctors.get(name)
    }

    /// `None` unless every constructor of the type is in hand, because a split
    /// over a partial list is not a case analysis.
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

    pub fn scheme(&self, name: &Symbol) -> Option<&'a ply_core::Scheme> {
        self.check.defs.get(name).map(|d| &d.scheme)
    }

    /// Whether a type has at least one value.
    ///
    /// Asked only about a guard's domain: `guard ⟹ body` over an empty domain
    /// is valid and says nothing, so a certificate may not claim
    /// `guard_satisfiable` without this. A type variable counts as inhabited,
    /// which is the standard reading of an uninterpreted sort and is what the
    /// property tier assumes when it monomorphises one.
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

    /// Whether two calls to this definition with equal arguments must answer
    /// equally — the assumption behind sharing one term between them.
    ///
    /// A name this crate cannot see is **not** pure. An unestablished purity has
    /// to read as impure: a call that performs may answer differently each time,
    /// so `f() - f() == 0` is not a theorem about an effectful `f`, and a term
    /// shared between the two would make it one.
    ///
    /// The row is checked as well as the footprint, because a row-polymorphic
    /// definition has an empty footprint and an instantiation that performs.
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

    /// A definition the prover may inline: not in a recursive component, and
    /// with an empty footprint.
    pub fn unfoldable(&self, name: &Symbol) -> Option<Unfoldable<'a>> {
        if self.recursive.contains(name) {
            return None;
        }
        if !self.check.defs.get(name)?.footprint.is_empty() {
            return None;
        }
        let (module, def) = self.defs.get(name)?;
        Some(Unfoldable {
            name: name.clone(),
            def,
            module: *module,
        })
    }
}

/// Removes any sum type whose declared variant count disagrees with the number
/// of constructors in hand. Splitting over a partial list would be a case
/// analysis that missed a case, which is the one way this rule turns unsound.
fn drop_incomplete(program: &Program, sums: &mut BTreeMap<Symbol, Vec<Symbol>>) {
    let mut declared: BTreeMap<Symbol, usize> = BTreeMap::new();
    // The prelude's ADTs are declared by the *language* rather than by a file,
    // so the check below — which reads the program's `type` items — would drop
    // them and refuse to split on an `Option`. Their declaration is no less
    // complete for having no source: `prelude::ADTS` is the whole of it.
    for adt in ply_core::prelude::ADTS {
        declared.insert(Symbol::new(adt.name), adt.variants.len());
    }
    for module in &program.modules {
        for item in &module.items {
            if let Item::Type(def) = item
                && let TypeDefBody::Sum(variants) = &def.body
            {
                declared.insert(module.name.qualify(&def.name.name), variants.len());
            }
        }
    }
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

/// Every nominal type some constructor of which reaches a `Float`, as a least
/// fixed point so that a chain — `type Rate = R(Float)`, `type Row = W(Rate)` —
/// and a recursive declaration both settle.
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

/// The sum types with at least one value: a type is inhabited once some
/// constructor's every field is, which is the least fixed point of that rule.
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

/// Every definition in a cycle of the call graph, by Tarjan's algorithm, run
/// iteratively so a deep program cannot overflow the host stack.
fn recursive_definitions(
    defs: &HashMap<Symbol, (usize, &FnDef)>,
    resolved: &Resolved,
) -> BTreeSet<Symbol> {
    let mut names: Vec<Symbol> = defs.keys().cloned().collect();
    names.sort();
    let index: HashMap<&Symbol, usize> = names.iter().enumerate().map(|(i, n)| (n, i)).collect();

    let mut edges: Vec<Vec<usize>> = vec![Vec::new(); names.len()];
    for (i, name) in names.iter().enumerate() {
        let (module, def) = defs[name];
        let mut referenced = BTreeSet::new();
        collect_references(&def.body, module, resolved, &mut referenced);
        edges[i] = referenced
            .iter()
            .filter_map(|r| index.get(r).copied())
            .collect();
    }

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

/// Every top-level value a body could name. Local binders are not tracked, so a
/// local that shadows a definition still contributes an edge — which can only
/// grow the recursive set, and a definition wrongly called recursive is one the
/// prover declines to unfold.
fn collect_references(expr: &Expr, module: usize, resolved: &Resolved, out: &mut BTreeSet<Symbol>) {
    let mut stack = vec![expr];
    while let Some(e) = stack.pop() {
        match &e.kind {
            ExprKind::Var(q) => {
                if let Ok(binding) = resolved.lookup(module, Namespace::Value, q) {
                    out.insert(binding.qualified.clone());
                }
            }
            ExprKind::Lit(_) => {}
            ExprKind::Binary { lhs, rhs, .. } => {
                stack.push(lhs);
                stack.push(rhs);
            }
            ExprKind::Unary { operand, .. } => stack.push(operand),
            ExprKind::Lambda { body, .. } => stack.push(body),
            ExprKind::App { func, args } => {
                stack.push(func);
                stack.extend(args);
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                stack.push(cond);
                stack.push(then_branch);
                stack.push(else_branch);
            }
            ExprKind::Match { scrutinee, arms } => {
                stack.push(scrutinee);
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        stack.push(guard);
                    }
                    stack.push(&arm.body);
                }
            }
            ExprKind::Block { stmts, tail } => {
                for stmt in stmts {
                    match stmt {
                        Stmt::Let { value, .. } => stack.push(value),
                        Stmt::Expr(e) => stack.push(e),
                    }
                }
                stack.extend(tail.as_deref());
            }
            ExprKind::Record { fields } => stack.extend(fields.iter().map(|(_, v)| v)),
            ExprKind::RecordUpdate { base, fields } => {
                stack.push(base);
                stack.extend(fields.iter().map(|(_, v)| v));
            }
            ExprKind::Field { base, .. } => stack.push(base),
            ExprKind::Try { operand } => stack.push(operand),
            ExprKind::List { items } => stack.extend(items),
            ExprKind::Perform { args, .. } => stack.extend(args),
            ExprKind::Handle {
                body,
                clauses,
                return_clause,
            } => {
                stack.push(body);
                for clause in clauses {
                    stack.push(&clause.body);
                }
                if let Some(r) = return_clause {
                    stack.push(&r.body);
                }
            }
            ExprKind::WithCell { init, body, .. } => {
                stack.push(init);
                stack.push(body);
            }
            ExprKind::WithRegion { body, .. } | ExprKind::Simulate { body } => stack.push(body),
        }
    }
}
