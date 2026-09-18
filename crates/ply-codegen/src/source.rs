//! The program a unit compiles out of: the front end's answer over it, and each module's text.
//! Every table here is read from a [`Front`], not the syntax tree.

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
    pub front: &'static Front,
    /// [`Front::check`].
    pub check: &'static CheckOutput,
    tables: Tables,
    /// Each root's cache key: its hash, plus the texts' layout once attached. Empty: no caching.
    pub keys: HashMap<String, String>,
    /// Each module's source text, by module name; the emitter requires them.
    pub texts: HashMap<String, String>,
}

/// A test's root name, by its place among its module's tests; must match `ply_eval`'s runner.
pub fn test_root_name(ordinal: usize) -> Symbol {
    Symbol::new(format!("test#{ordinal}"))
}

/// A law's guard or body as a root, by its place among the module's laws; binders are its params.
pub fn law_root_name(ordinal: usize, part: &str) -> Symbol {
    Symbol::new(format!("law#{ordinal}.{part}"))
}

/// A definition's `requires` or `ensures` clause as a root: `<owner>#requires#<k>` over the
/// owner's parameters, `<owner>#ensures#<k>` over them and then `result`.
pub fn clause_root_name(owner: &Symbol, kind: &str, ordinal: usize) -> Symbol {
    Symbol::new(format!("{owner}#{kind}#{ordinal}"))
}

/// The cache key of each root (definition, spec clause, test, law part), from its hash. Walks
/// [`Front::ordinals`] in the hasher's order, so `test#N` gets the `N`th test hash.
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
                    // A clause is keyed by its own hash: specs are erased from the owner's hash.
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

/// A [`Front`] assembled from the syntax tree around a check the caller already holds.
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

/// Fill what the syntax tree carries and [`CheckOutput`] does not: written signatures, types,
/// effect visibility, test label spans, law guard literals and effect sets.
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

/// A written atom resolved as `ply-cli`'s `signature::atom_of` does; an unresolved effect drops it.
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

/// A guard's literals, deduplicated, in the stack order `ply-cli`'s witness search walks it.
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
                // A negated literal is a bound in its own right; the operand is still walked.
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

/// The hasher's item order and stored bodies; a name in two namespaces is ordered once.
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
    /// Every constructor with its arity; the emitted C names tags by position here.
    ctors: Vec<(Symbol, usize)>,
    roots: Vec<String>,
    arities: HashMap<String, usize>,
    /// Roots whose parameters and answer are all `Int` or `Bool`.
    scalars: HashSet<String>,
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
                            // `ensures` also takes `result`.
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
                        // A test is a nullary root, never scalar since it answers anything.
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

/// The prelude's constructors, then each module's in program order. Not `CheckOutput::ctors`'
/// dependency order, or emitted tags would move with the import graph.
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
    /// A source over an already-checked program, with no keys, so nothing is cached.
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

    pub fn keyed(
        program: &'static Program,
        resolved: &'static Resolved,
        check: &'static CheckOutput,
        keys: HashMap<String, String>,
    ) -> Source {
        let front = front_of(program, resolved, check, HashOutput::default(), None);
        Source::from_front(program, resolved, Box::leak(Box::new(front)), keys)
    }

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

    /// Every constructor, by program-wide name, with its arity.
    pub fn ctors(&self) -> Vec<(Symbol, usize)> {
        self.tables.ctors.clone()
    }

    /// Every root: each `fn` in source order, then tests, then laws and clauses.
    pub fn functions(&self) -> Vec<String> {
        self.tables.roots.clone()
    }

    pub fn arity_of(&self, name: &str) -> Option<usize> {
        self.tables.arities.get(name).copied()
    }

    /// Whether every parameter and the answer are `Int` or `Bool`.
    pub fn scalar_signature(&self, name: &str) -> bool {
        self.tables.scalars.contains(name)
    }

    /// Each module's source, in program order: what a stored span's module index refers to.
    pub fn module_sources(&self) -> Vec<SourceId> {
        self.tables.modules.iter().map(|(_, s)| *s).collect()
    }

    pub fn module_count(&self) -> usize {
        self.tables.modules.len()
    }

    pub fn module_names(&self) -> impl Iterator<Item = &Symbol> {
        self.tables.modules.iter().map(|(name, _)| name)
    }
}
