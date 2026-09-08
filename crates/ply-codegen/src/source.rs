//! The program a unit compiles out of, in the three pieces the machine already holds.

use ply_core::CheckOutput;
use ply_span::Symbol;
use ply_syntax::ast::{FnDef, Generics, Ident, Item, Program, TestDef, Visibility};
use ply_syntax::resolve::Resolved;
use std::collections::HashMap;

/// A checked program, borrowed for as long as the unit compiled from it lives.
pub struct Source {
    pub program: &'static Program,
    pub resolved: &'static Resolved,
    pub check: &'static CheckOutput,
    /// Every definition by program-wide name, with the index of its module: the code generator
    /// asks for one at each name it resolves.
    definitions: HashMap<String, (&'static FnDef, usize)>,
    /// The tests, as the program-wide names of the roots synthesized for them.
    test_roots: Vec<String>,
    /// What each definition's emitted code is a function of, when the caller knows: its hash over
    /// its own text and everything it references. Empty when nobody supplied any, and then nothing
    /// is kept between runs.
    pub keys: HashMap<String, String>,
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
    /// A source over a program that is already `'static`.
    pub fn new(
        program: &'static Program,
        resolved: &'static Resolved,
        check: &'static CheckOutput,
    ) -> Source {
        let mut definitions = HashMap::new();
        let mut roots = Vec::new();
        for (index, module) in program.modules.iter().enumerate() {
            let mut ordinal = 0;
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
                        let def: &'static FnDef =
                            Box::leak(Box::new(test_as_definition(test, &name)));
                        let qualified = module.name.qualify(&name).to_string();
                        definitions.insert(qualified.clone(), (def, index));
                        roots.push(qualified);
                        ordinal += 1;
                    }
                    _ => {}
                }
            }
        }
        Source {
            program,
            resolved,
            check,
            definitions,
            test_roots: roots,
            keys: HashMap::new(),
            regions: std::sync::OnceLock::new(),
            stack_handled: std::sync::OnceLock::new(),
        }
    }

    /// The same, told what each definition's code is a function of.
    pub fn keyed(
        program: &'static Program,
        resolved: &'static Resolved,
        check: &'static CheckOutput,
        keys: HashMap<String, String>,
    ) -> Source {
        Source {
            keys,
            regions: std::sync::OnceLock::new(),
            stack_handled: std::sync::OnceLock::new(),
            ..Source::new(program, resolved, check)
        }
    }

    /// The definition a program-wide name denotes, and the index of the module its bare names
    /// resolve in — the pair the machine keys everything on.
    pub fn definition(&self, name: &str) -> Option<(&'static FnDef, usize)> {
        self.definitions.get(name).copied()
    }

    /// Every sum-type constructor in the program, by program-wide name, with its arity — the table
    /// `Machine::build` assembles and `lookup` reads.
    pub fn ctors(&self) -> Vec<(Symbol, usize)> {
        let mut out: Vec<(Symbol, usize)> = ply_core::prelude::ctor_arities();
        for module in &self.program.modules {
            for item in &module.items {
                if let Item::Type(t) = item
                    && let ply_syntax::ast::TypeDefBody::Sum(variants) = &t.body
                {
                    for v in variants {
                        out.push((module.name.qualify(&v.name.name), v.fields.len()));
                    }
                }
            }
        }
        out
    }

    /// Every function in the program, by program-wide name, in source order.
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

    pub fn functions(&self) -> Vec<String> {
        let mut out = Vec::new();
        for module in &self.program.modules {
            for item in &module.items {
                if let Item::Fn(def) = item {
                    out.push(module.name.qualify(&def.name.name).to_string());
                }
            }
        }
        out.extend(self.test_roots.iter().cloned());
        out
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
