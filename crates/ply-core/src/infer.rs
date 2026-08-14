use crate::env::{TypeEnv, generalize, instantiate};
use crate::prelude;
use crate::print::{Printer, region_of, region_type_name};
use crate::scc::sccs;
use crate::ty::{EffectAtom, Footprint, Resource, Row, RowVar, Scheme, TyVar, Type};
use crate::unify::{Fresh, Subst, UnifyError, unify, unify_row};
use crate::{
    CheckOutput, CtorInfo, DefInfo, EffectInfo, Known, LawBinder, LawInfo, ModuleInfo, OpInfo,
    SpecInfo, TestInfo,
};
use indexmap::IndexMap;
use ply_span::{Diagnostic, Severity, Span, Symbol, codes};
use ply_syntax::ast::*;
use ply_syntax::resolve::{Namespace, Resolved, Scope, resolve};
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::BTreeSet;

/// The effect under which `with_cell` regions publish their atoms. Reserved:
/// a user `effect cell` would silently gain the power to observe region state.
const CELL: &str = "cell";

/// Bound in an `ensures` clause to the definition's return value, beside the
/// parameters rather than inside them. Not a keyword: a definition with no
/// `ensures` may still call a parameter `result`.
const RESULT: &str = "result";

const BUILTIN_TYPES: &[(&str, usize)] = &[
    ("Int", 0),
    ("Bool", 0),
    ("String", 0),
    ("Bytes", 0),
    ("Unit", 0),
    ("List", 1),
    ("Cell", 1),
    (prelude::TASK_TYPE, 1),
];

pub fn check_program_with(
    program: &Program,
    resolved: &Resolved,
    known: &Known,
) -> Result<CheckOutput, Vec<Diagnostic>> {
    let mut c = Checker::new(program, resolved, known);
    for module in &program.modules {
        c.record_module(module);
    }
    for &i in &resolved.order {
        c.module = i;
        let module = &program.modules[i];
        c.collect_types(module);
        c.collect_effects(module);
        c.collect_ctors(module);
        c.check_value_namespace(module);
    }
    for &i in &resolved.order {
        c.module = i;
        c.check_fns(&program.modules[i]);
        c.check_specs(&program.modules[i]);
    }
    // Load order, not dependency order: this position is the same index
    // `HashOutput::tests` and `HashOutput::laws` are built on, and those walk
    // the program.
    for (i, module) in program.modules.iter().enumerate() {
        c.module = i;
        c.check_tests(module);
        c.check_laws(module);
    }
    c.check_comparisons();
    c.check_simulations();
    if c.diags.iter().any(|d| d.severity == Severity::Error) {
        Err(c.diags)
    } else {
        Ok(CheckOutput {
            defs: c.defs,
            tests: c.tests,
            laws: c.laws,
            effects: c.effects,
            ctors: c.ctors,
            modules: c.modules,
        })
    }
}

pub fn check_module(module: &Module) -> Result<CheckOutput, Vec<Diagnostic>> {
    let program = Program::single(module.clone());
    let resolved = resolve(&program)?;
    check_program_with(&program, &resolved, &Known::default())
}

#[derive(Clone, Debug)]
struct TypeDecl {
    params: Vec<Symbol>,
    alias: Option<TypeExpr>,
    /// An alias body is written in its own module's scope, so expanding one at a
    /// use site in another module has to resolve its names back there.
    owner: usize,
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

/// Every map below is keyed by the program-wide name, so two modules may
/// declare the same simple name without either one being rewritten.
struct Checker<'a> {
    program: &'a Program,
    resolved: &'a Resolved,
    known: &'a Known,
    /// The module whose declarations and imports a bare name is resolved in.
    module: usize,
    subst: Subst,
    fresh: Fresh,
    env: TypeEnv,
    diags: Vec<Diagnostic>,
    types: IndexMap<Symbol, TypeDecl>,
    effects: IndexMap<Symbol, EffectInfo>,
    ctors: IndexMap<Symbol, CtorInfo>,
    defs: IndexMap<Symbol, DefInfo>,
    tests: Vec<TestInfo>,
    laws: Vec<LawInfo>,
    modules: IndexMap<Symbol, ModuleInfo>,
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
    /// `simulate` regions, checked for the same reason: a region's result type
    /// and the row of what it calls are both routinely unsolved while its own
    /// definition is still being walked.
    simulations: Vec<Simulation>,
    /// What each definition carrying clauses has its clauses typed against.
    /// Recorded where the signature is built rather than rebuilt in
    /// [`Checker::check_specs`], so that a clause cannot report a second copy of
    /// a signature error the body already reported.
    spec_envs: FxHashMap<Symbol, SpecEnv>,
    /// The clause being walked, so `result` outside an `ensures` can say where
    /// it is bound instead.
    spec_kind: Option<SpecKind>,
}

/// Where a spec expression sits, which decides both how it is named in a
/// diagnostic and — the part that matters — what its row may carry.
#[derive(Clone, Copy)]
enum SpecSite {
    Requires,
    Ensures,
    Where,
    LawBody,
}

impl SpecSite {
    fn of(kind: SpecKind) -> SpecSite {
        match kind {
            SpecKind::Requires => SpecSite::Requires,
            SpecKind::Ensures => SpecSite::Ensures,
        }
    }

    /// Only a law body has an exception, and it is exactly `{sim.read}`. A
    /// pre/post condition is a claim about one call, not about a search, and
    /// there is no seed at a call site to name; a guard decides which values the
    /// law is a claim about, and a domain that depends on a seed is a different
    /// domain per run.
    fn allowed(self) -> BTreeSet<EffectAtom> {
        match self {
            SpecSite::LawBody => [prelude::seed_atom()].into(),
            _ => BTreeSet::new(),
        }
    }

    fn phrase(self) -> &'static str {
        match self {
            SpecSite::Requires => "a `requires` clause",
            SpecSite::Ensures => "an `ensures` clause",
            SpecSite::Where => "a `where` guard",
            SpecSite::LawBody => "a law body",
        }
    }

    fn short(self) -> &'static str {
        match self {
            SpecSite::Requires => "`requires`",
            SpecSite::Ensures => "`ensures`",
            SpecSite::Where => "`where` guard",
            SpecSite::LawBody => "law body",
        }
    }
}

/// The scope a definition's clauses see: its own parameters and result, and the
/// generic names its annotations were written in.
struct SpecEnv {
    ty_params: FxHashMap<Symbol, Type>,
    row_params: FxHashMap<Symbol, RowVar>,
    params: Vec<Type>,
    ret: Type,
}

/// One `simulate` region, as [`Checker::check_simulations`] revisits it.
struct Simulation {
    span: Span,
    body_span: Span,
    body_ty: Type,
    body_row: Row,
}

impl<'a> Checker<'a> {
    fn new(program: &'a Program, resolved: &'a Resolved, known: &'a Known) -> Self {
        let mut c = Checker {
            program,
            resolved,
            known,
            module: 0,
            subst: Subst::new(),
            fresh: Fresh::default(),
            env: TypeEnv::new(),
            diags: Vec::new(),
            types: IndexMap::new(),
            effects: prelude::effects(),
            ctors: IndexMap::new(),
            defs: IndexMap::new(),
            tests: Vec::new(),
            laws: Vec::new(),
            modules: IndexMap::new(),
            ty_params: FxHashMap::default(),
            row_params: FxHashMap::default(),
            auto_ty_params: false,
            alias_stack: Vec::new(),
            performs: Vec::new(),
            comparisons: Vec::new(),
            simulations: Vec::new(),
            spec_envs: FxHashMap::default(),
            spec_kind: None,
        };
        c.install_prelude();
        c
    }

    fn name_of(&self, module: usize) -> ModuleName {
        self.program.modules[module].name.clone()
    }

    fn qualify(&self, name: &Symbol) -> Symbol {
        self.program.modules[self.module].name.qualify(name)
    }

    fn scope(&self) -> &'a Scope {
        &self.resolved.scopes[self.module]
    }

    fn record_module(&mut self, module: &Module) {
        let mut items = Vec::new();
        for item in &module.items {
            let Some(name) = item.name() else { continue };
            items.push(module.name.qualify(&name.name));
            if let Item::Type(def) = item
                && let TypeDefBody::Sum(variants) = &def.body
            {
                items.extend(variants.iter().map(|v| module.name.qualify(&v.name.name)));
            }
        }
        let mut imports: Vec<ModuleName> = Vec::new();
        for import in &module.imports {
            let name = import.module_name();
            if !imports.contains(&name) {
                imports.push(name);
            }
        }
        self.modules.insert(
            module.name.as_symbol().clone(),
            ModuleInfo {
                name: module.name.clone(),
                source: module.source,
                items,
                imports,
            },
        );
    }

    /// The order below is normative rather than an optimization: a local always
    /// wins, and a module's own items always shadow the prelude.
    fn value_key(&mut self, q: &QName) -> Option<ValueKey> {
        if !q.is_bare() {
            let name = self.global(Namespace::Value, q)?;
            return Some(ValueKey {
                name,
                prelude: false,
            });
        }
        let name = q.symbol();
        if self.env.depth_of(name).is_some_and(|depth| depth > 0) {
            return Some(ValueKey {
                name: name.clone(),
                prelude: false,
            });
        }
        if let Some(binding) = self.scope().get(Namespace::Value, name) {
            return Some(ValueKey {
                name: binding.qualified.clone(),
                prelude: false,
            });
        }
        if self.env.depth_of(name) == Some(0) {
            return Some(ValueKey {
                name: name.clone(),
                prelude: true,
            });
        }
        self.unknown_name(q);
        None
    }

    /// `cell_get` / `cell_set` are call forms rather than schemes, so the
    /// application rule has to recognise them before inferring the callee — and
    /// silently, since a local or a module item of that name is not one.
    fn cell_form(&self, q: &QName) -> Option<Mode> {
        if !q.is_bare() || self.env.depth_of(q.symbol()) != Some(0) {
            return None;
        }
        if self.scope().get(Namespace::Value, q.symbol()).is_some() {
            return None;
        }
        match q.symbol().as_str() {
            "cell_get" => Some(Mode::Read),
            "cell_set" => Some(Mode::Write),
            _ => None,
        }
    }

    /// Silent counterpart to [`Checker::global`], for the second look a
    /// diagnostic needs after the reference has already been resolved once.
    fn declared_value(&self, q: &QName) -> Option<Symbol> {
        if q.is_bare() {
            self.scope()
                .get(Namespace::Value, q.symbol())
                .map(|b| b.qualified.clone())
        } else {
            self.resolved
                .lookup(self.module, Namespace::Value, q)
                .ok()
                .map(|b| b.qualified.clone())
        }
    }

    /// The name a reference denotes, program-wide. Locals are the caller's
    /// business: they win over everything here, and are looked up first.
    fn global(&mut self, ns: Namespace, q: &QName) -> Option<Symbol> {
        if q.is_bare()
            && let Some(binding) = self.scope().get(ns, q.symbol())
        {
            return Some(binding.qualified.clone());
        }
        if q.is_bare() {
            return None;
        }
        match self.resolved.lookup(self.module, ns, q) {
            Ok(binding) => Some(binding.qualified.clone()),
            Err(d) => {
                self.diags.push(d);
                None
            }
        }
    }

    fn install_prelude(&mut self) {
        let a = self.fresh.ty_var();
        let b = self.fresh.ty_var();
        let e = self.fresh.row_var();
        let (ta, tb, re) = (Type::Var(a), Type::Var(b), Row::open(e));

        let mono = |params: Vec<Type>, ret: Type| Scheme {
            ty_vars: vec![],
            row_vars: vec![],
            ty: Type::Fn {
                params,
                ret: Box::new(ret),
                effects: Row::empty(),
            },
        };
        let poly =
            |ty_vars: Vec<_>, row_vars: Vec<_>, params: Vec<Type>, ret: Type, eff: Row| Scheme {
                ty_vars,
                row_vars,
                ty: Type::Fn {
                    params,
                    ret: Box::new(ret),
                    effects: eff,
                },
            };

        let entries: Vec<(&str, Scheme)> = vec![
            ("assert", mono(vec![Type::bool()], Type::unit())),
            (
                "assert_eq",
                poly(
                    vec![a],
                    vec![],
                    vec![ta.clone(), ta.clone()],
                    Type::unit(),
                    Row::empty(),
                ),
            ),
            (
                "len",
                poly(
                    vec![a],
                    vec![],
                    vec![Type::list(ta.clone())],
                    Type::int(),
                    Row::empty(),
                ),
            ),
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
            (
                "range",
                mono(vec![Type::int(), Type::int()], Type::list(Type::int())),
            ),
            ("int_to_string", mono(vec![Type::int()], Type::string())),
            (
                "string_concat",
                mono(vec![Type::string(), Type::string()], Type::string()),
            ),
            ("bytes_len", mono(vec![Type::bytes()], Type::int())),
            (
                "bytes_at",
                mono(vec![Type::bytes(), Type::int()], Type::int()),
            ),
            (
                "bytes_slice",
                mono(vec![Type::bytes(), Type::int(), Type::int()], Type::bytes()),
            ),
            (
                "bytes_concat",
                mono(vec![Type::bytes(), Type::bytes()], Type::bytes()),
            ),
            ("bytes_of_string", mono(vec![Type::string()], Type::bytes())),
            ("bytes_is_utf8", mono(vec![Type::bytes()], Type::bool())),
            ("string_of_bytes", mono(vec![Type::bytes()], Type::string())),
            (
                "string_of_bytes_lossy",
                mono(vec![Type::bytes()], Type::string()),
            ),
            // `len` is `(List<a>) -> Int` and Ply has no type-directed
            // dispatch, so a String's length needs its own name.
            ("string_len", mono(vec![Type::string()], Type::int())),
            (
                "string_slice",
                mono(
                    vec![Type::string(), Type::int(), Type::int()],
                    Type::string(),
                ),
            ),
            (
                "string_split",
                mono(
                    vec![Type::string(), Type::string()],
                    Type::list(Type::string()),
                ),
            ),
            ("string_trim", mono(vec![Type::string()], Type::string())),
            ("string_lower", mono(vec![Type::string()], Type::string())),
            ("string_upper", mono(vec![Type::string()], Type::string())),
            (
                "string_starts_with",
                mono(vec![Type::string(), Type::string()], Type::bool()),
            ),
            (
                "string_ends_with",
                mono(vec![Type::string(), Type::string()], Type::bool()),
            ),
            (
                "string_contains",
                mono(vec![Type::string(), Type::string()], Type::bool()),
            ),
            (
                "string_find",
                mono(vec![Type::string(), Type::string()], Type::int()),
            ),
            (
                "panic",
                poly(
                    vec![a],
                    vec![],
                    vec![Type::string()],
                    ta.clone(),
                    Row::empty(),
                ),
            ),
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
            poly(
                vec![a, b],
                vec![],
                vec![cell_ty.clone()],
                ta.clone(),
                Row::empty(),
            ),
        );
        self.env.bind_global(
            Symbol::new("cell_set"),
            poly(
                vec![a, b],
                vec![],
                vec![cell_ty, ta],
                Type::unit(),
                Row::empty(),
            ),
        );
    }

    fn collect_types(&mut self, module: &Module) {
        for item in &module.items {
            let Item::Type(def) = item else { continue };
            let name = module.name.qualify(&def.name.name);
            if let Some(prev) = self.types.get(&name) {
                self.duplicate(&def.name, prev.span, "type");
                continue;
            }
            if BUILTIN_TYPES
                .iter()
                .any(|(b, _)| *b == def.name.name.as_str())
            {
                self.diags.push(
                    Diagnostic::error(
                        codes::DUPLICATE_DEFINITION,
                        format!("`{}` is a builtin type", def.name.name),
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
                    owner: self.module,
                    span: def.span,
                },
            );
        }
    }

    fn collect_effects(&mut self, module: &Module) {
        for item in &module.items {
            let Item::Effect(def) = item else { continue };
            let name = module.name.qualify(&def.name.name);
            if def.name.name.as_str() == CELL {
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
            if prelude::is_prelude_effect(&name) {
                self.prelude_collision(&def.name, &name);
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
                        scheme: None,
                    },
                );
            }
            self.effects.insert(
                name.clone(),
                EffectInfo {
                    name,
                    module: module.name.clone(),
                    simple_name: def.name.name.clone(),
                    nondet: def.nondet,
                    ops,
                    span: def.span,
                },
            );
        }
    }

    fn collect_ctors(&mut self, module: &Module) {
        for item in &module.items {
            let Item::Type(def) = item else { continue };
            let TypeDefBody::Sum(variants) = &def.body else {
                continue;
            };
            let type_name = module.name.qualify(&def.name.name);
            let Some(decl) = self.types.get(&type_name).cloned() else {
                continue;
            };

            self.ty_params.clear();
            let mut vars = Vec::new();
            for p in &decl.params {
                let v = self.fresh.ty_var();
                vars.push(v);
                self.ty_params.insert(p.clone(), Type::Var(v));
            }
            let result = Type::Con(
                type_name.clone(),
                vars.iter().map(|v| Type::Var(*v)).collect(),
            );
            for (index, variant) in variants.iter().enumerate() {
                let ctor_name = module.name.qualify(&variant.name.name);
                if let Some(prev) = self.ctors.get(&ctor_name) {
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
                let scheme = Scheme {
                    ty_vars: vars.clone(),
                    row_vars: vec![],
                    ty,
                };
                self.env.bind_global(ctor_name.clone(), scheme.clone());
                self.ctors.insert(
                    ctor_name.clone(),
                    CtorInfo {
                        name: ctor_name,
                        module: module.name.clone(),
                        simple_name: variant.name.name.clone(),
                        type_name: type_name.clone(),
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

    /// Functions and constructors share one namespace, so one module cannot
    /// declare both under a name. It takes a pass of its own because the two
    /// kinds are collected into separate tables: neither can see the other, and
    /// the later binder silently wins, leaving the constructor unreachable.
    fn check_value_namespace(&mut self, module: &Module) {
        let mut seen: FxHashMap<Symbol, (Span, bool)> = FxHashMap::default();
        for item in &module.items {
            let declared: Vec<(&Ident, bool)> = match item {
                Item::Fn(def) => vec![(&def.name, true)],
                Item::Type(def) => match &def.body {
                    TypeDefBody::Sum(variants) => {
                        variants.iter().map(|v| (&v.name, false)).collect()
                    }
                    TypeDefBody::Alias(_) => Vec::new(),
                },
                Item::Effect(_) | Item::Test(_) | Item::Law(_) => Vec::new(),
            };
            for (name, is_fn) in declared {
                match seen.get(&name.name) {
                    // Two of a kind is already reported where that kind's table
                    // is built, in that kind's own wording.
                    Some(&(_, was_fn)) if was_fn == is_fn => {}
                    Some(&(first, _)) => self.value_collision(module, name, first, is_fn),
                    None => {
                        seen.insert(name.name.clone(), (name.span, is_fn));
                    }
                }
            }
        }
    }

    fn value_collision(&mut self, module: &Module, name: &Ident, first: Span, later_is_fn: bool) {
        let (later, earlier) = if later_is_fn {
            ("function", "constructor")
        } else {
            ("constructor", "function")
        };
        let where_ = if module.name.is_anonymous() {
            String::new()
        } else {
            format!(" in module `{}`", module.name)
        };
        self.diags.push(
            Diagnostic::error(
                codes::DUPLICATE_DEFINITION,
                format!("`{}` is defined twice{where_}", name.name),
            )
            .primary(name.span, format!("redefined here as a {later}"))
            .secondary(first, format!("first defined here as a {earlier}"))
            .note(format!(
                "functions and constructors share one namespace: nothing written `{}` could say \
                 which of the two it means",
                name.name
            ))
            .note("rename one of them"),
        );
    }

    /// The prelude occupies four program-wide names. Only an anonymous module
    /// can claim one — anywhere else the declaration is `<module>.clock` and
    /// shadows the prelude by the ordinary resolution order.
    fn prelude_collision(&mut self, name: &Ident, qualified: &Symbol) {
        self.diags.push(
            Diagnostic::error(
                codes::DUPLICATE_DEFINITION,
                format!("`{qualified}` is a prelude effect"),
            )
            .primary(name.span, "declared by the language")
            .note(format!(
                "the language declares `{}` and `{}`; `simulate {{ .. }}` handles the first three",
                prelude::SIMULATED.join("`, `"),
                prelude::SIM
            ))
            .note(format!(
                "rename this effect, or put it in a named module, where its program-wide name \
                 would be `<module>.{}` and would shadow the prelude",
                name.name
            )),
        );
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
                fields
                    .iter()
                    .map(|(k, v)| (k.name.clone(), self.conv_type(v)))
                    .collect(),
            ),
            TypeExpr::Fn {
                params,
                ret,
                effects,
                span: _,
            } => Type::Fn {
                params: params.iter().map(|p| self.conv_type(p)).collect(),
                ret: Box::new(self.conv_type(ret)),
                effects: effects
                    .as_ref()
                    .map(|r| self.conv_row(r))
                    .unwrap_or_default(),
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
            .note(format!(
                "add `{}` to the generic list, e.g. `fn f<{}>(..)`",
                id.name, id.name
            )),
        );
        self.fresh.ty()
    }

    fn conv_con(&mut self, name: &QName, args: &[TypeExpr], span: Span) -> Type {
        if name.is_bare() && args.is_empty() && self.ty_params.contains_key(name.symbol()) {
            return self.ty_params[name.symbol()].clone();
        }
        let args: Vec<Type> = args.iter().map(|a| self.conv_type(a)).collect();

        if name.is_bare()
            && let Some((_, arity)) = BUILTIN_TYPES
                .iter()
                .find(|(b, _)| *b == name.symbol().as_str())
        {
            if args.len() != *arity {
                self.arity_error(span, name.symbol(), *arity, args.len(), "type arguments");
                return self.fresh.ty();
            }
            if name.symbol().as_str() == "Cell" {
                // A written `Cell<T>` says nothing about which region the cell
                // came from, so leave the region open and let unification with a
                // `with_cell` binder decide it.
                let region = self.fresh.ty();
                return Type::Con(Symbol::new("Cell"), vec![region, args[0].clone()]);
            }
            return Type::Con(name.symbol().clone(), args);
        }

        let Some(qualified) = self.global(Namespace::Type, name) else {
            if name.is_bare() {
                if args.is_empty() && self.auto_ty_params {
                    return self.type_param(&name.name);
                }
                self.unknown_type(name);
            }
            return self.fresh.ty();
        };

        // Declared but not collected: the declaration itself was rejected, and
        // that diagnostic is the one worth reading.
        let Some(decl) = self.types.get(&qualified).cloned() else {
            return self.fresh.ty();
        };

        if args.len() != decl.params.len() {
            self.arity_error(
                span,
                name.symbol(),
                decl.params.len(),
                args.len(),
                "type arguments",
            );
            return self.fresh.ty();
        }

        let Some(alias) = decl.alias.clone() else {
            return Type::Con(qualified, args);
        };

        if self.alias_stack.contains(&qualified) {
            self.diags.push(
                Diagnostic::error(
                    codes::UNKNOWN_TYPE,
                    format!("type alias `{}` expands into itself", name.symbol()),
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
        self.alias_stack.push(qualified);
        let owner = std::mem::replace(&mut self.module, decl.owner);
        let expanded = self.conv_type(&alias);
        self.module = owner;
        self.alias_stack.pop();
        self.ty_params = saved;
        expanded
    }

    fn unknown_type(&mut self, name: &QName) {
        let mut d = Diagnostic::error(
            codes::UNKNOWN_TYPE,
            format!("unknown type `{}`", name.symbol()),
        )
        .primary(name.span, "not found")
        .note("declare it with `type`, or check the spelling");
        if let Some(module) = self.exporter(Namespace::Type, name.symbol()) {
            d = d.note(format!(
                "module `{module}` exports it: `import {module} ({})`",
                name.symbol()
            ));
        }
        self.diags.push(d);
    }

    /// A module that exports this name, so a missing `import` reads as a missing
    /// import rather than a missing definition.
    fn exporter(&self, ns: Namespace, name: &Symbol) -> Option<ModuleName> {
        let me = self.module;
        self.resolved
            .declarations
            .iter()
            .enumerate()
            .find(|(i, d)| *i != me && d.get(ns, name).is_some_and(|decl| decl.vis.is_public()))
            .map(|(i, _)| self.name_of(i))
    }

    fn arity_error(
        &mut self,
        span: Span,
        name: &Symbol,
        expected: usize,
        found: usize,
        what: &str,
    ) {
        let what = if expected == 1 {
            what.trim_end_matches('s')
        } else {
            what
        };
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
        let effect = self.effect_name(&a.effect)?;
        let resource = match &a.resource {
            Some(r) => Resource::Named(r.name.clone()),
            None => Resource::Singleton,
        };
        if let Some(info) = self.effects.get(&effect)
            && !info.ops.values().any(|o| o.mode == a.mode)
        {
            let known: Vec<String> = info
                .ops
                .values()
                .map(|o| format!("{}.{}", info.simple_name, o.mode.as_str()))
                .collect();
            self.diags.push(
                Diagnostic::error(
                    codes::UNKNOWN_OPERATION,
                    format!(
                        "effect `{effect}` declares no `{}` operation",
                        a.mode.as_str()
                    ),
                )
                .primary(
                    a.span,
                    format!("no `{}` operation to perform", a.mode.as_str()),
                )
                .note(format!(
                    "`{effect}` can perform: {}",
                    dedup(known).join(", ")
                )),
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

    /// `cell` is not in [`Checker::effects`] — it is the builtin regions
    /// perform under, and no declaration produces it.
    ///
    /// The prelude effects are, and are consulted last: a module's own items and
    /// its imports shadow them, which is the ordinary resolution order and what
    /// leaves `examples/clock.ply`'s `effect clock` uninvolved.
    fn effect_name(&mut self, q: &QName) -> Option<Symbol> {
        if q.is_bare() && q.symbol().as_str() == CELL {
            return Some(Symbol::new(CELL));
        }
        match self.global(Namespace::Effect, q) {
            Some(name) if self.effects.contains_key(&name) => Some(name),
            Some(_) => None,
            None => {
                if q.is_bare() && prelude::is_prelude_effect(q.symbol()) {
                    return Some(q.symbol().clone());
                }
                if q.is_bare() {
                    self.unknown_effect(q);
                }
                None
            }
        }
    }

    fn unknown_effect(&mut self, q: &QName) {
        let known: Vec<String> = self
            .scope()
            .effects
            .keys()
            .map(|k| format!("`{k}`"))
            .collect();
        let mut d = Diagnostic::error(
            codes::UNKNOWN_EFFECT,
            format!("unknown effect `{}`", q.symbol()),
        )
        .primary(q.span, "not declared");
        d = if known.is_empty() {
            d.note("declare it with `effect <name> { .. }`")
        } else {
            d.note(format!("effects in scope: {}", known.join(", ")))
        };
        if let Some(module) = self.exporter(Namespace::Effect, q.symbol()) {
            d = d.note(format!(
                "module `{module}` declares it: `import {module}` and write `{}::{}`",
                module.default_binder(),
                q.symbol()
            ));
        }
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

        // Only a bare reference can reach a definition in this same module: a
        // qualified one names an imported module, and a module cannot import
        // itself. So mutual recursion stays a within-module question.
        let adj: Vec<Vec<usize>> = fns
            .iter()
            .map(|d| {
                let mut refs = Vec::new();
                collect_refs(&d.body, &mut refs);
                let mut seen = FxHashSet::default();
                refs.iter()
                    .filter(|r| r.is_bare())
                    .filter_map(|r| index.get(r.symbol()).copied())
                    .filter(|i| seen.insert(*i))
                    .collect()
            })
            .collect();

        for comp in sccs(fns.len(), &adj) {
            let names: Vec<Symbol> = comp
                .iter()
                .map(|&i| self.qualify(&fns[i].name.name))
                .collect();
            if self.publish_known(module, &comp, &fns, &names) {
                continue;
            }
            let mut sigs = Vec::new();
            for (slot, &i) in comp.iter().enumerate() {
                let sig = self.signature(fns[i]);
                self.env
                    .bind_global(names[slot].clone(), Scheme::mono(sig.fn_ty.clone()));
                sigs.push(sig);
            }
            for (slot, &i) in comp.iter().enumerate() {
                self.check_fn_body(fns[i], &sigs[slot]);
                self.record_spec_env(fns[i], &names[slot], &sigs[slot]);
            }
            for sig in &sigs {
                self.close_unreachable_row(sig);
            }
            for name in &names {
                self.env.remove_global(name);
            }
            for (slot, &i) in comp.iter().enumerate() {
                let def = fns[i];
                let scheme = generalize(&self.subst, &mut self.env, &sigs[slot].fn_ty);
                let row = self.subst.resolve_row(&sigs[slot].published_row);
                self.env.bind_global(names[slot].clone(), scheme.clone());
                self.defs.insert(
                    names[slot].clone(),
                    DefInfo {
                        name: names[slot].clone(),
                        module: module.name.clone(),
                        simple_name: def.name.name.clone(),
                        scheme,
                        footprint: Footprint(row.atoms),
                        spec: Vec::new(),
                        span: def.span,
                    },
                );
            }
        }
    }

    /// Publishes a mutually recursive group straight from the interfaces the
    /// caller supplied, without walking a single body. All or nothing: the
    /// members of a group are typed together, so one of them missing means the
    /// group has to be inferred.
    ///
    /// The scheme's quantified variables are renumbered onto this run's counter
    /// first. A stored scheme is canonicalized, so its variables start at zero,
    /// and this run has almost certainly already bound those numbers to
    /// something else — leaving them alone would let `generalize` resolve them
    /// through the substitution and read a variable that was never this
    /// scheme's.
    fn publish_known(
        &mut self,
        module: &Module,
        comp: &[usize],
        fns: &[&FnDef],
        names: &[Symbol],
    ) -> bool {
        if !names.iter().all(|n| self.known.defs.contains_key(n)) {
            return false;
        }
        for (slot, &i) in comp.iter().enumerate() {
            let entry = self.known.defs[&names[slot]].clone();
            let scheme = self.adopt(&entry.scheme);
            // A spec is erased from the definition's hash, so a spec edit does
            // not move it and gate 2 skips a definition whose clause is new.
            // The clauses are typed anyway, against the restored interface.
            if !fns[i].spec.is_empty() {
                let sig = self.signature(fns[i]);
                let published = instantiate(&scheme, &mut self.fresh);
                let _ = unify(&mut self.subst, &mut self.fresh, &sig.fn_ty, &published);
                self.record_spec_env(fns[i], &names[slot], &sig);
            }
            self.env.bind_global(names[slot].clone(), scheme.clone());
            self.defs.insert(
                names[slot].clone(),
                DefInfo {
                    name: names[slot].clone(),
                    module: module.name.clone(),
                    simple_name: fns[i].name.name.clone(),
                    scheme,
                    footprint: entry.footprint.clone(),
                    spec: Vec::new(),
                    span: fns[i].span,
                },
            );
        }
        true
    }

    /// The same scheme with its quantified variables drawn from this run.
    fn adopt(&mut self, scheme: &Scheme) -> Scheme {
        if scheme.ty_vars.is_empty() && scheme.row_vars.is_empty() {
            return scheme.clone();
        }
        let ty_vars: Vec<TyVar> = scheme.ty_vars.iter().map(|_| self.fresh.ty_var()).collect();
        let row_vars: Vec<RowVar> = scheme
            .row_vars
            .iter()
            .map(|_| self.fresh.row_var())
            .collect();
        let ty = crate::env::rename_scheme(scheme, &ty_vars, &row_vars);
        Scheme {
            ty_vars,
            row_vars,
            ty,
        }
    }

    /// A tail that no parameter or result type mentions can never be filled in
    /// by a caller, so leaving it quantified would publish a function as
    /// effect-polymorphic when it is simply pure.
    fn close_unreachable_row(&mut self, sig: &Signature) {
        let Type::Fn {
            params,
            ret,
            effects,
        } = self.subst.resolve_ty(&sig.fn_ty)
        else {
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
            let _ = unify_row(
                &mut self.subst,
                &mut self.fresh,
                &Row::empty(),
                &Row::open(tail),
            );
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
                if let Err(e) = unify_row(
                    &mut self.subst,
                    &mut self.fresh,
                    &sig.published_row,
                    &body_row,
                ) {
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

        let extra: Vec<EffectAtom> = inferred
            .atoms
            .difference(&declared.atoms)
            .cloned()
            .collect();
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
                .secondary(
                    ann_span,
                    format!("declared row is {}", printer.row(&declared)),
                )
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
                let target = Row {
                    atoms: BTreeSet::new(),
                    tail,
                };
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

    fn record_spec_env(&mut self, def: &FnDef, name: &Symbol, sig: &Signature) {
        if def.spec.is_empty() {
            return;
        }
        self.spec_envs.insert(
            name.clone(),
            SpecEnv {
                ty_params: sig.ty_params.clone(),
                row_params: sig.row_params.clone(),
                params: sig.params.clone(),
                ret: sig.ret.clone(),
            },
        );
    }

    /// Clauses are typed after every definition in the module is published,
    /// rather than beside the body they are attached to: a clause may name
    /// anything the module can reach, while a body's SCC decides only what the
    /// body itself reaches. Every module this one imports was checked earlier in
    /// `Resolved::order`, so every name a clause can write is already bound.
    ///
    /// This pass runs whether or not gate 2 skipped the definition. A spec is
    /// erased from the definition's hash, so a spec edit does not move it, and
    /// skipping here would leave a new clause never typed.
    fn check_specs(&mut self, module: &Module) {
        for item in &module.items {
            let Item::Fn(def) = item else { continue };
            if def.spec.is_empty() {
                continue;
            }
            let name = module.name.qualify(&def.name.name);
            let Some(env) = self.spec_envs.remove(&name) else {
                continue;
            };
            self.check_result_shadow(def);

            let mut spec = Vec::with_capacity(def.spec.len());
            for (index, clause) in def.spec.iter().enumerate() {
                let footprint = self.check_clause(def, clause, &env);
                spec.push(SpecInfo {
                    kind: clause.kind,
                    index,
                    footprint,
                    span: clause.span,
                });
            }
            if let Some(info) = self.defs.get_mut(&name) {
                info.spec = spec;
            }
        }
        self.ty_params.clear();
        self.row_params.clear();
    }

    fn check_clause(&mut self, def: &FnDef, clause: &SpecClause, env: &SpecEnv) -> Footprint {
        self.ty_params = env.ty_params.clone();
        self.row_params = env.row_params.clone();
        self.performs.clear();

        self.env.push();
        for (p, t) in def.params.iter().zip(&env.params) {
            self.env.bind(p.name.name.clone(), Scheme::mono(t.clone()));
        }
        if clause.kind == SpecKind::Ensures {
            self.env
                .bind(Symbol::new(RESULT), Scheme::mono(env.ret.clone()));
        }
        let outer = self.spec_kind.replace(clause.kind);
        let (ty, row) = self.infer(&clause.expr);
        self.spec_kind = outer;
        self.env.pop();

        self.expect(
            clause.expr.span,
            &Type::bool(),
            &ty,
            "a spec clause states a proposition",
        );
        let row = self.subst.resolve_row(&row);
        self.check_spec_purity(SpecSite::of(clause.kind), clause.span, &row);
        row.to_footprint()
    }

    /// `result` is introduced beside the parameters, so a definition that has
    /// both is reported rather than silently resolved. Shadowing the parameter
    /// would change what an existing postcondition means depending on a
    /// parameter name; shadowing `result` would make the postcondition
    /// unwritable with no diagnostic.
    fn check_result_shadow(&mut self, def: &FnDef) {
        let Some(ensures) = def.spec.iter().find(|c| c.kind == SpecKind::Ensures) else {
            return;
        };
        for p in &def.params {
            if p.name.name.as_str() != RESULT {
                continue;
            }
            self.diags.push(
                Diagnostic::error(
                    codes::DUPLICATE_DEFINITION,
                    format!("`{RESULT}` is bound twice on `{}`", def.name.name),
                )
                .primary(p.name.span, "this parameter is named `result`")
                .secondary(ensures.span, "`ensures` binds `result` to the return value")
                .note(
                    "rename the parameter; a definition with no `ensures` may still call one \
                     `result`",
                ),
            );
        }
    }

    /// A spec expression's row must be empty. A claim that can perform an effect
    /// can change what it observes, and an obligation that writes to the
    /// resource it judges reports the post-state it caused.
    ///
    /// The one exception belongs to [`SpecSite::LawBody`], which may carry
    /// `sim.read` — a read of an input no program can write, and the only way a
    /// claim about every interleaving can be stated. It is structural rather
    /// than a parameter so that a guard cannot be handed it by mistake.
    fn check_spec_purity(&mut self, site: SpecSite, at: Span, row: &Row) {
        let allowed = site.allowed();
        for atom in row.atoms.difference(&allowed) {
            let (span, direct) = match self.site_for(atom) {
                Some(s) => (s.span, s.direct),
                None => (at, false),
            };
            let performs = if direct { "performs" } else { "reaches" };
            let mut d = Diagnostic::error(
                codes::EFFECT_IN_SPEC,
                format!("{} cannot perform an effect", site.phrase()),
            )
            .primary(span, format!("{performs} `{atom}`"))
            .secondary(at, format!("this {} must be pure", site.short()))
            .note(
                "a claim that can perform an effect can change what it observes: an obligation \
                 that writes to the resource it judges reports the post-state it caused",
            )
            .note("and a discharge evaluates it hundreds of times, against a world nothing set up");
            for atom in &allowed {
                d = d.note(format!(
                    "a law body may carry `{atom}` — the seed a `simulate` region reads — and \
                     nothing else"
                ));
            }
            self.diags.push(d.note(format!(
                "compute this with a pure function, or handle `{}` inside the definition and \
                 state a claim about its result",
                atom.effect
            )));
        }

        if row.tail.is_some() {
            let mut printer = Printer::new();
            let shown = printer.row(row);
            self.diags.push(
                Diagnostic::error(
                    codes::EFFECT_IN_SPEC,
                    format!("the row of this {} is not known to be empty", site.short()),
                )
                .primary(at, format!("has row {shown}"))
                .note(
                    "an effect variable is pure for some instantiations and not for others, and a \
                     claim has to hold for every one",
                )
                .note("call something whose row is closed and empty here"),
            );
        }
    }

    fn check_laws(&mut self, module: &Module) {
        let mut labels: FxHashMap<&str, Span> = FxHashMap::default();
        for item in &module.items {
            let Item::Law(def) = item else { continue };
            if let Some(&first) = labels.get(def.name.as_str()) {
                self.diags.push(
                    Diagnostic::error(
                        codes::DUPLICATE_DEFINITION,
                        format!("two laws are labelled {:?}", def.name),
                    )
                    .primary(def.name_span, "relabelled here")
                    .secondary(first, "first labelled here")
                    .note("a law is keyed `<module>.<label>`, so one label names one claim"),
                );
                continue;
            }
            labels.insert(def.name.as_str(), def.name_span);
            self.check_law(module, def);
        }
    }

    fn check_law(&mut self, module: &Module, def: &LawDef) {
        self.ty_params.clear();
        self.row_params.clear();
        self.performs.clear();

        // A binder's type may name a type variable, which is quantified over the
        // law: the prover reads one as an uninterpreted sort, so a proof over it
        // holds for every instantiation.
        self.auto_ty_params = true;
        let types: Vec<Type> = def.binders.iter().map(|b| self.law_binder(b)).collect();
        self.auto_ty_params = false;
        let sorts: Vec<TyVar> = self
            .ty_params
            .values()
            .filter_map(|t| match t {
                Type::Var(v) => Some(*v),
                _ => None,
            })
            .collect();
        for v in sorts {
            self.subst.mark_rigid_ty(v);
        }

        self.env.push();
        for (b, t) in def.binders.iter().zip(&types) {
            self.env.bind(b.name.name.clone(), Scheme::mono(t.clone()));
        }

        if let Some(guard) = &def.guard {
            self.performs.clear();
            let (ty, row) = self.infer(guard);
            self.expect(
                guard.span,
                &Type::bool(),
                &ty,
                "a `where` guard states a proposition",
            );
            // Exactly pure, unlike the body: the guard decides which values the
            // law is a claim about, and a domain that depends on a seed is a
            // different domain per run.
            let row = self.subst.resolve_row(&row);
            self.check_spec_purity(SpecSite::Where, guard.span, &row);
        }

        self.performs.clear();
        let (ty, row) = self.infer(&def.body);
        self.expect(
            def.body.span,
            &Type::bool(),
            &ty,
            "a law states a proposition",
        );
        let row = self.subst.resolve_row(&row);
        self.check_spec_purity(SpecSite::LawBody, def.body.span, &row);
        self.env.pop();

        let binders: Vec<LawBinder> = def
            .binders
            .iter()
            .zip(&types)
            .map(|(b, t)| LawBinder {
                name: b.name.name.clone(),
                ty: self.subst.resolve_ty(t),
                span: b.span,
            })
            .collect();
        self.laws.push(LawInfo {
            name: def.name.clone(),
            module: module.name.clone(),
            key: module.name.qualify(&Symbol::new(&def.name)),
            index: self.laws.len(),
            binders,
            has_guard: def.guard.is_some(),
            footprint: row.to_footprint(),
            span: def.span,
        });
        self.ty_params.clear();
        self.row_params.clear();
    }

    /// A binder's type has to be one the generator can inhabit, or the law is a
    /// claim nobody can ever check. Refused where it is written rather than
    /// reported as a gap when it is discharged, because that is where the author
    /// can still do something about it.
    fn law_binder(&mut self, binder: &Binder) -> Type {
        if let Some(span) = self.handler_mention(&binder.ty) {
            self.handler_quantification(span);
            return self.fresh.ty();
        }
        if let Some(reason) = written_row_reason(&binder.ty) {
            self.unquantifiable(binder, reason);
            return self.fresh.ty();
        }
        let ty = self.conv_type(&binder.ty);
        if let Some(reason) = self.ungeneratable(&ty, &mut FxHashSet::default()) {
            self.unquantifiable(binder, reason);
        }
        ty
    }

    fn unquantifiable(&mut self, binder: &Binder, reason: String) {
        self.diags.push(
            Diagnostic::error(
                codes::UNQUANTIFIABLE_TYPE,
                format!("`forall` cannot quantify over `{}`", binder.name.name),
            )
            .primary(binder.ty.span(), reason)
            .note(
                "a law over a type nothing can produce is a claim nothing can ever check, so it \
                 is refused here rather than reported as a gap at discharge time",
            ),
        );
    }

    /// A handler is syntax rather than a value, so there is nothing for a binder
    /// to range over. Recognised by the name it would have to be written under,
    /// because the type it needs does not exist and the errors that absence
    /// produces — an unknown type, or a stray type variable — say nothing about
    /// why.
    fn handler_mention(&self, te: &TypeExpr) -> Option<Span> {
        let named = |name: &Symbol, span: Span| -> Option<Span> {
            let handler = matches!(name.as_str(), "Handler" | "handler");
            (handler && self.scope().get(Namespace::Type, name).is_none()).then_some(span)
        };
        match te {
            TypeExpr::Unit { .. } => None,
            TypeExpr::Var(id) => named(&id.name, id.span),
            TypeExpr::Con { name, args, span } => name
                .is_bare()
                .then(|| named(name.symbol(), *span))
                .flatten()
                .or_else(|| args.iter().find_map(|a| self.handler_mention(a))),
            TypeExpr::Fn { params, ret, .. } => params
                .iter()
                .chain([ret.as_ref()])
                .find_map(|t| self.handler_mention(t)),
            TypeExpr::Record { fields, .. } => {
                fields.iter().find_map(|(_, t)| self.handler_mention(t))
            }
        }
    }

    fn handler_quantification(&mut self, span: Span) {
        self.diags.push(
            Diagnostic::error(
                codes::UNQUANTIFIABLE_TYPE,
                "`forall` cannot quantify over a handler",
            )
            .primary(span, "a handler is syntax, not a value")
            .note(
                "`handle body with { .. }` is an expression form: there is no type a handler \
                 inhabits and no value for a binder to range over",
            )
            .note(
                "docs/adr/0007-specs.md §3.2 records what a handler-parametric law would take — \
                 a handler type, a `Value::Handler`, a normalization story and a generator — and \
                 why the evidence such a law could produce is too weak to be worth it yet",
            )
            .note("quantify over the values the handler would answer with instead"),
        );
    }

    /// Whether the generator can inhabit this type. Walks a user type's declared
    /// fields as well as its arguments, so a record holding a `Cell` is refused
    /// with the same message the `Cell` itself would get.
    fn ungeneratable(&self, ty: &Type, seen: &mut FxHashSet<Symbol>) -> Option<String> {
        match ty {
            Type::Var(_) => None,
            Type::Con(name, args) => {
                if name.as_str() == "Cell" {
                    return Some(
                        "a `Cell` is created by `with_cell` and exists only inside its region"
                            .to_string(),
                    );
                }
                if name.as_str() == prelude::TASK_TYPE {
                    return Some(
                        "a `Task` is a key into the scheduler of the `simulate` region that \
                         spawned it"
                            .to_string(),
                    );
                }
                if let Some(reason) = args.iter().find_map(|a| self.ungeneratable(a, seen)) {
                    return Some(reason);
                }
                if !seen.insert(name.clone()) {
                    return None;
                }
                self.ctors
                    .values()
                    .filter(|c| &c.type_name == name)
                    .flat_map(|c| &c.fields)
                    .find_map(|f| self.ungeneratable(f, seen))
            }
            Type::Fn {
                params,
                ret,
                effects,
            } => {
                if !effects.is_pure() {
                    return Some(format!(
                        "a function with row {effects} cannot be applied inside a pure body"
                    ));
                }
                params
                    .iter()
                    .chain([ret.as_ref()])
                    .find_map(|t| self.ungeneratable(t, seen))
            }
            Type::Record(fields) => fields.values().find_map(|f| self.ungeneratable(f, seen)),
        }
    }

    fn check_tests(&mut self, module: &Module) {
        let known = self.known.tests.get(module.name.as_symbol());
        let mut position = 0;
        for item in &module.items {
            let Item::Test(def) = item else { continue };
            let cached = known
                .and_then(|slots| slots.get(position))
                .and_then(Option::as_ref);
            position += 1;

            // A cached footprint carries the determinism verdict with it: an
            // effect's `nondet` marker is part of its declaration's hash, so a
            // test whose hash is unchanged cannot have acquired a nondeterminism
            // that was absent when it was checked.
            let footprint = match cached {
                Some(entry) => entry.footprint.clone(),
                None => {
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
                    footprint
                }
            };
            self.tests.push(TestInfo {
                name: def.name.clone(),
                module: module.name.clone(),
                key: module.name.qualify(&Symbol::new(&def.name)),
                index: self.tests.len(),
                nondet: def.nondet,
                footprint,
                span: def.span,
            });
        }
    }

    fn check_determinism(&mut self, def: &TestDef, footprint: &Footprint) {
        for atom in footprint.atoms() {
            let Some(info) = self.effects.get(&atom.effect) else {
                continue;
            };
            if !info.nondet {
                continue;
            }
            let effect = atom.effect.clone();
            // The language ships a handler for three of these, so pointing at
            // `handle .. with { .. }` would send a reader to write by hand what
            // `simulate` already installs — and with none of its ordering
            // semantics.
            let remedy = if prelude::is_simulated(atom) {
                "handle it here, e.g. `simulate { <body> }`".to_string()
            } else {
                format!(
                    "handle it here, e.g. `handle <body> with {{ {} }}`",
                    self.example_clause(&effect, atom)
                )
            };
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
            .secondary(
                def.name_span,
                format!("test `{}` is deterministic", def.name),
            );
            if !direct {
                d = d.note(format!(
                    "`{atom}` is performed inside something this expression calls"
                ));
            }
            d = d.note(remedy).note(
                "or declare this `test/nondet`, which opts out of the cache and re-runs every time",
            );
            self.diags.push(d);
        }
    }

    /// A suggestion has to be writable where it is suggested: the program-wide
    /// `store.db` is not syntax, `store::db` or a bare `db` is.
    fn as_written(&self, effect: &Symbol) -> String {
        let scope = self.scope();
        if let Some((name, _)) = scope.effects.iter().find(|(_, b)| &b.qualified == effect) {
            return name.to_string();
        }
        let simple = self
            .effects
            .get(effect)
            .map(|info| info.simple_name.clone())
            .unwrap_or_else(|| Symbol::new(effect.as_str()));
        for (binder, &(owner, _)) in &scope.modules {
            if self.resolved.declarations[owner]
                .get(Namespace::Effect, &simple)
                .is_some_and(|d| d.qualified == *effect)
            {
                return format!("{binder}::{simple}");
            }
        }
        simple.to_string()
    }

    fn example_clause(&self, effect: &Symbol, atom: &EffectAtom) -> String {
        let written = self.as_written(effect);
        let Some(info) = self.effects.get(effect) else {
            return format!("{written}.<op>() -> <value>");
        };
        let op = info
            .ops
            .values()
            .find(|o| {
                o.mode == atom.mode
                    && matches!(&atom.resource, Resource::Named(_)) == o.resource_param
            })
            .or_else(|| info.ops.values().find(|o| o.mode == atom.mode))
            .or_else(|| info.ops.values().next());
        match op {
            Some(op) => {
                let resource = match (&atom.resource, op.resource_param) {
                    (Resource::Named(r), true) => format!("[{r}]"),
                    _ => String::new(),
                };
                let args: Vec<String> = (0..op.params.len()).map(|i| format!("a{i}")).collect();
                format!(
                    "{written}.{}{resource}({}) -> <value>",
                    op.name,
                    args.join(", ")
                )
            }
            None => format!("{written}.<op>() -> <value>"),
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
        let kept: Vec<PerformSite> = self
            .performs
            .drain(range)
            .filter(|s| !handled.contains(&s.atom))
            .collect();
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

            ExprKind::Var(q) => {
                let key = match self.value_key(q) {
                    Some(key) => key,
                    None => return (self.fresh.ty(), Row::empty()),
                };
                if key.prelude && is_cell_builtin(&key.name) {
                    self.diags.push(
                        Diagnostic::error(
                            codes::RESOURCE_REQUIRED,
                            format!("`{}` must be called directly", key.name),
                        )
                        .primary(q.span, "used as a value")
                        .note(
                            "the atom it performs names the region of the cell it is given, so it \
                             cannot be passed around as a first-class function",
                        ),
                    );
                    return (self.fresh.ty(), Row::empty());
                }
                match self.env.lookup(&key.name) {
                    Some(scheme) => {
                        let scheme = scheme.clone();
                        (instantiate(&scheme, &mut self.fresh), Row::empty())
                    }
                    None => {
                        self.unknown_name(q);
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
                (
                    Type::Fn {
                        params: ptys,
                        ret: Box::new(ret),
                        effects: row,
                    },
                    Row::empty(),
                )
            }

            ExprKind::App { func, args } => self.infer_app(e, func, args),

            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
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

            ExprKind::Perform {
                effect,
                op,
                resource,
                args,
            } => self.infer_perform(e, effect, op, resource.as_ref(), args),

            ExprKind::Handle {
                body,
                clauses,
                return_clause,
            } => self.infer_handle(e, body, clauses, return_clause.as_deref()),

            ExprKind::WithCell {
                resource,
                init,
                binder,
                body,
            } => self.infer_with_cell(e, resource, init, binder, body),

            ExprKind::Simulate { body } => self.infer_simulate(e, body),
        }
    }

    fn infer_stmt(&mut self, stmt: &Stmt) -> Row {
        match stmt {
            Stmt::Expr(e) => self.infer(e).1,
            Stmt::Let {
                pat,
                ty,
                value,
                span: _,
            } => {
                let (mut vt, row) = self.infer(value);
                if let Some(annotation) = ty {
                    let want = self.conv_type(annotation);
                    self.expect(value.span, &want, &vt, "`let` annotation");
                    vt = want;
                }
                let generalizable = matches!(pat.kind, PatternKind::Var(_) | PatternKind::Wildcard);
                let mut bindings = Vec::new();
                self.bind_pattern(pat, &vt, &mut bindings);
                for (name, t) in bindings {
                    let scheme = if generalizable {
                        generalize(&self.subst, &mut self.env, &t)
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
                self.expect(
                    rhs.span,
                    &lt,
                    &rt,
                    "both sides of a comparison must have one type",
                );
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
                Diagnostic::error(
                    codes::TYPE_MISMATCH,
                    "functions cannot be compared for equality",
                )
                .primary(
                    span,
                    format!("both sides have type `{}`", printer.ty(&resolved)),
                )
                .note("compare the results of calling them instead"),
            );
        }
    }

    fn infer_app(&mut self, e: &Expr, func: &Expr, args: &[Expr]) -> (Type, Row) {
        if let ExprKind::Var(q) = &func.kind
            && let Some(mode) = self.cell_form(q)
        {
            return self.infer_cell_op(e, args, mode);
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
            Type::Fn {
                params,
                ret,
                effects,
            } => {
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
                        .primary(
                            e.span,
                            format!("expected {}, found {}", params.len(), args.len()),
                        )
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
        let name = if mode == Mode::Read {
            "cell_get"
        } else {
            "cell_set"
        };
        let mut row = Row::empty();
        let mut tys = Vec::new();
        for a in args {
            let (t, r) = self.infer(a);
            row = self.join(e.span, row, r);
            tys.push(t);
        }
        if args.len() != expected {
            self.arity_error(
                e.span,
                &Symbol::new(name),
                expected,
                args.len(),
                "arguments",
            );
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
            let ret = if mode == Mode::Read {
                elem
            } else {
                Type::unit()
            };
            return (ret, row);
        };

        let atom = EffectAtom::new(CELL, Resource::Named(resource), mode);
        self.record(atom.clone(), e.span, true);
        let row = self.join(e.span, row, Row::singleton(atom));
        let ret = if mode == Mode::Read {
            elem
        } else {
            Type::unit()
        };
        (ret, row)
    }

    fn infer_perform(
        &mut self,
        e: &Expr,
        effect: &QName,
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

        let (params, ret, op_row) = self.instantiate_op(&op_info);
        if params.len() != args.len() {
            self.diags.push(
                Diagnostic::error(
                    codes::ARITY_MISMATCH,
                    format!(
                        "`{}.{}` takes {} argument{}, but {}",
                        info.simple_name,
                        op_info.name,
                        params.len(),
                        if params.len() == 1 { "" } else { "s" },
                        supplied(args.len())
                    ),
                )
                .primary(
                    e.span,
                    format!("expected {}, found {}", params.len(), args.len()),
                )
                .secondary(op_info.span, "declared here"),
            );
            return (ret, row);
        }
        for ((want, got), arg) in params.iter().zip(&arg_tys).zip(args) {
            self.expect(arg.span, want, got, "operation argument");
        }

        let atom = EffectAtom::new(info.name.clone(), res, op_info.mode);
        self.record(atom.clone(), e.span, true);
        // The operation's own row, which only a prelude operation has: it is how
        // the effects of a `task.spawn` body reach the row of the code that
        // spawned it, and therefore the test's footprint and the conflict graph.
        // Resolved first, exactly as an application resolves a callee's row —
        // two spawns whose tails have already been solved to different closed
        // rows must union, not unify.
        let op_row = self.subst.resolve_row(&op_row);
        for atom in &op_row.atoms {
            self.record(atom.clone(), e.span, false);
        }
        let row = self.join(e.span, row, op_row);
        let row = self.join(e.span, row, Row::singleton(atom));
        (ret, row)
    }

    fn resolve_op(&mut self, effect: &QName, op: &Ident) -> Option<(EffectInfo, OpInfo)> {
        let name = self.effect_name(effect)?;
        let Some(info) = self.effects.get(&name).cloned() else {
            if name.as_str() == CELL {
                self.diags.push(
                    Diagnostic::error(
                        codes::UNKNOWN_EFFECT,
                        "`cell` is a builtin effect and cannot be performed directly",
                    )
                    .primary(effect.span, "not a declared effect")
                    .note(
                        "read and write a region's cell with `cell_get(c)` and `cell_set(c, v)` \
                         inside `with_cell[r](..) { c -> .. }`",
                    ),
                );
            }
            return None;
        };
        let Some(op_info) = info.ops.get(&op.name).cloned() else {
            let known: Vec<String> = info.ops.keys().map(|k| format!("`{k}`")).collect();
            let mut d = Diagnostic::error(
                codes::UNKNOWN_OPERATION,
                format!(
                    "effect `{}` has no operation `{}`",
                    effect.symbol(),
                    op.name
                ),
            )
            .primary(op.span, "not declared")
            .secondary(info.span, format!("`{}` is declared here", info.name));
            d = if known.is_empty() {
                d.note(format!("`{}` declares no operations", info.name))
            } else {
                d.note(format!(
                    "operations of `{}`: {}",
                    info.name,
                    known.join(", ")
                ))
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
                        self.as_written(&info.name),
                        op.name
                    )),
                );
                None
            }
            (false, Some(r)) => {
                self.diags.push(
                    Diagnostic::error(
                        codes::RESOURCE_REQUIRED,
                        format!("`{}.{}` is not resource-parameterized", info.name, op.name),
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

    /// One code path for both kinds of operation: a user-declared one has no
    /// scheme, so its signature is built here with an empty row, and today's
    /// rule is this rule at that row.
    fn instantiate_op(&mut self, op: &OpInfo) -> (Vec<Type>, Type, Row) {
        let scheme = match &op.scheme {
            Some(scheme) => scheme.clone(),
            None => Scheme {
                ty_vars: op_free_vars(op),
                row_vars: vec![],
                ty: Type::Fn {
                    params: op.params.clone(),
                    ret: Box::new(op.ret.clone()),
                    effects: Row::empty(),
                },
            },
        };
        match instantiate(&scheme, &mut self.fresh) {
            Type::Fn {
                params,
                ret,
                effects,
            } => (params, *ret, effects),
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

        // One `ρ_κ` per `handle` and not one per clause: every clause's
        // continuation is the same residual computation. Allocated only when a
        // clause actually binds one, so that a program without any general
        // clause produces the identical variable numbering it did before.
        let general = clauses.iter().any(|c| c.resume.is_some());
        let continuation_row = general.then(|| self.fresh.row_var());
        let result_var = general.then(|| self.fresh.ty());

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
            // A prelude operation's own row is not the clause's to carry: the
            // clause receives the argument and whatever it does with it — call
            // the spawned body, or not — is already in the clause's own row.
            let (params, ret, _) = self.instantiate_op(&op_info);
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
            if let Some(binder) = &clause.resume {
                let (result, row) = (
                    result_var.clone().expect("a general clause is present"),
                    continuation_row.expect("a general clause is present"),
                );
                self.env.bind(
                    binder.name.clone(),
                    Scheme::mono(Type::Fn {
                        params: vec![ret.clone()],
                        ret: Box::new(result),
                        effects: Row::open(row),
                    }),
                );
            }
            let (clause_ty, clause_row) = self.infer(&clause.body);
            self.env.pop();
            // A general clause's body *is* the `handle`'s answer, so it is
            // checked against the result rather than against the operation's
            // return type; the tail-resumptive form is the one whose body has to
            // be something the perform site can receive.
            match (&clause.resume, &result_var) {
                (Some(_), Some(result)) => self.expect(
                    clause.body.span,
                    result,
                    &clause_ty,
                    "a clause that binds a continuation returns the `handle`'s result",
                ),
                _ => self.expect(
                    clause.body.span,
                    &ret,
                    &clause_ty,
                    "a handler clause returns the operation's result",
                ),
            }
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

        if let Some(result) = &result_var {
            self.expect(
                e.span,
                result,
                &result_ty,
                "every clause of a `handle` produces its result",
            );
        }

        let remaining = body_row.without(&handled);
        let row = self.join(e.span, remaining, clause_rows);
        let row = self.join(e.span, row, return_row);
        if let Some(v) = continuation_row {
            self.subst.solve_continuation_row(v, &row);
        }
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
                .primary(
                    body.span,
                    format!("this has type `{}`", printer.ty(&resolved)),
                )
                .note("read the cell inside the region and return the value instead"),
            );
        }

        let row = self.join(e.span, init_row, body_row.without(&handled));
        (body_ty, row)
    }

    /// The `handle` rule with a fixed clause set, plus one atom of its own:
    ///
    /// ```text
    /// Γ ⊢ simulate { body } : T / ( (ρ_b \ {task.*, clock.*, random.*}) ∪ {sim.read} )
    /// ```
    ///
    /// The seeded scheduler's clauses read and write only state created at the
    /// region's entry and destroyed at its exit, so the `⋃ row(clause_i)` term a
    /// hand-written handler owes is empty and the only thing added is the seed.
    /// `cell` is deliberately not discharged: a `with_cell` outside a region
    /// holding state the tasks inside share is how tasks share memory.
    fn infer_simulate(&mut self, e: &Expr, body: &Expr) -> (Type, Row) {
        let mark = self.performs.len();
        let (body_ty, body_row) = self.infer(body);
        let handled = prelude::simulated_atoms();
        self.discharge(mark..self.performs.len(), &handled);

        self.simulations.push(Simulation {
            span: e.span,
            body_span: body.span,
            body_ty: body_ty.clone(),
            body_row: body_row.clone(),
        });

        let seed = prelude::seed_atom();
        self.record(seed.clone(), e.span, true);
        let row = self.subst.resolve_row(&body_row).without(&handled);
        let row = self.join(e.span, row, Row::singleton(seed));
        (body_ty, row)
    }

    /// Nesting and task escape, asked once the module is solved. Both questions
    /// are about a row and a type that a region's own definition routinely
    /// leaves unsolved while it is being walked.
    fn check_simulations(&mut self) {
        let seed = prelude::seed_atom();
        let sites: Vec<Simulation> = std::mem::take(&mut self.simulations);
        for site in &sites {
            if self.subst.resolve_row(&site.body_row).contains(&seed) {
                self.nested_simulation(site, &sites);
            }
            let ty = self.subst.resolve_ty(&site.body_ty);
            if prelude::mentions_task(&ty) {
                let mut printer = Printer::new();
                self.diags.push(
                    Diagnostic::error(
                        codes::TASK_ESCAPES_SCOPE,
                        "a task escapes the `simulate` region that spawned it",
                    )
                    .primary(
                        site.body_span,
                        format!("this has type `{}`", printer.ty(&ty)),
                    )
                    .note(
                        "a `Task` is a key into the region's scheduler, and the scheduler ends \
                         with the region",
                    )
                    .note("`join` the task inside the region and return its value instead"),
                );
            }
        }
    }

    fn nested_simulation(&mut self, site: &Simulation, sites: &[Simulation]) {
        let inner = sites
            .iter()
            .find(|other| other.span != site.span && encloses(site.body_span, other.span));
        let mut d = Diagnostic::error(
            codes::NESTED_SIMULATION,
            "a `simulate` region inside another `simulate` region",
        )
        .primary(site.span, "this region's body reaches `sim.read`");
        if let Some(inner) = inner {
            d = d.secondary(inner.span, "the region it reaches is here");
        } else {
            d = d.note("something this body calls carries `sim.read`, so it enters a region too");
        }
        self.diags.push(
            d.note(
                "two schedulers means two notions of `runnable`, and a task in the inner region \
                 blocking the outer one",
            )
            .note("hoist the inner region out, or drop it and let the outer one schedule"),
        );
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
                let variants: Vec<&CtorInfo> = self
                    .ctors
                    .values()
                    .filter(|c| &c.type_name == name)
                    .collect();
                if variants.is_empty() {
                    vec!["other values".to_string()]
                } else {
                    let covered: FxHashSet<Symbol> = unguarded
                        .clone()
                        .filter_map(|a| match &a.pat.kind {
                            PatternKind::Ctor { name, args } if args.iter().all(is_irrefutable) => {
                                self.declared_value(name)
                            }
                            _ => None,
                        })
                        .collect();
                    variants
                        .iter()
                        .filter(|c| !covered.contains(&c.name))
                        .map(|c| format!("`{}`", c.simple_name))
                        .collect()
                }
            }
            _ => vec!["other values".to_string()],
        };

        if missing.is_empty() {
            return;
        }
        self.diags.push(
            Diagnostic::error(
                codes::NON_EXHAUSTIVE_MATCH,
                "match does not cover every case",
            )
            .primary(e.span, format!("not covered: {}", missing.join(", ")))
            .note("add the missing arms, or a `_` arm"),
        );
    }

    fn bind_pattern(&mut self, pat: &Pattern, scrutinee: &Type, out: &mut Vec<(Symbol, Type)>) {
        match &pat.kind {
            PatternKind::Wildcard => {}
            PatternKind::Var(id) => out.push((id.name.clone(), scrutinee.clone())),
            PatternKind::Lit(l) => {
                let t = lit_type(l);
                self.expect(pat.span, scrutinee, &t, "pattern literal");
            }
            PatternKind::Ctor { name, args } => {
                let info = self
                    .global(Namespace::Value, name)
                    .and_then(|qualified| self.ctors.get(&qualified).cloned());
                let Some(info) = info else {
                    if name.is_bare() || self.declared_value(name).is_some() {
                        self.diags.push(
                            Diagnostic::error(
                                codes::UNKNOWN_NAME,
                                format!("unknown constructor `{}`", name.symbol()),
                            )
                            .primary(name.span, "not found")
                            .note("constructors come from a `type` declaration with variants"),
                        );
                    }
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
                self.expect(
                    pat.span,
                    scrutinee,
                    &Type::list(elem.clone()),
                    "list pattern",
                );
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
                        let named: FxHashSet<&Symbol> =
                            fields.iter().map(|(n, _)| &n.name).collect();
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
            && let Err(e) = unify_row(
                &mut self.subst,
                &mut self.fresh,
                &Row::open(x),
                &Row::open(y),
            )
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
                let (expected, found) = (
                    self.subst.resolve_ty(expected),
                    self.subst.resolve_ty(found),
                );
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
                Diagnostic::error(
                    codes::OCCURS_CHECK,
                    format!("infinite effect row: {context}"),
                )
                .primary(span, format!("`{v}` would have to equal `{r}`"))
                .note("an effect row cannot contain itself")
            }
            UnifyError::RowMismatch { expected, found } => {
                let (expected, found) = (
                    self.subst.resolve_row(expected),
                    self.subst.resolve_row(found),
                );
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

    fn unknown_name(&mut self, q: &QName) {
        let mut d = Diagnostic::error(
            codes::UNKNOWN_NAME,
            format!("unknown name `{}`", q.symbol()),
        )
        .primary(q.span, "not found in this scope");
        if q.is_bare()
            && q.symbol().as_str() == RESULT
            && self.spec_kind == Some(SpecKind::Requires)
        {
            d = d.note(
                "`result` is bound only in an `ensures`: a precondition that could name the \
                 result would be a claim about a value the call has not produced yet",
            );
        }
        if let Some(near) = self.nearest_name(q.symbol()) {
            d = d.note(format!("a name in scope looks similar: `{near}`"));
        }
        if let Some(module) = self.exporter(Namespace::Value, q.symbol()) {
            d = d.note(format!(
                "module `{module}` exports it: add `import {module} ({})`",
                q.symbol()
            ));
        }
        self.diags.push(d);
    }

    /// Candidates are the simple names this module can actually write, not the
    /// program-wide ones: suggesting `store.orders.place` for `plce` would name
    /// something the file cannot say.
    fn nearest_name(&self, name: &Symbol) -> Option<Symbol> {
        let mut best: Option<(usize, Symbol)> = None;
        for c in self.scope().values.keys() {
            let d = edit_distance(name.as_str(), c.as_str());
            if d > 0
                && d * 3 <= name.as_str().len().max(1)
                && best.as_ref().is_none_or(|(b, _)| d < *b)
            {
                best = Some((d, c.clone()));
            }
        }
        best.map(|(_, s)| s)
    }
}

/// A resolved value reference: the [`TypeEnv`] key it denotes, and whether it
/// landed on the prelude rather than on a local or a module item.
struct ValueKey {
    name: Symbol,
    prelude: bool,
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
        Lit::Bytes(_) => Type::bytes(),
        Lit::Unit => Type::unit(),
    }
}

/// A written effect row inside a `forall` binder's type, which the row
/// conversion would otherwise report as an unbound row variable — a message
/// about the generic list of a definition that is not there.
fn written_row_reason(te: &TypeExpr) -> Option<String> {
    match te {
        TypeExpr::Var(_) | TypeExpr::Unit { .. } => None,
        TypeExpr::Con { args, .. } => args.iter().find_map(written_row_reason),
        TypeExpr::Record { fields, .. } => fields.iter().find_map(|(_, t)| written_row_reason(t)),
        TypeExpr::Fn {
            params,
            ret,
            effects,
            ..
        } => {
            if let Some(row) = effects {
                if row.tail.is_some() {
                    return Some(
                        "an effect variable has no binder here, and a law quantifies over values \
                         rather than over rows"
                            .to_string(),
                    );
                }
                if let Some(atom) = row.atoms.first() {
                    return Some(format!(
                        "applying this inside the body would perform `{}`, and a law body is pure",
                        atom.effect
                    ));
                }
            }
            params
                .iter()
                .chain([ret.as_ref()])
                .find_map(written_row_reason)
        }
    }
}

fn is_cell_builtin(name: &Symbol) -> bool {
    matches!(name.as_str(), "cell_get" | "cell_set")
}

fn encloses(outer: Span, inner: Span) -> bool {
    !outer.is_dummy()
        && !inner.is_dummy()
        && outer.source == inner.source
        && outer.start <= inner.start
        && inner.end <= outer.end
}

fn supplied(n: usize) -> String {
    if n == 1 {
        "1 was supplied".to_string()
    } else {
        format!("{n} were supplied")
    }
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
    let bound = if open_from == usize::MAX {
        longest
    } else {
        open_from
    };
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

/// Grows for the same reason [`Infer::infer`] does: it walks the same tree, so a
/// chain deep enough to need the growth there needs it here too.
fn collect_refs<'a>(e: &'a Expr, out: &mut Vec<&'a QName>) {
    const RED_ZONE: usize = 256 * 1024;
    const NEW_SEGMENT: usize = 2 * 1024 * 1024;
    stacker::maybe_grow(RED_ZONE, NEW_SEGMENT, || collect_refs_inner(e, out))
}

fn collect_refs_inner<'a>(e: &'a Expr, out: &mut Vec<&'a QName>) {
    match &e.kind {
        ExprKind::Lit(_) => {}
        ExprKind::Var(q) => out.push(q),
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
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
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
        ExprKind::Handle {
            body,
            clauses,
            return_clause,
        } => {
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
        ExprKind::Simulate { body } => collect_refs(body, out),
    }
}
