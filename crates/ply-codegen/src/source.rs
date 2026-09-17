//! The program a unit compiles out of: the front end's answer over it, and the tree the reference
//! emitter still reads bodies from.
//!
//! **Every table here is the front end's, not the tree's** (ADR 0052 §1). The constructor table,
//! the roots the unit compiles, their cache keys, their arities and the module list all come from
//! a [`Front`] — the port's when it can serve one, and the Rust chain's otherwise — so that the
//! day the tree goes, what is left to move is the reference emitter and nothing around it. The
//! tree is read for one thing: the body `definition` hands the reference emitter.

use anyhow::Result;
use ply_hash::HashOutput;
use ply_hash::body::BodySet;
use ply_span::{SourceId, Span, Symbol};
use ply_syntax::ast::{
    Expr, FnDef, Generics, Ident, Item, Param, Program, QName, SpecKind, TestDef, TypeExpr,
    Visibility,
};
use ply_syntax::resolve::Resolved;
use ply_ty::{CheckOutput, Front, Hashed, LawInfo, Ordinal, Type};
use std::collections::{HashMap, HashSet};

/// A checked program, borrowed for as long as the unit compiled from it lives.
pub struct Source {
    pub program: &'static Program,
    pub resolved: &'static Resolved,
    /// The front end's whole answer over this program: what every table below is read from.
    pub front: &'static Front,
    /// [`Front::check`], which is where a name's scheme, footprint and spec are read from.
    pub check: &'static CheckOutput,
    /// Every definition by program-wide name, with the index of its module: the code generator
    /// asks for one at each name it resolves.
    ///
    /// The tests', laws' and clauses' entries are synthesized here because the reference emitter
    /// reads a body from the tree; their *names* are the front end's, as [`Tables::roots`] lists
    /// them, so the root set is one answer rather than two walks that have to agree.
    definitions: HashMap<String, (&'static FnDef, usize)>,
    tables: Tables,
    /// What each definition's emitted code is a function of, when the caller knows: its hash over
    /// its own text and everything it references. Empty when nobody supplied any, and then nothing
    /// is kept between runs.
    pub keys: HashMap<String, String>,
    /// Each module's source text, by module name, when the caller has it: what a second emitter
    /// is handed, since it reads the program from its text. Empty otherwise.
    pub texts: HashMap<String, String>,
    regions: std::sync::OnceLock<ply_eval::region_kind::Regions>,
    stack_handled: std::sync::OnceLock<StackHandled>,
}

/// What some handler on the stack could answer, anywhere in this program.
///
/// The question a `perform` in a compiled body has to settle is whether it can reach a handler
/// rather than the host. A handler on the stack resumes, and resuming means capturing the frame
/// the `perform` is in, which a compiled frame cannot give; the host *returns*, which is a call.
/// So the compilable `perform` is the one that provably finds no stack handler.
///
/// It is a whole-program property and that is what makes it sound against an interpreted caller: a
/// compiled body can be called from inside an interpreted `handle`, but if the program declares no
/// handler for the operation, no frame above it can be one.
#[derive(Default)]
pub struct StackHandled {
    /// `effect.op` of every `handle` clause written anywhere, resource ignored -- a clause with a
    /// resource is counted for every resource, which refuses more than it must and never less.
    ops: std::collections::HashSet<String>,
    /// The brand of every `with_cell`, whose operations a cell answers.
    resources: std::collections::HashSet<Symbol>,
}

impl StackHandled {
    /// Whether a `perform` of this operation could find a handler rather than the host.
    ///
    /// `task.*` is answered by a `simulate` opening a region rather than by a handler at all, so it
    /// counts as reachable however the program is written.
    pub fn could_reach(&self, effect: &str, op: &str, resource: Option<&Symbol>) -> bool {
        if effect == "task" {
            return true;
        }
        if self.ops.contains(&format!("{effect}.{op}")) {
            return true;
        }
        resource.is_some_and(|r| self.resources.contains(r))
    }
}

/// The name a test's root takes: its place among its module's tests, which `ply_eval`'s test
/// runner computes the same way when it offers the test.
pub fn test_root_name(ordinal: usize) -> Symbol {
    Symbol::new(format!("test#{ordinal}"))
}

/// A law's guard or body as a root: `law#<ordinal>.guard`, `law#<ordinal>.body`, the ordinal its
/// place among the module's laws and the binders its parameters (ADR 0045 §"The facade").
pub fn law_root_name(ordinal: usize, part: &str) -> Symbol {
    Symbol::new(format!("law#{ordinal}.{part}"))
}

/// A definition's `requires` or `ensures` clause as a root: `<owner>#requires#<k>` over the
/// owner's parameters, `<owner>#ensures#<k>` over them and then `result`.
pub fn clause_root_name(owner: &Symbol, kind: &str, ordinal: usize) -> Symbol {
    Symbol::new(format!("{owner}#{kind}#{ordinal}"))
}

/// The cache key each keyable root — a definition, its spec clauses, a test, a law's guard and
/// body — is kept under, keyed by its hash. Under tier-only these keys are also what make a test
/// or a law a ROOT the unit compiles (ADR 0045 §"The facade"); without them a `Unit` would not
/// hold `test#N` and the tier could not enter it.
///
/// Read from [`Front::ordinals`] — every module in program order, its keyable items in source
/// order — which is the walk the hasher numbers tests and laws by, so a `test#N` here is the
/// `N`th hash of `HashOutput::tests`.
pub fn emit_keys(front: &Front) -> HashMap<String, String> {
    let laws: HashMap<&Symbol, &LawInfo> = front.check.laws.iter().map(|l| (&l.key, l)).collect();
    let mut keys = HashMap::new();
    let (mut test_at, mut law_at) = (0, 0);
    for (module, items) in &front.ordinals {
        let (mut ordinal, mut law_ordinal) = (0, 0);
        for item in items {
            match item {
                Ordinal::Fn(name, kinds) => {
                    let Some(h) = front.hashes.defs.get(name) else {
                        continue;
                    };
                    // **A clause is keyed by its own hash, not by its owner's.** A definition's
                    // hash is over its normalized body, which a spec is erased from, so editing
                    // `ensures result == x + 1` into `ensures result == x + 3` leaves the owner's
                    // hash where it was -- and the clause root compiled from the old sentence was
                    // served back, so the prover judged the edited spec by the proposition it
                    // replaced. `HashOutput::specs` runs parallel to the clauses this ordinal
                    // names and covers the clause's own bytes.
                    let clauses = front.hashes.specs.get(name);
                    let (mut requires, mut ensures) = (0, 0);
                    for (i, kind) in kinds.iter().enumerate() {
                        let (kind, k) = match kind {
                            SpecKind::Requires => {
                                requires += 1;
                                ("requires", requires - 1)
                            }
                            SpecKind::Ensures => {
                                ensures += 1;
                                ("ensures", ensures - 1)
                            }
                        };
                        let own = clauses.and_then(|cs| cs.get(i)).unwrap_or(h);
                        keys.insert(
                            clause_root(name, kind, k),
                            format!("{}#{kind}#{k}", own.to_hex()),
                        );
                    }
                    keys.insert(name.to_string(), h.to_hex());
                }
                Ordinal::Law(key) => {
                    if let Some(h) = front.hashes.laws.get(law_at) {
                        for part in ["guard", "body"] {
                            if part == "guard" && !laws.get(key).is_some_and(|l| l.has_guard) {
                                continue;
                            }
                            keys.insert(
                                qualified(module, &law_root_name(law_ordinal, part)),
                                format!("{}#{part}", h.to_hex()),
                            );
                        }
                    }
                    law_ordinal += 1;
                    law_at += 1;
                }
                Ordinal::Test(_) => {
                    if let Some(h) = front.hashes.tests.get(test_at) {
                        keys.insert(qualified(module, &test_root_name(ordinal)), h.to_hex());
                    }
                    ordinal += 1;
                    test_at += 1;
                }
            }
        }
    }
    keys
}

/// A program-wide name in `module`, as `ModuleName::qualify` spells it.
fn qualified(module: &Symbol, name: &Symbol) -> String {
    if module.as_str().is_empty() {
        return name.to_string();
    }
    format!("{module}.{name}")
}

/// One clause of `owner`, whose name is already program-wide.
fn clause_root(owner: &Symbol, kind: &str, ordinal: usize) -> String {
    format!("{owner}#{kind}#{ordinal}")
}

/// Whether `name` is one of the roots above rather than a written definition or a test.
pub fn is_spec_root(name: &str) -> bool {
    let local = name.rsplit('.').next().unwrap_or(name);
    name.contains(".law#") || local.contains("#requires#") || local.contains("#ensures#")
}

/// The front end's answer over `program`: the Rust chain's while it still runs, and the port's
/// where it is asked for.
///
/// The port *is* a front end — it reads text — so asking it is a second front end over the
/// program and the standard library, for every unit. That is the suite's time rather than a
/// rounding error: asking it everywhere took a quiet run of `main` from 144 s to 254 s. So the
/// chain answers while it exists, `PLY_FRONT=port` asks the port instead, and one CI gate runs
/// with it set so that path stays exercised end to end. The differentials hold the two answers
/// to each other byte for byte, which is what lets a unit be built from either, and the port
/// becomes the only answer when the chain goes (ADR 0052 §1).
pub fn front_for(
    program: &Program,
    resolved: &Resolved,
    check: &CheckOutput,
    texts: &HashMap<String, String>,
) -> Result<Front> {
    let sources: Option<Vec<(String, String)>> = program
        .modules
        .iter()
        .map(|m| {
            let name = m.name.to_string();
            texts.get(&name).map(|text| (name, text.clone()))
        })
        .collect();
    if std::env::var("PLY_FRONT").as_deref() == Ok("port")
        && let Some(sources) = sources
        && !sources.is_empty()
        && crate::c::producer::mode() != "ref"
    {
        let ids: Vec<SourceId> = program.modules.iter().map(|m| m.source).collect();
        return crate::c::producer::front(&sources, &ids);
    }
    // A program that does not hash is one nothing is kept between runs for: the roots and the
    // tables are still this program's and every cache key is simply absent, which is what the
    // caller that supplied no hashes at all gets too.
    let (hashes, bodies) = match ply_hash::hash_program_with_bodies(program, resolved) {
        Ok((hashes, bodies)) => (hashes, Some(bodies)),
        Err(_) => (HashOutput::default(), None),
    };
    Ok(front_of(program, resolved, check, hashes, bodies.as_ref()))
}

/// The Rust chain's answer as a [`Front`], from the pieces the driver already holds.
///
/// `ply_ty::front` is the protocol and `crates/ply-compiler/ply/front.ply` writes the same text;
/// this assembles the reference's side of it, so that the tables below have one shape whichever
/// front end answered.
pub fn front_of(
    program: &Program,
    resolved: &Resolved,
    check: &CheckOutput,
    hashes: HashOutput,
    bodies: Option<&BodySet>,
) -> Front {
    let order = resolved
        .order
        .iter()
        .map(|&i| program.modules[i].name.as_symbol().clone())
        .collect();
    let ordinals = program
        .modules
        .iter()
        .map(|module| {
            let items = module
                .items
                .iter()
                .filter_map(|item| match item {
                    Item::Fn(d) => Some(Ordinal::Fn(
                        module.name.qualify(&d.name.name),
                        d.spec.iter().map(|c| c.kind).collect(),
                    )),
                    Item::Test(d) => {
                        Some(Ordinal::Test(module.name.qualify(&Symbol::new(&d.name))))
                    }
                    Item::Law(d) => Some(Ordinal::Law(module.name.qualify(&Symbol::new(&d.name)))),
                    Item::Type(_) | Item::Effect(_) | Item::Derive(_) | Item::EffectSet(_) => None,
                })
                .collect();
            (module.name.as_symbol().clone(), items)
        })
        .collect();
    let mut front = Front {
        diagnostics: Vec::new(),
        order,
        check: check.clone(),
        hashes,
        hash_order: Vec::new(),
        ordinals,
        bodies: Vec::new(),
        test_bodies: Vec::new(),
    };
    if let Some(bodies) = bodies {
        fill_bodies(&mut front, program, bodies);
    }
    front
}

/// The hasher's item order and the body it stored for each name: every module in program order,
/// its items in source order, a name declared in two namespaces once in the order and twice here.
fn fill_bodies(front: &mut Front, program: &Program, bodies: &BodySet) {
    let (mut order, mut stored) = (Vec::new(), Vec::new());
    let mut named: HashSet<Symbol> = HashSet::new();
    let (mut tests, mut laws) = (0, 0);
    for module in &program.modules {
        for item in &module.items {
            let (name, hash) = match item {
                Item::Fn(d) => {
                    let name = module.name.qualify(&d.name.name);
                    let hash = front.hashes.defs.get(&name).copied();
                    (name, hash)
                }
                Item::Type(d) => {
                    let name = module.name.qualify(&d.name.name);
                    let hash = front.hashes.decls.get(&name).copied();
                    (name, hash)
                }
                Item::Effect(d) => {
                    let name = module.name.qualify(&d.name.name);
                    let hash = front.hashes.decls.get(&name).copied();
                    (name, hash)
                }
                Item::Test(_) => {
                    order.push(Hashed::Test(tests));
                    tests += 1;
                    continue;
                }
                Item::Law(_) => {
                    order.push(Hashed::Law(laws));
                    laws += 1;
                    continue;
                }
                Item::Derive(_) | Item::EffectSet(_) => continue,
            };
            if named.insert(name.clone()) {
                order.push(Hashed::Def(name.clone()));
            }
            if let Some(body) = hash.and_then(|h| bodies.get(h)) {
                stored.push((name, body.as_bytes().to_vec()));
            }
        }
    }
    front.hash_order = order;
    front.bodies = stored;
    front.test_bodies = bodies
        .tests()
        .iter()
        .map(|b| b.as_bytes().to_vec())
        .collect();
}

/// Everything a unit is emitted against, read once from the front end's answer.
struct Tables {
    /// Every sum-type constructor with its arity — the table `Machine::build` assembles, and the
    /// one the emitted C names its tags by position in.
    ctors: Vec<(Symbol, usize)>,
    /// Every root the fixpoint is offered, in the order it is offered them.
    roots: Vec<String>,
    /// How many arguments each root takes; absent for a name that is no root of this program.
    arities: HashMap<String, usize>,
    /// The roots whose parameters and answer are all `Int` or `Bool`: what
    /// `PLY_CODEGEN_REGISTER=narrow` registers.
    scalars: HashSet<String>,
    /// Every module in program order, with the source its spans point into.
    modules: Vec<(Symbol, SourceId)>,
}

impl Tables {
    fn of(front: &Front) -> Tables {
        let laws: HashMap<&Symbol, &LawInfo> =
            front.check.laws.iter().map(|l| (&l.key, l)).collect();
        let mut t = Tables {
            ctors: ctors_of(front),
            roots: Vec::new(),
            arities: HashMap::new(),
            scalars: HashSet::new(),
            modules: Vec::new(),
        };
        let (mut tests, mut specs) = (Vec::new(), Vec::new());
        for (module, items) in &front.ordinals {
            if let Some(info) = front.check.modules.get(module) {
                t.modules.push((module.clone(), info.source));
            }
            let (mut ordinal, mut law_ordinal) = (0, 0);
            for item in items {
                match item {
                    Ordinal::Fn(name, kinds) => {
                        let root = name.to_string();
                        t.roots.push(root.clone());
                        let Some(def) = front.check.defs.get(name) else {
                            continue;
                        };
                        let (params, ret) = signature(&def.scheme.ty);
                        let scalar_params = params.iter().all(is_scalar);
                        t.note(root.clone(), params.len(), scalar_params && is_scalar(ret));
                        let (mut requires, mut ensures) = (0, 0);
                        for kind in kinds {
                            // A proposition answers `Bool` over the owner's parameters, and an
                            // `ensures` over those and then `result`.
                            let (kind, k, arity, scalar) = match kind {
                                SpecKind::Requires => {
                                    requires += 1;
                                    ("requires", requires - 1, params.len(), scalar_params)
                                }
                                SpecKind::Ensures => {
                                    ensures += 1;
                                    (
                                        "ensures",
                                        ensures - 1,
                                        params.len() + 1,
                                        scalar_params && is_scalar(ret),
                                    )
                                }
                            };
                            let clause = clause_root(name, kind, k);
                            t.note(clause.clone(), arity, scalar);
                            specs.push(clause);
                        }
                    }
                    Ordinal::Test(_) => {
                        // A test is a root the machine enters whole: a nullary definition of its
                        // body, named by its place among its module's tests, which is the name the
                        // test runner offers. It answers whatever the body does, so it is never
                        // registered under the narrow registry.
                        let root = qualified(module, &test_root_name(ordinal));
                        t.note(root.clone(), 0, false);
                        tests.push(root);
                        ordinal += 1;
                    }
                    Ordinal::Law(key) => {
                        let law = laws.get(key);
                        let binders = law.map(|l| l.binders.as_slice()).unwrap_or_default();
                        for part in ["guard", "body"] {
                            if part == "guard" && !law.is_some_and(|l| l.has_guard) {
                                continue;
                            }
                            let root = qualified(module, &law_root_name(law_ordinal, part));
                            let scalar = binders.iter().all(|b| is_scalar(&b.ty));
                            t.note(root.clone(), binders.len(), scalar);
                            specs.push(root);
                        }
                        law_ordinal += 1;
                    }
                }
            }
        }
        t.roots.extend(tests);
        t.roots.extend(specs);
        t
    }

    fn note(&mut self, root: String, arity: usize, scalar: bool) {
        if scalar {
            self.scalars.insert(root.clone());
        }
        self.arities.insert(root, arity);
    }
}

/// The prelude's constructors, then each module's in declaration order, the modules in program
/// order.
///
/// **Not `CheckOutput::ctors`'s own order.** The checker collects a module's types in *dependency*
/// order, and the emitted C names its tags by position in this table, so a unit whose tags moved
/// with the import graph would be a different unit for the same program. Within one module that
/// order is source order, which is what this keeps.
fn ctors_of(front: &Front) -> Vec<(Symbol, usize)> {
    let mut out: Vec<(Symbol, usize)> = ply_core::prelude::ctor_arities();
    let prelude: HashSet<Symbol> = out.iter().map(|(name, _)| name.clone()).collect();
    for (module, _) in &front.ordinals {
        out.extend(
            front
                .check
                .ctors
                .iter()
                .filter(|(name, c)| c.module.as_symbol() == module && !prelude.contains(*name))
                .map(|(name, c)| (name.clone(), c.arity)),
        );
    }
    out
}

/// A definition's parameters and its answer, as the checker published them.
fn signature(ty: &Type) -> (&[Type], &Type) {
    match ty {
        Type::Fn { params, ret, .. } => (params, ret),
        other => (&[], other),
    }
}

fn is_scalar(ty: &Type) -> bool {
    matches!(ty, Type::Con(name, args) if args.is_empty() && matches!(name.as_str(), "Int" | "Bool"))
}

fn bool_type(span: Span) -> TypeExpr {
    TypeExpr::Con {
        name: QName::bare(Ident {
            name: Symbol::new("Bool"),
            span,
        }),
        args: Vec::new(),
        span,
    }
}

/// A proposition over bound names as a definition answering `Bool`, so the unit compiles it
/// like any other and the judge enters it with the names' values as arguments.
fn proposition_as_definition(name: &Symbol, params: Vec<Param>, body: &Expr, span: Span) -> FnDef {
    FnDef {
        vis: Visibility::Private,
        name: Ident {
            name: name.clone(),
            span,
        },
        generics: Generics {
            types: Vec::new(),
            effects: Vec::new(),
        },
        params,
        ret: Some(bool_type(span)),
        effects: None,
        constraints: Vec::new(),
        derived: None,
        spec: Vec::new(),
        reuse: None,
        body: body.clone(),
        span,
    }
}

/// The owner's parameters as a proposition's, when every one has a written type; a clause over
/// an unannotated parameter has no root, and the judge evaluates it as before.
fn typed_params(def: &FnDef) -> Option<Vec<Param>> {
    def.params
        .iter()
        .map(|p| {
            p.ty.as_ref().map(|ty| Param {
                name: p.name.clone(),
                ty: Some(ty.clone()),
                default: None,
                span: p.span,
            })
        })
        .collect()
}

/// A test's body as a nullary definition, so the fragment compiles it like any other.
fn test_as_definition(test: &TestDef, name: &Symbol) -> FnDef {
    FnDef {
        vis: Visibility::Private,
        name: Ident {
            name: name.clone(),
            span: test.name_span,
        },
        generics: Generics {
            types: Vec::new(),
            effects: Vec::new(),
        },
        params: Vec::new(),
        ret: None,
        effects: None,
        constraints: Vec::new(),
        derived: None,
        spec: Vec::new(),
        reuse: None,
        body: test.body.clone(),
        span: test.span,
    }
}

impl Source {
    /// A source over a program the Rust chain has already answered for, with no hashes: nothing is
    /// kept between runs for it.
    pub fn new(
        program: &'static Program,
        resolved: &'static Resolved,
        check: &'static CheckOutput,
    ) -> Source {
        let front = front_of(program, resolved, check, HashOutput::default(), None);
        Source::from_front(
            program,
            resolved,
            Box::leak(Box::new(front)),
            HashMap::new(),
        )
    }

    /// The same, told what each definition's code is a function of.
    pub fn keyed(
        program: &'static Program,
        resolved: &'static Resolved,
        check: &'static CheckOutput,
        keys: HashMap<String, String>,
    ) -> Source {
        let front = front_of(program, resolved, check, HashOutput::default(), None);
        Source::from_front(program, resolved, Box::leak(Box::new(front)), keys)
    }

    /// A source over a front end's answer the caller computed once, through [`front_for`]: how a
    /// `Unit` is built, so the port's front end runs once for the unit rather than once per table.
    pub fn from_front(
        program: &'static Program,
        resolved: &'static Resolved,
        front: &'static Front,
        keys: HashMap<String, String>,
    ) -> Source {
        Source {
            program,
            resolved,
            front,
            check: &front.check,
            definitions: synthesized(program),
            tables: Tables::of(front),
            keys,
            texts: HashMap::new(),
            regions: std::sync::OnceLock::new(),
            stack_handled: std::sync::OnceLock::new(),
        }
    }
}

/// Every definition of the program by program-wide name, with the index of the module its bare
/// names resolve in — and the tests', laws' and clauses' propositions synthesized as definitions,
/// because the reference emitter reads a *body* from the tree.
///
/// The names are the front end's: [`Tables::roots`] lists exactly these, walking the same items in
/// the same order, so the root set is one answer rather than two walks that have to agree.
fn synthesized(program: &'static Program) -> HashMap<String, (&'static FnDef, usize)> {
    let mut definitions = HashMap::new();
    for (index, module) in program.modules.iter().enumerate() {
        let mut ordinal = 0;
        let mut law_ordinal = 0;
        for item in &module.items {
            match item {
                Item::Fn(def) => {
                    definitions
                        .entry(module.name.qualify(&def.name.name).to_string())
                        .or_insert((&**def, index));
                }
                // A test is a root the machine enters whole: a nullary definition of its
                // body, named by its place among the module's tests, which is the name the
                // test runner offers.
                Item::Test(test) => {
                    let name = test_root_name(ordinal);
                    let def: &'static FnDef = Box::leak(Box::new(test_as_definition(test, &name)));
                    definitions.insert(module.name.qualify(&name).to_string(), (def, index));
                    ordinal += 1;
                }
                Item::Law(law) => {
                    let params: Vec<Param> = law
                        .binders
                        .iter()
                        .map(|b| Param {
                            name: b.name.clone(),
                            ty: Some(b.ty.clone()),
                            default: None,
                            span: b.span,
                        })
                        .collect();
                    let parts = [("guard", law.guard.as_ref()), ("body", Some(&law.body))];
                    for (part, expr) in parts {
                        let Some(expr) = expr else { continue };
                        let name = law_root_name(law_ordinal, part);
                        let def: &'static FnDef = Box::leak(Box::new(proposition_as_definition(
                            &name,
                            params.clone(),
                            expr,
                            law.span,
                        )));
                        definitions.insert(module.name.qualify(&name).to_string(), (def, index));
                    }
                    law_ordinal += 1;
                }
                _ => {}
            }
            if let Item::Fn(def) = item
                && let Some(params) = typed_params(def)
            {
                let mut counts = (0usize, 0usize);
                for clause in &def.spec {
                    let (kind, ordinal, params) = match clause.kind {
                        SpecKind::Requires => {
                            counts.0 += 1;
                            ("requires", counts.0 - 1, params.clone())
                        }
                        SpecKind::Ensures => {
                            counts.1 += 1;
                            let Some(ret) = &def.ret else { continue };
                            let mut with_result = params.clone();
                            with_result.push(Param {
                                name: Ident {
                                    name: Symbol::new("result"),
                                    span: def.span,
                                },
                                ty: Some(ret.clone()),
                                default: None,
                                span: def.span,
                            });
                            ("ensures", counts.1 - 1, with_result)
                        }
                    };
                    let name = clause_root_name(&def.name.name, kind, ordinal);
                    let root: &'static FnDef = Box::leak(Box::new(proposition_as_definition(
                        &name,
                        params,
                        &clause.expr,
                        clause.span,
                    )));
                    definitions.insert(module.name.qualify(&name).to_string(), (root, index));
                }
            }
        }
    }
    definitions
}

impl Source {
    /// The same source, with each module's text.
    pub fn with_texts(mut self, texts: HashMap<String, String>) -> Source {
        self.texts = texts;
        self
    }

    /// The definition a program-wide name denotes, and the index of the module its bare names
    /// resolve in — the pair the machine keys everything on.
    pub fn definition(&self, name: &str) -> Option<(&'static FnDef, usize)> {
        self.definitions.get(name).copied()
    }

    /// Every sum-type constructor in the program, by program-wide name, with its arity — the table
    /// `Machine::build` assembles and `lookup` reads.
    pub fn ctors(&self) -> Vec<(Symbol, usize)> {
        self.tables.ctors.clone()
    }

    /// The regions this program opens, inferred once and kept. A `with cell` site asks whether it
    /// opens one, which decides whether a tier with no frame to close it on can carry the site.
    pub fn regions(&self) -> &ply_eval::region_kind::Regions {
        self.regions
            .get_or_init(|| ply_eval::region_kind::infer(self.program, self.resolved))
    }

    /// [`StackHandled`] for this program, walked once and kept.
    pub fn stack_handled(&self) -> &StackHandled {
        self.stack_handled.get_or_init(|| {
            let mut out = StackHandled::default();
            for name in self.functions() {
                let Some((def, _)) = self.definition(&name) else {
                    continue;
                };
                let params: Vec<Symbol> = def.params.iter().map(|p| p.name.name.clone()).collect();
                // The *unoptimised* body: inlining can only bring more handlers into a body, never
                // fewer, and this has to be an answer about the program rather than about one
                // emitter's settings.
                walk_handlers(&ply_eval::code::lower_fn(&params, &def.body).code, &mut out);
            }
            out
        })
    }

    /// Every root this unit compiles: every `fn` by program-wide name in source order, then the
    /// tests' roots, then the laws' and clauses' propositions.
    pub fn functions(&self) -> Vec<String> {
        self.tables.roots.clone()
    }

    /// How many arguments a root takes, and `None` for a name that is no root of this program.
    pub fn arity_of(&self, name: &str) -> Option<usize> {
        self.tables.arities.get(name).copied()
    }

    /// Whether every parameter and the answer are `Int` or `Bool`, which is the only part of a
    /// program `PLY_CODEGEN_REGISTER=narrow` offers the machine.
    pub fn scalar_signature(&self, name: &str) -> bool {
        self.tables.scalars.contains(name)
    }

    /// Each module's source, in program order: what a body's stored span is an index into.
    pub fn module_sources(&self) -> Vec<SourceId> {
        self.tables.modules.iter().map(|(_, s)| *s).collect()
    }

    pub fn module_count(&self) -> usize {
        self.tables.modules.len()
    }

    /// Every module's name, in program order.
    pub fn module_names(&self) -> impl Iterator<Item = &Symbol> {
        self.tables.modules.iter().map(|(name, _)| name)
    }
}

/// Every `handle` clause and `with_cell` brand in one lowered body, lambdas and clause bodies
/// included.
fn walk_handlers(code: &ply_eval::code::Code, out: &mut StackHandled) {
    use ply_eval::code::NodeKind as N;
    match &code.kind {
        N::Handle { body, clauses, ret } => {
            for c in clauses.iter() {
                out.ops
                    .insert(format!("{}.{}", c.effect.symbol().as_str(), c.op.as_str()));
            }
            walk_handlers(body, out);
            for c in clauses.iter() {
                walk_handlers(&c.body, out);
            }
            if let Some(r) = ret {
                walk_handlers(&r.body, out);
            }
        }
        N::WithCell {
            resource,
            init,
            body,
            ..
        } => {
            out.resources.insert(resource.clone());
            walk_handlers(init, out);
            walk_handlers(body, out);
        }
        N::Lit(..) | N::Var { .. } => {}
        N::Unary { operand, .. } => walk_handlers(operand, out),
        N::Binary { lhs, rhs, .. } => {
            walk_handlers(lhs, out);
            walk_handlers(rhs, out);
        }
        N::If {
            cond,
            then_branch,
            else_branch,
        } => {
            walk_handlers(cond, out);
            walk_handlers(then_branch, out);
            walk_handlers(else_branch, out);
        }
        N::Block { stmts, tail } => {
            for s in stmts.iter() {
                match s {
                    ply_eval::code::Stmt::Let { value, .. } => walk_handlers(value, out),
                    ply_eval::code::Stmt::Expr { code } => walk_handlers(code, out),
                }
            }
            if let Some(t) = tail {
                walk_handlers(t, out);
            }
        }
        N::Field { base, .. } => walk_handlers(base, out),
        N::Record { fields } => fields.iter().for_each(|(_, e)| walk_handlers(e, out)),
        N::RecordUpdate { base, sets, .. } => {
            walk_handlers(base, out);
            sets.iter().for_each(|(_, e)| walk_handlers(e, out));
        }
        N::List { items } => items.iter().for_each(|i| walk_handlers(i, out)),
        N::App { func, args } => {
            walk_handlers(func, out);
            args.iter().for_each(|a| walk_handlers(a, out));
        }
        N::Match { scrutinee, arms } => {
            walk_handlers(scrutinee, out);
            for a in arms.iter() {
                if let Some(g) = &a.guard {
                    walk_handlers(g, out);
                }
                walk_handlers(&a.body, out);
            }
        }
        N::Lambda { body, .. } | N::Simulate { body, .. } | N::WithRegion { body } => {
            walk_handlers(body, out)
        }
        N::Perform { args, .. } => args.iter().for_each(|a| walk_handlers(a, out)),
    }
}
