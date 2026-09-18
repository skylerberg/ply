//! The program a unit compiles out of: the front end's answer over it, and each module's text.
//!
//! **Every table here is the front end's, not the tree's** (ADR 0052 §1). The constructor table,
//! the roots the unit compiles, their cache keys, their arities and the module list all come from
//! a [`Front`].

use ply_hash::HashOutput;
use ply_hash::body::BodySet;
use ply_span::{SourceId, Symbol};
use ply_syntax::ast::{AtomExpr, Expr, ExprKind, Item, Lit, Program, QName, SpecKind, Stmt, UnOp};
use ply_syntax::resolve::{Namespace, Resolved};
use ply_ty::{
    CheckOutput, DefWritten, EffectAtom, EffectSet, Footprint, Front, Hashed, LawInfo, Literal,
    Ordinal, Resource, Type, TypeDecl, WrittenParam,
};
use std::collections::{HashMap, HashSet};

/// A checked program, borrowed for as long as the unit compiled from it lives.
pub struct Source {
    pub program: &'static Program,
    pub resolved: &'static Resolved,
    /// The front end's whole answer over this program: what every table below is read from.
    pub front: &'static Front,
    /// [`Front::check`], which is where a name's scheme, footprint and spec are read from.
    pub check: &'static CheckOutput,
    tables: Tables,
    /// What each definition's emitted code is a function of, when the caller knows: its hash over
    /// its own text and everything it references, and, once texts are attached, the texts its
    /// spans are offsets into. Empty when nobody supplied any, and then nothing is kept between
    /// runs.
    pub keys: HashMap<String, String>,
    /// Each module's source text, by module name: what the emitter reads the program from, so a
    /// build over a source without them is refused.
    pub texts: HashMap<String, String>,
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

fn layout_digest<'a>(
    modules: impl Iterator<Item = &'a Symbol>,
    texts: &HashMap<String, String>,
) -> String {
    let mut h = blake3::Hasher::new();
    for module in modules {
        let text = texts.get(module.as_str()).map_or("", String::as_str);
        for part in [module.as_str(), text] {
            h.update(&(part.len() as u64).to_le_bytes());
            h.update(part.as_bytes());
        }
    }
    h.finalize().to_hex()[..32].to_string()
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

/// A [`Front`] assembled from the tree around a check the caller already holds, for a source the
/// port's whole answer was not asked for: the tables below then have one shape either way.
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
        ..Front::default()
    };
    if let Some(bodies) = bodies {
        fill_bodies(&mut front, program, bodies);
    }
    fill_written(&mut front, program, resolved);
    front
}

/// The builtin effect `cell`, which is written bare and resolves to itself.
const CELL: &str = "cell";

/// What the syntax tree carries and [`CheckOutput`] does not: each `fn`'s visibility, its `reuse`
/// marker and its parameters as written; every `type`, which no table of the checker's holds; each
/// `effect`'s visibility; every test's label span; the literals each law's guard mentions; and each
/// module's `effect set`s, resolved the way a row that names one is.
///
/// Every assembler of a [`Front`] from the Rust chain calls this, so that a dump written from one
/// carries what the port's dump carries and the differential compares the two.
pub fn fill_written(front: &mut Front, program: &Program, resolved: &Resolved) {
    let mut defs_written = std::mem::take(&mut front.defs_written);
    let mut types = std::mem::take(&mut front.types);
    let mut effects_written = std::mem::take(&mut front.effects_written);
    let mut effect_sets = std::mem::take(&mut front.effect_sets);
    let (mut test_name_spans, mut law_literals) = (Vec::new(), Vec::new());
    {
        let check = &front.check;
        for (index, module) in program.modules.iter().enumerate() {
            let mut sets = Vec::new();
            for item in &module.items {
                match item {
                    Item::Fn(d) => {
                        defs_written.insert(
                            module.name.qualify(&d.name.name),
                            DefWritten {
                                vis: d.vis,
                                reuse: d.reuse.is_some(),
                                params: d
                                    .params
                                    .iter()
                                    .map(|p| WrittenParam {
                                        name: p.name.name.clone(),
                                        span: p.span,
                                    })
                                    .collect(),
                            },
                        );
                    }
                    Item::Type(d) => {
                        let name = module.name.qualify(&d.name.name);
                        types.insert(
                            name.clone(),
                            TypeDecl {
                                name,
                                module: module.name.clone(),
                                simple_name: d.name.name.clone(),
                                vis: d.vis,
                                arity: d.params.len(),
                                span: d.span,
                            },
                        );
                    }
                    Item::Effect(d) => {
                        effects_written.insert(module.name.qualify(&d.name.name), d.vis);
                    }
                    Item::Test(d) => test_name_spans.push(d.name_span),
                    Item::Law(d) => {
                        let mut literals = Vec::new();
                        if let Some(guard) = &d.guard {
                            collect_literals(guard, &mut literals);
                        }
                        law_literals.push(literals);
                    }
                    Item::EffectSet(d) => sets.push(EffectSet {
                        name: d.name.name.clone(),
                        includes: d.includes.iter().map(|q| q.symbol().clone()).collect(),
                        atoms: Footprint::from_atoms(
                            d.expansion
                                .iter()
                                .filter_map(|a| set_atom(a, resolved, check, index)),
                        ),
                    }),
                    Item::Derive(_) => {}
                }
            }
            if !sets.is_empty() {
                effect_sets.insert(module.name.as_symbol().clone(), sets);
            }
        }
    }
    front.defs_written = defs_written;
    front.types = types;
    front.effects_written = effects_written;
    front.effect_sets = effect_sets;
    front.test_name_spans = test_name_spans;
    front.law_literals = law_literals;
}

/// A written atom as the program-wide atom a row would carry, exactly as `ply-cli`'s
/// `signature::atom_of` resolves one: an effect that resolves to nothing drops its atom.
fn set_atom(
    atom: &AtomExpr,
    resolved: &Resolved,
    check: &CheckOutput,
    module: usize,
) -> Option<EffectAtom> {
    let effect = set_effect(&atom.effect, resolved, check, module)?;
    let resource = match &atom.resource {
        Some(r) => Resource::Named(r.name.clone()),
        None => Resource::Singleton,
    };
    Some(EffectAtom::new(effect, resource, atom.mode))
}

fn set_effect(
    q: &QName,
    resolved: &Resolved,
    check: &CheckOutput,
    module: usize,
) -> Option<Symbol> {
    if q.is_bare() && q.symbol().as_str() == CELL {
        return Some(Symbol::new(CELL));
    }
    match resolved.lookup(module, Namespace::Effect, q) {
        Ok(binding) if check.effects.contains_key(&binding.qualified) => {
            Some(binding.qualified.clone())
        }
        _ if q.is_bare() && ply_ty::prelude::is_prelude_effect(q.symbol()) => {
            Some(q.symbol().clone())
        }
        _ => None,
    }
}

/// The literals a guard is written in terms of, in the order `ply-cli`'s witness search walks it:
/// a stack, children pushed in source order and taken last-first, each value kept the first time it
/// is reached. That order seeds a search rather than deciding an answer, which is why it is pinned
/// here rather than sorted.
fn collect_literals(expr: &Expr, out: &mut Vec<Literal>) {
    let mut stack = vec![expr];
    while let Some(e) = stack.pop() {
        match &e.kind {
            ExprKind::Lit(Lit::Int(k)) => keep(out, Literal::Int(*k)),
            ExprKind::Lit(Lit::Str(s)) => keep(out, Literal::Str(s.clone())),
            ExprKind::Lit(Lit::Bytes(b)) => keep(out, Literal::Bytes(b.clone())),
            ExprKind::Lit(_) | ExprKind::Var(_) => {}
            ExprKind::Binary { lhs, rhs, .. } => {
                stack.push(lhs);
                stack.push(rhs);
            }
            ExprKind::Unary { op, operand } => {
                // `-1000000` is a negation of a literal in the tree and a bound in the guard, so
                // the value the search wants is the negated one — and the literal under it is
                // still reached, as the walk goes on to the operand either way.
                if let (UnOp::Neg, ExprKind::Lit(Lit::Int(k))) = (op, &operand.kind) {
                    keep(out, Literal::Int(k.saturating_neg()));
                }
                stack.push(operand);
            }
            ExprKind::App { func, args, .. } => {
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
            ExprKind::Lambda { body, .. } => stack.push(body),
            ExprKind::Match { scrutinee, arms } => {
                stack.push(scrutinee);
                for arm in arms {
                    stack.extend(arm.guard.iter());
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
            ExprKind::Handle { body, .. } => stack.push(body),
            ExprKind::WithCell { init, body, .. } => {
                stack.push(init);
                stack.push(body);
            }
            ExprKind::WithRegion { body, .. } | ExprKind::Simulate { body } => stack.push(body),
        }
    }
}

fn keep(out: &mut Vec<Literal>, literal: Literal) {
    if !out.contains(&literal) {
        out.push(literal);
    }
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
    let mut out: Vec<(Symbol, usize)> = ply_ty::prelude::ctor_arities();
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

    /// A source over a front end's answer the caller computed once: how a `Unit` is built, so the
    /// port's front end runs once for the unit rather than once per table.
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
            tables: Tables::of(front),
            keys,
            texts: HashMap::new(),
        }
    }

    /// The same source, with each module's text.
    pub fn with_texts(mut self, texts: HashMap<String, String>) -> Source {
        // A body's C bakes in byte offsets and module indices, which no definition hash covers.
        let layout = layout_digest(self.module_names(), &texts);
        for key in self.keys.values_mut() {
            key.push('@');
            key.push_str(&layout);
        }
        self.texts = texts;
        self
    }

    /// Every sum-type constructor in the program, by program-wide name, with its arity — the table
    /// `Machine::build` assembles and `lookup` reads.
    pub fn ctors(&self) -> Vec<(Symbol, usize)> {
        self.tables.ctors.clone()
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
