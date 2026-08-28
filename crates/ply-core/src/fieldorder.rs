//! `W0611` — a growing container built somewhere other than the last
//! sub-expression of its enclosing node.
//!
//! ## The rule, and why it needs a pass rather than a paragraph
//!
//! [`ply_eval::rc::carry`] is `if remaining { env.clone() } else {
//! Env::empty() }`, and every one of its eight call sites passes `remaining` =
//! *is there another sub-expression after this one*. When there is, the pending
//! frame holds a second reference to the scope for the whole of that
//! sub-expression's evaluation, `Env::take_unique` refuses at the first shared
//! link, the read clones instead of moving, and `push` takes its copying
//! branch. Run that in a fold and the fold is quadratic.
//!
//! Two things make this undiagnosable by reading one line:
//!
//! - **It is positional, not about the variable.** `spikes/ply-lexer/GAPS.md`
//!   §1 records two withdrawn explanations, both of which said it was about the
//!   last *mention* of the state variable. Binding every other field to a `let`
//!   before the push makes the read the last mention and leaves the program
//!   quadratic: GAPS.md's column 3, 0.31s → 1.11s → 3.68s over one doubling
//!   each.
//! - **It is not local.** ADR 0020 §5.2 measured a correctly-written callee made
//!   quadratic by its caller, where the offending trailing sub-expression is a
//!   literal constant: `carry` never asks what the remaining sub-expression
//!   *reads*. So no property of a definition decides whether its `push` copies;
//!   the composition does. That is why this pass computes an interprocedural
//!   [`grows`] summary before it walks anything.
//!
//! ## What it answers, and what it does not
//!
//! Fires on a `push`, or on a call to a definition that grows a container,
//! sitting anywhere but the last sub-expression of every node between it and
//! its definition's body. Two things it deliberately does not claim:
//!
//! - **`List` and `push` only.** `builtins.rs:460` and `:472` are the only
//!   `rc::note_update` call sites in the tree, so `rc::Stats::updates` counts
//!   `push` and nothing else. A firing on a `Map` would be a claim with no
//!   counter able to check it, which is the failure this pass exists inside a
//!   measurement to avoid.
//! - **It does not know whether the container is short.** A growing call in
//!   non-final position whose list is bounded — `spikes/ply-lexer-nesting`'s
//!   `grouped_ten` is that shape — costs O(bound) per step and is linear. The
//!   pass cannot decide that statically and the diagnostic does not pretend to;
//!   it states the two facts it has and points at `ply run --json`'s
//!   `counters.in_place`, which settles it with a number.
//!
//! ## Handler clauses
//!
//! A `handle` body is analyzed as a non-final position, because
//! `handler::enter_handle` puts the scope in the `Prompt` and the prompt
//! outlives the body. A clause body and a `return` clause body are analyzed as
//! **their own roots**: `handler::leave_handle` evaluates the return arm under
//! `prompt.env.bind(...)`, a scope built for it rather than the enclosing
//! expression's. Nothing in the oracle exercises either, and this comment is the
//! extent of the claim.

use crate::scc::sccs;
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::{Expr, ExprKind, FnDef, Item, ModuleName, Program, QName, Stmt};
use ply_syntax::resolve::{Namespace, Resolved};
use rustc_hash::{FxHashMap, FxHashSet};

/// The builtin whose copying branch this pass exists to predict.
const PUSH: &str = "push";

const RED_ZONE: usize = 256 * 1024;
const NEW_SEGMENT: usize = 2 * 1024 * 1024;

/// One `fn` in the program, keyed the way [`Resolved`] keys one.
struct Def<'a> {
    /// The fully-qualified name, which is what a resolved reference yields.
    qualified: Symbol,
    module: usize,
    def: &'a FnDef,
}

/// Where a sub-expression sits, in the only terms the machine cares about.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    /// Nothing follows it in its enclosing node, so `carry` hands the pending
    /// frame `Env::empty()` and the enclosing position's answer is inherited.
    Last,
    /// Another sub-expression follows, so the frame keeps the scope.
    Carried,
    /// A body entered with a scope built for it: a lambda, a handler clause.
    /// Its own analysis starts over.
    Barrier,
}

/// Every `W0611` in the program, in source order per module.
///
/// `program` must be complete — every module parsed. A module whose AST is
/// absent contributes no definitions, and a callee this pass cannot see would
/// answer "does not grow", which is a silent miss rather than a loud one. The
/// caller is responsible for completing the parse first; `ply check
/// --field-order` does.
pub fn check(program: &Program, resolved: &Resolved) -> Vec<Diagnostic> {
    check_where(program, resolved, |_| true)
}

/// [`check`], reporting only in the modules `report` accepts.
///
/// The **summary** is still computed over the whole program, and that is not an
/// optimization detail: the trap is a property of a composition, so a caller in
/// a project and a callee in the standard library are one question. Narrowing
/// the call graph to the reported modules would answer "does not grow" for
/// every library function and silently lose every firing that crosses the
/// boundary. Only the reporting is scoped.
pub fn check_where(
    program: &Program,
    resolved: &Resolved,
    report: impl Fn(&ModuleName) -> bool,
) -> Vec<Diagnostic> {
    let defs = index(program, resolved);
    let by_key: FxHashMap<Symbol, usize> = defs
        .iter()
        .enumerate()
        .map(|(i, d)| (d.qualified.clone(), i))
        .collect();
    let grows = summarize(&defs, resolved, &by_key);

    let mut wanted = vec![true; program.modules.len()];
    for module in &program.modules {
        if let Some(i) = resolved.index_of(&module.name) {
            wanted[i] = report(&module.name);
        }
    }

    let mut out = Vec::new();
    for def in &defs {
        if !wanted.get(def.module).copied().unwrap_or(true) {
            continue;
        }
        let mut walk = Walk {
            resolved,
            by_key: &by_key,
            grows: &grows,
            module: def.module,
            locals: def.def.params.iter().map(|p| p.name.name.clone()).collect(),
            out: &mut out,
        };
        walk.expr(&def.def.body, true);
    }
    out
}

fn index<'a>(program: &'a Program, resolved: &Resolved) -> Vec<Def<'a>> {
    let mut defs = Vec::new();
    for module in &program.modules {
        let Some(index) = resolved.index_of(&module.name) else {
            continue;
        };
        for item in &module.items {
            // `Derive` and `EffectSet` declare nothing a reference can reach;
            // a `derive` has already been expanded into ordinary `Fn` items.
            let Item::Fn(def) = item else { continue };
            let Some(binding) = resolved
                .declared
                .get(index)
                .and_then(|b| b.get(Namespace::Value, &def.name.name))
            else {
                continue;
            };
            defs.push(Def {
                qualified: binding.qualified.clone(),
                module: index,
                def,
            });
        }
    }
    defs
}

/// "A call to this definition can answer a container a `push` inside it
/// produced."
///
/// Deliberately an **over**-approximation: any `push` anywhere in the body
/// counts, whether or not its result reaches the return value. The direction is
/// the whole point. An under-approximation would answer "does not grow" for
/// something that does, and a caller composing it in non-final position would
/// get silence over exactly the space nobody looked at — which
/// `CONTRIBUTING.md` records as this project's most expensive defect class. An
/// over-approximation costs a false positive, which the flag, the note and the
/// counter between them make cheap.
fn summarize(
    defs: &[Def<'_>],
    resolved: &Resolved,
    by_key: &FxHashMap<Symbol, usize>,
) -> Vec<bool> {
    let mut grows: Vec<bool> = Vec::with_capacity(defs.len());
    let mut adj: Vec<Vec<usize>> = Vec::with_capacity(defs.len());
    for def in defs {
        let mut found = Found {
            resolved,
            by_key,
            module: def.module,
            locals: def.def.params.iter().map(|p| p.name.name.clone()).collect(),
            pushes: false,
            calls: Vec::new(),
        };
        found.expr(&def.def.body);
        grows.push(found.pushes);
        adj.push(found.calls);
    }

    // Reverse topological order, so a component is reached after everything it
    // calls. A recursive or mutually recursive group is one component and is
    // iterated to a fixpoint inside itself.
    for component in sccs(defs.len(), &adj) {
        loop {
            let mut moved = false;
            for &i in &component {
                if grows[i] {
                    continue;
                }
                if adj[i].iter().any(|&j| grows[j]) {
                    grows[i] = true;
                    moved = true;
                }
            }
            if !moved {
                break;
            }
        }
    }
    grows
}

/// What a definition's body calls, and whether it pushes.
struct Found<'a> {
    resolved: &'a Resolved,
    by_key: &'a FxHashMap<Symbol, usize>,
    module: usize,
    locals: FxHashSet<Symbol>,
    pushes: bool,
    calls: Vec<usize>,
}

impl Found<'_> {
    fn expr(&mut self, e: &Expr) {
        stacker::maybe_grow(RED_ZONE, NEW_SEGMENT, || {
            if let ExprKind::App { func, .. } = &e.kind
                && let ExprKind::Var(q) = &func.kind
            {
                match classify(self.resolved, self.by_key, &self.locals, self.module, q) {
                    Callee::Push => self.pushes = true,
                    Callee::Def(i) => self.calls.push(i),
                    Callee::Opaque => {}
                }
            }
            let mut locals = self.locals.clone();
            for (child, _) in children(e, &mut locals) {
                self.locals = locals.clone();
                self.expr(child);
            }
            self.locals = locals;
        });
    }
}

/// The positional walk.
struct Walk<'a> {
    resolved: &'a Resolved,
    by_key: &'a FxHashMap<Symbol, usize>,
    grows: &'a [bool],
    module: usize,
    locals: FxHashSet<Symbol>,
    out: &'a mut Vec<Diagnostic>,
}

impl Walk<'_> {
    /// `reached_last` is cumulative: it is true only when **every** node
    /// between here and the definition's body put this sub-expression last.
    /// One carried frame anywhere up the chain is enough, which is why the flag
    /// is threaded rather than recomputed — `f(g(0, push(xs, i)), 1)` copies
    /// even though the `push` is last inside `g`.
    fn expr(&mut self, e: &Expr, reached_last: bool) {
        stacker::maybe_grow(RED_ZONE, NEW_SEGMENT, || {
            if !reached_last {
                self.report(e);
            }
            let mut locals = self.locals.clone();
            for (child, slot) in children(e, &mut locals) {
                let child_last = match slot {
                    Slot::Last => reached_last,
                    Slot::Carried => false,
                    Slot::Barrier => true,
                };
                let outer = std::mem::replace(&mut self.locals, locals.clone());
                self.expr(child, child_last);
                self.locals = outer;
            }
            self.locals = locals;
        });
    }

    fn report(&mut self, e: &Expr) {
        let ExprKind::App { func, args } = &e.kind else {
            return;
        };
        let ExprKind::Var(q) = &func.kind else {
            return;
        };
        match classify(self.resolved, self.by_key, &self.locals, self.module, q) {
            Callee::Push => {
                // A list this expression just built has exactly one owner, so
                // `Arc::get_mut` succeeds however many frames hold the scope:
                // the carried scope holds bindings, and a fresh list is in
                // none of them. Firing here would be a false positive with a
                // mechanism behind it.
                if args.first().is_some_and(fresh) {
                    return;
                }
                self.out.push(
                    Diagnostic::warning(
                        codes::FIELD_ORDER_COPY,
                        "this `push` copies the list instead of growing it",
                    )
                    .primary(e.span, "not the last sub-expression of its enclosing node")
                    .note(
                        "a pending frame holds the scope while anything after this one is \
                         evaluated, so the list is at two owners and `push` copies it; in a \
                         loop that is quadratic",
                    )
                    .note(
                        "move it into the last position of its enclosing node — last record \
                         field, last call argument, last list element",
                    )
                    .note(
                        "`ply run --json` reports `counters.in_place`: it is near 0 when this \
                         is costing what it looks like it costs, and near 1 when the list is \
                         short enough not to matter",
                    ),
                );
            }
            Callee::Def(i) if self.grows[i] => {
                self.out.push(
                    Diagnostic::warning(
                        codes::FIELD_ORDER_COPY,
                        format!("`{q}` grows a container here, so this call copies it"),
                    )
                    .primary(e.span, "not the last sub-expression of its enclosing node")
                    .note(
                        "the callee is written correctly; the position of this call is what \
                         decides it. `rc::carry` never asks what the remaining sub-expressions \
                         read, so even a constant after this call is enough",
                    )
                    .note(
                        "move the call into the last position of its enclosing node, or make \
                         what follows it precede it",
                    )
                    .note(
                        "`ply run --json` reports `counters.in_place`: this is a cost only if \
                         the container it grows is long",
                    ),
                );
            }
            Callee::Def(_) | Callee::Opaque => {}
        }
    }
}

/// What a call's callee is.
enum Callee {
    /// The `push` builtin, not shadowed.
    Push,
    /// A definition of this program, by index.
    Def(usize),
    /// A local, another builtin, or a name that does not resolve. Nothing is
    /// claimed about it.
    Opaque,
}

fn classify(
    resolved: &Resolved,
    by_key: &FxHashMap<Symbol, usize>,
    locals: &FxHashSet<Symbol>,
    module: usize,
    q: &QName,
) -> Callee {
    // A local wins unconditionally, and `Resolved::lookup`'s contract is that
    // it is only asked about names local lookup already missed.
    if q.is_bare() && locals.contains(q.symbol()) {
        return Callee::Opaque;
    }
    if let Ok(binding) = resolved.lookup(module, Namespace::Value, q)
        && let Some(&i) = by_key.get(&binding.qualified)
    {
        return Callee::Def(i);
    }
    // Only now: a module that declares its own `push` shadows the prelude, so
    // the resolved answer above is consulted first.
    if q.is_bare() && q.symbol().as_str() == PUSH {
        return Callee::Push;
    }
    Callee::Opaque
}

/// A container this expression just built, which nothing else can reach.
///
/// `push(push([], a), b)` is the builder idiom and both of its pushes are in
/// place whatever frame holds the scope, so neither is a firing.
fn fresh(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::List { .. } => true,
        ExprKind::App { func, args } => {
            matches!(&func.kind, ExprKind::Var(q) if q.is_bare() && q.symbol().as_str() == PUSH)
                && args.first().is_some_and(fresh)
        }
        _ => false,
    }
}

/// Every sub-expression of `e`, with the slot the machine evaluates it in, and
/// the binders `e` adds to the scope its children see.
///
/// **Exhaustive, with no `_` arm on purpose.** A node kind added later must not
/// default to "last": that would be silence over a shape nobody considered,
/// which is the defect class this pass exists to close. It must fail to compile
/// instead.
///
/// Every row is read off the evaluator rather than off the language reference:
///
/// | node | carried, and where the evaluator says so |
/// | --- | --- |
/// | `Binary` | `lhs` — `machine.rs:1013` pushes `Frame::BinaryRhs` holding `env.clone()`; `rhs` runs under `BinaryApply`/`ShortCircuit`, which hold none |
/// | `App` | `func` when there are arguments (`machine.rs:1035`, `carry(&env, !args.is_empty())`) and every argument but the last (`frame.rs:107`, `:142`) |
/// | `If` | `cond` — `machine.rs:1050` pushes `Frame::If` holding `env.clone()`; the branches inherit |
/// | `Match` | `scrutinee` (`machine.rs:1067`) and each guard (`machine.rs:2206`, `Frame::MatchGuard`); arm bodies inherit |
/// | `Block` | every statement — `machine.rs:2161` pushes `Frame::BlockStep` holding `scope.release(dead)`, which is why GAPS.md §1 column 4 measures a `let` failing to rescue it; the tail inherits |
/// | `Record` | every field but the last (`machine.rs:1092`, `frame.rs:263`) |
/// | `List` | every item but the last (`machine.rs:1122`, `frame.rs:301`) |
/// | `Perform` | every argument but the last (`handler.rs:208`) |
/// | `Handle` | the body — `handler.rs:152` puts `env.clone()` in the `Prompt`, which outlives it |
/// | `WithCell` | `init` — `handler.rs:388` pushes `Frame::WithCellBody` holding `env.clone()`; the body inherits |
/// | `Unary`, `Field`, `WithRegion`, `Simulate` | nothing: no frame on those paths holds a scope |
fn children<'a>(e: &'a Expr, locals: &mut FxHashSet<Symbol>) -> Vec<(&'a Expr, Slot)> {
    match &e.kind {
        ExprKind::Lit(_) | ExprKind::Var(_) => Vec::new(),

        ExprKind::Binary { lhs, rhs, .. } => vec![(&**lhs, Slot::Carried), (&**rhs, Slot::Last)],

        ExprKind::Unary { operand, .. } => vec![(&**operand, Slot::Last)],

        ExprKind::Lambda { params, body } => {
            locals.extend(params.iter().map(|p| p.name.name.clone()));
            vec![(&**body, Slot::Barrier)]
        }

        ExprKind::App { func, args } => {
            let mut out = vec![(
                &**func,
                if args.is_empty() {
                    Slot::Last
                } else {
                    Slot::Carried
                },
            )];
            out.extend(positional(args));
            out
        }

        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => vec![
            (&**cond, Slot::Carried),
            (&**then_branch, Slot::Last),
            (&**else_branch, Slot::Last),
        ],

        ExprKind::Match { scrutinee, arms } => {
            let mut out = vec![(&**scrutinee, Slot::Carried)];
            for arm in arms {
                // A pattern's binders are in scope for the guard and the body.
                // They are added to the shared set rather than per arm: a name
                // wrongly believed local only ever yields `Callee::Opaque`,
                // which under-reports on that one call, and arms cannot see
                // each other's binders in any program that type-checks.
                pattern_binders(&arm.pat, locals);
                if let Some(guard) = &arm.guard {
                    out.push((guard, Slot::Carried));
                }
                out.push((&arm.body, Slot::Last));
            }
            out
        }

        ExprKind::Block { stmts, tail } => {
            let mut out = Vec::with_capacity(stmts.len() + 1);
            for stmt in stmts {
                match stmt {
                    Stmt::Let { pat, value, .. } => {
                        out.push((&**value, Slot::Carried));
                        pattern_binders(pat, locals);
                    }
                    Stmt::Expr(value) => out.push((value, Slot::Carried)),
                }
            }
            if let Some(tail) = tail {
                out.push((&**tail, Slot::Last));
            }
            out
        }

        ExprKind::Record { fields } => positional(fields.iter().map(|(_, e)| e)),

        ExprKind::Field { base, .. } => vec![(&**base, Slot::Last)],

        ExprKind::List { items } => positional(items),

        ExprKind::Perform { args, .. } => positional(args),

        ExprKind::Handle {
            body,
            clauses,
            return_clause,
        } => {
            let mut out = vec![(&**body, Slot::Carried)];
            for clause in clauses {
                locals.extend(clause.params.iter().map(|p| p.name.clone()));
                if let Some(resume) = &clause.resume {
                    locals.insert(resume.name.clone());
                }
                out.push((&clause.body, Slot::Barrier));
            }
            if let Some(ret) = return_clause {
                locals.insert(ret.binder.name.clone());
                out.push((&ret.body, Slot::Barrier));
            }
            out
        }

        ExprKind::WithCell {
            init, binder, body, ..
        } => {
            let init = (&**init, Slot::Carried);
            locals.insert(binder.name.clone());
            vec![init, (&**body, Slot::Last)]
        }

        ExprKind::WithRegion { body, .. } => vec![(&**body, Slot::Last)],

        ExprKind::Simulate { body } => vec![(&**body, Slot::Last)],
    }
}

/// A left-to-right sequence in which only the last sub-expression is evaluated
/// with no frame holding the scope.
fn positional<'a, I>(items: I) -> Vec<(&'a Expr, Slot)>
where
    I: IntoIterator<Item = &'a Expr>,
    I::IntoIter: ExactSizeIterator,
{
    let items = items.into_iter();
    let last = items.len().saturating_sub(1);
    items
        .enumerate()
        .map(|(i, e)| (e, if i == last { Slot::Last } else { Slot::Carried }))
        .collect()
}

fn pattern_binders(pat: &ply_syntax::ast::Pattern, out: &mut FxHashSet<Symbol>) {
    use ply_syntax::ast::PatternKind;
    match &pat.kind {
        PatternKind::Wildcard | PatternKind::Lit(_) => {}
        PatternKind::Var(id) => {
            out.insert(id.name.clone());
        }
        PatternKind::Ctor { args, .. } => {
            for arg in args {
                pattern_binders(arg, out);
            }
        }
        PatternKind::Record { fields, .. } => {
            for (_, p) in fields {
                pattern_binders(p, out);
            }
        }
        PatternKind::List { items, rest } => {
            for p in items {
                pattern_binders(p, out);
            }
            if let Some(rest) = rest {
                pattern_binders(rest, out);
            }
        }
    }
}

/// One firing, with the definition it sits in.
///
/// The definition is what the rule is *about* — the trap is a property of a
/// composition, and "which function do I have to rewrite" is the question a
/// firing answers. `crates/ply-eval/tests/field_order_oracle.rs` compares this
/// set against `rc::Stats` per definition, which is only possible because the
/// name comes out with the span.
#[derive(Clone, Debug)]
pub struct Firing {
    /// The fully-qualified name of the definition the site is in.
    pub definition: Symbol,
    /// The simple name, as written.
    pub simple: Symbol,
    pub span: Span,
}

/// Every `W0611` site, with the definition it is in.
pub fn firings(program: &Program, resolved: &Resolved) -> Vec<Firing> {
    let defs = index(program, resolved);
    let by_key: FxHashMap<Symbol, usize> = defs
        .iter()
        .enumerate()
        .map(|(i, d)| (d.qualified.clone(), i))
        .collect();
    let grows = summarize(&defs, resolved, &by_key);

    let mut out = Vec::new();
    for def in &defs {
        let mut found = Vec::new();
        let mut walk = Walk {
            resolved,
            by_key: &by_key,
            grows: &grows,
            module: def.module,
            locals: def.def.params.iter().map(|p| p.name.name.clone()).collect(),
            out: &mut found,
        };
        walk.expr(&def.def.body, true);
        out.extend(found.iter().filter_map(|d| {
            Some(Firing {
                definition: def.qualified.clone(),
                simple: def.def.name.name.clone(),
                span: d.primary_span()?,
            })
        }));
    }
    out
}
