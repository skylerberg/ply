use crate::env::{TypeEnv, generalize, instantiate};
use crate::print::{Printer, region_of, region_type_name};
use crate::scc::sccs;
use crate::ty::{EffectAtom, Footprint, Resource, Row, RowVar, Scheme, Type};
use crate::unify::{Fresh, Subst, UnifyError, unify, unify_row};
use crate::{CheckOutput, CtorInfo, DefInfo, EffectInfo, OpInfo, TestInfo};
use indexmap::IndexMap;
use ply_span::{Diagnostic, Severity, Span, Symbol, codes};
use ply_syntax::ast::*;
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::BTreeSet;

/// The effect under which `with_cell` regions publish their atoms. Reserved:
/// a user `effect cell` would silently gain the power to observe region state.
const CELL: &str = "cell";

const BUILTIN_TYPES: &[(&str, usize)] =
    &[("Int", 0), ("Bool", 0), ("String", 0), ("Unit", 0), ("List", 1), ("Cell", 1)];

pub fn check_module(module: &Module) -> Result<CheckOutput, Vec<Diagnostic>> {
    let mut c = Checker::new();
    c.collect_types(module);
    c.collect_effects(module);
    c.collect_ctors(module);
    c.check_fns(module);
    c.check_tests(module);
    c.check_comparisons();
    if c.diags.iter().any(|d| d.severity == Severity::Error) {
        Err(c.diags)
    } else {
        Ok(CheckOutput { defs: c.defs, tests: c.tests, effects: c.effects, ctors: c.ctors })
    }
}

#[derive(Clone, Debug)]
struct TypeDecl {
    params: Vec<Symbol>,
    alias: Option<TypeExpr>,
    span: Span,
}

/// Where an atom entered the current definition's row. `direct` marks a literal
/// `eff.op(..)` expression, whose span is worth more to a reader than the span of
/// a call that merely propagates the atom.
#[derive(Clone, Debug)]
struct PerformSite {
    atom: EffectAtom,
    span: Span,
    direct: bool,
}

struct Checker {
    subst: Subst,
    fresh: Fresh,
    env: TypeEnv,
    diags: Vec<Diagnostic>,
    types: IndexMap<Symbol, TypeDecl>,
    effects: IndexMap<Symbol, EffectInfo>,
    ctors: IndexMap<Symbol, CtorInfo>,
    defs: IndexMap<Symbol, DefInfo>,
    tests: Vec<TestInfo>,
    ty_params: FxHashMap<Symbol, Type>,
    row_params: FxHashMap<Symbol, RowVar>,
    /// Effect operation signatures have no generic list of their own, so a type
    /// variable appearing in one is implicitly quantified over the operation.
    auto_ty_params: bool,
    alias_stack: Vec<Symbol>,
    performs: Vec<PerformSite>,
    /// Operand types of `==` / `!=`, checked once the whole module is solved
    /// because the type at a comparison is often still a variable when it is
    /// first seen.
    comparisons: Vec<(Span, Type)>,
}

impl Checker {
    fn new() -> Self {
        let mut c = Checker {
            subst: Subst::new(),
            fresh: Fresh::default(),
            env: TypeEnv::new(),
            diags: Vec::new(),
            types: IndexMap::new(),
            effects: IndexMap::new(),
            ctors: IndexMap::new(),
            defs: IndexMap::new(),
            tests: Vec::new(),
            ty_params: FxHashMap::default(),
            row_params: FxHashMap::default(),
            auto_ty_params: false,
            alias_stack: Vec::new(),
            performs: Vec::new(),
            comparisons: Vec::new(),
        };
        c.install_prelude();
        c
    }

    fn install_prelude(&mut self) {
        let a = self.fresh.ty_var();
        let b = self.fresh.ty_var();
        let e = self.fresh.row_var();
        let (ta, tb, re) = (Type::Var(a), Type::Var(b), Row::open(e));

        let mono = |params: Vec<Type>, ret: Type| Scheme {
            ty_vars: vec![],
            row_vars: vec![],
            ty: Type::Fn { params, ret: Box::new(ret), effects: Row::empty() },
        };
        let poly = |ty_vars: Vec<_>, row_vars: Vec<_>, params: Vec<Type>, ret: Type, eff: Row| {
            Scheme {
                ty_vars,
                row_vars,
                ty: Type::Fn { params, ret: Box::new(ret), effects: eff },
            }
        };

        let entries: Vec<(&str, Scheme)> = vec![
            ("assert", mono(vec![Type::bool()], Type::unit())),
            (
                "assert_eq",
                poly(vec![a], vec![], vec![ta.clone(), ta.clone()], Type::unit(), Row::empty()),
            ),
            ("len", poly(vec![a], vec![], vec![Type::list(ta.clone())], Type::int(), Row::empty())),
            (
                "push",
                poly(
                    vec![a],
                    vec![],
                    vec![Type::list(ta.clone()), ta.clone()],
                    Type::list(ta.clone()),
                    Row::empty(),
                ),
            ),
            (
                "map",
                poly(
                    vec![a, b],
                    vec![e],
                    vec![
                        Type::list(ta.clone()),
                        Type::Fn {
                            params: vec![ta.clone()],
                            ret: Box::new(tb.clone()),
                            effects: re.clone(),
                        },
                    ],
                    Type::list(tb.clone()),
                    re.clone(),
                ),
            ),
            (
                "filter",
                poly(
                    vec![a],
                    vec![e],
                    vec![
                        Type::list(ta.clone()),
                        Type::Fn {
                            params: vec![ta.clone()],
                            ret: Box::new(Type::bool()),
                            effects: re.clone(),
                        },
                    ],
                    Type::list(ta.clone()),
                    re.clone(),
                ),
            ),
            (
                "fold",
                poly(
                    vec![a, b],
                    vec![e],
                    vec![
                        Type::list(ta.clone()),
                        tb.clone(),
                        Type::Fn {
                            params: vec![tb.clone(), ta.clone()],
                            ret: Box::new(tb.clone()),
                            effects: re.clone(),
                        },
                    ],
                    tb.clone(),
                    re.clone(),
                ),
            ),
            ("range", mono(vec![Type::int(), Type::int()], Type::list(Type::int()))),
            ("int_to_string", mono(vec![Type::int()], Type::string())),
            ("string_concat", mono(vec![Type::string(), Type::string()], Type::string())),
            ("panic", poly(vec![a], vec![], vec![Type::string()], ta.clone(), Row::empty())),
        ];
        for (name, scheme) in entries {
            self.env.bind_global(Symbol::new(name), scheme);
        }

        // `cell_get` / `cell_set` are handled as call forms rather than schemes:
        // the atom they perform names the region of their argument, which no
        // row expressible in `Row` can be polymorphic in.
        let cell_ty = Type::Con(Symbol::new("Cell"), vec![Type::Var(b), ta.clone()]);
        self.env.bind_global(
            Symbol::new("cell_get"),
            poly(vec![a, b], vec![], vec![cell_ty.clone()], ta.clone(), Row::empty()),
        );
        self.env.bind_global(
            Symbol::new("cell_set"),
            poly(vec![a, b], vec![], vec![cell_ty, ta], Type::unit(), Row::empty()),
        );
    }

    fn collect_types(&mut self, module: &Module) {
        for item in &module.items {
            let Item::Type(def) = item else { continue };
            let name = def.name.name.clone();
            if let Some(prev) = self.types.get(&name) {
                self.duplicate(&def.name, prev.span, "type");
                continue;
            }
            if BUILTIN_TYPES.iter().any(|(b, _)| *b == name.as_str()) {
                self.diags.push(
                    Diagnostic::error(
                        codes::DUPLICATE_DEFINITION,
                        format!("`{name}` is a builtin type"),
                    )
                    .primary(def.name.span, "cannot be redefined")
                    .note("choose a different name for this type"),
                );
                continue;
            }
            let alias = match &def.body {
                TypeDefBody::Alias(t) => Some(t.clone()),
                TypeDefBody::Sum(_) => None,
            };
            self.types.insert(
                name,
                TypeDecl {
                    params: def.params.iter().map(|p| p.name.clone()).collect(),
                    alias,
                    span: def.span,
                },
            );
        }
    }

    fn collect_effects(&mut self, module: &Module) {
        for item in &module.items {
            let Item::Effect(def) = item else { continue };
            let name = def.name.name.clone();
            if name.as_str() == CELL {
                self.diags.push(
                    Diagnostic::error(
                        codes::DUPLICATE_DEFINITION,
                        "`cell` is a builtin effect".to_string(),
                    )
                    .primary(def.name.span, "reserved for `with_cell` regions")
                    .note("rename this effect"),
                );
                continue;
            }
            if let Some(prev) = self.effects.get(&name) {
                self.duplicate(&def.name, prev.span, "effect");
                continue;
            }
            let mut ops: IndexMap<Symbol, OpInfo> = IndexMap::new();
            for op in &def.ops {
                if let Some(prev) = ops.get(&op.name.name) {
                    self.duplicate(&op.name, prev.span, "operation");
                    continue;
                }
                self.ty_params.clear();
                self.auto_ty_params = true;
                let params: Vec<Type> = op.params.iter().map(|p| self.conv_type(p)).collect();
                let ret = self.conv_type(&op.ret);
                self.auto_ty_params = false;
                self.ty_params.clear();
                ops.insert(
                    op.name.name.clone(),
                    OpInfo {
                        name: op.name.name.clone(),
                        mode: op.mode,
                        resource_param: op.resource_param,
                        params,
                        ret,
                        span: op.span,
                    },
                );
            }
            self.effects.insert(
                name.clone(),
                EffectInfo { name, nondet: def.nondet, ops, span: def.span },
            );
        }
    }

    fn collect_ctors(&mut self, module: &Module) {
        for item in &module.items {
            let Item::Type(def) = item else { continue };
            let TypeDefBody::Sum(variants) = &def.body else { continue };
            let Some(decl) = self.types.get(&def.name.name).cloned() else { continue };

            self.ty_params.clear();
            let mut vars = Vec::new();
            for p in &decl.params {
                let v = self.fresh.ty_var();
                vars.push(v);
                self.ty_params.insert(p.clone(), Type::Var(v));
            }
            let result = Type::Con(
                def.name.name.clone(),
                vars.iter().map(|v| Type::Var(*v)).collect(),
            );
            for (index, variant) in variants.iter().enumerate() {
                if let Some(prev) = self.ctors.get(&variant.name.name) {
                    self.duplicate(&variant.name, prev.span, "constructor");
                    continue;
                }
                let fields: Vec<Type> = variant.fields.iter().map(|f| self.conv_type(f)).collect();
                let ty = if fields.is_empty() {
                    result.clone()
                } else {
                    Type::Fn {
                        params: fields.clone(),
                        ret: Box::new(result.clone()),
                        effects: Row::empty(),
                    }
                };
                let scheme = Scheme { ty_vars: vars.clone(), row_vars: vec![], ty };
                self.env.bind_global(variant.name.name.clone(), scheme.clone());
                self.ctors.insert(
                    variant.name.name.clone(),
                    CtorInfo {
                        name: variant.name.name.clone(),
                        type_name: def.name.name.clone(),
                        index,
                        arity: fields.len(),
                        fields,
                        scheme,
                        span: variant.span,
                    },
                );
            }
            self.ty_params.clear();
        }
    }

    fn duplicate(&mut self, name: &Ident, prev: Span, what: &str) {
        self.diags.push(
            Diagnostic::error(
                codes::DUPLICATE_DEFINITION,
                format!("{what} `{}` is defined more than once", name.name),
            )
            .primary(name.span, "redefined here")
            .secondary(prev, "first defined here")
            .note("rename one of them"),
        );
    }

    fn conv_type(&mut self, te: &TypeExpr) -> Type {
        match te {
            TypeExpr::Var(id) => self.type_param(id),
            TypeExpr::Unit { .. } => Type::unit(),
            TypeExpr::Record { fields, .. } => Type::Record(
                fields.iter().map(|(k, v)| (k.name.clone(), self.conv_type(v))).collect(),
            ),
            TypeExpr::Fn { params, ret, effects, span: _ } => Type::Fn {
                params: params.iter().map(|p| self.conv_type(p)).collect(),
                ret: Box::new(self.conv_type(ret)),
                effects: effects.as_ref().map(|r| self.conv_row(r)).unwrap_or_default(),
            },
            TypeExpr::Con { name, args, span } => self.conv_con(name, args, *span),
        }
    }

    fn type_param(&mut self, id: &Ident) -> Type {
        if let Some(t) = self.ty_params.get(&id.name) {
            return t.clone();
        }
        if self.auto_ty_params {
            let t = self.fresh.ty();
            self.ty_params.insert(id.name.clone(), t.clone());
            return t;
        }
        self.diags.push(
            Diagnostic::error(
                codes::UNKNOWN_TYPE,
                format!("unknown type variable `{}`", id.name),
            )
            .primary(id.span, "not declared")
            .note(format!("add `{}` to the generic list, e.g. `fn f<{}>(..)`", id.name, id.name)),
        );
        self.fresh.ty()
    }

    fn conv_con(&mut self, name: &Ident, args: &[TypeExpr], span: Span) -> Type {
        if args.is_empty() && self.ty_params.contains_key(&name.name) {
            return self.ty_params[&name.name].clone();
        }
        let args: Vec<Type> = args.iter().map(|a| self.conv_type(a)).collect();

        if let Some((_, arity)) = BUILTIN_TYPES.iter().find(|(b, _)| *b == name.name.as_str()) {
            if args.len() != *arity {
                self.arity_error(span, &name.name, *arity, args.len(), "type arguments");
                return self.fresh.ty();
            }
            if name.name.as_str() == "Cell" {
                // A written `Cell<T>` says nothing about which region the cell
                // came from, so leave the region open and let unification with a
                // `with_cell` binder decide it.
                let region = self.fresh.ty();
                return Type::Con(Symbol::new("Cell"), vec![region, args[0].clone()]);
            }
            return Type::Con(name.name.clone(), args);
        }

        let Some(decl) = self.types.get(&name.name).cloned() else {
            if args.is_empty() && self.auto_ty_params {
                return self.type_param(name);
            }
            self.diags.push(
                Diagnostic::error(codes::UNKNOWN_TYPE, format!("unknown type `{}`", name.name))
                    .primary(name.span, "not found")
                    .note("declare it with `type`, or check the spelling"),
            );
            return self.fresh.ty();
        };

        if args.len() != decl.params.len() {
            self.arity_error(span, &name.name, decl.params.len(), args.len(), "type arguments");
            return self.fresh.ty();
        }

        let Some(alias) = decl.alias.clone() else {
            return Type::Con(name.name.clone(), args);
        };

        if self.alias_stack.contains(&name.name) {
            self.diags.push(
                Diagnostic::error(
                    codes::UNKNOWN_TYPE,
                    format!("type alias `{}` expands into itself", name.name),
                )
                .primary(name.span, "cyclic alias")
                .note("break the cycle, or make this a `type` with variants"),
            );
            return self.fresh.ty();
        }

        let saved = std::mem::take(&mut self.ty_params);
        for (p, a) in decl.params.iter().zip(&args) {
            self.ty_params.insert(p.clone(), a.clone());
        }
        self.alias_stack.push(name.name.clone());
        let expanded = self.conv_type(&alias);
        self.alias_stack.pop();
        self.ty_params = saved;
        expanded
    }

    fn arity_error(&mut self, span: Span, name: &Symbol, expected: usize, found: usize, what: &str) {
        let what = if expected == 1 { what.trim_end_matches('s') } else { what };
        self.diags.push(
            Diagnostic::error(
                codes::ARITY_MISMATCH,
                format!("`{name}` takes {expected} {what}, but {}", supplied(found)),
            )
            .primary(span, format!("expected {expected}, found {found}")),
        );
    }

    fn conv_row(&mut self, row: &RowExpr) -> Row {
        let mut atoms = BTreeSet::new();
        for a in &row.atoms {
            if let Some(atom) = self.conv_atom(a) {
                atoms.insert(atom);
            }
        }
        let tail = row.tail.as_ref().and_then(|t| self.row_param(t));
        Row { atoms, tail }
    }

    fn conv_atom(&mut self, a: &AtomExpr) -> Option<EffectAtom> {
        let effect = a.effect.name.clone();
        if effect.as_str() != CELL && !self.effects.contains_key(&effect) {
            self.unknown_effect(&a.effect);
            return None;
        }
        let resource = match &a.resource {
            Some(r) => Resource::Named(r.name.clone()),
            None => Resource::Singleton,
        };
        if let Some(info) = self.effects.get(&effect)
            && !info.ops.values().any(|o| o.mode == a.mode)
        {
            let known: Vec<String> =
                info.ops.values().map(|o| format!("{}.{}", effect, o.mode.as_str())).collect();
            self.diags.push(
                Diagnostic::error(
                    codes::UNKNOWN_OPERATION,
                    format!("effect `{effect}` declares no `{}` operation", a.mode.as_str()),
                )
                .primary(a.span, format!("no `{}` operation to perform", a.mode.as_str()))
                .note(format!("`{effect}` can perform: {}", dedup(known).join(", "))),
            );
            return None;
        }
        Some(EffectAtom::new(effect, resource, a.mode))
    }

    fn row_param(&mut self, id: &Ident) -> Option<RowVar> {
        if let Some(v) = self.row_params.get(&id.name) {
            return Some(*v);
        }
        self.diags.push(
            Diagnostic::error(
                codes::UNBOUND_ROW_VAR,
                format!("unknown effect variable `{}`", id.name),
            )
            .primary(id.span, "not declared")
            .note(format!(
                "declare it in the generic list, e.g. `fn f<| {}>(..) / {{| {}}}`",
                id.name, id.name
            )),
        );
        None
    }

    fn unknown_effect(&mut self, id: &Ident) {
        let known: Vec<String> = self.effects.keys().map(|k| format!("`{k}`")).collect();
        let mut d = Diagnostic::error(
            codes::UNKNOWN_EFFECT,
            format!("unknown effect `{}`", id.name),
        )
        .primary(id.span, "not declared");
        d = if known.is_empty() {
            d.note("declare it with `effect <name> { .. }`")
        } else {
            d.note(format!("effects in scope: {}", known.join(", ")))
        };
        self.diags.push(d);
    }

    fn check_fns(&mut self, module: &Module) {
        let mut fns: Vec<&FnDef> = Vec::new();
        let mut index: FxHashMap<Symbol, usize> = FxHashMap::default();
        for item in &module.items {
            let Item::Fn(def) = item else { continue };
            if let Some(prev) = index.get(&def.name.name) {
                let prev_span = fns[*prev].name.span;
                self.duplicate(&def.name, prev_span, "function");
                continue;
            }
            if is_cell_builtin(&def.name.name) {
                self.diags.push(
                    Diagnostic::error(
                        codes::DUPLICATE_DEFINITION,
                        format!("`{}` is a builtin", def.name.name),
                    )
                    .primary(def.name.span, "cannot be redefined")
                    .note("region-scoped cell access is a call form, not an ordinary function"),
                );
                continue;
            }
            index.insert(def.name.name.clone(), fns.len());
            fns.push(def);
        }

        let adj: Vec<Vec<usize>> = fns
            .iter()
            .map(|d| {
                let mut refs = Vec::new();
                collect_refs(&d.body, &mut refs);
                let mut seen = FxHashSet::default();
                refs.iter()
                    .filter_map(|r| index.get(r).copied())
                    .filter(|i| seen.insert(*i))
                    .collect()
            })
            .collect();

        for comp in sccs(fns.len(), &adj) {
            let mut sigs = Vec::new();
            for &i in &comp {
                let sig = self.signature(fns[i]);
                self.env.bind_global(fns[i].name.name.clone(), Scheme::mono(sig.fn_ty.clone()));
                sigs.push(sig);
            }
            for (slot, &i) in comp.iter().enumerate() {
                self.check_fn_body(fns[i], &sigs[slot]);
            }
            for sig in &sigs {
                self.close_unreachable_row(sig);
            }
            for &i in &comp {
                self.env.remove_global(&fns[i].name.name);
            }
            for (slot, &i) in comp.iter().enumerate() {
                let def = fns[i];
                let scheme = generalize(&self.subst, &self.env, &sigs[slot].fn_ty);
                let row = self.subst.resolve_row(&sigs[slot].published_row);
                self.env.bind_global(def.name.name.clone(), scheme.clone());
                self.defs.insert(
                    def.name.name.clone(),
                    DefInfo {
                        name: def.name.name.clone(),
                        scheme,
                        footprint: Footprint(row.atoms),
                        span: def.span,
                    },
                );
            }
        }
    }

    /// A tail that no parameter or result type mentions can never be filled in
    /// by a caller, so leaving it quantified would publish a function as
    /// effect-polymorphic when it is simply pure.
    fn close_unreachable_row(&mut self, sig: &Signature) {
        let Type::Fn { params, ret, effects } = self.subst.resolve_ty(&sig.fn_ty) else {
            return;
        };
        let Some(tail) = effects.tail else { return };
        if self.subst.is_rigid_row(tail) {
            return;
        }
        let (mut tys, mut rows) = (BTreeSet::new(), BTreeSet::new());
        for p in &params {
            self.subst.free_vars(p, &mut tys, &mut rows);
        }
        self.subst.free_vars(&ret, &mut tys, &mut rows);
        if !rows.contains(&tail) {
            let _ = unify_row(&mut self.subst, &mut self.fresh, &Row::empty(), &Row::open(tail));
        }
    }

    fn signature(&mut self, def: &FnDef) -> Signature {
        self.ty_params.clear();
        self.row_params.clear();
        for p in &def.generics.types {
            let v = self.fresh.ty_var();
            self.subst.mark_rigid_ty(v);
            self.ty_params.insert(p.name.clone(), Type::Var(v));
        }
        for p in &def.generics.effects {
            let v = self.fresh.row_var();
            self.subst.mark_rigid_row(v);
            self.row_params.insert(p.name.clone(), v);
        }
        let params: Vec<Type> = def
            .params
            .iter()
            .map(|p| match &p.ty {
                Some(t) => self.conv_type(t),
                None => self.fresh.ty(),
            })
            .collect();
        let ret = match &def.ret {
            Some(t) => self.conv_type(t),
            None => self.fresh.ty(),
        };
        let declared = def.effects.as_ref().map(|r| self.conv_row(r));
        let row = declared.clone().unwrap_or_else(|| self.fresh.row());
        Signature {
            ty_params: std::mem::take(&mut self.ty_params),
            row_params: std::mem::take(&mut self.row_params),
            fn_ty: Type::Fn {
                params: params.clone(),
                ret: Box::new(ret.clone()),
                effects: row.clone(),
            },
            params,
            ret,
            declared,
            published_row: row,
        }
    }

    fn check_fn_body(&mut self, def: &FnDef, sig: &Signature) {
        self.ty_params = sig.ty_params.clone();
        self.row_params = sig.row_params.clone();
        self.performs.clear();

        self.env.push();
        for (p, t) in def.params.iter().zip(&sig.params) {
            self.env.bind(p.name.name.clone(), Scheme::mono(t.clone()));
        }
        let (body_ty, body_row) = self.infer(&def.body);
        self.expect(def.body.span, &sig.ret, &body_ty, "function body type");
        self.env.pop();

        match &sig.declared {
            Some(declared) => self.check_upper_bound(def, declared, &body_row),
            None => {
                if let Err(e) = unify_row(&mut self.subst, &mut self.fresh, &sig.published_row, &body_row) {
                    self.report_unify(&e, def.body.span, "inferred effect row");
                }
            }
        }
        self.ty_params.clear();
        self.row_params.clear();
    }

    /// A `/ {...}` annotation bounds the inferred row from above: everything the
    /// body can do must be listed, but the annotation may list more.
    fn check_upper_bound(&mut self, def: &FnDef, declared: &Row, inferred: &Row) {
        let ann_span = def.effects.as_ref().map(|r| r.span).unwrap_or(def.span);
        let inferred = self.subst.resolve_row(inferred);
        let declared = self.subst.resolve_row(declared);

        let extra: Vec<EffectAtom> = inferred.atoms.difference(&declared.atoms).cloned().collect();
        if !extra.is_empty() {
            let names: Vec<String> = extra.iter().map(|a| format!("`{a}`")).collect();
            let mut d = Diagnostic::error(
                codes::EFFECT_NOT_PERMITTED,
                format!(
                    "effect{} not permitted by the signature of `{}`: {}",
                    if extra.len() == 1 { "" } else { "s" },
                    def.name.name,
                    names.join(", ")
                ),
            );
            let mut anchored = false;
            for atom in &extra {
                if let Some(site) = self.site_for(atom) {
                    d = d.primary(site.span, format!("performs `{atom}`"));
                    anchored = true;
                }
            }
            if !anchored {
                d = d.primary(def.body.span, "this body performs more than it declares");
            }
            let mut printer = Printer::new();
            d = d
                .secondary(ann_span, format!("declared row is {}", printer.row(&declared)))
                .note(format!(
                    "add {} to the `/ {{..}}` annotation, or handle {} inside `{}`",
                    names.join(", "),
                    if extra.len() == 1 { "it" } else { "them" },
                    def.name.name
                ));
            self.diags.push(d);
        }

        match (inferred.tail, declared.tail) {
            (None, _) => {}
            (Some(v), Some(w)) if v == w => {}
            (Some(v), tail) => {
                let target = Row { atoms: BTreeSet::new(), tail };
                if unify_row(&mut self.subst, &mut self.fresh, &target, &Row::open(v)).is_err() {
                    let mut printer = Printer::new();
                    let shown = printer.row(&inferred);
                    self.diags.push(
                        Diagnostic::error(
                            codes::EFFECT_NOT_PERMITTED,
                            format!(
                                "the signature of `{}` does not permit the effects its body forwards",
                                def.name.name
                            ),
                        )
                        .primary(def.body.span, format!("body has row {shown}"))
                        .secondary(ann_span, "declared row cannot absorb it")
                        .note("add the missing effect variable to the generic list, or widen the annotation"),
                    );
                }
            }
        }
    }

    fn check_tests(&mut self, module: &Module) {
        let mut index = 0usize;
        for item in &module.items {
            let Item::Test(def) = item else { continue };
            self.ty_params.clear();
            self.row_params.clear();
            self.performs.clear();

            self.env.push();
            let (_, row) = self.infer(&def.body);
            self.env.pop();

            let row = self.subst.resolve_row(&row);
            let footprint = Footprint(row.atoms.clone());
            if !def.nondet {
                self.check_determinism(def, &footprint);
            }
            self.tests.push(TestInfo {
                name: def.name.clone(),
                index,
                nondet: def.nondet,
                footprint,
                span: def.span,
            });
            index += 1;
        }
    }

    fn check_determinism(&mut self, def: &TestDef, footprint: &Footprint) {
        for atom in footprint.atoms() {
            let Some(info) = self.effects.get(&atom.effect) else { continue };
            if !info.nondet {
                continue;
            }
            let effect = atom.effect.clone();
            let example = self.example_clause(&effect, atom);
            let (span, direct) = match self.site_for(atom) {
                Some(site) => (site.span, site.direct),
                None => (def.body.span, false),
            };
            let label = if direct {
                format!("performs `{atom}`, and `{effect}` is declared `nondet`")
            } else {
                format!("reaches `{atom}`, and `{effect}` is declared `nondet`")
            };
            let mut d = Diagnostic::error(
                codes::NONDET_IN_DET_TEST,
                "nondeterministic effect in a deterministic test",
            )
            .primary(span, label)
            .secondary(def.name_span, format!("test `{}` is deterministic", def.name));
            if !direct {
                d = d.note(format!(
                    "`{atom}` is performed inside something this expression calls"
                ));
            }
            d = d
                .note(format!(
                    "handle it here, e.g. `handle <body> with {{ {example} }}`"
                ))
                .note(
                    "or declare this `test/nondet`, which opts out of the cache and re-runs every time",
                );
            self.diags.push(d);
        }
    }

    fn example_clause(&self, effect: &Symbol, atom: &EffectAtom) -> String {
        let Some(info) = self.effects.get(effect) else {
            return format!("{effect}.<op>() -> <value>");
        };
        let op = info
            .ops
            .values()
            .find(|o| o.mode == atom.mode && matches!(&atom.resource, Resource::Named(_)) == o.resource_param)
            .or_else(|| info.ops.values().find(|o| o.mode == atom.mode))
            .or_else(|| info.ops.values().next());
        match op {
            Some(op) => {
                let resource = match (&atom.resource, op.resource_param) {
                    (Resource::Named(r), true) => format!("[{r}]"),
                    _ => String::new(),
                };
                let args: Vec<String> =
                    (0..op.params.len()).map(|i| format!("a{i}")).collect();
                format!("{effect}.{}{resource}({}) -> <value>", op.name, args.join(", "))
            }
            None => format!("{effect}.<op>() -> <value>"),
        }
    }

    fn site_for(&self, atom: &EffectAtom) -> Option<&PerformSite> {
        self.performs
            .iter()
            .find(|s| s.direct && &s.atom == atom)
            .or_else(|| self.performs.iter().find(|s| &s.atom == atom))
    }

    fn record(&mut self, atom: EffectAtom, span: Span, direct: bool) {
        if !span.is_dummy() {
            self.performs.push(PerformSite { atom, span, direct });
        }
    }

    /// Atoms discharged by a handler cannot be the reason a test is
    /// nondeterministic, so their perform sites stop being evidence.
    fn discharge(&mut self, range: std::ops::Range<usize>, handled: &BTreeSet<EffectAtom>) {
        let start = range.start;
        let kept: Vec<PerformSite> =
            self.performs.drain(range).filter(|s| !handled.contains(&s.atom)).collect();
        self.performs.splice(start..start, kept);
    }

    /// A left-leaning operator chain is parsed iteratively, so the parser's
    /// nesting limit does not bound this walk and a generated definition can
    /// nest deeper than the native stack. Overflowing is an abort, which no
    /// caller can catch and report.
    fn infer(&mut self, e: &Expr) -> (Type, Row) {
        const RED_ZONE: usize = 256 * 1024;
        const NEW_SEGMENT: usize = 2 * 1024 * 1024;
        stacker::maybe_grow(RED_ZONE, NEW_SEGMENT, || self.infer_inner(e))
    }

    fn infer_inner(&mut self, e: &Expr) -> (Type, Row) {
        match &e.kind {
            ExprKind::Lit(l) => (lit_type(l), Row::empty()),

            ExprKind::Var(id) => {
                if is_cell_builtin(&id.name) && self.env.depth_of(&id.name) == Some(0) {
                    self.diags.push(
                        Diagnostic::error(
                            codes::RESOURCE_REQUIRED,
                            format!("`{}` must be called directly", id.name),
                        )
                        .primary(id.span, "used as a value")
                        .note(
                            "the atom it performs names the region of the cell it is given, so it \
                             cannot be passed around as a first-class function",
                        ),
                    );
                    return (self.fresh.ty(), Row::empty());
                }
                match self.env.lookup(&id.name) {
                    Some(scheme) => {
                        let scheme = scheme.clone();
                        (instantiate(&scheme, &mut self.fresh), Row::empty())
                    }
                    None => {
                        self.unknown_name(id);
                        (self.fresh.ty(), Row::empty())
                    }
                }
            }

            ExprKind::Unary { op, operand } => {
                let (t, row) = self.infer(operand);
                let want = match op {
                    UnOp::Neg => Type::int(),
                    UnOp::Not => Type::bool(),
                };
                self.expect(operand.span, &want, &t, "operand of a unary operator");
                (want, row)
            }

            ExprKind::Binary { op, lhs, rhs } => self.infer_binary(e, *op, lhs, rhs),

            ExprKind::Lambda { params, body } => {
                self.env.push();
                let mut ptys = Vec::new();
                for p in params {
                    let t = match &p.ty {
                        Some(t) => self.conv_type(t),
                        None => self.fresh.ty(),
                    };
                    self.env.bind(p.name.name.clone(), Scheme::mono(t.clone()));
                    ptys.push(t);
                }
                let (ret, row) = self.infer(body);
                self.env.pop();
                (Type::Fn { params: ptys, ret: Box::new(ret), effects: row }, Row::empty())
            }

            ExprKind::App { func, args } => self.infer_app(e, func, args),

            ExprKind::If { cond, then_branch, else_branch } => {
                let (ct, crow) = self.infer(cond);
                self.expect(cond.span, &Type::bool(), &ct, "condition of `if`");
                let (tt, trow) = self.infer(then_branch);
                let (et, erow) = self.infer(else_branch);
                self.expect(else_branch.span, &tt, &et, "branches of `if` must agree");
                let row = self.join(e.span, crow, trow);
                let row = self.join(e.span, row, erow);
                (tt, row)
            }

            ExprKind::Match { scrutinee, arms } => self.infer_match(e, scrutinee, arms),

            ExprKind::Block { stmts, tail } => {
                self.env.push();
                let mut row = Row::empty();
                for stmt in stmts {
                    let r = self.infer_stmt(stmt);
                    row = self.join(e.span, row, r);
                }
                let ty = match tail {
                    Some(t) => {
                        let (ty, r) = self.infer(t);
                        row = self.join(e.span, row, r);
                        ty
                    }
                    None => Type::unit(),
                };
                self.env.pop();
                (ty, row)
            }

            ExprKind::Record { fields } => {
                let mut map = std::collections::BTreeMap::new();
                let mut row = Row::empty();
                for (name, value) in fields {
                    let (t, r) = self.infer(value);
                    row = self.join(e.span, row, r);
                    map.insert(name.name.clone(), t);
                }
                (Type::Record(map), row)
            }

            ExprKind::Field { base, field } => {
                let (bt, row) = self.infer(base);
                let bt = self.subst.resolve_ty(&bt);
                match &bt {
                    Type::Record(fields) => match fields.get(&field.name) {
                        Some(t) => (t.clone(), row),
                        None => {
                            let known: Vec<String> =
                                fields.keys().map(|k| format!("`{k}`")).collect();
                            self.diags.push(
                                Diagnostic::error(
                                    codes::UNKNOWN_NAME,
                                    format!("no field `{}` on this record", field.name),
                                )
                                .primary(field.span, "unknown field")
                                .note(if known.is_empty() {
                                    "the record has no fields".to_string()
                                } else {
                                    format!("available fields: {}", known.join(", "))
                                }),
                            );
                            (self.fresh.ty(), row)
                        }
                    },
                    _ => {
                        let mut printer = Printer::new();
                        let shown = printer.ty(&bt);
                        self.diags.push(
                            Diagnostic::error(
                                codes::TYPE_MISMATCH,
                                format!("cannot read field `{}` here", field.name),
                            )
                            .primary(base.span, format!("this has type `{shown}`, not a record"))
                            .note("annotate the value so its record type is known"),
                        );
                        (self.fresh.ty(), row)
                    }
                }
            }

            ExprKind::List { items } => {
                let elem = self.fresh.ty();
                let mut row = Row::empty();
                for item in items {
                    let (t, r) = self.infer(item);
                    row = self.join(e.span, row, r);
                    self.expect(item.span, &elem, &t, "list elements must agree");
                }
                (Type::list(elem), row)
            }

            ExprKind::Perform { effect, op, resource, args } => {
                self.infer_perform(e, effect, op, resource.as_ref(), args)
            }

            ExprKind::Handle { body, clauses, return_clause } => {
                self.infer_handle(e, body, clauses, return_clause.as_deref())
            }

            ExprKind::WithCell { resource, init, binder, body } => {
                self.infer_with_cell(e, resource, init, binder, body)
            }
        }
    }

    fn infer_stmt(&mut self, stmt: &Stmt) -> Row {
        match stmt {
            Stmt::Expr(e) => self.infer(e).1,
            Stmt::Let { pat, ty, value, span: _ } => {
                let (mut vt, row) = self.infer(value);
                if let Some(annotation) = ty {
                    let want = self.conv_type(annotation);
                    self.expect(value.span, &want, &vt, "`let` annotation");
                    vt = want;
                }
                let generalizable =
                    matches!(pat.kind, PatternKind::Var(_) | PatternKind::Wildcard);
                let mut bindings = Vec::new();
                self.bind_pattern(pat, &vt, &mut bindings);
                for (name, t) in bindings {
                    let scheme = if generalizable {
                        generalize(&self.subst, &self.env, &t)
                    } else {
                        Scheme::mono(t)
                    };
                    self.env.bind(name, scheme);
                }
                row
            }
        }
    }

    fn infer_binary(&mut self, e: &Expr, op: BinOp, lhs: &Expr, rhs: &Expr) -> (Type, Row) {
        let (lt, lrow) = self.infer(lhs);
        let (rt, rrow) = self.infer(rhs);
        let row = self.join(e.span, lrow, rrow);
        let (operand, result) = match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                (Some(Type::int()), Type::int())
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => (Some(Type::int()), Type::bool()),
            BinOp::And | BinOp::Or => (Some(Type::bool()), Type::bool()),
            BinOp::Concat => (Some(Type::string()), Type::string()),
            BinOp::Eq | BinOp::Ne => (None, Type::bool()),
        };
        match operand {
            Some(want) => {
                self.expect(lhs.span, &want, &lt, "left operand");
                self.expect(rhs.span, &want, &rt, "right operand");
            }
            None => {
                self.expect(rhs.span, &lt, &rt, "both sides of a comparison must have one type");
                self.comparisons.push((e.span, lt.clone()));
            }
        }
        (result, row)
    }

    /// Equality is structural, and there is no structural equality on a
    /// function.
    fn check_comparisons(&mut self) {
        for (span, ty) in std::mem::take(&mut self.comparisons) {
            let resolved = self.subst.resolve_ty(&ty);
            if !contains_fn(&resolved) {
                continue;
            }
            let mut printer = Printer::new();
            self.diags.push(
                Diagnostic::error(codes::TYPE_MISMATCH, "functions cannot be compared for equality")
                    .primary(span, format!("both sides have type `{}`", printer.ty(&resolved)))
                    .note("compare the results of calling them instead"),
            );
        }
    }

    fn infer_app(&mut self, e: &Expr, func: &Expr, args: &[Expr]) -> (Type, Row) {
        if let ExprKind::Var(id) = &func.kind
            && self.env.depth_of(&id.name) == Some(0)
        {
            match id.name.as_str() {
                "cell_get" => return self.infer_cell_op(e, args, Mode::Read),
                "cell_set" => return self.infer_cell_op(e, args, Mode::Write),
                _ => {}
            }
        }

        let (ft, mut row) = self.infer(func);
        let mut arg_tys = Vec::new();
        for a in args {
            let (t, r) = self.infer(a);
            row = self.join(e.span, row, r);
            arg_tys.push(t);
        }

        let resolved = self.subst.resolve_ty(&ft);
        match resolved {
            Type::Fn { params, ret, effects } => {
                if params.len() != args.len() {
                    self.diags.push(
                        Diagnostic::error(
                            codes::ARITY_MISMATCH,
                            format!(
                                "this function takes {} argument{}, but {}",
                                params.len(),
                                if params.len() == 1 { "" } else { "s" },
                                supplied(args.len())
                            ),
                        )
                        .primary(e.span, format!("expected {}, found {}", params.len(), args.len()))
                        .secondary(func.span, "this is the function being called"),
                    );
                    return (*ret, self.join(e.span, row, effects));
                }
                for ((want, got), arg) in params.iter().zip(&arg_tys).zip(args) {
                    self.expect(arg.span, want, got, "argument type");
                }
                let effects = self.subst.resolve_row(&effects);
                for atom in &effects.atoms {
                    self.record(atom.clone(), e.span, false);
                }
                let row = self.join(e.span, row, effects);
                (*ret, row)
            }
            Type::Var(_) => {
                let ret = self.fresh.ty();
                let eff = self.fresh.row();
                let want = Type::Fn {
                    params: arg_tys,
                    ret: Box::new(ret.clone()),
                    effects: eff.clone(),
                };
                self.expect(func.span, &want, &ft, "callee type");
                let row = self.join(e.span, row, eff);
                (ret, row)
            }
            other => {
                let mut printer = Printer::new();
                let shown = printer.ty(&other);
                self.diags.push(
                    Diagnostic::error(codes::NOT_A_FUNCTION, "this is not a function")
                        .primary(func.span, format!("has type `{shown}`"))
                        .note("only function values can be applied"),
                );
                (self.fresh.ty(), row)
            }
        }
    }

    fn infer_cell_op(&mut self, e: &Expr, args: &[Expr], mode: Mode) -> (Type, Row) {
        let expected = if mode == Mode::Read { 1 } else { 2 };
        let name = if mode == Mode::Read { "cell_get" } else { "cell_set" };
        let mut row = Row::empty();
        let mut tys = Vec::new();
        for a in args {
            let (t, r) = self.infer(a);
            row = self.join(e.span, row, r);
            tys.push(t);
        }
        if args.len() != expected {
            self.arity_error(e.span, &Symbol::new(name), expected, args.len(), "arguments");
            return (self.fresh.ty(), row);
        }

        let region = self.fresh.ty();
        let elem = self.fresh.ty();
        let cell = Type::Con(Symbol::new("Cell"), vec![region.clone(), elem.clone()]);
        self.expect(args[0].span, &cell, &tys[0], "cell argument");
        if mode == Mode::Write {
            self.expect(args[1].span, &elem, &tys[1], "value stored into the cell");
        }

        let region = self.subst.resolve_ty(&region);
        let Some(resource) = region_of(&region).map(Symbol::new) else {
            self.diags.push(
                Diagnostic::error(
                    codes::RESOURCE_REQUIRED,
                    format!("cannot tell which `with_cell` region `{name}` acts on"),
                )
                .primary(args[0].span, "this cell's region is unknown here")
                .note(
                    "call it on the binder introduced by `with_cell[r](..) { c -> .. }`, so the \
                     `cell.read[r]` / `cell.write[r]` atoms can be discharged at the region boundary",
                ),
            );
            let ret = if mode == Mode::Read { elem } else { Type::unit() };
            return (ret, row);
        };

        let atom = EffectAtom::new(CELL, Resource::Named(resource), mode);
        self.record(atom.clone(), e.span, true);
        let row = self.join(e.span, row, Row::singleton(atom));
        let ret = if mode == Mode::Read { elem } else { Type::unit() };
        (ret, row)
    }

    fn infer_perform(
        &mut self,
        e: &Expr,
        effect: &Ident,
        op: &Ident,
        resource: Option<&Ident>,
        args: &[Expr],
    ) -> (Type, Row) {
        let mut row = Row::empty();
        let mut arg_tys = Vec::new();
        for a in args {
            let (t, r) = self.infer(a);
            row = self.join(e.span, row, r);
            arg_tys.push(t);
        }

        let Some((info, op_info)) = self.resolve_op(effect, op) else {
            return (self.fresh.ty(), row);
        };
        let Some(res) = self.resource_for(&info, &op_info, resource, e.span) else {
            return (self.fresh.ty(), row);
        };

        let (params, ret) = self.instantiate_op(&op_info);
        if params.len() != args.len() {
            self.diags.push(
                Diagnostic::error(
                    codes::ARITY_MISMATCH,
                    format!(
                        "`{}.{}` takes {} argument{}, but {}",
                        info.name,
                        op_info.name,
                        params.len(),
                        if params.len() == 1 { "" } else { "s" },
                        supplied(args.len())
                    ),
                )
                .primary(e.span, format!("expected {}, found {}", params.len(), args.len()))
                .secondary(op_info.span, "declared here"),
            );
            return (ret, row);
        }
        for ((want, got), arg) in params.iter().zip(&arg_tys).zip(args) {
            self.expect(arg.span, want, got, "operation argument");
        }

        let atom = EffectAtom::new(info.name.clone(), res, op_info.mode);
        self.record(atom.clone(), e.span, true);
        let row = self.join(e.span, row, Row::singleton(atom));
        (ret, row)
    }

    fn resolve_op(&mut self, effect: &Ident, op: &Ident) -> Option<(EffectInfo, OpInfo)> {
        let Some(info) = self.effects.get(&effect.name).cloned() else {
            self.unknown_effect(effect);
            return None;
        };
        let Some(op_info) = info.ops.get(&op.name).cloned() else {
            let known: Vec<String> = info.ops.keys().map(|k| format!("`{k}`")).collect();
            let mut d = Diagnostic::error(
                codes::UNKNOWN_OPERATION,
                format!("effect `{}` has no operation `{}`", effect.name, op.name),
            )
            .primary(op.span, "not declared")
            .secondary(info.span, format!("`{}` is declared here", info.name));
            d = if known.is_empty() {
                d.note(format!("`{}` declares no operations", info.name))
            } else {
                d.note(format!("operations of `{}`: {}", info.name, known.join(", ")))
            };
            self.diags.push(d);
            return None;
        };
        Some((info, op_info))
    }

    fn resource_for(
        &mut self,
        info: &EffectInfo,
        op: &OpInfo,
        resource: Option<&Ident>,
        span: Span,
    ) -> Option<Resource> {
        match (op.resource_param, resource) {
            (true, Some(r)) => Some(Resource::Named(r.name.clone())),
            (false, None) => Some(Resource::Singleton),
            (true, None) => {
                self.diags.push(
                    Diagnostic::error(
                        codes::RESOURCE_REQUIRED,
                        format!(
                            "`{}.{}` is declared with a resource parameter",
                            info.name, op.name
                        ),
                    )
                    .primary(span, "missing the `[resource]` label")
                    .secondary(op.span, format!("declared as `{}[r](..)`", op.name))
                    .note(format!(
                        "write `{}.{}[<resource>](..)`; the label is what lets two tests be \
                         scheduled together",
                        info.name, op.name
                    )),
                );
                None
            }
            (false, Some(r)) => {
                self.diags.push(
                    Diagnostic::error(
                        codes::RESOURCE_REQUIRED,
                        format!(
                            "`{}.{}` is not resource-parameterized",
                            info.name, op.name
                        ),
                    )
                    .primary(r.span, "unexpected resource label")
                    .secondary(op.span, "declared without `[r]`")
                    .note(format!(
                        "drop the label, or declare it as `{} {}[r](..)`",
                        op.mode.as_str(),
                        op.name
                    )),
                );
                None
            }
        }
    }

    fn instantiate_op(&mut self, op: &OpInfo) -> (Vec<Type>, Type) {
        let scheme = Scheme {
            ty_vars: op_free_vars(op),
            row_vars: vec![],
            ty: Type::Fn {
                params: op.params.clone(),
                ret: Box::new(op.ret.clone()),
                effects: Row::empty(),
            },
        };
        match instantiate(&scheme, &mut self.fresh) {
            Type::Fn { params, ret, .. } => (params, *ret),
            _ => unreachable!("operation schemes are always function types"),
        }
    }

    fn infer_handle(
        &mut self,
        e: &Expr,
        body: &Expr,
        clauses: &[HandleClause],
        return_clause: Option<&ReturnClause>,
    ) -> (Type, Row) {
        let mark = self.performs.len();
        let (body_ty, body_row) = self.infer(body);
        let after_body = self.performs.len();

        let mut handled: BTreeSet<EffectAtom> = BTreeSet::new();
        let mut clause_rows = Row::empty();
        for clause in clauses {
            let Some((info, op_info)) = self.resolve_op(&clause.effect, &clause.op) else {
                continue;
            };
            let Some(res) =
                self.resource_for(&info, &op_info, clause.resource.as_ref(), clause.span)
            else {
                continue;
            };
            let (params, ret) = self.instantiate_op(&op_info);
            if params.len() != clause.params.len() {
                self.diags.push(
                    Diagnostic::error(
                        codes::ARITY_MISMATCH,
                        format!(
                            "the clause for `{}.{}` binds {} parameter{}, but the operation takes {}",
                            info.name,
                            op_info.name,
                            clause.params.len(),
                            if clause.params.len() == 1 { "" } else { "s" },
                            params.len()
                        ),
                    )
                    .primary(clause.span, format!("expected {} parameters", params.len()))
                    .secondary(op_info.span, "declared here"),
                );
                continue;
            }

            let atom = EffectAtom::new(info.name.clone(), res, op_info.mode);
            if !handled.insert(atom.clone()) {
                self.diags.push(
                    Diagnostic::warning(
                        codes::DUPLICATE_DEFINITION,
                        format!("`{atom}` is handled more than once"),
                    )
                    .primary(clause.span, "this clause is unreachable")
                    .note("the first matching clause wins at run time"),
                );
            }

            self.env.push();
            for (name, t) in clause.params.iter().zip(&params) {
                self.env.bind(name.name.clone(), Scheme::mono(t.clone()));
            }
            let (clause_ty, clause_row) = self.infer(&clause.body);
            self.env.pop();
            self.expect(
                clause.body.span,
                &ret,
                &clause_ty,
                "a handler clause returns the operation's result",
            );
            clause_rows = self.join(e.span, clause_rows, clause_row);
        }

        self.discharge(mark..after_body, &handled);

        let (result_ty, return_row) = match return_clause {
            Some(rc) => {
                self.env.push();
                self.env.bind(rc.binder.name.clone(), Scheme::mono(body_ty));
                let (t, r) = self.infer(&rc.body);
                self.env.pop();
                (t, r)
            }
            None => (body_ty, Row::empty()),
        };

        let remaining = body_row.without(&handled);
        let row = self.join(e.span, remaining, clause_rows);
        let row = self.join(e.span, row, return_row);
        (result_ty, row)
    }

    fn infer_with_cell(
        &mut self,
        e: &Expr,
        resource: &Ident,
        init: &Expr,
        binder: &Ident,
        body: &Expr,
    ) -> (Type, Row) {
        let (init_ty, init_row) = self.infer(init);
        let region = Type::con(&region_type_name(resource.name.as_str()));
        let cell = Type::Con(Symbol::new("Cell"), vec![region, init_ty]);

        let mark = self.performs.len();
        self.env.push();
        self.env.bind(binder.name.clone(), Scheme::mono(cell));
        let (body_ty, body_row) = self.infer(body);
        self.env.pop();

        let handled: BTreeSet<EffectAtom> = [
            EffectAtom::new(CELL, Resource::Named(resource.name.clone()), Mode::Read),
            EffectAtom::new(CELL, Resource::Named(resource.name.clone()), Mode::Write),
        ]
        .into();
        self.discharge(mark..self.performs.len(), &handled);

        // The region tag only discharges the cell's atoms if the cell cannot
        // outlive it; a `Cell` in the result would carry atoms nothing can ever
        // handle again.
        let resolved = self.subst.resolve_ty(&body_ty);
        if mentions_region(&resolved, resource.name.as_str()) {
            let mut printer = Printer::new();
            self.diags.push(
                Diagnostic::error(
                    codes::TYPE_MISMATCH,
                    format!("the cell escapes its `with_cell[{}]` region", resource.name),
                )
                .primary(body.span, format!("this has type `{}`", printer.ty(&resolved)))
                .note("read the cell inside the region and return the value instead"),
            );
        }

        let row = self.join(e.span, init_row, body_row.without(&handled));
        (body_ty, row)
    }

    fn infer_match(&mut self, e: &Expr, scrutinee: &Expr, arms: &[MatchArm]) -> (Type, Row) {
        let (st, mut row) = self.infer(scrutinee);
        let result = self.fresh.ty();
        for arm in arms {
            self.env.push();
            let mut bindings = Vec::new();
            self.bind_pattern(&arm.pat, &st, &mut bindings);
            for (name, t) in bindings {
                self.env.bind(name, Scheme::mono(t));
            }
            if let Some(guard) = &arm.guard {
                let (gt, grow) = self.infer(guard);
                self.expect(guard.span, &Type::bool(), &gt, "match guard");
                row = self.join(e.span, row, grow);
            }
            let (at, arow) = self.infer(&arm.body);
            row = self.join(e.span, row, arow);
            self.env.pop();
            self.expect(arm.body.span, &result, &at, "match arms must agree");
        }
        self.check_exhaustive(e, &st, arms);
        (result, row)
    }

    fn check_exhaustive(&mut self, e: &Expr, scrutinee: &Type, arms: &[MatchArm]) {
        let unguarded = arms.iter().filter(|a| a.guard.is_none());
        if unguarded.clone().any(|a| is_irrefutable(&a.pat)) {
            return;
        }
        let scrutinee = self.subst.resolve_ty(scrutinee);

        let missing: Vec<String> = match &scrutinee {
            Type::Con(name, _) if name.as_str() == "List" => {
                missing_list_lengths(unguarded.clone())
            }
            Type::Con(name, _) if name.as_str() == "Bool" => {
                let mut want = vec![true, false];
                for arm in unguarded.clone() {
                    if let PatternKind::Lit(Lit::Bool(b)) = &arm.pat.kind {
                        want.retain(|w| w != b);
                    }
                }
                want.iter().map(|b| format!("`{b}`")).collect()
            }
            Type::Con(name, _) => {
                let variants: Vec<&CtorInfo> =
                    self.ctors.values().filter(|c| &c.type_name == name).collect();
                if variants.is_empty() {
                    vec!["other values".to_string()]
                } else {
                    let covered: FxHashSet<Symbol> = unguarded
                        .clone()
                        .filter_map(|a| match &a.pat.kind {
                            PatternKind::Ctor { name, args }
                                if args.iter().all(is_irrefutable) =>
                            {
                                Some(name.name.clone())
                            }
                            _ => None,
                        })
                        .collect();
                    variants
                        .iter()
                        .filter(|c| !covered.contains(&c.name))
                        .map(|c| format!("`{}`", c.name))
                        .collect()
                }
            }
            _ => vec!["other values".to_string()],
        };

        if missing.is_empty() {
            return;
        }
        self.diags.push(
            Diagnostic::error(codes::NON_EXHAUSTIVE_MATCH, "match does not cover every case")
                .primary(e.span, format!("not covered: {}", missing.join(", ")))
                .note("add the missing arms, or a `_` arm"),
        );
    }

    fn bind_pattern(
        &mut self,
        pat: &Pattern,
        scrutinee: &Type,
        out: &mut Vec<(Symbol, Type)>,
    ) {
        match &pat.kind {
            PatternKind::Wildcard => {}
            PatternKind::Var(id) => out.push((id.name.clone(), scrutinee.clone())),
            PatternKind::Lit(l) => {
                let t = lit_type(l);
                self.expect(pat.span, scrutinee, &t, "pattern literal");
            }
            PatternKind::Ctor { name, args } => {
                let Some(info) = self.ctors.get(&name.name).cloned() else {
                    self.diags.push(
                        Diagnostic::error(
                            codes::UNKNOWN_NAME,
                            format!("unknown constructor `{}`", name.name),
                        )
                        .primary(name.span, "not found")
                        .note("constructors come from a `type` declaration with variants"),
                    );
                    return;
                };
                let ty = instantiate(&info.scheme, &mut self.fresh);
                let (fields, result) = match ty {
                    Type::Fn { params, ret, .. } => (params, *ret),
                    other => (Vec::new(), other),
                };
                self.expect(pat.span, scrutinee, &result, "pattern constructor");
                if fields.len() != args.len() {
                    self.diags.push(
                        Diagnostic::error(
                            codes::ARITY_MISMATCH,
                            format!(
                                "constructor `{}` takes {} field{}, but the pattern binds {}",
                                info.name,
                                fields.len(),
                                if fields.len() == 1 { "" } else { "s" },
                                args.len()
                            ),
                        )
                        .primary(pat.span, format!("expected {} fields", fields.len()))
                        .secondary(info.span, "declared here"),
                    );
                    return;
                }
                for (sub, t) in args.iter().zip(&fields) {
                    self.bind_pattern(sub, t, out);
                }
            }
            PatternKind::List { items, rest } => {
                let elem = self.fresh.ty();
                self.expect(pat.span, scrutinee, &Type::list(elem.clone()), "list pattern");
                for item in items {
                    self.bind_pattern(item, &elem, out);
                }
                if let Some(rest) = rest {
                    self.bind_pattern(rest, &Type::list(elem), out);
                }
            }
            PatternKind::Record { fields, rest } => {
                let resolved = self.subst.resolve_ty(scrutinee);
                if let Type::Record(known) = &resolved {
                    for (name, sub) in fields {
                        match known.get(&name.name) {
                            Some(t) => {
                                let t = t.clone();
                                self.bind_pattern(sub, &t, out);
                            }
                            None => self.diags.push(
                                Diagnostic::error(
                                    codes::UNKNOWN_NAME,
                                    format!("no field `{}` on this record", name.name),
                                )
                                .primary(name.span, "unknown field"),
                            ),
                        }
                    }
                    if !*rest {
                        let named: FxHashSet<&Symbol> = fields.iter().map(|(n, _)| &n.name).collect();
                        let omitted: Vec<String> = known
                            .keys()
                            .filter(|k| !named.contains(k))
                            .map(|k| format!("`{k}`"))
                            .collect();
                        if !omitted.is_empty() {
                            self.diags.push(
                                Diagnostic::error(
                                    codes::TYPE_MISMATCH,
                                    "record pattern does not name every field",
                                )
                                .primary(pat.span, format!("missing: {}", omitted.join(", ")))
                                .note("add the missing fields, or end the pattern with `..`"),
                            );
                        }
                    }
                    return;
                }
                if *rest {
                    self.diags.push(
                        Diagnostic::error(
                            codes::TYPE_MISMATCH,
                            "cannot infer the record type of this pattern",
                        )
                        .primary(pat.span, "a `..` pattern needs the record type to be known")
                        .note("annotate the matched value"),
                    );
                    return;
                }
                let mut map = std::collections::BTreeMap::new();
                for (name, _) in fields {
                    map.insert(name.name.clone(), self.fresh.ty());
                }
                let want = Type::Record(map.clone());
                self.expect(pat.span, scrutinee, &want, "record pattern");
                for (name, sub) in fields {
                    let t = map[&name.name].clone();
                    self.bind_pattern(sub, &t, out);
                }
            }
        }
    }

    fn join(&mut self, span: Span, a: Row, b: Row) -> Row {
        if let (Some(x), Some(y)) = (a.tail, b.tail)
            && x != y
            && let Err(e) = unify_row(&mut self.subst, &mut self.fresh, &Row::open(x), &Row::open(y))
        {
            self.report_unify(&e, span, "this expression combines two effect variables");
        }
        a.union(&b)
    }

    fn expect(&mut self, span: Span, expected: &Type, found: &Type, context: &str) {
        if let Err(e) = unify(&mut self.subst, &mut self.fresh, expected, found) {
            self.report_unify(&e, span, context);
        }
    }

    fn report_unify(&mut self, err: &UnifyError, span: Span, context: &str) {
        let mut printer = Printer::new();
        let d = match err {
            UnifyError::Mismatch { expected, found } => {
                let (expected, found) =
                    (self.subst.resolve_ty(expected), self.subst.resolve_ty(found));
                let (e, f) = (printer.ty(&expected), printer.ty(&found));
                Diagnostic::error(codes::TYPE_MISMATCH, format!("type mismatch: {context}"))
                    .primary(span, format!("expected `{e}`, found `{f}`"))
            }
            UnifyError::Arity { expected, found } => Diagnostic::error(
                codes::ARITY_MISMATCH,
                format!("function arity mismatch: {context}"),
            )
            .primary(
                span,
                format!("expected {expected} parameter(s), found {found}"),
            ),
            UnifyError::OccursTy { var, ty } => {
                let (v, t) = (printer.ty(&Type::Var(*var)), printer.ty(ty));
                Diagnostic::error(codes::OCCURS_CHECK, format!("infinite type: {context}"))
                    .primary(span, format!("`{v}` would have to equal `{t}`"))
                    .note("this usually means a recursive value is missing a constructor")
            }
            UnifyError::OccursRow { var, row } => {
                let (v, r) = (printer.row(&Row::open(*var)), printer.row(row));
                Diagnostic::error(codes::OCCURS_CHECK, format!("infinite effect row: {context}"))
                    .primary(span, format!("`{v}` would have to equal `{r}`"))
                    .note("an effect row cannot contain itself")
            }
            UnifyError::RowMismatch { expected, found } => {
                let (expected, found) =
                    (self.subst.resolve_row(expected), self.subst.resolve_row(found));
                let extra: Vec<String> = found
                    .atoms
                    .difference(&expected.atoms)
                    .map(|a| format!("`{a}`"))
                    .collect();
                let (e, f) = (printer.row(&expected), printer.row(&found));
                let mut d = Diagnostic::error(
                    codes::EFFECT_NOT_PERMITTED,
                    format!("effect row mismatch: {context}"),
                )
                .primary(span, format!("expected {e}, found {f}"));
                if !extra.is_empty() {
                    d = d.note(format!("not permitted here: {}", extra.join(", ")));
                }
                d
            }
        };
        self.diags.push(d);
    }

    fn unknown_name(&mut self, id: &Ident) {
        let mut d =
            Diagnostic::error(codes::UNKNOWN_NAME, format!("unknown name `{}`", id.name))
                .primary(id.span, "not found in this scope");
        if let Some(near) = self.nearest_name(&id.name) {
            d = d.note(format!("a name in scope looks similar: `{near}`"));
        }
        self.diags.push(d);
    }

    fn nearest_name(&self, name: &Symbol) -> Option<Symbol> {
        let mut best: Option<(usize, Symbol)> = None;
        let candidates = self.defs.keys().chain(self.ctors.keys());
        for c in candidates {
            let d = edit_distance(name.as_str(), c.as_str());
            if d > 0 && d * 3 <= name.as_str().len().max(1) && best.as_ref().is_none_or(|(b, _)| d < *b)
            {
                best = Some((d, c.clone()));
            }
        }
        best.map(|(_, s)| s)
    }
}

struct Signature {
    ty_params: FxHashMap<Symbol, Type>,
    row_params: FxHashMap<Symbol, RowVar>,
    fn_ty: Type,
    params: Vec<Type>,
    ret: Type,
    declared: Option<Row>,
    published_row: Row,
}

fn lit_type(l: &Lit) -> Type {
    match l {
        Lit::Int(_) => Type::int(),
        Lit::Bool(_) => Type::bool(),
        Lit::Str(_) => Type::string(),
        Lit::Unit => Type::unit(),
    }
}

fn is_cell_builtin(name: &Symbol) -> bool {
    matches!(name.as_str(), "cell_get" | "cell_set")
}

fn supplied(n: usize) -> String {
    if n == 1 { "1 was supplied".to_string() } else { format!("{n} were supplied") }
}

fn contains_fn(t: &Type) -> bool {
    match t {
        Type::Fn { .. } => true,
        Type::Con(_, args) => args.iter().any(contains_fn),
        Type::Record(fields) => fields.values().any(contains_fn),
        Type::Var(_) => false,
    }
}

fn mentions_region(t: &Type, resource: &str) -> bool {
    match t {
        Type::Con(_, args) => {
            region_of(t) == Some(resource) || args.iter().any(|a| mentions_region(a, resource))
        }
        Type::Fn { params, ret, .. } => {
            params.iter().any(|p| mentions_region(p, resource)) || mentions_region(ret, resource)
        }
        Type::Record(fields) => fields.values().any(|f| mentions_region(f, resource)),
        Type::Var(_) => false,
    }
}

fn is_irrefutable(p: &Pattern) -> bool {
    match &p.kind {
        PatternKind::Wildcard | PatternKind::Var(_) => true,
        // A record is a product: naming fields never rules a value out, and the
        // field set is checked against the type separately.
        PatternKind::Record { fields, .. } => fields.iter().all(|(_, f)| is_irrefutable(f)),
        PatternKind::List { items, rest } => {
            items.is_empty() && rest.as_deref().is_some_and(is_irrefutable)
        }
        PatternKind::Lit(_) | PatternKind::Ctor { .. } => false,
    }
}

fn missing_list_lengths<'a>(arms: impl Iterator<Item = &'a MatchArm>) -> Vec<String> {
    let mut exact = FxHashSet::default();
    let mut open_from = usize::MAX;
    for arm in arms {
        let PatternKind::List { items, rest } = &arm.pat.kind else {
            continue;
        };
        if !items.iter().all(is_irrefutable) {
            continue;
        }
        match rest {
            Some(r) if is_irrefutable(r) => open_from = open_from.min(items.len()),
            Some(_) => {}
            None => {
                exact.insert(items.len());
            }
        }
    }
    let longest = exact.iter().copied().max().map_or(0, |n| n + 1);
    let bound = if open_from == usize::MAX { longest } else { open_from };
    let mut missing: Vec<String> = (0..bound)
        .filter(|n| !exact.contains(n))
        .map(|n| match n {
            0 => "the empty list".to_string(),
            1 => "lists of 1 element".to_string(),
            n => format!("lists of {n} elements"),
        })
        .collect();
    if open_from == usize::MAX {
        missing.push(format!("lists longer than {}", longest.saturating_sub(1)));
    }
    missing
}

fn op_free_vars(op: &OpInfo) -> Vec<crate::ty::TyVar> {
    let mut out = BTreeSet::new();
    for p in &op.params {
        collect_ty_vars(p, &mut out);
    }
    collect_ty_vars(&op.ret, &mut out);
    out.into_iter().collect()
}

fn collect_ty_vars(t: &Type, out: &mut BTreeSet<crate::ty::TyVar>) {
    match t {
        Type::Var(v) => {
            out.insert(*v);
        }
        Type::Con(_, args) => args.iter().for_each(|a| collect_ty_vars(a, out)),
        Type::Fn { params, ret, .. } => {
            params.iter().for_each(|p| collect_ty_vars(p, out));
            collect_ty_vars(ret, out);
        }
        Type::Record(fields) => fields.values().for_each(|f| collect_ty_vars(f, out)),
    }
}

fn dedup(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v.dedup();
    v
}

fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

fn collect_refs(e: &Expr, out: &mut Vec<Symbol>) {
    match &e.kind {
        ExprKind::Lit(_) => {}
        ExprKind::Var(id) => out.push(id.name.clone()),
        ExprKind::Binary { lhs, rhs, .. } => {
            collect_refs(lhs, out);
            collect_refs(rhs, out);
        }
        ExprKind::Unary { operand, .. } => collect_refs(operand, out),
        ExprKind::Lambda { body, .. } => collect_refs(body, out),
        ExprKind::App { func, args } => {
            collect_refs(func, out);
            args.iter().for_each(|a| collect_refs(a, out));
        }
        ExprKind::If { cond, then_branch, else_branch } => {
            collect_refs(cond, out);
            collect_refs(then_branch, out);
            collect_refs(else_branch, out);
        }
        ExprKind::Match { scrutinee, arms } => {
            collect_refs(scrutinee, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    collect_refs(g, out);
                }
                collect_refs(&arm.body, out);
            }
        }
        ExprKind::Block { stmts, tail } => {
            for s in stmts {
                match s {
                    Stmt::Expr(e) => collect_refs(e, out),
                    Stmt::Let { value, .. } => collect_refs(value, out),
                }
            }
            if let Some(t) = tail {
                collect_refs(t, out);
            }
        }
        ExprKind::Record { fields } => fields.iter().for_each(|(_, v)| collect_refs(v, out)),
        ExprKind::Field { base, .. } => collect_refs(base, out),
        ExprKind::List { items } => items.iter().for_each(|i| collect_refs(i, out)),
        ExprKind::Perform { args, .. } => args.iter().for_each(|a| collect_refs(a, out)),
        ExprKind::Handle { body, clauses, return_clause } => {
            collect_refs(body, out);
            clauses.iter().for_each(|c| collect_refs(&c.body, out));
            if let Some(rc) = return_clause {
                collect_refs(&rc.body, out);
            }
        }
        ExprKind::WithCell { init, body, .. } => {
            collect_refs(init, out);
            collect_refs(body, out);
        }
    }
}
