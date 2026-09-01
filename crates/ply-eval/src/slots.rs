//! Every binding in a barrier gets an index, and every variable occurrence resolves to one.
//!
//! ADR 0034 §4's prerequisite. The measurements in §4 leave one shape standing — a frame that holds
//! values by index, so that narrowing what it holds is clearing slots rather than building a scope —
//! and this is the half of that which can be checked before any of it runs. Nothing here changes
//! evaluation: `Env` still answers every lookup by name. What this buys is that when the machine
//! switches to slots, the assignment it switches to has already been wrong-checked against the
//! names, on every module the corpus loads.
//!
//! It is a **forward** pass, unlike lowering, because a slot is decided by what is in scope to the
//! left of an occurrence and lowering walks right to left for liveness. The two meet through the
//! occurrence's span.

use ply_span::Symbol;
use ply_syntax::ast::{Expr, ExprKind, Pattern, PatternKind, Stmt as AstStmt};
use rustc_hash::FxHashMap;

/// One barrier's slots, in the order they are bound.
#[derive(Debug, Default, Clone)]
pub struct Barrier {
    pub names: Vec<Symbol>,
}

/// What the pass answers.
#[derive(Debug, Default)]
pub struct Slots {
    /// A bare variable occurrence, by node identity: the barrier it reads from, and the slot.
    ///
    /// **Keyed by the node's address, not its span.** Expansion — `?` and record update —
    /// synthesizes nodes that reuse the span they came from, so one span can carry two different
    /// occurrences and a span-keyed map answers for whichever it filed last. `?`'s expansion put an
    /// `Err` constructor and a generated `?0` read at one span, which is how that was found. The
    /// address is unique for as long as the tree is alive, which is as long as this map is.
    ///
    /// Absent for a name no binder in the barrier introduces — a definition, a constructor or a
    /// builtin — which is the same set `Live` declines to track.
    pub of_var: FxHashMap<usize, (u32, u32)>,
    /// Every barrier this pass opened, in the order it opened them.
    pub barriers: Vec<Barrier>,
}

struct Scope<'a> {
    /// Innermost last, so a shadowed name resolves to the binder nearest the occurrence.
    live: Vec<(Symbol, u32)>,
    barrier: Barrier,
    /// Which barrier this scope is, as an index into [`Slots::barriers`].
    at: u32,
    out: &'a mut Slots,
}

impl Scope<'_> {
    fn bind(&mut self, name: &Symbol) -> u32 {
        let slot = self.barrier.names.len() as u32;
        self.barrier.names.push(name.clone());
        self.live.push((name.clone(), slot));
        slot
    }

    fn resolve(&self, name: &Symbol) -> Option<u32> {
        self.live
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, s)| *s)
    }
}

/// Resolves one barrier: `params` take the first slots, then the body is walked.
fn barrier(params: &[Symbol], body: &Expr, out: &mut Slots) -> u32 {
    let at = out.barriers.len() as u32;
    out.barriers.push(Barrier::default());
    let mut scope = Scope {
        live: Vec::new(),
        barrier: Barrier::default(),
        at,
        out,
    };
    for p in params {
        scope.bind(p);
    }
    walk(body, &mut scope);
    let table = std::mem::take(&mut scope.barrier);
    out.barriers[at as usize] = table;
    at
}

fn pattern(pat: &Pattern, scope: &mut Scope) {
    crate::limit::grow(|| match &pat.kind {
        PatternKind::Var(name) => {
            scope.bind(&name.name);
        }
        PatternKind::Ctor { args, .. } => {
            for a in args {
                pattern(a, scope);
            }
        }
        PatternKind::Record { fields, .. } => {
            for (_, p) in fields {
                pattern(p, scope);
            }
        }
        _ => {}
    });
}

fn walk(e: &Expr, scope: &mut Scope) {
    crate::limit::grow(|| match &e.kind {
        ExprKind::Lit(_) => {}
        ExprKind::Var(q) => {
            if q.is_bare()
                && let Some(slot) = scope.resolve(q.symbol())
            {
                let at = scope.at;
                scope
                    .out
                    .of_var
                    .insert(std::ptr::from_ref(e) as usize, (at, slot));
            }
        }
        // A lambda is its own barrier: its body's names index its own table, not this one.
        ExprKind::Lambda { params, body } => {
            let params: Vec<Symbol> = params.iter().map(|p| p.name.name.clone()).collect();
            let _ = barrier(&params, body, scope.out);
        }
        ExprKind::Unary { operand, .. } => walk(operand, scope),
        ExprKind::Binary { lhs, rhs, .. } => {
            walk(lhs, scope);
            walk(rhs, scope);
        }
        ExprKind::App { func, args, .. } => {
            walk(func, scope);
            for a in args {
                walk(a, scope);
            }
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            walk(cond, scope);
            walk(then_branch, scope);
            walk(else_branch, scope);
        }
        ExprKind::Match { scrutinee, arms } => {
            walk(scrutinee, scope);
            for arm in arms {
                let depth = scope.live.len();
                pattern(&arm.pat, scope);
                if let Some(g) = &arm.guard {
                    walk(g, scope);
                }
                walk(&arm.body, scope);
                scope.live.truncate(depth);
            }
        }
        ExprKind::Block { stmts, tail } => {
            let depth = scope.live.len();
            for stmt in stmts {
                match stmt {
                    // The value is evaluated before the binder is in scope.
                    AstStmt::Let { pat, value, .. } => {
                        walk(value, scope);
                        pattern(pat, scope);
                    }
                    AstStmt::Expr(e) => walk(e, scope),
                }
            }
            if let Some(t) = tail {
                walk(t, scope);
            }
            scope.live.truncate(depth);
        }
        ExprKind::Record { fields } => {
            for (_, value) in fields {
                walk(value, scope);
            }
        }
        ExprKind::RecordUpdate { base, fields } => {
            walk(base, scope);
            for (_, value) in fields {
                walk(value, scope);
            }
        }
        ExprKind::Field { base, .. } => walk(base, scope),
        ExprKind::Try { operand } => walk(operand, scope),
        ExprKind::List { items } => {
            for item in items {
                walk(item, scope);
            }
        }
        ExprKind::Perform { args, .. } => {
            for a in args {
                walk(a, scope);
            }
        }
        // A clause body and a return clause are barriers of their own, and they run *below* their
        // own handler, so neither sees the handled body's bindings.
        ExprKind::Handle {
            body,
            clauses,
            return_clause,
        } => {
            walk(body, scope);
            for clause in clauses {
                let mut params: Vec<Symbol> =
                    clause.params.iter().map(|p| p.name.clone()).collect();
                params.extend(clause.resume.iter().map(|r| r.name.clone()));
                let _ = barrier(&params, &clause.body, scope.out);
            }
            if let Some(ret) = return_clause {
                let _ = barrier(std::slice::from_ref(&ret.binder.name), &ret.body, scope.out);
            }
        }
        ExprKind::WithCell {
            init, binder, body, ..
        } => {
            walk(init, scope);
            let depth = scope.live.len();
            scope.bind(&binder.name);
            walk(body, scope);
            scope.live.truncate(depth);
        }
        ExprKind::WithRegion { body, .. } => walk(body, scope),
        ExprKind::Simulate { body, .. } => {
            let _ = barrier(&[], body, scope.out);
        }
    });
}

/// Resolves a function body and everything nested in it.
pub fn resolve(params: &[Symbol], body: &Expr) -> Slots {
    let mut out = Slots::default();
    let _ = barrier(params, body, &mut out);
    out
}
