//! Every binding in every barrier gets a slot, every variable occurrence resolves to one, and
//! every barrier knows which enclosing slots its free variables copy from.
//!
//! ADR 0034's prerequisite, and since the slot rewrite the pass the machine actually runs on:
//! `code::lower_fn` consults this table when it builds slot-addressed nodes, and
//! `slot_resolution.rs` wrong-checks the assignment against the names on every module the
//! repository ships.
//!
//! It is a **forward** pass, unlike the ownership half of lowering, because a slot is decided by
//! what is in scope to the left of an occurrence and ownership walks right to left for liveness.
//! The two meet through node identity.

use ply_span::Symbol;
use ply_syntax::ast::{Expr, ExprKind, Pattern, PatternKind, Stmt as AstStmt};
use rustc_hash::FxHashMap;

/// One barrier's slots: its parameters first, then its binders and discovered free variables in
/// the order the walk met them.
#[derive(Debug, Default, Clone)]
pub struct Barrier {
    pub names: Vec<Symbol>,
    /// How many leading `names` are the barrier's parameters.
    pub params: u32,
    /// The free variables this barrier copies in when it is entered: `(from, to)`, where `from`
    /// is a slot of the **parent** barrier and `to` is a slot of this one.
    pub captures: Vec<(u32, u32)>,
    /// The barrier this one's free variables resolve into.
    pub parent: Option<u32>,
}

impl Barrier {
    /// The window size an activation of this barrier needs.
    pub fn size(&self) -> u32 {
        self.names.len() as u32
    }
}

/// What the pass answers, keyed by node identity.
///
/// **Keyed by the node's address, not its span.** Expansion — `?` and record update —
/// synthesizes nodes that reuse the span they came from, so one span can carry two different
/// occurrences and a span-keyed map answers for whichever it filed last. The address is unique
/// for as long as the tree is alive, which is as long as this map is.
#[derive(Debug, Default)]
pub struct Slots {
    /// A bare variable occurrence: the barrier it reads from, and the slot.
    ///
    /// Absent for a name no binder in scope introduces — a definition, a constructor or a
    /// builtin.
    pub of_var: FxHashMap<usize, (u32, u32)>,
    /// A `Pattern::Var` binder, or a `with_cell` binder's `Ident`: the slot it writes, within
    /// the barrier it is bound in.
    pub of_binder: FxHashMap<usize, u32>,
    /// A barrier's **body** expression: the barrier opened for it. The root body is barrier 0.
    pub of_barrier: FxHashMap<usize, u32>,
    /// Every barrier this pass opened, in the order it opened them.
    pub barriers: Vec<Barrier>,
}

impl Slots {
    pub fn var(&self, e: &Expr) -> Option<(u32, u32)> {
        self.of_var.get(&addr_expr(e)).copied()
    }

    pub fn binder_of_pattern(&self, p: &Pattern) -> Option<u32> {
        self.of_binder.get(&addr_pat(p)).copied()
    }

    pub fn binder_of_ident(&self, id: &ply_syntax::ast::Ident) -> Option<u32> {
        self.of_binder.get(&addr_ident(id)).copied()
    }

    pub fn barrier_of(&self, body: &Expr) -> Option<u32> {
        self.of_barrier.get(&addr_expr(body)).copied()
    }
}

fn addr_expr(e: &Expr) -> usize {
    std::ptr::from_ref(e) as usize
}

fn addr_pat(p: &Pattern) -> usize {
    std::ptr::from_ref(p) as usize
}

fn addr_ident(id: &ply_syntax::ast::Ident) -> usize {
    std::ptr::from_ref(id) as usize
}

/// One open barrier during the walk.
struct Frame {
    /// Innermost last, so a shadowed name resolves to the binder nearest the occurrence.
    live: Vec<(Symbol, u32)>,
    /// The capture slots this barrier has already threaded, one per free name. Kept apart from
    /// `live` because a capture is barrier-wide: block scoping truncates `live` and must not
    /// forget a capture, and a binder of the same name must still shadow it.
    caps: Vec<(Symbol, u32)>,
    /// Which barrier this frame is, as an index into [`Slots::barriers`].
    at: u32,
}

impl Frame {
    /// A local binder if one is in scope, else this barrier's capture of the name.
    fn find(&self, name: &Symbol) -> Option<u32> {
        self.live
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .or_else(|| self.caps.iter().find(|(n, _)| n == name))
            .map(|(_, s)| *s)
    }
}

struct Walker<'a> {
    stack: Vec<Frame>,
    out: &'a mut Slots,
}

impl Walker<'_> {
    fn top(&mut self) -> &mut Frame {
        self.stack.last_mut().expect("a barrier is open")
    }

    /// Binds `name` in the innermost barrier, appending a slot for it.
    fn bind(&mut self, name: &Symbol) -> u32 {
        let frame = self.stack.last_mut().expect("a barrier is open");
        let at = frame.at as usize;
        let barrier = &mut self.out.barriers[at];
        let slot = barrier.names.len() as u32;
        barrier.names.push(name.clone());
        frame.live.push((name.clone(), slot));
        slot
    }

    /// Resolves `name` at the innermost barrier, threading a capture chain through every barrier
    /// between the occurrence and the binding when the name is free.
    fn resolve(&mut self, name: &Symbol) -> Option<(u32, u32)> {
        // The depth in `self.stack` whose scope has the name, innermost frame checked first.
        let mut found: Option<(usize, u32)> = None;
        for (depth, frame) in self.stack.iter().enumerate().rev() {
            if let Some(slot) = frame.find(name) {
                found = Some((depth, slot));
                break;
            }
        }
        let (depth, mut slot) = found?;
        // Free in every barrier inside `depth`: give each a capture slot, chaining outside in.
        for d in depth + 1..self.stack.len() {
            let at = self.stack[d].at as usize;
            let existing = self.stack[d]
                .caps
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, s)| *s);
            let to = match existing {
                Some(s) => s,
                None => {
                    let barrier = &mut self.out.barriers[at];
                    let s = barrier.names.len() as u32;
                    barrier.names.push(name.clone());
                    barrier.captures.push((slot, s));
                    self.stack[d].caps.push((name.clone(), s));
                    s
                }
            };
            slot = to;
        }
        let at = self.stack.last().expect("a barrier is open").at;
        Some((at, slot))
    }

    /// Opens a barrier over `params` and walks `body` inside it.
    fn barrier(&mut self, params: &[Symbol], body: &Expr) -> u32 {
        let at = self.out.barriers.len() as u32;
        let parent = self.stack.last().map(|f| f.at);
        self.out.barriers.push(Barrier {
            names: Vec::new(),
            params: params.len() as u32,
            captures: Vec::new(),
            parent,
        });
        self.stack.push(Frame {
            live: Vec::new(),
            caps: Vec::new(),
            at,
        });
        for p in params {
            self.bind(p);
        }
        self.out.of_barrier.insert(addr_expr(body), at);
        self.walk(body);
        self.stack.pop();
        at
    }

    fn pattern(&mut self, pat: &Pattern) {
        crate::limit::grow(|| match &pat.kind {
            PatternKind::Var(_) => {
                let slot = self.bind(&binder_name(pat).expect("a var pattern names its binder"));
                self.out.of_binder.insert(addr_pat(pat), slot);
            }
            PatternKind::Ctor { args, .. } => {
                for a in args {
                    self.pattern(a);
                }
            }
            PatternKind::Record { fields, .. } => {
                for (_, p) in fields {
                    self.pattern(p);
                }
            }
            PatternKind::List { items, rest } => {
                for item in items {
                    self.pattern(item);
                }
                if let Some(rest) = rest {
                    self.pattern(rest);
                }
            }
            PatternKind::Wildcard | PatternKind::Lit(_) => {}
        });
    }

    fn walk(&mut self, e: &Expr) {
        crate::limit::grow(|| match &e.kind {
            ExprKind::Lit(_) => {}
            ExprKind::Var(q) => {
                if q.is_bare()
                    && let Some(found) = self.resolve(q.symbol())
                {
                    self.out.of_var.insert(addr_expr(e), found);
                }
            }
            ExprKind::Lambda { params, body, .. } => {
                let params: Vec<Symbol> = params.iter().map(|p| p.name.name.clone()).collect();
                let _ = self.barrier(&params, body);
            }
            ExprKind::Unary { operand, .. } => self.walk(operand),
            ExprKind::Binary { lhs, rhs, .. } => {
                self.walk(lhs);
                self.walk(rhs);
            }
            ExprKind::App { func, args, .. } => {
                self.walk(func);
                for a in args {
                    self.walk(a);
                }
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.walk(cond);
                self.walk(then_branch);
                self.walk(else_branch);
            }
            ExprKind::Match { scrutinee, arms } => {
                self.walk(scrutinee);
                for arm in arms {
                    let depth = self.top().live.len();
                    self.pattern(&arm.pat);
                    if let Some(g) = &arm.guard {
                        self.walk(g);
                    }
                    self.walk(&arm.body);
                    self.top().live.truncate(depth);
                }
            }
            ExprKind::Block { stmts, tail } => {
                let depth = self.top().live.len();
                for stmt in stmts {
                    match stmt {
                        // The value is evaluated before the binder is in scope.
                        AstStmt::Let { pat, value, .. } => {
                            self.walk(value);
                            self.pattern(pat);
                        }
                        AstStmt::Expr(e) => self.walk(e),
                    }
                }
                if let Some(t) = tail {
                    self.walk(t);
                }
                self.top().live.truncate(depth);
            }
            ExprKind::Record { fields } => {
                for (_, value) in fields {
                    self.walk(value);
                }
            }
            ExprKind::RecordUpdate { base, fields } => {
                self.walk(base);
                for (_, value) in fields {
                    self.walk(value);
                }
            }
            ExprKind::Field { base, .. } => self.walk(base),
            ExprKind::Try { operand } => self.walk(operand),
            ExprKind::List { items } => {
                for item in items {
                    self.walk(item);
                }
            }
            ExprKind::Perform { args, .. } => {
                for a in args {
                    self.walk(a);
                }
            }
            // A clause body and a return clause are barriers of their own, and they run *below*
            // their own handler, so neither sees the handled body's bindings.
            ExprKind::Handle {
                body,
                clauses,
                return_clause,
            } => {
                self.walk(body);
                for clause in clauses {
                    let mut params: Vec<Symbol> =
                        clause.params.iter().map(|p| p.name.clone()).collect();
                    params.extend(clause.resume.iter().map(|r| r.name.clone()));
                    let _ = self.barrier(&params, &clause.body);
                }
                if let Some(ret) = return_clause {
                    let _ = self.barrier(std::slice::from_ref(&ret.binder.name), &ret.body);
                }
            }
            ExprKind::WithCell {
                init, binder, body, ..
            } => {
                self.walk(init);
                let depth = self.top().live.len();
                let slot = self.bind(&binder.name);
                self.out.of_binder.insert(addr_ident(binder), slot);
                self.walk(body);
                self.top().live.truncate(depth);
            }
            ExprKind::WithRegion { body, .. } => self.walk(body),
            // A barrier: a region's tasks interleave, so the body's control is its own — what it
            // reads from the enclosing scope it copies in at the region's entry.
            ExprKind::Simulate { body, .. } => {
                let _ = self.barrier(&[], body);
            }
        });
    }
}

fn binder_name(p: &Pattern) -> Option<Symbol> {
    match &p.kind {
        PatternKind::Var(id) => Some(id.name.clone()),
        _ => None,
    }
}

/// Resolves a function body and everything nested in it.
pub fn resolve(params: &[Symbol], body: &Expr) -> Slots {
    let mut out = Slots::default();
    let mut walker = Walker {
        stack: Vec::new(),
        out: &mut out,
    };
    let _ = walker.barrier(params, body);
    out
}
