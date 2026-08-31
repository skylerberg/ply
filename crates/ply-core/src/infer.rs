use crate::derivable::{Adt, Context as Derivability, Why, ordered};
use crate::env::{TypeEnv, generalize, instantiate, instantiate_with};
use crate::prelude;
use crate::print::{Printer, region_of, region_type_name};
use crate::scc::sccs;
use crate::ty::{EffectAtom, Footprint, Resource, Row, RowVar, Scheme, TyVar, Type};
use crate::unify::{Fresh, Subst, UnifyError, unify, unify_row};
use crate::{
    CheckOutput, CtorInfo, DefConstraint, DefInfo, EffectInfo, Known, LawBinder, LawInfo,
    ModuleInfo, OpInfo, SpecInfo, TestInfo,
};
use indexmap::IndexMap;
use ply_span::{Diagnostic, Severity, Span, Symbol, codes};
use ply_syntax::ast::*;
use ply_syntax::resolve::{Namespace, Resolved, Scope, resolve};
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::{BTreeMap, BTreeSet};

/// The effect under which `with_cell` regions publish their atoms. Reserved:
/// a user `effect cell` would silently gain the power to observe region state.
const CELL: &str = "cell";

/// Bound in an `ensures` clause to the definition's return value, beside the
/// parameters rather than inside them. Not a keyword: a definition with no
/// `ensures` may still call a parameter `result`.
const RESULT: &str = "result";

/// Names no `type` item may claim, with their arities. The prelude's ADTs are
/// appended by [`builtin_types`] rather than repeated here, so the one table in
/// `prelude` decides both what exists and what is reserved.
const BUILTIN_TYPE_CONS: &[(&str, usize)] = &[
    ("Int", 0),
    ("Bool", 0),
    ("String", 0),
    ("Bytes", 0),
    ("Float", 0),
    ("Decimal", 0),
    ("Unit", 0),
    ("List", 1),
    ("Map", 2),
    ("Cell", 1),
    (prelude::TASK_TYPE, 1),
    // A builtin rather than a `std` record, because `type Secret<a> = {value: a}`
    // is one field access from useless and a project could declare its own.
    (crate::ty::SECRET, 1),
];

fn builtin_types() -> Vec<(&'static str, usize)> {
    let mut out = BUILTIN_TYPE_CONS.to_vec();
    out.extend(prelude::ADTS.iter().map(|a| (a.name, a.params.len())));
    out
}

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
    // Whatever `check_fns` did not drain: a spec clause, a test body, a law.
    // Nothing here is generalized, so the position only has to be after every
    // expression has been walked.
    c.settle_numerics();
    c.check_comparisons();
    c.check_map_keys();
    c.check_constraints();
    c.check_derives(program);
    c.attribute_generated(program);
    c.check_simulations();
    c.check_regions();
    // Last, because it is the only pass that needs every module's `DefInfo` to
    // exist: it answers a question about a definition's callees.
    c.mark_internal_effects(program);
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
    let mut program = Program::single(module.clone());
    // A `derive` is expanded before anything is resolved, here as in the
    // driver: what resolution and inference see is ordinary definitions.
    let diags = ply_derive::expand_program(&mut program);
    if !diags.is_empty() {
        return Err(diags);
    }
    let resolved = resolve(&mut program)?;
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

/// A `Map` type as it was written or instantiated, with everything
/// [`Checker::check_map_keys`] needs to judge it once the module is solved. The
/// assumptions travel with the site because they belong to the signature the
/// site sits in, and the check runs long after that signature is out of scope.
struct MapKeySite {
    span: Span,
    scope: u32,
    key: Type,
    assumed: FxHashSet<TyVar>,
    params: Vec<(TyVar, Symbol)>,
}

/// A reference that instantiated a parameter its callee constrained, with
/// everything [`Checker::check_constraints`] needs once the module is solved.
///
/// The whole point of the constraint is that this is where the error lands: an
/// agent that gets "`(Int) -> Int` has no JSON encoding" at the call it wrote
/// fixes the call, and one that gets it from inside an expansion searches.
struct ConstraintSite {
    span: Span,
    deriver: Deriver,
    /// The type the constrained parameter was instantiated with.
    ty: Type,
    /// The callee, and where its signature is, for the secondary label.
    callee: Symbol,
    callee_span: Span,
    /// The constraints in force at the call, so a body may assume its own.
    assumed: FxHashSet<TyVar>,
    /// The enclosing signature's type parameters, so "add `where derivable(json,
    /// a)`" can name `a` rather than a variable number.
    params: Vec<(TyVar, Symbol)>,
}

fn clone_adt(a: &Adt) -> Adt {
    Adt {
        params: a.params.clone(),
        fields: a.fields.clone(),
    }
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
    /// The type parameters the signature being checked declared `where
    /// derivable(D, ·)` for, by deriver. A `Map` key that is one of the `ord`
    /// ones is well-formed, and the body may assume it: that is what "checked
    /// at the signature, not at the use" means, and it is why omitting the
    /// clause is an error on the signature rather than a mystery at a call.
    assumed: FxHashMap<Deriver, FxHashSet<TyVar>>,
    /// Effect operation signatures have no generic list of their own, so a type
    /// variable appearing in one is implicitly quantified over the operation.
    auto_ty_params: bool,
    alias_stack: Vec<Symbol>,
    /// Written atoms already refused, by span.
    ///
    /// An `effect set` is expanded by the parser, so one atom written once in a
    /// set is spliced into every row that names it. Without this, an effect the
    /// program does not declare produces one identical diagnostic per row —
    /// pointing, correctly, at the one place there is to fix.
    refused_atoms: FxHashSet<Span>,
    performs: Vec<PerformSite>,
    /// Operand types of `==` / `!=`, checked once the whole module is solved
    /// because the type at a comparison is often still a variable when it is
    /// first seen.
    comparisons: Vec<(Span, Type)>,
    /// Every `Map` key type this run has seen written or instantiated, checked
    /// once the module is solved for the same reason `comparisons` is: the key
    /// is usually still a variable where the map first appears, and `Map<Float,
    /// v>` is only visible once unification has pinned it.
    map_keys: Vec<MapKeySite>,
    /// Ticks once per definition, test and law. A deferred check dedupes on it
    /// rather than on a span, because one bad type is usually recorded twice —
    /// at the annotation that wrote it and at the call that instantiated it —
    /// and two errors for one edit is two things to read and one thing to fix.
    scope: u32,
    /// The published `where` clauses of every definition checked so far,
    /// including the ones restored from a cached interface. A call site is
    /// checked against these, which is what makes the error local to the call
    /// rather than to something the callee does three definitions deeper.
    constraints: IndexMap<Symbol, Vec<DefConstraint>>,
    /// Call sites instantiating a constrained parameter, judged once the module
    /// is solved for the same reason `map_keys` is: the type argument at a call
    /// is routinely still a variable when the call is first walked.
    constrained_uses: Vec<ConstraintSite>,
    /// Arithmetic and ordered comparisons whose operand type has yet to be
    /// pinned to one of the three numeric types. Drained by
    /// [`Checker::settle_numerics`] before the definition around them is
    /// generalized.
    numerics: Vec<Numeric>,
    /// `simulate` regions, checked for the same reason: a region's result type
    /// and the row of what it calls are both routinely unsolved while its own
    /// definition is still being walked.
    simulations: Vec<Simulation>,
    /// The `with_region[r]` scopes enclosing the expression being walked,
    /// innermost last. `with_cell[r]` consults it to decide whether it opens a
    /// region of its own or allocates into one that is already open.
    open_regions: Vec<OpenRegion>,
    /// Closed regions, checked by [`Checker::check_regions`] once the module is
    /// solved.
    regions: Vec<RegionSite>,
    /// How many `simulate` regions enclose the expression being walked. A task
    /// cannot outlive the scheduler that runs it, so a region that opened before
    /// the enclosing `simulate` did is one a spawned task can outlive.
    simulate_depth: u32,
    /// What each definition carrying clauses has its clauses typed against.
    /// Recorded where the signature is built rather than rebuilt in
    /// [`Checker::check_specs`], so that a clause cannot report a second copy of
    /// a signature error the body already reported.
    spec_envs: FxHashMap<Symbol, SpecEnv>,
    /// The clause being walked, so `result` outside an `ensures` can say where
    /// it is bound instead.
    spec_kind: Option<SpecKind>,
    /// Set while a definition a `derive` generated is being checked. A failure
    /// inside one is Ply's problem to explain, not the user's to decode: the
    /// body is not in the file and there is nothing at its span to edit.
    derived: Option<Derived>,
}

/// One use of an operator that is defined at more than one numeric type, kept
/// until the operand type is known.
struct Numeric {
    span: Span,
    op: BinOp,
    ty: Type,
}

/// Where a spec expression sits, which decides both how it is named in a
/// diagnostic and — the part that matters — what its row may carry.
#[derive(Clone, Copy, PartialEq, Eq)]
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

/// A region scope while its body is being walked.
struct OpenRegion {
    name: Symbol,
    source: RegionSource,
    span: Span,
    /// [`Checker::simulate_depth`] where the region opened. A `task.spawn`
    /// reached at this same depth is scheduled by a `simulate` that started
    /// before the region did and therefore outlives it.
    simulate_depth: u32,
    spawns: Vec<SpawnSite>,
    handoffs: Vec<HandoffSite>,
}

/// How a region was written. ADR 0017 §1 makes both of these regions and the
/// four escape checks run on both; what differs is only what the exit
/// diagnostic says, because a bare `with_cell`'s escaping value is always the
/// cell and has said so since before regions existed.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RegionSource {
    WithRegion,
    WithCell,
}

/// One closed region — a `with_region[r]` or a `with_cell[r]` that opened one of
/// its own — as [`Checker::check_regions`] revisits it.
struct RegionSite {
    name: Symbol,
    source: RegionSource,
    /// The region's name in the source, which is where `r` is said to open.
    span: Span,
    body_span: Span,
    body_ty: Type,
    /// Every position the body's value leaves through. All of them carry
    /// `body_ty`; what the list buys is a label on the expression that produced
    /// the value rather than on the whole region.
    exits: Vec<Span>,
    outer: Vec<OuterBinding>,
    spawns: Vec<SpawnSite>,
    handoffs: Vec<HandoffSite>,
}

/// A name that was in scope before a region opened, and where the region's body
/// first mentions it. A brand reaching this type is a store into something that
/// outlives the region.
struct OuterBinding {
    name: Symbol,
    ty: Type,
    site: Span,
}

/// An argument handed to `task.spawn` inside a region.
struct SpawnSite {
    span: Span,
    ty: Type,
}

/// An argument handed to a user-declared operation inside a region.
///
/// The perform site and the handler instantiate the operation's signature
/// separately, so a declared type variable in an argument position is a hole the
/// brand falls through: the handler receives the value at a variable of its own
/// and may store or return it, and no type there mentions the region. The type
/// recorded is the argument's, at the site that wrote it.
struct HandoffSite {
    span: Span,
    /// `sink.put`, for the diagnostic.
    op: String,
    ty: Type,
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
            refused_atoms: FxHashSet::default(),
            performs: Vec::new(),
            comparisons: Vec::new(),
            map_keys: Vec::new(),
            scope: 0,
            assumed: FxHashMap::default(),
            constraints: IndexMap::new(),
            constrained_uses: Vec::new(),
            numerics: Vec::new(),
            simulations: Vec::new(),
            open_regions: Vec::new(),
            regions: Vec::new(),
            simulate_depth: 0,
            spec_envs: FxHashMap::default(),
            spec_kind: None,
            derived: None,
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
    /// The program-wide name a constructor reference denotes, falling back to
    /// the prelude's — which no module declares, so nothing qualifies it.
    ///
    /// A module that declares its own `Some` shadows the prelude's, exactly as
    /// one declaring its own `len` does.
    ///
    /// Reporting, through [`Checker::global`]: a qualified constructor that is
    /// private is `E0107` and the prelude fallback must not swallow it.
    fn ctor_name(&mut self, q: &QName) -> Option<Symbol> {
        if let Some(name) = self.global(Namespace::Value, q) {
            return Some(name);
        }
        self.prelude_ctor(q)
    }

    /// [`Checker::ctor_name`] without the diagnostic, for the passes that only
    /// ask which constructor a pattern *covers*. The reference has already been
    /// reported by the pass that bound it.
    fn covered_ctor(&self, q: &QName) -> Option<Symbol> {
        self.declared_value(q).or_else(|| self.prelude_ctor(q))
    }

    fn prelude_ctor(&self, q: &QName) -> Option<Symbol> {
        let bare = q.is_bare().then(|| q.symbol().clone())?;
        self.ctors.contains_key(&bare).then_some(bare)
    }

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
                let what = if ns == Namespace::Type {
                    "type"
                } else {
                    "definition"
                };
                if !self.unresolved_in_derived(q, what) {
                    self.diags.push(d);
                }
                None
            }
        }
    }

    /// The prelude's ADTs, as constructors in the value namespace and entries in
    /// `ctors`.
    ///
    /// They are registered exactly as a module's are, so exhaustiveness, pattern
    /// binding, the property generator and the prover's case split all see them
    /// through the paths they already use. Their program-wide name is the bare
    /// one — no module declares them — which is the same convention the prelude's
    /// effects and functions follow.
    fn install_prelude_adts(&mut self) {
        for (name, info) in prelude::ctors() {
            self.env.bind_global(name.clone(), info.scheme.clone());
            self.ctors.insert(name, info);
        }
    }

    fn install_prelude(&mut self) {
        self.install_prelude_adts();
        let a = self.fresh.ty_var();
        let b = self.fresh.ty_var();
        let c = self.fresh.ty_var();
        let e = self.fresh.row_var();
        let (ta, tb, tc, re) = (Type::Var(a), Type::Var(b), Type::Var(c), Row::open(e));
        // `map_fold`'s accumulator needs a third variable; `a` and `b` are the
        // key and the value everywhere below, which is what keeps the twelve
        // signatures readable side by side. `iterate` reuses `a` as its seed and
        // `b` as the value its step stops with — two, because a loop that could
        // only stop with its own seed would have to run one more round to
        // report anything it computed.
        let map_ty = Type::map(ta.clone(), tb.clone());
        let entry_ty = Type::Record(BTreeMap::from([
            (Symbol::new("key"), ta.clone()),
            (Symbol::new("value"), tb.clone()),
        ]));

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
            // The list index, in the shape `json.ply:800` and `db.ply:604`
            // already recurse by hand. Total — it refuses no index — which is
            // what puts it in `TOTAL_BUILTINS` where a raising index could not
            // go. `bytes_at` raises; ADR 0027 §2 is why the two containers are
            // indexed by different conventions.
            (
                "list_at",
                poly(
                    vec![a],
                    vec![],
                    vec![Type::list(ta.clone()), Type::int()],
                    Type::option(ta.clone()),
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
                "iterate",
                poly(
                    vec![a, b],
                    vec![e],
                    vec![
                        ta.clone(),
                        Type::int(),
                        Type::Fn {
                            params: vec![ta.clone()],
                            ret: Box::new(Type::iter(ta.clone(), tb.clone())),
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
            // One allocation over the whole list. Folding `bytes_concat` across
            // a read loop's answers copies the accumulated prefix once per read,
            // which is quadratic in the size of a message a peer chooses.
            (
                "bytes_concat_all",
                mono(vec![Type::list(Type::bytes())], Type::bytes()),
            ),
            ("bytes_of_string", mono(vec![Type::string()], Type::bytes())),
            ("bytes_is_utf8", mono(vec![Type::bytes()], Type::bool())),
            // The searching builtins. W1's request path scanned its head five
            // times, each scan a `fold` over `range` that boxed an `Int` per
            // byte and could not stop early; these are the same searches with
            // the pass count and the allocation removed, which is an algorithm
            // rather than a constant factor.
            (
                "bytes_index_of",
                mono(
                    vec![Type::bytes(), Type::bytes()],
                    Type::option(Type::int()),
                ),
            ),
            (
                "bytes_index_of_from",
                mono(
                    vec![Type::bytes(), Type::bytes(), Type::int()],
                    Type::option(Type::int()),
                ),
            ),
            (
                "bytes_index_of_byte",
                mono(vec![Type::bytes(), Type::int()], Type::option(Type::int())),
            ),
            (
                "bytes_starts_with",
                mono(vec![Type::bytes(), Type::bytes()], Type::bool()),
            ),
            (
                "bytes_ends_with",
                mono(vec![Type::bytes(), Type::bytes()], Type::bool()),
            ),
            (
                "bytes_split",
                mono(
                    vec![Type::bytes(), Type::bytes()],
                    Type::list(Type::bytes()),
                ),
            ),
            // The byte class is a `Bytes` of its members rather than an enum,
            // so it is totally general with no closed set to extend and the
            // membership bitmap costs the set's length rather than the
            // buffer's.
            (
                "bytes_scan",
                mono(
                    vec![Type::bytes(), Type::int(), Type::bytes(), Type::int()],
                    Type::int(),
                ),
            ),
            (
                "bytes_scan_until",
                mono(
                    vec![Type::bytes(), Type::int(), Type::bytes(), Type::int()],
                    Type::int(),
                ),
            ),
            (
                "bytes_position",
                poly(
                    vec![],
                    vec![e],
                    vec![
                        Type::bytes(),
                        Type::int(),
                        Type::Fn {
                            params: vec![Type::int()],
                            ret: Box::new(Type::bool()),
                            effects: re.clone(),
                        },
                    ],
                    Type::option(Type::int()),
                    re.clone(),
                ),
            ),
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
            // The canonical total order over values, which is the order `Map`
            // iterates in. `derive ord` produces a dictionary that calls this
            // rather than a walk of its own, because two definitions of one
            // type's order are two things that can disagree — and the one a map
            // uses is not negotiable.
            (
                "compare",
                poly(
                    vec![a],
                    vec![],
                    vec![ta.clone(), ta.clone()],
                    Type::Con(Symbol::new("Ordering"), vec![]),
                    Row::empty(),
                ),
            ),
            // The same order, under a name a module may not declare — see
            // [`is_reserved_builtin`]. `derive ord` emits *this* call, because
            // a generated body that named `compare` would name whatever the
            // deriving module bound to it, and a dictionary built out of that
            // is the second order §2 rests on not existing.
            (
                "compare_values",
                poly(
                    vec![a],
                    vec![],
                    vec![ta.clone(), ta.clone()],
                    Type::Con(Symbol::new("Ordering"), vec![]),
                    Row::empty(),
                ),
            ),
            // `Map`. Every `k` carries `derivable(ord, k)`, which is not in the
            // `Scheme` — it is checked where a concrete key type appears, by
            // `check_map_keys`, because a constraint on a *builtin* has no
            // signature the user can read the requirement off.
            (
                "map_new",
                poly(vec![a, b], vec![], vec![], map_ty.clone(), Row::empty()),
            ),
            (
                "map_insert",
                poly(
                    vec![a, b],
                    vec![],
                    vec![map_ty.clone(), ta.clone(), tb.clone()],
                    map_ty.clone(),
                    Row::empty(),
                ),
            ),
            (
                "map_get",
                poly(
                    vec![a, b],
                    vec![],
                    vec![map_ty.clone(), ta.clone()],
                    Type::option(tb.clone()),
                    Row::empty(),
                ),
            ),
            (
                "map_contains",
                poly(
                    vec![a, b],
                    vec![],
                    vec![map_ty.clone(), ta.clone()],
                    Type::bool(),
                    Row::empty(),
                ),
            ),
            (
                "map_remove",
                poly(
                    vec![a, b],
                    vec![],
                    vec![map_ty.clone(), ta.clone()],
                    map_ty.clone(),
                    Row::empty(),
                ),
            ),
            (
                "map_len",
                poly(
                    vec![a, b],
                    vec![],
                    vec![map_ty.clone()],
                    Type::int(),
                    Row::empty(),
                ),
            ),
            (
                "map_keys",
                poly(
                    vec![a, b],
                    vec![],
                    vec![map_ty.clone()],
                    Type::list(ta.clone()),
                    Row::empty(),
                ),
            ),
            (
                "map_values",
                poly(
                    vec![a, b],
                    vec![],
                    vec![map_ty.clone()],
                    Type::list(tb.clone()),
                    Row::empty(),
                ),
            ),
            (
                "map_entries",
                poly(
                    vec![a, b],
                    vec![],
                    vec![map_ty.clone()],
                    Type::list(entry_ty.clone()),
                    Row::empty(),
                ),
            ),
            (
                "map_of_entries",
                poly(
                    vec![a, b],
                    vec![],
                    vec![Type::list(entry_ty)],
                    map_ty.clone(),
                    Row::empty(),
                ),
            ),
            (
                "map_merge",
                poly(
                    vec![a, b],
                    vec![],
                    vec![map_ty.clone(), map_ty.clone()],
                    map_ty.clone(),
                    Row::empty(),
                ),
            ),
            // The only impure one, and only because it threads its function's
            // row. It visits entries in ascending key order, and the keys are
            // canonical (`ply_eval::value::canonical_key` — ascending order
            // alone was not enough, because two spellings of one `Decimal` are
            // one key and two strings), so a fold over a map is a function of
            // the map's contents rather than of how it was built — which is
            // what makes a seeded replay take the same branch.
            (
                "map_fold",
                poly(
                    vec![a, b, c],
                    vec![e],
                    vec![
                        map_ty,
                        tc.clone(),
                        Type::Fn {
                            params: vec![tc.clone(), ta.clone(), tb.clone()],
                            ret: Box::new(tc.clone()),
                            effects: re.clone(),
                        },
                    ],
                    tc,
                    re.clone(),
                ),
            ),
            // The scale and the rounding mode are arguments because `/` on
            // `Decimal` is `E0209`: an operator would have to round, and a
            // rounding nobody wrote down is the defect the type exists to
            // prevent. Here it is written down.
            (
                "decimal_div",
                mono(
                    vec![
                        Type::decimal(),
                        Type::decimal(),
                        Type::int(),
                        Type::con("Rounding"),
                    ],
                    Type::decimal(),
                ),
            ),
            (
                "decimal_round",
                mono(
                    vec![Type::decimal(), Type::int(), Type::con("Rounding")],
                    Type::decimal(),
                ),
            ),
            ("decimal_of_int", mono(vec![Type::int()], Type::decimal())),
            (
                "int_of_decimal",
                mono(
                    vec![Type::decimal(), Type::con("Rounding")],
                    Type::option(Type::int()),
                ),
            ),
            (
                "float_of_decimal",
                mono(vec![Type::decimal()], Type::float()),
            ),
            (
                "decimal_of_float",
                mono(vec![Type::float()], Type::option(Type::decimal())),
            ),
            (
                "decimal_of_string",
                mono(vec![Type::string()], Type::option(Type::decimal())),
            ),
            (
                "decimal_to_string",
                mono(vec![Type::decimal()], Type::string()),
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
            // The only introduction. It does not consume its argument — Ply is a
            // value language — so containment starts where the `Secret` starts,
            // which ADR 0015 §2.5 (2) states rather than hides.
            (
                SECRET_OF_STRING,
                mono(vec![Type::string()], Type::secret(Type::string())),
            ),
            // The elimination, and it answers one bit. Constant-time over the
            // compared bytes; not rate-limited, so a program that loops it over
            // candidates recovers the value (§2.5 (3)).
            (
                SECRET_VERIFY,
                mono(
                    vec![Type::secret(Type::string()), Type::string()],
                    Type::bool(),
                ),
            ),
            // Presence is deliberately observable: an operator must be able to
            // tell a missing credential from a wrong one.
            (
                SECRET_IS_EMPTY,
                poly(
                    vec![a],
                    vec![],
                    vec![Type::secret(ta.clone())],
                    Type::bool(),
                    Row::empty(),
                ),
            ),
        ];
        for (name, scheme) in entries {
            self.env.bind_global(Symbol::new(name), scheme);
        }

        // The two builtins with a `where` clause. `Float` has no total order the
        // language can honour — `NaN != NaN`, so a key would have no position
        // the next lookup could find it at — and this is what refuses it at the
        // call rather than letting `total_cmp` answer where a program can see it
        // disagree with `==`.
        for name in ["compare", "compare_values"] {
            self.constraints.insert(
                Symbol::new(name),
                vec![DefConstraint {
                    deriver: Deriver::Ord,
                    param: 0,
                }],
            );
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
            if builtin_types()
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
                for (ty, written) in params.iter().zip(&op.params) {
                    self.reject_declared_cell(ty, written.span(), &op.name.name, "a parameter");
                }
                self.reject_declared_cell(&ret, op.ret.span(), &op.name.name, "a result");
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
                for (field, written) in fields.iter().zip(&variant.fields) {
                    self.reject_declared_cell(field, written.span(), &variant.name.name, "a field");
                }
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

    /// A declaration whose type mentions a `Cell`, which is the one place a
    /// brand can go where nothing can see it again.
    ///
    /// A variant's field types and an operation's signature are converted
    /// **once**, when the declaration is collected, and the region argument a
    /// written `Cell<T>` gets is a fresh variable that the surrounding scheme
    /// does not quantify. So the first `with_cell[r]` value that reaches one
    /// solves that single variable to `r` for the whole program, and the value's
    /// own type is then the bare `Foo` — a brand with nowhere to appear, and
    /// every escape through it invisible.
    ///
    /// Making the declaration generic in the cell keeps the brand in the type
    /// argument, where every check in this file already looks, so that is what
    /// the note says. Written through an alias it is the same refusal, because
    /// the check is on the converted type rather than on what was spelled.
    fn reject_declared_cell(&mut self, ty: &Type, span: Span, owner: &Symbol, what: &str) {
        if !mentions_cell(ty) {
            return;
        }
        self.diags.push(
            Diagnostic::error(
                codes::REGION_ESCAPE,
                format!("`{owner}` declares {what} that mentions a `Cell`"),
            )
            .primary(span, "a declaration is outside every region there is")
            .note(
                "a declared type is fixed once for the whole program, so the region a cell that \
                 reached it came from would be pinned by whichever value got there first, and no \
                 later check could tell that the cell had outlived it",
            )
            .note(
                "take the cell as a type parameter instead, so the region stays in the type \
                 argument and the escape check can see it",
            ),
        );
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
                Item::Effect(_)
                | Item::Test(_)
                | Item::Law(_)
                | Item::Derive(_)
                | Item::EffectSet(_) => Vec::new(),
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
            && let Some((_, arity)) = builtin_types()
                .iter()
                .find(|(b, _)| *b == name.symbol().as_str())
                .copied()
        {
            if args.len() != arity {
                self.arity_error(span, name.symbol(), arity, args.len(), "type arguments");
                return self.fresh.ty();
            }
            if name.symbol().as_str() == "Cell" {
                // A written `Cell<T>` says nothing about which region the cell
                // came from, so leave the region open and let unification with a
                // `with_cell` binder decide it.
                let region = self.fresh.ty();
                return Type::Con(Symbol::new("Cell"), vec![region, args[0].clone()]);
            }
            let built = Type::Con(name.symbol().clone(), args);
            self.note_map_keys(span, &built);
            return built;
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
        if self.unresolved_in_derived(name, "type") {
            return;
        }
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

    /// A reference a *generated* body emits that does not resolve.
    ///
    /// Reported as `E0206` against the `derive`, never as a bare `E0101`
    /// pointing at source the user never wrote. It means one of two things and
    /// the note says which: a field's type has no derivation of its own, or the
    /// module cannot see the deriver's runtime module.
    fn unresolved_in_derived(&mut self, q: &QName, what: &str) -> bool {
        let Some(derived) = self.derived.clone() else {
            return false;
        };
        let d = Diagnostic::error(
            codes::NOT_DERIVABLE,
            format!(
                "`{}` cannot be derived for `{}`",
                derived.deriver, derived.target
            ),
        )
        .primary(
            q.span,
            format!("the generated body needs the {what} `{q}`, which is not in scope"),
        )
        .note(format!(
            "a derivation composes through named types by calling their own dictionaries, \
             so every field type needs `derive {} for` it in the module that declares it",
            derived.deriver
        ))
        .note(format!(
            "`{}` is what `derive {} for <that type>` generates",
            q.symbol(),
            derived.deriver
        ));
        self.diags.push(d);
        true
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
        if self.refused_atoms.contains(&a.span) {
            return None;
        }
        let Some(effect) = self.effect_name(&a.effect) else {
            self.refused_atoms.insert(a.span);
            return None;
        };
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
            self.refused_atoms.insert(a.span);
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
        // `d.encode(x)` is `effect.op(args)` by the grammar, and a dictionary is
        // a record of functions, so the language's central idiom for one reads
        // as a perform. Nothing here can tell them apart without scope, but this
        // can say which one the user meant.
        if q.is_bare() && self.env.depth_of(q.symbol()).is_some() {
            self.diags.push(
                d.note(format!(
                    "`{}` is a value in scope, so this looks like a field call rather than a \
                     perform: write `({}.<field>)(..)` to call a function held in a record",
                    q.symbol(),
                    q.symbol()
                ))
                .note("`a.b(c)` is an effect operation; parentheses make it a field access"),
            );
            return;
        }
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

    /// A `derive` whose definitions are not in `items` means expansion did not
    /// run over this module.
    ///
    /// Nothing downstream can notice on its own: `Item::Derive` declares no
    /// name, so every walker is right to skip it, and the result is a
    /// definition that silently does not exist. One list, and this is what
    /// checks that it is the list.
    fn check_expanded(&mut self, module: &Module) {
        for item in &module.items {
            let Item::Derive(def) = item else { continue };
            if expanded_here(module, def) {
                continue;
            }
            self.diags.push(
                Diagnostic::error(
                    codes::INTERNAL_ERROR,
                    format!(
                        "`derive {} for {}` was never expanded",
                        def.deriver, def.target.name
                    ),
                )
                .primary(def.span, "no definition was generated for this")
                .note(
                    "this is a compiler bug: whatever loaded this module skipped derivation, \
                     so a definition the program refers to does not exist",
                ),
            );
        }
    }

    fn check_fns(&mut self, module: &Module) {
        self.check_expanded(module);
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
            if def.name.name.as_str() == COMPARE_VALUES {
                self.diags.push(
                    Diagnostic::error(
                        codes::DUPLICATE_DEFINITION,
                        format!("`{COMPARE_VALUES}` is a builtin"),
                    )
                    .primary(def.name.span, "cannot be redefined")
                    .note(
                        "it is the order `Map` iterates in, and `derive ord` builds a dictionary \
                         out of it — a module that could redefine it would give one type two \
                         orders",
                    )
                    .note("`compare` is the same operation under a name you may shadow"),
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
                let mut seen = FxHashSet::default();
                Refs::of(&d.body)
                    .bare()
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
                self.derived = fns[i].derived.clone();
                let sig = self.signature(fns[i]);
                self.derived = None;
                self.env
                    .bind_global(names[slot].clone(), Scheme::mono(sig.fn_ty.clone()));
                sigs.push(sig);
            }
            let mut performed = Vec::with_capacity(comp.len());
            for (slot, &i) in comp.iter().enumerate() {
                performed.push(self.check_fn_body(fns[i], &sigs[slot]));
                self.record_spec_env(fns[i], &names[slot], &sigs[slot]);
            }
            // After the whole component, not after each member: a caller in the
            // group can be what pins a callee's operand type, and defaulting one
            // body at a time would decide `Int` before the other body said
            // `Decimal`.
            self.settle_numerics();
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
                // The clauses are read against the signature's own parameters,
                // which `signature` left in `ty_params` and `check_fn_body`
                // restored.
                self.ty_params = sigs[slot].ty_params.clone();
                let constraints = self.published_constraints(def, &scheme);
                self.ty_params.clear();
                self.constraints
                    .insert(names[slot].clone(), constraints.clone());
                self.defs.insert(
                    names[slot].clone(),
                    DefInfo {
                        name: names[slot].clone(),
                        module: module.name.clone(),
                        simple_name: def.name.name.clone(),
                        scheme,
                        footprint: Footprint(row.atoms),
                        performed: Footprint(self.subst.resolve_row(&performed[slot]).atoms),
                        row_aliases: row_aliases(def),
                        constraints,
                        spec: Vec::new(),
                        // Lowered by `mark_internal_effects` once every module
                        // is walked, and only for a definition it cleared.
                        internally_effectful: true,
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
            // The clauses are typed anyway, against the restored interface. A
            // `where` clause is *not* erased, but it is read from the same
            // source the interface was published from, so it is recovered here
            // rather than stored: a caller that changed must be checked against
            // the constraints its callee actually declares, and a callee whose
            // file was skipped is one no parsed module can reach.
            let mut constraints = Vec::new();
            if !fns[i].spec.is_empty() || !fns[i].constraints.is_empty() {
                let sig = self.signature(fns[i]);
                let (published, args) = instantiate_with(&scheme, &mut self.fresh);
                let _ = unify(&mut self.subst, &mut self.fresh, &sig.fn_ty, &published);
                constraints = self.recovered_constraints(fns[i], &sig, &args);
                self.record_spec_env(fns[i], &names[slot], &sig);
            }
            self.env.bind_global(names[slot].clone(), scheme.clone());
            self.constraints
                .insert(names[slot].clone(), constraints.clone());
            self.defs.insert(
                names[slot].clone(),
                DefInfo {
                    name: names[slot].clone(),
                    module: module.name.clone(),
                    simple_name: fns[i].name.name.clone(),
                    scheme,
                    footprint: entry.footprint.clone(),
                    performed: entry.performed.clone(),
                    row_aliases: row_aliases(fns[i]),
                    constraints,
                    spec: Vec::new(),
                    // A restored interface says nothing about this, and the
                    // body is right here — `mark_internal_effects` walks it
                    // like any other.
                    internally_effectful: true,
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
        self.scope += 1;
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
        self.assumed = self.constraints_in_force(&def.constraints);
        self.check_where(def);
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
            scope: self.scope,
        }
    }

    /// The parameters a signature declared derivable, by deriver, as the type
    /// variables they became. A clause naming something the generic list does
    /// not bind contributes nothing here — that is [`Checker::check_where`]'s
    /// business, and swallowing it here would turn one error into two.
    fn constraints_in_force(
        &self,
        constraints: &[Constraint],
    ) -> FxHashMap<Deriver, FxHashSet<TyVar>> {
        let mut out: FxHashMap<Deriver, FxHashSet<TyVar>> = FxHashMap::default();
        for c in constraints {
            if let Some(Type::Var(v)) = self.ty_params.get(&c.param.name) {
                out.entry(c.deriver).or_default().insert(*v);
            }
        }
        out
    }

    fn assumed(&self, deriver: Deriver) -> FxHashSet<TyVar> {
        self.assumed.get(&deriver).cloned().unwrap_or_default()
    }

    /// The type parameters of the signature being walked, so a diagnostic can
    /// name the one a missing clause is about.
    fn rigid_params(&self) -> Vec<(TyVar, Symbol)> {
        self.ty_params
            .iter()
            .filter_map(|(name, t)| match t {
                Type::Var(v) => Some((*v, name.clone())),
                _ => None,
            })
            .collect()
    }

    /// A `where` clause naming a parameter the signature does not bind. The
    /// grammar admits only an identifier, so this is the whole of what can go
    /// wrong with one.
    fn check_where(&mut self, def: &FnDef) {
        for c in &def.constraints {
            if self.ty_params.contains_key(&c.param.name) {
                continue;
            }
            let bound: Vec<String> = def
                .generics
                .types
                .iter()
                .map(|g| format!("`{}`", g.name))
                .collect();
            let mut d = Diagnostic::error(
                codes::UNKNOWN_TYPE,
                format!(
                    "`{}` is not a type parameter of this signature",
                    c.param.name
                ),
            )
            .primary(c.param.span, "not bound by the generic list");
            d = if bound.is_empty() {
                d.note(format!(
                    "`{}` takes no type parameters, and a constraint on a concrete type is \
                     either already true or already an error",
                    def.name.name
                ))
            } else {
                d.note(format!("this signature binds {}", bound.join(", ")))
            };
            self.diags.push(d);
        }
    }

    /// The same, for a definition restored from a cached interface: the scheme
    /// came back with quantified variables of its own, and unifying it with the
    /// signature just built is what says which of them each parameter is.
    fn recovered_constraints(
        &mut self,
        def: &FnDef,
        sig: &Signature,
        args: &[Type],
    ) -> Vec<DefConstraint> {
        let mut out = Vec::new();
        for c in &def.constraints {
            let Some(Type::Var(v)) = sig.ty_params.get(&c.param.name) else {
                continue;
            };
            let found = args
                .iter()
                .position(|a| matches!(self.subst.resolve_ty(a), Type::Var(w) if w == *v));
            if let Some(param) = found {
                out.push(DefConstraint {
                    deriver: c.deriver,
                    param,
                });
            }
        }
        out.sort_by_key(|c| (c.param, c.deriver.tag()));
        out.dedup();
        out
    }

    /// The published form of a signature's `where` clauses: the deriver and the
    /// position of the parameter in the generalized scheme.
    ///
    /// Sorted and deduplicated, exactly as the hash encodes them, so that
    /// reordering two clauses or writing one twice publishes the same interface
    /// as writing them once in the other order.
    fn published_constraints(&self, def: &FnDef, scheme: &Scheme) -> Vec<DefConstraint> {
        let mut out: Vec<DefConstraint> = def
            .constraints
            .iter()
            .filter_map(|c| {
                let Some(Type::Var(v)) = self.ty_params.get(&c.param.name) else {
                    return None;
                };
                let param = scheme.ty_vars.iter().position(|q| q == v)?;
                Some(DefConstraint {
                    deriver: c.deriver,
                    param,
                })
            })
            .collect();
        out.sort_by_key(|c| (c.param, c.deriver.tag()));
        out.dedup();
        out
    }

    /// Answers the row the body itself performed, which is the published row
    /// only when there is no annotation to widen it.
    fn check_fn_body(&mut self, def: &FnDef, sig: &Signature) -> Row {
        self.derived = def.derived.clone();
        self.scope = sig.scope;
        self.ty_params = sig.ty_params.clone();
        self.row_params = sig.row_params.clone();
        self.assumed = self.constraints_in_force(&def.constraints);
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
        self.assumed.clear();
        self.derived = None;
        body_row
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
            if site == SpecSite::LawBody {
                d = d.note(
                    "a law whose body reaches the world is written `law/host`, which declares the \
                     relaxation rather than taking it silently, and can never be `proved`",
                );
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
            self.scope += 1;
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
        // A `law/host`'s body may carry any row: it says so in its own
        // declaration, which is what makes it auditable. The guard above is
        // unaffected — it stays pure whatever the law is, because a guard
        // decides the domain and a guard that could act would be choosing which
        // cases to be judged on.
        if !def.host {
            self.check_spec_purity(SpecSite::LawBody, def.body.span, &row);
        }
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
            host: def.host,
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
                // A generator that minted credentials and a shrinker that
                // printed counterexamples is a leak by construction, and the
                // code for both already exists.
                if name.as_str() == crate::ty::SECRET {
                    return Some(
                        "a `Secret` is a credential: quantifying over one would have the \
                         generator mint credentials and the shrinker print counterexamples"
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
            self.scope += 1;
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
                        let (ty, args) = instantiate_with(&scheme, &mut self.fresh);
                        // There is no map literal, so a map value comes from a
                        // reference — a builtin or a user function — or from a
                        // written annotation. Noting both is what makes the key
                        // check total without a walk over every solved type.
                        self.note_map_keys(q.span, &ty);
                        self.note_constraints(q.span, &key.name, &args);
                        (ty, Row::empty())
                    }
                    None => {
                        self.unknown_name(q);
                        (self.fresh.ty(), Row::empty())
                    }
                }
            }

            ExprKind::Unary { op, operand } => {
                let (t, row) = self.infer(operand);
                if let UnOp::Neg = op {
                    // Negation is the only way to write `-0.0`, so it settles at
                    // whichever numeric type its operand has, exactly as a
                    // binary operator does.
                    self.numerics.push(Numeric {
                        span: e.span,
                        op: BinOp::Sub,
                        ty: t.clone(),
                    });
                    return (t, row);
                }
                let want = Type::bool();
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

            // `named` is empty: `defaults::expand` cleared it in `resolve`,
            // and `no_named_argument_survives_resolve_anywhere_in_the_tree`
            // is what makes that an invariant rather than a hope.
            ExprKind::App { func, args, .. } => self.infer_app(e, func, args),

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

            // A record update's type is its base's type, and that is arranged by
            // expansion rather than by a rule here: by the time inference runs,
            // `{..s, a: 1}` *is* the literal that copies `s`'s other fields, so
            // the width is checked by the same exact-key-set unification every
            // record literal meets. Inferring the sugar here as if it were a
            // plain record would silently type a record of the wrong width.
            ExprKind::RecordUpdate { .. } => unreachable!(
                "`{{..b, f: e}}` is expanded away by `ply_syntax::parse_module`; the guard is \
                 `no_record_update_survives_parse_module_anywhere_in_the_tree`"
            ),

            // There is deliberately no typing rule for `?`, and no row rule
            // either. By the time inference runs, `e?` *is* the `match` its
            // longhand would have been, so its type and its row are that
            // `match`'s — which is the point: a rule here would have to thread
            // the enclosing function's return type and its row, and would be a
            // second account of what `?` means.
            ExprKind::Try { .. } => unreachable!(
                "`e?` is expanded away by `ply_syntax::parse_module`; the guard is \
                 `no_try_survives_parse_module_anywhere_in_the_tree`"
            ),

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

            ExprKind::WithRegion { region, body } => self.infer_with_region(region, body),

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
        // Three numeric types, and no type-directed dispatch to pick between
        // them, so the operand type is unified across the two sides here and
        // *which* of the three it is is settled once the enclosing definition
        // has been inferred. Deciding it at the node would read `x + 1.0` as
        // `Int` arithmetic on the way to the right operand.
        let arithmetic = matches!(
            op,
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem
        );
        let ordered = matches!(op, BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge);
        if arithmetic || ordered {
            let context = if arithmetic {
                "both operands of an arithmetic operator have one type"
            } else {
                "both sides of a comparison must have one type"
            };
            if self.expect(rhs.span, &lt, &rt, context) {
                self.numerics.push(Numeric {
                    span: e.span,
                    op,
                    ty: lt.clone(),
                });
            }
            return (if arithmetic { lt } else { Type::bool() }, row);
        }

        let (operand, result) = match op {
            BinOp::And | BinOp::Or => (Some(Type::bool()), Type::bool()),
            BinOp::Concat => (Some(Type::string()), Type::string()),
            _ => (None, Type::bool()),
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

    /// Decides which numeric type each `+`, `-`, `*`, `/`, `%`, `<`, `<=`, `>`
    /// or `>=` was applied at, now that the definition around it has been
    /// inferred.
    ///
    /// **Called before generalization, and that is the point.** An operand type
    /// nothing pinned defaults to `Int`; leaving it open would publish
    /// `fn add(a, b) = a + b` as `<t>(t, t) -> t`, a signature whose body works
    /// at three types and whose declaration claims every one.
    fn settle_numerics(&mut self) {
        for entry in std::mem::take(&mut self.numerics) {
            let ty = self.subst.resolve_ty(&entry.ty);
            let name = match &ty {
                Type::Var(_) => {
                    let _ = unify(&mut self.subst, &mut self.fresh, &ty, &Type::int());
                    continue;
                }
                Type::Con(name, args) if args.is_empty() => name.clone(),
                _ => {
                    self.numeric_mismatch(&entry, &ty);
                    continue;
                }
            };
            match (name.as_str(), entry.op) {
                ("Int" | "Float", _) => {}
                // The one place W2 refuses what every other language allows.
                // The exact quotient of two decimals is not a decimal, so `/`
                // would have to round — and a rounding nobody wrote down is the
                // defect this type exists to prevent.
                ("Decimal", BinOp::Div) => self.diags.push(
                    Diagnostic::error(codes::DECIMAL_DIVISION, "`/` is not defined on `Decimal`")
                        .primary(
                            entry.span,
                            "the exact quotient of two decimals is not a decimal",
                        )
                        .note(
                            "an operator would have to round, and a rounding nobody wrote down is \
                         the defect `Decimal` exists to prevent",
                        )
                        .note("call `decimal_div(a, b, 2, HalfEven)` and say how to round"),
                ),
                ("Decimal", _) => {}
                _ => self.numeric_mismatch(&entry, &ty),
            }
        }
    }

    fn numeric_mismatch(&mut self, entry: &Numeric, ty: &Type) {
        let mut printer = Printer::new();
        let what = if matches!(entry.op, BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge) {
            "ordered comparison"
        } else {
            "arithmetic"
        };
        self.diags.push(
            Diagnostic::error(
                codes::TYPE_MISMATCH,
                format!(
                    "`{}` is not defined on `{}`",
                    entry.op.text(),
                    printer.ty(ty)
                ),
            )
            .primary(entry.span, format!("{what} here"))
            .note("`Int`, `Float` and `Decimal` are the numeric types"),
        );
    }

    /// Notes every `Map` in `ty` so that [`Checker::check_map_keys`] can judge
    /// its key type once the module is solved.
    ///
    /// Called where a type is *written* and where a scheme is *instantiated*,
    /// which between them is every way a `Map` can enter a program: there is no
    /// map literal, so a map value comes from a builtin or from an annotation
    /// and from nowhere else.
    fn note_map_keys(&mut self, span: Span, ty: &Type) {
        fn walk(ty: &Type, out: &mut Vec<Type>) {
            match ty {
                Type::Con(name, args) => {
                    if name.as_str() == "Map" && args.len() == 2 {
                        out.push(args[0].clone());
                    }
                    args.iter().for_each(|a| walk(a, out));
                }
                Type::Fn { params, ret, .. } => {
                    params.iter().for_each(|p| walk(p, out));
                    walk(ret, out);
                }
                Type::Record(fields) => fields.values().for_each(|f| walk(f, out)),
                Type::Var(_) => {}
            }
        }
        let mut found = Vec::new();
        walk(ty, &mut found);
        if found.is_empty() {
            return;
        }
        let params: Vec<(TyVar, Symbol)> = self
            .ty_params
            .iter()
            .filter_map(|(name, t)| match t {
                Type::Var(v) => Some((*v, name.clone())),
                _ => None,
            })
            .collect();
        for key in found {
            self.map_keys.push(MapKeySite {
                span,
                scope: self.scope,
                key,
                assumed: self.assumed(Deriver::Ord),
                params: params.clone(),
            });
        }
    }

    /// `derivable(D, T)` over the `derive`'s target as a **solved** type.
    ///
    /// The deriver's own walk runs before resolution, so it reads the field's
    /// spelling and an alias is opaque to it: `type MyUnit = Unit` inside an
    /// `Option` reaches `option_json(unit_json())`, which writes `Some(())` and
    /// `None` as the same bytes, and the syntactic walk cannot see it. Here an
    /// alias is already expanded, so the one predicate answers for both
    /// spellings.
    ///
    /// Only for a target the deriver accepted — a `derive` it already refused
    /// has its diagnostic, and a second one naming the same field is noise —
    /// and only for a target with no parameters, whose instantiations are what
    /// the generated signature's `where derivable(D, ·)` covers at the call
    /// site.
    fn check_derives(&mut self, program: &Program) {
        let adts = self.adt_shapes();
        let lookup = |name: &Symbol| adts.get(name).map(clone_adt);
        let assumed = FxHashSet::default();
        let mut diags = Vec::new();
        for (i, module) in program.modules.iter().enumerate() {
            for item in &module.items {
                let Item::Derive(def) = item else { continue };
                if !expanded_here(module, def) {
                    continue;
                }
                self.module = i;
                let Some(ty) = self.derive_target(module, &def.target.name) else {
                    continue;
                };
                let cx = Derivability {
                    adt: &lookup,
                    assumed: &assumed,
                };
                let Err(blocked) = crate::derivable::derivable(def.deriver, &ty, &cx) else {
                    continue;
                };
                let mut printer = Printer::new();
                let blocker = printer.ty(&blocked.ty);
                let mut d = Diagnostic::error(
                    codes::NOT_DERIVABLE,
                    format!(
                        "`{}` cannot be derived for `{}`",
                        def.deriver, def.target.name
                    ),
                )
                .primary(
                    def.span,
                    format!("`{blocker}` has no {}", what(def.deriver)),
                );
                d = match blocked.why {
                    Why::NullInsideOption => d
                        .note(format!("`{blocker}` writes `None` and its payload alike"))
                        .note(ply_derive::rules::NULL_IN_OPTION_NOTE),
                    Why::FloatIsNotOrdered => {
                        d.note("`Float` has no total order: `NaN` is not equal to itself")
                    }
                    Why::Function => d.note("a function has no encoding to derive"),
                    Why::Handle(what) => d.note(format!(
                        "a `{what}` names a location rather than a value, so it has no encoding"
                    )),
                    Why::Secret(deriver) => secret_notes(d, deriver),
                    Why::Unconstrained(_) => continue,
                };
                diags.push(d.note(format!(
                    "write the dictionary for `{}` by hand, or change the field",
                    def.target.name
                )));
            }
        }
        self.diags.extend(diags);
    }

    /// The `derive` target as a solved type, for a target with no parameters.
    ///
    /// An alias body is converted here rather than read as `Type::Con`, and any
    /// diagnostic that conversion produces is dropped: the declaration was
    /// already converted where it is written, and reporting it twice would name
    /// the `derive` for a mistake in the `type`.
    fn derive_target(&mut self, module: &Module, target: &Symbol) -> Option<Type> {
        let qualified = module.name.qualify(target);
        let decl = self.types.get(&qualified).cloned()?;
        if !decl.params.is_empty() {
            return None;
        }
        let Some(alias) = decl.alias else {
            return Some(Type::Con(qualified, Vec::new()));
        };
        let mark = self.diags.len();
        let saved = std::mem::take(&mut self.ty_params);
        self.alias_stack.push(qualified);
        let owner = std::mem::replace(&mut self.module, decl.owner);
        let expanded = self.conv_type(&alias);
        self.module = owner;
        self.alias_stack.pop();
        self.ty_params = saved;
        self.diags.truncate(mark);
        Some(expanded)
    }

    /// Anything a generated definition failed on, other than a name it could not
    /// see, is **Ply's** fault.
    ///
    /// Derivation is total and structural: if generation succeeded, checking
    /// must succeed. The user did not write the body, cannot see it and cannot
    /// fix it, so calling one of these their error would send them looking in
    /// their own tree. This runs last so that it catches the deferred checks —
    /// constraints, map keys, comparisons — as well as the direct ones, and it
    /// keys on span because a generated definition's every span is its
    /// `derive`'s.
    fn attribute_generated(&mut self, program: &Program) {
        let mut spans: FxHashSet<Span> = FxHashSet::default();
        for module in &program.modules {
            for item in &module.items {
                if let Item::Fn(def) = item
                    && def.derived.is_some()
                {
                    spans.insert(def.span);
                }
            }
        }
        if spans.is_empty() {
            return;
        }
        for d in &mut self.diags {
            if d.code == codes::NOT_DERIVABLE
                || !d.primary_span().is_some_and(|s| spans.contains(&s))
            {
                continue;
            }
            let original = std::mem::replace(
                &mut d.message,
                String::from("a derived definition does not typecheck"),
            );
            d.code = codes::INTERNAL_ERROR;
            d.notes.insert(
                0,
                format!("this is a compiler bug in the deriver: {original}"),
            );
            d.notes.push(String::from(
                "derivation is total, so a generated body that fails to check is Ply's fault \
                 rather than yours",
            ));
        }
    }

    /// Notes a reference that instantiated a parameter its callee constrained.
    ///
    /// Only a reference: a constraint is a claim about a type argument, and a
    /// type argument is chosen exactly where a scheme is instantiated. A
    /// recursive call inside the definition's own group is bound
    /// monomorphically and instantiates nothing, which is right — it passes the
    /// very parameter the clause is about.
    fn note_constraints(&mut self, span: Span, callee: &Symbol, args: &[Type]) {
        let Some(constraints) = self.constraints.get(callee) else {
            return;
        };
        if constraints.is_empty() {
            return;
        }
        let sites: Vec<ConstraintSite> = constraints
            .iter()
            .filter_map(|c| {
                Some(ConstraintSite {
                    span,
                    deriver: c.deriver,
                    ty: args.get(c.param)?.clone(),
                    callee: callee.clone(),
                    callee_span: self.defs.get(callee).map(|d| d.span).unwrap_or(span),
                    assumed: self.assumed(c.deriver),
                    params: self.rigid_params(),
                })
            })
            .collect();
        self.constrained_uses.extend(sites);
    }

    /// `where derivable(D, a)` at the call sites that instantiate `a`.
    ///
    /// This is the one axis on which structural reflection is genuinely worse
    /// than typeclasses: an error deep inside an expansion is a search, and a
    /// search inside an edit loop is what an agent pays for. There is no
    /// resolution and no instance selection here — it is a predicate on a type,
    /// checked at a boundary, which is C++'s concepts and nothing more.
    fn check_constraints(&mut self) {
        let sites = std::mem::take(&mut self.constrained_uses);
        if sites.is_empty() {
            return;
        }
        let adts = self.adt_shapes();
        let lookup = |name: &Symbol| adts.get(name).map(clone_adt);
        let mut reported: FxHashSet<(Span, String, u8)> = FxHashSet::default();
        let mut diags = Vec::new();
        for site in sites {
            let ty = self.subst.resolve_ty(&site.ty);
            let cx = Derivability {
                adt: &lookup,
                assumed: &site.assumed,
            };
            let Err(blocked) = crate::derivable::derivable(site.deriver, &ty, &cx) else {
                continue;
            };
            // A flexible variable is a type argument nothing pinned rather than
            // one that is wrong — the call is ambiguous, and whatever pins it
            // later is what this check is about.
            if let Why::Unconstrained(v) = blocked.why
                && !self.subst.is_rigid_ty(v)
            {
                continue;
            }
            let mut printer = Printer::new();
            let rendered = printer.ty(&ty);
            if !reported.insert((site.span, rendered.clone(), site.deriver.tag())) {
                continue;
            }
            let blocker = printer.ty(&blocked.ty);
            let simple = simple_name(&site.callee);
            let mut d = Diagnostic::error(
                codes::NOT_DERIVABLE,
                format!("`{rendered}` cannot be derived for `{}`", site.deriver),
            )
            .primary(
                site.span,
                format!("`{simple}` requires `derivable({}, ·)` here", site.deriver),
            )
            .secondary(
                site.callee_span,
                format!("`{simple}` declares the constraint on its signature"),
            );
            d = match blocked.why {
                Why::Function => d.note(format!(
                    "`{blocker}` is a function, and a function has no {}",
                    what(site.deriver)
                )),
                Why::Handle(what) => d.note(format!(
                    "a `{what}` names a location rather than a value, so it has no encoding"
                )),
                Why::FloatIsNotOrdered => d
                    .note("`Float` has no total order: `NaN` is not equal to itself")
                    .note("use `Decimal` for exact numbers"),
                Why::NullInsideOption => d
                    .note(format!("`{blocker}` writes `None` and its payload alike"))
                    .note(ply_derive::rules::NULL_IN_OPTION_NOTE),
                Why::Secret(deriver) => secret_notes(d, deriver),
                Why::Unconstrained(v) => {
                    let name = site
                        .params
                        .iter()
                        .find(|(p, _)| *p == v)
                        .map(|(_, n)| n.to_string())
                        .unwrap_or(blocker);
                    d.note(format!(
                        "add `where derivable({}, {name})` to the signature this call sits in; \
                         the body may then assume it",
                        site.deriver
                    ))
                }
            };
            diags.push(d);
        }
        self.diags.extend(diags);
    }

    /// A `Map` key type must be ordered — exactly `derivable(ord, k)`, the same
    /// predicate `derive ord` walks.
    ///
    /// The whole reason `Map` is a language primitive rather than a library is
    /// that its iteration order is canonical, and an order is only canonical if
    /// every key it holds has one. `Float` is the case that makes this concrete:
    /// `total_cmp` is a total order on the *Rust* value, and NaN still is not
    /// equal to itself in the language, so a `Map<Float, v>` would be a lookup
    /// that fails to find what it just inserted.
    fn check_map_keys(&mut self) {
        let sites = std::mem::take(&mut self.map_keys);
        if sites.is_empty() {
            return;
        }
        let adts = self.adt_shapes();
        let lookup = |name: &Symbol| adts.get(name).map(clone_adt);
        let mut reported: FxHashSet<(u32, String)> = FxHashSet::default();
        let mut diags = Vec::new();
        for site in sites {
            let key = self.subst.resolve_ty(&site.key);
            let cx = Derivability {
                adt: &lookup,
                assumed: &site.assumed,
            };
            let Err(blocked) = ordered(&key, &cx) else {
                continue;
            };
            // An unsolved *flexible* variable is a key nothing pinned, not a key
            // that is wrong: `let m = map_new(); map_len(m)` never names one.
            // A rigid one came from a generic list, and that is the signature
            // the constraint belongs on.
            if let Why::Unconstrained(v) = blocked.why
                && !self.subst.is_rigid_ty(v)
            {
                continue;
            }
            let mut printer = Printer::new();
            let rendered = printer.ty(&key);
            if !reported.insert((site.scope, rendered.clone())) {
                continue;
            }
            let blocker = printer.ty(&blocked.ty);
            let mut d = Diagnostic::error(
                codes::NOT_DERIVABLE,
                format!("`{blocker}` is not an ordered type, so it cannot be a `Map` key"),
            )
            .primary(site.span, format!("this map's key type is `{rendered}`"));
            d = match blocked.why {
                Why::FloatIsNotOrdered => d
                    .note(
                        "`Float` has no total order: `NaN` is not equal to itself, so a key \
                         would have no position the next lookup could find it at",
                    )
                    .note("use `Decimal` for exact numbers, or key on something else"),
                Why::Function => d.note("a function has no order to sort keys by"),
                Why::Handle(what) => d.note(format!(
                    "a `{what}` names a location rather than a value, so two of them have no order"
                )),
                // A `json` refusal, and this question is `ord`'s.
                Why::NullInsideOption => d,
                Why::Secret(deriver) => secret_notes(d, deriver),
                Why::Unconstrained(v) => {
                    let name = site
                        .params
                        .iter()
                        .find(|(p, _)| *p == v)
                        .map(|(_, n)| n.to_string())
                        .unwrap_or(blocker);
                    d.note(format!(
                        "add `where derivable(ord, {name})` to this signature; the body may then                          assume it"
                    ))
                }
            };
            diags.push(d);
        }
        self.diags.extend(diags);
    }

    /// Every sum type this run declared, flattened for
    /// [`derivable`](crate::derivable::derivable). Built once per check rather
    /// than per site: it is a scan of `ctors`, and a program with a thousand
    /// constructors has no business paying for it per `Map`.
    fn adt_shapes(&self) -> FxHashMap<Symbol, Adt> {
        let mut out: FxHashMap<Symbol, Adt> = FxHashMap::default();
        for ctor in self.ctors.values() {
            let entry = out.entry(ctor.type_name.clone()).or_insert_with(|| Adt {
                params: ctor.scheme.ty_vars.clone(),
                fields: Vec::new(),
            });
            entry.fields.extend(ctor.fields.iter().cloned());
        }
        out
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

        self.note_spawn(&info, &op_info, args, &arg_tys);
        self.note_handoff(&info, &op_info, args, &arg_tys);

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
    fn op_scheme(&self, op: &OpInfo) -> Scheme {
        match &op.scheme {
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
        }
    }

    /// A perform site's view of an operation: fresh variables the call is free
    /// to pin, exactly as a call to a polymorphic function is.
    fn instantiate_op(&mut self, op: &OpInfo) -> (Vec<Type>, Type, Row) {
        let scheme = self.op_scheme(op);
        match instantiate(&scheme, &mut self.fresh) {
            Type::Fn {
                params,
                ret,
                effects,
            } => (params, *ret, effects),
            _ => unreachable!("operation schemes are always function types"),
        }
    }

    /// A **handler clause's** view of the same operation: the operation's own
    /// type variables are rigid, so the clause is checked once against every
    /// instantiation a perform site could choose.
    ///
    /// This is not a refinement, it is the soundness of the construct. A row
    /// carries atoms and no types, so a `handle` sees only that its body may
    /// perform `db.read[t]` — never which `a` a perform three definitions deeper
    /// unified the operation's `-> a` with. Instantiating the clause with
    /// ordinary fresh variables therefore lets the *clause* choose a type
    /// nothing ever unifies with the caller's: for `read fetch[k](s) -> a`, the
    /// clause `fetch[k](s) -> s` answers a `Secret<String>` while
    /// `vault.fetch[k](s)` at a site that wanted a `String` is happily typed
    /// `String`, and `ply check` accepts a program that laundered a credential
    /// into an unrelated type. Rigid variables are what say "for every `a`",
    /// which is the obligation a clause actually carries.
    ///
    /// The cost, stated: an operation whose return type is a variable the
    /// parameters do not determine can only be handled by a clause that produces
    /// one from a parameter, and no clause can produce a `List<a>` out of
    /// nothing. That is correct — such an operation is unsound, not merely
    /// awkward — and `examples/store.ply` was rewritten to declare what each of
    /// its tables holds rather than to rely on it.
    fn instantiate_op_for_clause(&mut self, op: &OpInfo) -> (Vec<Type>, Type, Row) {
        let scheme = self.op_scheme(op);
        let ty_vars: Vec<TyVar> = scheme.ty_vars.iter().map(|_| self.fresh.ty_var()).collect();
        let row_vars: Vec<RowVar> = scheme
            .row_vars
            .iter()
            .map(|_| self.fresh.row_var())
            .collect();
        for v in &ty_vars {
            self.subst.mark_rigid_ty(*v);
        }
        for v in &row_vars {
            self.subst.mark_rigid_row(*v);
        }
        match crate::env::rename_scheme(&scheme, &ty_vars, &row_vars) {
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
        // Keyed on what `Stack::find_handler` dispatches on, which is the
        // operation and not its atom. `net.recv[conn]`, `net.send[conn]` and
        // `net.close[conn]` are one atom and three reachable clauses.
        let mut selected: BTreeSet<(Symbol, Symbol, Resource)> = BTreeSet::new();
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
            let (params, ret, _) = self.instantiate_op_for_clause(&op_info);
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

            let key = (info.name.clone(), op_info.name.clone(), res.clone());
            let atom = EffectAtom::new(info.name.clone(), res, op_info.mode);
            handled.insert(atom);
            if !selected.insert(key) {
                self.diags.push(
                    Diagnostic::warning(
                        codes::DUPLICATE_DEFINITION,
                        format!(
                            "`{}.{}{}` is handled more than once",
                            info.name,
                            op_info.name,
                            match clause.resource.as_ref() {
                                Some(r) => format!("[{}]", r.name),
                                None => String::new(),
                            }
                        ),
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
                (Some(_), Some(result)) => {
                    self.expect(
                        clause.body.span,
                        result,
                        &clause_ty,
                        "a clause that binds a continuation returns the `handle`'s result",
                    );
                }
                _ => {
                    let ok = self.expect(
                        clause.body.span,
                        &ret,
                        &clause_ty,
                        "a handler clause returns the operation's result",
                    );
                    // The one mismatch whose cause is not visible in the two
                    // types: the operation is polymorphic, so the clause owes an
                    // answer for *every* instantiation and the type it was
                    // checked against is a variable it may not choose.
                    if !ok && !op_free_vars(&op_info).is_empty() {
                        self.note_universal_clause(&info, &op_info);
                    }
                }
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

    /// ADR 0017 §1: a cell is a value allocated in `r`.
    ///
    /// Written inside `with_region[r]` the cell belongs to that region and is
    /// discharged and checked there, so it may outlive this `{ .. }` and not the
    /// region. Written on its own, `with_cell[r]` **is** the region — so it
    /// opens one, files a [`RegionSite`] like any other, and is held to all four
    /// of [`Checker::check_regions`]'s rules rather than only to the shape of
    /// its own result type.
    ///
    /// That last part is the fix for the route ADR 0017 lists first among the
    /// ways this could go wrong: a closure that captured the cell need not
    /// mention it in its parameters or its result, only in its effect row, and
    /// filing no site meant nothing ever looked at the row, at what the body
    /// stored into an enclosing binding, at what it spawned, or at what it
    /// handed to an operation.
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

        // A cell allocated into an enclosing region of the same brand belongs to
        // that region: it opens nothing here and is checked at that region's
        // boundary.
        if self.region_is_open(&resource.name) {
            self.env.push();
            self.env.bind(binder.name.clone(), Scheme::mono(cell));
            let (body_ty, body_row) = self.infer(body);
            self.env.pop();
            return (body_ty, self.join(e.span, init_row, body_row));
        }

        let outer = self.outer_bindings(body);
        self.open_regions.push(OpenRegion {
            name: resource.name.clone(),
            source: RegionSource::WithCell,
            span: resource.span,
            simulate_depth: self.simulate_depth,
            spawns: Vec::new(),
            handoffs: Vec::new(),
        });

        let mark = self.performs.len();
        self.env.push();
        self.env.bind(binder.name.clone(), Scheme::mono(cell));
        let (body_ty, body_row) = self.infer(body);
        self.env.pop();
        let (spawns, handoffs) = self
            .open_regions
            .pop()
            .map(|r| (r.spawns, r.handoffs))
            .unwrap_or_default();

        let handled: BTreeSet<EffectAtom> = [
            EffectAtom::new(CELL, Resource::Named(resource.name.clone()), Mode::Read),
            EffectAtom::new(CELL, Resource::Named(resource.name.clone()), Mode::Write),
        ]
        .into();
        self.discharge(mark..self.performs.len(), &handled);

        let mut exits = Vec::new();
        region_exits(body, &mut exits);
        let exits = exits.iter().map(|x| x.span).collect();

        self.regions.push(RegionSite {
            name: resource.name.clone(),
            source: RegionSource::WithCell,
            span: resource.span,
            body_span: body.span,
            body_ty: body_ty.clone(),
            exits,
            outer,
            spawns,
            handoffs,
        });

        let row = self.join(e.span, init_row, body_row.without(&handled));
        (body_ty, row)
    }

    fn region_is_open(&self, name: &Symbol) -> bool {
        self.open_regions.iter().any(|r| &r.name == name)
    }

    /// Files a `task.spawn` argument against every open `with_region[r]` the
    /// spawned task could outlive. A region that opened *after* the enclosing
    /// `simulate` is not one of them: that scheduler ends inside the region, and
    /// so does every task it is running.
    ///
    /// A bare `with_cell[r]` is deliberately excluded. "A cell reaching a task
    /// is how tasks share memory" is CONTRACTS §`simulate`'s rule and ADR 0005's
    /// monotone world made it sound; refusing it would change what a landed
    /// program means from "runs" to "does not compile", which ADR 0017's
    /// governing property forbids. It is also safe under ADR 0017 §3 as amended:
    /// a `task` operation anywhere in a region makes it `shared`
    /// (`region_kind::Cause::Task`), and a `shared` region's slots outlive its
    /// close for exactly this reason. `with_region` keeps the stricter rule
    /// because it is new syntax with no program depending on the loose one.
    fn note_spawn(&mut self, info: &EffectInfo, op: &OpInfo, args: &[Expr], tys: &[Type]) {
        if info.name.as_str() != prelude::TASK || op.name.as_str() != prelude::SPAWN_OP {
            return;
        }
        let depth = self.simulate_depth;
        for region in &mut self.open_regions {
            if region.simulate_depth < depth || region.source == RegionSource::WithCell {
                continue;
            }
            for (arg, ty) in args.iter().zip(tys) {
                region.spawns.push(SpawnSite {
                    span: arg.span,
                    ty: ty.clone(),
                });
            }
        }
    }

    /// Files every argument of a user-declared operation against every open
    /// region, because the handler that receives it can be installed anywhere —
    /// including outside the region, or in another definition entirely.
    ///
    /// `task` is excluded because [`Checker::note_spawn`] already decides it more
    /// precisely: the handler is the enclosing `simulate` and the region knows
    /// whether that scheduler started inside it. `cell` never reaches here at
    /// all — [`Checker::resolve_op`] refuses a written `cell.read[r]`, so the
    /// only cell operations are the builtins the region itself discharges.
    fn note_handoff(&mut self, info: &EffectInfo, op: &OpInfo, args: &[Expr], tys: &[Type]) {
        if info.name.as_str() == prelude::TASK || self.open_regions.is_empty() {
            return;
        }
        let name = format!("{}.{}", info.simple_name, op.name);
        for region in &mut self.open_regions {
            for (arg, ty) in args.iter().zip(tys) {
                region.handoffs.push(HandoffSite {
                    span: arg.span,
                    op: name.clone(),
                    ty: ty.clone(),
                });
            }
        }
    }

    /// ```text
    /// Γ ⊢ body : T / ρ_b        no value branded `r` reaches an exit of `body`
    /// ─────────────────────────────────────────────────────────────────────────
    /// Γ ⊢ with_region[r] { body } : T / (ρ_b \ {cell.read[r], cell.write[r]})
    /// ```
    ///
    /// The premise is not decidable here — a brand routinely reaches an exit
    /// type only once the definition around the region has been solved — so the
    /// region records what it would have to look at and
    /// [`Checker::check_regions`] asks the question at the end of the run. The
    /// row half is not deferrable, because the atoms have to be off the
    /// `performs` list before the enclosing definition's row is judged.
    fn infer_with_region(&mut self, region: &Ident, body: &Expr) -> (Type, Row) {
        if let Some(open) = self
            .open_regions
            .iter()
            .find(|r| r.name == region.name)
            .map(|r| r.span)
        {
            self.diags.push(
                Diagnostic::error(
                    codes::REGION_ALREADY_OPEN,
                    format!("region `{}` is already open here", region.name),
                )
                .primary(region.span, "a second region under the same name")
                .secondary(
                    open,
                    format!("`{}` opens here and is still open", region.name),
                )
                .note(
                    "the brand is the name, so the two regions' values would have one type and \
                     closing the inner one would free what the outer one still holds",
                )
                .note("give the inner region a different name"),
            );
        }

        let outer = self.outer_bindings(body);
        self.open_regions.push(OpenRegion {
            name: region.name.clone(),
            source: RegionSource::WithRegion,
            span: region.span,
            simulate_depth: self.simulate_depth,
            spawns: Vec::new(),
            handoffs: Vec::new(),
        });

        let mark = self.performs.len();
        let (body_ty, body_row) = self.infer(body);
        let (spawns, handoffs) = self
            .open_regions
            .pop()
            .map(|r| (r.spawns, r.handoffs))
            .unwrap_or_default();

        let handled: BTreeSet<EffectAtom> = [
            EffectAtom::new(CELL, Resource::Named(region.name.clone()), Mode::Read),
            EffectAtom::new(CELL, Resource::Named(region.name.clone()), Mode::Write),
        ]
        .into();
        self.discharge(mark..self.performs.len(), &handled);

        // Every exit carries the region's own result type — a branch, an arm, a
        // `return` clause and a nested region's tail are all unified with it —
        // so what the walk buys is the span to report at, not a second type.
        let mut exits = Vec::new();
        region_exits(body, &mut exits);
        let exits = exits.iter().map(|x| x.span).collect();

        self.regions.push(RegionSite {
            name: region.name.clone(),
            source: RegionSource::WithRegion,
            span: region.span,
            body_span: body.span,
            body_ty: body_ty.clone(),
            exits,
            outer,
            spawns,
            handoffs,
        });

        let row = self.subst.resolve_row(&body_row).without(&handled);
        (body_ty, row)
    }

    /// The bindings a region could store into: everything the definition being
    /// checked has in scope when the region opens, paired with where each is
    /// first named inside it.
    ///
    /// Only bindings whose type still has an unsolved variable are kept. A
    /// binding already solved to a type with no variable in it cannot acquire a
    /// brand, and keeping them all would make every region pay for the whole
    /// enclosing scope.
    ///
    /// The global scope is excluded: a top-level scheme is generalized before it
    /// is bound, so its variables are quantified and unification inside a region
    /// reaches a fresh copy rather than the scheme itself.
    fn outer_bindings(&mut self, body: &Expr) -> Vec<OuterBinding> {
        let mut kept: Vec<(Symbol, Type)> = Vec::new();
        for (name, scheme) in self.env.locals() {
            let (mut tys, mut rows) = (BTreeSet::new(), BTreeSet::new());
            self.subst.free_vars(&scheme.ty, &mut tys, &mut rows);
            for v in &scheme.ty_vars {
                tys.remove(v);
            }
            for v in &scheme.row_vars {
                rows.remove(v);
            }
            if !tys.is_empty() || !rows.is_empty() {
                kept.push((name.clone(), scheme.ty.clone()));
            }
        }
        if kept.is_empty() {
            return Vec::new();
        }

        let refs = Refs::of(body);
        let mut first: FxHashMap<&Symbol, Span> = FxHashMap::default();
        for q in refs.bare() {
            first.entry(q.symbol()).or_insert(q.span);
        }

        kept.into_iter()
            .filter_map(|(name, ty)| {
                first.get(&name).map(|span| OuterBinding {
                    name,
                    ty,
                    site: *span,
                })
            })
            .collect()
    }

    /// ADR 0017 §2, asked once the module is solved for the reason
    /// [`Checker::check_simulations`] is asked then: the brand a value carries
    /// is routinely still an unsolved variable while the region around it is
    /// being walked, and a check that ran at the closing brace would answer
    /// about a type that had not been decided yet.
    fn check_regions(&mut self) {
        for site in std::mem::take(&mut self.regions) {
            self.check_region_exits(&site);
            self.check_region_stores(&site);
            self.check_region_spawns(&site);
            self.check_region_handoffs(&site);
        }
    }

    /// The value the region evaluates to. Reported at each position it leaves
    /// through — a branch, an arm, a nested region's tail — rather than at the
    /// whole body, so the label sits on the expression that produced it.
    ///
    /// Asked here rather than at the closing brace for the reason
    /// [`Checker::check_regions`] gives: the brand a value carries is routinely
    /// still an unsolved variable while the region is being walked, so the
    /// question is asked once the module is solved and the type is resolved. W2
    /// found the analogous hole reachable through a type alias precisely because
    /// the check ran too early.
    fn check_region_exits(&mut self, site: &RegionSite) {
        let ty = self.subst.resolve_ty(&site.body_ty);
        if !brand_in(&ty, site.name.as_str()) {
            return;
        }
        if site.source == RegionSource::WithCell {
            return self.report_escaping_cell(site, &ty);
        }
        let mut printer = Printer::new();
        let shown = printer.ty(&ty);
        let mut d = Diagnostic::error(
            codes::REGION_ESCAPE,
            format!(
                "a value allocated in region `{}` escapes the region",
                site.name
            ),
        );
        let spans = if site.exits.is_empty() {
            &[site.body_span][..]
        } else {
            &site.exits
        };
        for span in spans {
            d = d.primary(*span, format!("this has type `{shown}`"));
        }
        d = d.secondary(
            site.span,
            format!(
                "`{}` opens here, and everything allocated in it is freed at the region's `}}`",
                site.name
            ),
        );
        if carries_brand_in_row(&ty, site.name.as_str()) {
            d = d.note(format!(
                "the closure's row still carries `cell.read[{0}]` or `cell.write[{0}]`, so it \
                 reads or writes a value the region frees",
                site.name
            ));
        }
        self.diags.push(d.note(format!(
            "read the value inside the region and answer with something that does not mention \
                 `{}`",
            site.name
        )));
    }

    /// The same escape out of a bare `with_cell[r]`, which has reported it under
    /// [`codes::TYPE_MISMATCH`] since before `with_region` existed. The code and
    /// the wording stay put; what changed is that `brand_in` now looks at a
    /// function type's effect row, so a closure that carries the cell only in
    /// its row is caught rather than accepted.
    fn report_escaping_cell(&mut self, site: &RegionSite, ty: &Type) {
        let mut printer = Printer::new();
        let shown = printer.ty(ty);
        let mut d = Diagnostic::error(
            codes::TYPE_MISMATCH,
            format!("the cell escapes its `with_cell[{}]` region", site.name),
        );
        let spans = if site.exits.is_empty() {
            &[site.body_span][..]
        } else {
            &site.exits
        };
        for span in spans {
            d = d.primary(*span, format!("this has type `{shown}`"));
        }
        if carries_brand_in_row(ty, site.name.as_str()) {
            d = d.note(format!(
                "the closure's row still carries `cell.read[{0}]` or `cell.write[{0}]`, so it \
                 reads or writes a cell the region frees",
                site.name
            ));
        }
        self.diags
            .push(d.note("read the cell inside the region and return the value instead"));
    }

    /// A store into something the region did not create — the case the exit type
    /// cannot see, because the region's own result is then `Unit` and the brand
    /// is sitting in a binding that predates it.
    fn check_region_stores(&mut self, site: &RegionSite) {
        for binding in &site.outer {
            let ty = self.subst.resolve_ty(&binding.ty);
            if !brand_in(&ty, site.name.as_str()) {
                continue;
            }
            let mut printer = Printer::new();
            self.diags.push(
                Diagnostic::error(
                    codes::REGION_ESCAPE,
                    format!(
                        "a value allocated in region `{}` is stored where it outlives the region",
                        site.name
                    ),
                )
                .primary(
                    binding.site,
                    format!(
                        "`{}` has type `{}`, and it was bound before `{}` opened",
                        binding.name,
                        printer.ty(&ty),
                        site.name
                    ),
                )
                .secondary(
                    site.span,
                    format!("`{}` opens here and is freed at its `}}`", site.name),
                )
                .note(format!(
                    "storing into `{}` outlives the region, so what is stored may not mention `{}`",
                    binding.name, site.name
                )),
            );
        }
    }

    /// A task outlives the region that spawned it whenever the scheduler does,
    /// and the scheduler is the enclosing `simulate`. A `simulate` opened
    /// *inside* the region ends before the region does and is therefore safe;
    /// one opened outside it is not, and a branded value reaching that spawn is
    /// an escape into another task.
    fn check_region_spawns(&mut self, site: &RegionSite) {
        for spawn in &site.spawns {
            let ty = self.subst.resolve_ty(&spawn.ty);
            if !brand_in(&ty, site.name.as_str()) {
                continue;
            }
            let mut printer = Printer::new();
            self.diags.push(
                Diagnostic::error(
                    codes::REGION_ESCAPE,
                    format!(
                        "a value allocated in region `{}` is sent to another task",
                        site.name
                    ),
                )
                .primary(spawn.span, format!("this has type `{}`", printer.ty(&ty)))
                .secondary(
                    site.span,
                    format!("`{}` opens here and is freed at its `}}`", site.name),
                )
                .note(
                    "the scheduler that runs the task was installed outside the region, so the \
                     task can still be running when the region's memory is gone",
                )
                .note(format!(
                    "open the `simulate` region inside `with_region[{}]`, or pass the task \
                     something that does not mention `{0}`",
                    site.name
                )),
            );
        }
    }

    /// The route no other check can see, because at the far end of it there is
    /// no type left to look at.
    ///
    /// An operation is declared once for the whole program, so
    /// [`Checker::reject_declared_cell`] already refuses one that names a `Cell`.
    /// A declaration that names a *type variable* instead says nothing about
    /// cells and is not refused — and the perform site and the handler
    /// instantiate that variable independently, in different definitions. The
    /// perform site solves its copy to `Cell[r]<Int>`; the handler's copy stays a
    /// variable it may generalize, store into a binding that outlives the region,
    /// or answer the whole `handle` with. Nothing the handler holds mentions `r`,
    /// so the escape is invisible there and has to be refused here.
    fn check_region_handoffs(&mut self, site: &RegionSite) {
        for handoff in &site.handoffs {
            let ty = self.subst.resolve_ty(&handoff.ty);
            if !brand_in(&ty, site.name.as_str()) {
                continue;
            }
            let mut printer = Printer::new();
            let mut d = Diagnostic::error(
                codes::REGION_ESCAPE,
                format!(
                    "a value allocated in region `{}` is handed to `{}`",
                    site.name, handoff.op
                ),
            )
            .primary(handoff.span, format!("this has type `{}`", printer.ty(&ty)))
            .secondary(
                site.span,
                format!("`{}` opens here and is freed at its `}}`", site.name),
            );
            if carries_brand_in_row(&ty, site.name.as_str()) {
                d = d.note(format!(
                    "the closure's row still carries `cell.read[{0}]` or `cell.write[{0}]`, so \
                     whoever handles the operation can read or write a value the region frees",
                    site.name
                ));
            }
            self.diags.push(
                d.note(format!(
                    "`{}` is declared outside every region, so its type cannot say the value \
                     belongs to `{}`, and the handler — which may be installed anywhere, and may \
                     outlive the region — receives it at a type variable that mentions no region \
                     at all",
                    handoff.op, site.name
                ))
                .note(format!(
                    "read the value inside the region and perform the operation with something \
                     that does not mention `{}`",
                    site.name
                )),
            );
        }
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
        self.simulate_depth += 1;
        let (body_ty, body_row) = self.infer(body);
        self.simulate_depth -= 1;
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

    /// Fills in [`DefInfo::internally_effectful`] for every definition in the
    /// program: which of them can execute a `perform` their published row does
    /// not show.
    ///
    /// Every `DefInfo` is constructed with the flag **set**, and this is the
    /// only thing that clears it. So the answer to "was this definition
    /// walked?" and the answer to "may a compiled body be entered for it?" are
    /// the same answer, and a definition this pass never reaches keeps the
    /// conservative one.
    ///
    /// # Why it is transitive
    ///
    /// An atom performed inside a call either escapes — and the published row
    /// carries it, which is the gate `Machine::compiled_answer` already had —
    /// or it is discharged by a `handle` in the call's dynamic extent, and then
    /// nothing published records it at all. The `handle` that discharges it can
    /// be anywhere in that extent, not only in the body that performed. So a
    /// definition whose own body mentions neither `perform` nor `handle` still
    /// performs, invisibly, when something it calls discharges its own
    /// operations. Measured rather than reasoned: with
    /// `fn handled(x) = handle { touch(x) } with { .. }` and
    /// `fn wrapper(x) = handled(x)`, inference publishes an empty `footprint`
    /// *and* an empty `performed` for both, and running `wrapper` records
    /// `state.read` in `ply_eval::Trace`.
    ///
    /// # What is and is not an edge
    ///
    /// A reference is an edge when it denotes a definition of this program.
    /// A builtin, a constructor and a prelude name are not: the only route to
    /// `ply_eval::Trace::record` in either engine is an `ExprKind::Perform`
    /// (`machine.rs`'s `State::Perform`, `interp.rs`'s `perform`), which is
    /// source this pass walks. `with_cell` is deliberately not an effect here
    /// for the same reason `Trace` does not record one — a cell atom carries no
    /// resource label to reconstruct, and the two engines' arenas are compared
    /// slot by slot instead.
    ///
    /// Locals other than the definition's own parameters are not resolved away,
    /// so a lambda parameter or a `let` binder that shadows a definition's name
    /// contributes that definition's edge. That costs a definition its entry and
    /// never the other way round. Measured on `examples/` rather than guessed
    /// at, and it is why the parameter set is subtracted at all: without that
    /// one step **29** of the 953 empty-row definitions there are marked, and
    /// with it **11**. The eighteen were spurious in one shape —
    /// `desk.item_named(shelf, ..)` is `fold` over its own parameter and `desk`
    /// also declares `fn shelf`. The eleven that remain are real: `desk.under`
    /// is `handle { .. } with { signal.stopping() -> false }` and publishes an
    /// empty row, and the other ten reach it.
    ///
    /// A `requires` / `ensures` clause is not walked: it is erased before
    /// evaluation and cannot perform (`E0412`).
    fn mark_internal_effects(&mut self, program: &Program) {
        let mut index: FxHashMap<Symbol, usize> = FxHashMap::default();
        let mut names: Vec<Symbol> = Vec::new();
        for module in &program.modules {
            for item in &module.items {
                let Item::Fn(def) = item else { continue };
                let name = module.name.qualify(&def.name.name);
                // A module declaring the same name twice is `E0105`, and this
                // pass runs *before* the diagnostic is reported —
                // `check_program_with` collects and reports at the end. So the
                // second `fn f` reuses the first one's node rather than
                // claiming an index no vector has, and the two bodies' effects
                // merge, which is the conservative direction.
                // `a_definition_declared_twice_is_a_diagnostic_even_when_one_of_them_handles_an_effect`
                // is what fails without this: a pure duplicate is survivable
                // because `&&` short-circuits the out-of-bounds read, and one
                // written with `handle` is not.
                if !index.contains_key(&name) {
                    index.insert(name.clone(), names.len());
                    names.push(name);
                }
            }
        }

        let mut effects = vec![false; names.len()];
        // Callee -> callers, because propagation runs from a definition that
        // performs outwards to everything that can reach it.
        let mut callers: Vec<Vec<usize>> = vec![Vec::new(); names.len()];
        let mut pending: Vec<usize> = Vec::new();
        for (m, module) in program.modules.iter().enumerate() {
            self.module = m;
            for item in &module.items {
                let Item::Fn(def) = item else { continue };
                let Some(&at) = index.get(&module.name.qualify(&def.name.name)) else {
                    continue;
                };
                let refs = Refs::of(&def.body);
                if refs.effects && !effects[at] {
                    effects[at] = true;
                    pending.push(at);
                }
                // A parameter shadows a definition of the same name for the
                // whole body — an inner binder only shadows it further — so a
                // bare reference to one never denotes the global. Dropping
                // those is the one narrowing available without a scope-aware
                // walk, and it is worth having: `desk.item_named(shelf, sku)`
                // is `fold` over its own parameter, and `desk` also declares
                // `fn shelf`.
                let params: FxHashSet<&Symbol> = def.params.iter().map(|p| &p.name.name).collect();
                for q in &refs.names {
                    if q.is_bare() && params.contains(q.symbol()) {
                        continue;
                    }
                    let Some(callee) = self.declared_value(q) else {
                        continue;
                    };
                    if let Some(&to) = index.get(&callee) {
                        callers[to].push(at);
                    }
                }
            }
        }

        while let Some(at) = pending.pop() {
            for &caller in &callers[at] {
                if !effects[caller] {
                    effects[caller] = true;
                    pending.push(caller);
                }
            }
        }

        for (at, name) in names.iter().enumerate() {
            if let Some(def) = self.defs.get_mut(name) {
                def.internally_effectful = effects[at];
            }
        }
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
                                self.covered_ctor(name)
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
                    .ctor_name(name)
                    .and_then(|qualified| self.ctors.get(&qualified).cloned());
                let Some(info) = info else {
                    if name.is_bare() || self.declared_value(name).is_some() {
                        let mut d = Diagnostic::error(
                            codes::UNKNOWN_NAME,
                            format!("unknown constructor `{}`", name.symbol()),
                        )
                        .primary(name.span, "not found")
                        .note("constructors come from a `type` declaration with variants");
                        // A builtin type constructor has no `type` item to read,
                        // so the general note above sends the reader looking for
                        // a declaration that cannot exist. For `Secret` the
                        // absence is the mechanism, and saying so is the whole
                        // difference between a puzzle and an answer.
                        if let Some((builtin, _)) = builtin_types()
                            .iter()
                            .find(|(b, _)| *b == name.symbol().as_str())
                        {
                            d = d.note(format!("`{builtin}` is a builtin type and declares none"));
                            if *builtin == crate::ty::SECRET {
                                d = d.note(
                                    "that is deliberate: a pattern binding the payload would be a \
                                     one-line escape from every guarantee `Secret` makes",
                                )
                                .note(
                                    "use `secret_verify` to check a candidate, `secret_is_empty` \
                                     to check presence, or hand the whole `Secret` to a handler \
                                     clause",
                                );
                            }
                        }
                        self.diags.push(d);
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

    /// Whether the two unified, so a caller can stop rather than report a second
    /// diagnostic about a type the first one already explained.
    /// Why a clause for a polymorphic operation cannot answer a concrete type.
    ///
    /// Attached to the mismatch rather than raised beside it, because there is
    /// one mistake here and the reader needs one thing to read.
    fn note_universal_clause(&mut self, info: &EffectInfo, op: &OpInfo) {
        let Some(last) = self.diags.pop() else { return };
        let written = self.as_written(&info.name);
        self.diags.push(
            last.note(format!(
                "`{written}.{}` is declared with a type variable, so a clause for it has to answer \
                 every type a perform site could ask for",
                op.name
            ))
            .secondary(op.span, "declared here")
            .note(
                "a row carries atoms and no types, so the `handle` cannot see which type a perform \
                 site picked — answering a concrete one would be a value of a type the caller \
                 never asked for",
            )
            .note(
                "declare the operation at the type it actually answers, or produce the result from \
                 one of the clause's own parameters",
            ),
        );
    }

    fn expect(&mut self, span: Span, expected: &Type, found: &Type, context: &str) -> bool {
        if let Err(e) = unify(&mut self.subst, &mut self.fresh, expected, found) {
            self.report_unify(&e, span, context);
            return false;
        }
        true
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
        if self.unresolved_in_derived(q, "definition") {
            return;
        }
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
    /// The `Checker::scope` this signature was built under. A body is the same
    /// definition as its signature, and the two are walked in separate passes
    /// over a mutually recursive group, so the body restores it rather than
    /// taking whatever the counter reached in between.
    scope: u32,
}

fn lit_type(l: &Lit) -> Type {
    match l {
        Lit::Int(_) => Type::int(),
        Lit::Bool(_) => Type::bool(),
        Lit::Str(_) => Type::string(),
        Lit::Bytes(_) => Type::bytes(),
        Lit::Float(_) => Type::float(),
        Lit::Decimal { .. } => Type::decimal(),
        Lit::Unit => Type::unit(),
    }
}

/// The `effect set` names this definition's row was written with, in source
/// order. Only names a set could have: the parser has already refused a
/// qualified one, so nothing here needs qualifying.
fn row_aliases(def: &FnDef) -> Vec<Symbol> {
    def.effects
        .as_ref()
        .map(|r| r.aliases.iter().map(|q| q.symbol().clone()).collect())
        .unwrap_or_default()
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

/// The builtin a generated `OrdDict` is built out of, reserved so that no module
/// can supply it.
pub const COMPARE_VALUES: &str = "compare_values";

/// The three builtins over [`ty::SECRET`](crate::ty::SECRET), and the whole of
/// what a program may do with a credential. There is no `secret_expose`, no
/// `secret_len` and no `String` in any return type, so the only plaintext that
/// leaves is one a host operation was handed whole — which is `E0439`.
pub const SECRET_OF_STRING: &str = "secret_of_string";
pub const SECRET_VERIFY: &str = "secret_verify";
pub const SECRET_IS_EMPTY: &str = "secret_is_empty";

/// Why a derivation refused a `Secret`, in the wording all three call sites
/// print. One function because three copies of a security claim are three things
/// that can drift apart.
fn secret_notes(mut d: Diagnostic, deriver: Deriver) -> Diagnostic {
    d = d.note(ply_derive::rules::Refusal::Secret(deriver).reason());
    match ply_derive::rules::Refusal::Secret(deriver).note() {
        Some(note) => d.note(note),
        None => d,
    }
}

fn encloses(outer: Span, inner: Span) -> bool {
    !outer.is_dummy()
        && !inner.is_dummy()
        && outer.source == inner.source
        && outer.start <= inner.start
        && inner.end <= outer.end
}

/// The last segment of a program-wide name, which is what the source wrote.
fn simple_name(name: &Symbol) -> &str {
    name.as_str().rsplit('.').next().unwrap_or(name.as_str())
}

/// Whether expansion produced a definition for this `derive`.
fn expanded_here(module: &Module, def: &DeriveDef) -> bool {
    module.items.iter().any(|i| match i {
        Item::Fn(f) => f
            .derived
            .as_ref()
            .is_some_and(|d| d.deriver == def.deriver && d.target == def.target.name),
        _ => false,
    })
}

/// What a deriver is about, for "a function has no …".
fn what(deriver: Deriver) -> &'static str {
    match deriver {
        Deriver::Json => "JSON encoding",
        Deriver::Eq => "structural equality",
        Deriver::Ord => "order",
    }
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

/// Whether a **resolved** type carries region `r`'s brand, by either of the two
/// routes a brand travels.
///
/// The first is the type itself: the brand is a nullary constructor no lexer can
/// produce, sitting where a `Cell`'s region argument goes, and a walk into every
/// type argument, record field and function position finds it wherever it was
/// put — a list element, a `Map` key, a constructor's type argument, a returned
/// closure's result. Running on the resolved type is what closes the alias
/// route: `conv_type` expands an alias before a `Type` exists, so `Map<Key, Int>`
/// and `Map<String, Int>` are the same type here and get the same answer.
///
/// The second is a function's effect row. A closure that captured a value
/// allocated in `r` need not mention `r` in its parameters or its result — but
/// reading or writing that value is a `cell.read[r]` / `cell.write[r]` atom, and
/// the atom is in the closure's row whether the read is written in the closure
/// or in something the closure calls. That is what makes a capture visible to a
/// check that only ever looks at types.
fn brand_in(t: &Type, region: &str) -> bool {
    match t {
        Type::Var(_) => false,
        Type::Con(_, args) => {
            region_of(t) == Some(region) || args.iter().any(|a| brand_in(a, region))
        }
        Type::Fn {
            params,
            ret,
            effects,
        } => {
            row_brands(effects, region)
                || params.iter().any(|p| brand_in(p, region))
                || brand_in(ret, region)
        }
        Type::Record(fields) => fields.values().any(|f| brand_in(f, region)),
    }
}

/// Whether the brand reached this type through a function's row rather than
/// through its shape, which is the difference between "you returned the cell"
/// and "you returned something that reads it".
fn carries_brand_in_row(t: &Type, region: &str) -> bool {
    match t {
        Type::Var(_) => false,
        Type::Con(_, args) => args.iter().any(|a| carries_brand_in_row(a, region)),
        Type::Fn {
            params,
            ret,
            effects,
        } => {
            row_brands(effects, region)
                || params.iter().any(|p| carries_brand_in_row(p, region))
                || carries_brand_in_row(ret, region)
        }
        Type::Record(fields) => fields.values().any(|f| carries_brand_in_row(f, region)),
    }
}

/// Only `cell` atoms are region-scoped. A `db.read[users]` names a table and a
/// region named `users` names memory; treating the two as one label would refuse
/// programs for a collision of spelling.
fn row_brands(row: &Row, region: &str) -> bool {
    row.atoms.iter().any(|a| {
        a.effect.as_str() == CELL
            && matches!(&a.resource, Resource::Named(r) if r.as_str() == region)
    })
}

/// The positions a region body's value leaves through.
///
/// Every one of them is unified with the region's own result type, so the walk
/// decides *where* an escape is reported and never *whether* one happened. A
/// tail-resumptive clause is not an exit: its body has the operation's return
/// type, not the `handle`'s. A `handle` with a `return` clause exits through the
/// clause and not through the body, because a `return x -> 0` discards whatever
/// the body produced.
fn region_exits<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
    const RED_ZONE: usize = 256 * 1024;
    const NEW_SEGMENT: usize = 2 * 1024 * 1024;
    stacker::maybe_grow(RED_ZONE, NEW_SEGMENT, || match &e.kind {
        ExprKind::Block { tail, .. } => {
            if let Some(t) = tail {
                region_exits(t, out);
            }
        }
        ExprKind::If {
            then_branch,
            else_branch,
            ..
        } => {
            region_exits(then_branch, out);
            region_exits(else_branch, out);
        }
        ExprKind::Match { arms, .. } => arms.iter().for_each(|a| region_exits(&a.body, out)),
        ExprKind::WithCell { body, .. }
        | ExprKind::WithRegion { body, .. }
        | ExprKind::Simulate { body } => region_exits(body, out),
        ExprKind::Handle {
            body,
            clauses,
            return_clause,
        } => {
            match return_clause {
                Some(rc) => region_exits(&rc.body, out),
                None => region_exits(body, out),
            }
            for c in clauses.iter().filter(|c| c.resume.is_some()) {
                region_exits(&c.body, out);
            }
        }
        _ => out.push(e),
    })
}

/// Whether a declared type could carry a brand. The written row counts: a `cell`
/// atom is writable in an annotation — a cell that outlives its region through a
/// continuation puts one in a published footprint — so a field declared
/// `() -> Int / {cell.read[r]}` is a closure-shaped hiding place for exactly the
/// same reason a field declared `Cell<Int>` is a cell-shaped one.
fn mentions_cell(t: &Type) -> bool {
    match t {
        Type::Var(_) => false,
        Type::Con(name, args) => name.as_str() == "Cell" || args.iter().any(mentions_cell),
        Type::Fn {
            params,
            ret,
            effects,
        } => {
            effects.atoms.iter().any(|a| a.effect.as_str() == CELL)
                || params.iter().any(mentions_cell)
                || mentions_cell(ret)
        }
        Type::Record(fields) => fields.values().any(mentions_cell),
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

/// What one walk of a body answers: the names it mentions, and whether it is
/// written with an effect operation at all.
///
/// One walk rather than two because the second question is asked of every
/// definition in the program by [`Checker::mark_internal_effects`], and because
/// [`collect_refs_inner`] is an exhaustive `match` with no wildcard arm — so a
/// new [`ExprKind`] fails to compile here rather than silently answering
/// `false` for a form that can perform. That is the whole reason the flag lives
/// in this walker instead of one of its own.
#[derive(Default)]
struct Refs<'a> {
    names: Vec<&'a QName>,
    /// A `perform` or a `handle` written anywhere in this body, lambdas
    /// included. Not a claim that either one runs: a `perform` inside a lambda
    /// nothing calls sets this too, which costs a definition its entry into a
    /// compiled body and never the other way round.
    effects: bool,
}

impl<'a> Refs<'a> {
    fn of(e: &'a Expr) -> Refs<'a> {
        let mut out = Refs::default();
        collect_refs(e, &mut out);
        out
    }

    fn bare(&self) -> impl Iterator<Item = &&'a QName> {
        self.names.iter().filter(|q| q.is_bare())
    }
}

/// Grows for the same reason [`Infer::infer`] does: it walks the same tree, so a
/// chain deep enough to need the growth there needs it here too.
fn collect_refs<'a>(e: &'a Expr, out: &mut Refs<'a>) {
    const RED_ZONE: usize = 256 * 1024;
    const NEW_SEGMENT: usize = 2 * 1024 * 1024;
    stacker::maybe_grow(RED_ZONE, NEW_SEGMENT, || collect_refs_inner(e, out))
}

fn collect_refs_inner<'a>(e: &'a Expr, out: &mut Refs<'a>) {
    match &e.kind {
        ExprKind::Lit(_) => {}
        ExprKind::Var(q) => out.names.push(q),
        ExprKind::Binary { lhs, rhs, .. } => {
            collect_refs(lhs, out);
            collect_refs(rhs, out);
        }
        ExprKind::Unary { operand, .. } => collect_refs(operand, out),
        ExprKind::Lambda { body, .. } => collect_refs(body, out),
        ExprKind::App { func, args, .. } => {
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
        ExprKind::RecordUpdate { base, fields } => {
            collect_refs(base, out);
            fields.iter().for_each(|(_, v)| collect_refs(v, out));
        }
        ExprKind::Field { base, .. } => collect_refs(base, out),
        ExprKind::Try { operand } => collect_refs(operand, out),
        ExprKind::List { items } => items.iter().for_each(|i| collect_refs(i, out)),
        ExprKind::Perform { args, .. } => {
            out.effects = true;
            args.iter().for_each(|a| collect_refs(a, out));
        }
        ExprKind::Handle {
            body,
            clauses,
            return_clause,
        } => {
            out.effects = true;
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
        ExprKind::WithRegion { body, .. } => collect_refs(body, out),
        ExprKind::Simulate { body } => collect_refs(body, out),
    }
}
