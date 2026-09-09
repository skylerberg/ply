//! The interpreted front end (ADR 0047): one evaluator walks the lowered `code` the compiled
//! front end lowers to C, over the shared runtime, with no C compiler in the path.
//!
//! It reuses the leaf semantics the compiled tier shares — `strict_binary`, `apply_unary`,
//! `lit_matches`, `ctor_value`, and `builtins::{call, advance}` — so a body it evaluates answers
//! what the compiled code answers by construction. It carries the first-order language,
//! `with_cell`, and tail-resumptive `handle`/`perform`, recording each performed atom so its
//! footprint matches the tier's. Where it reaches a construct it does not carry — a clause that
//! binds `resume`, a `simulate`, a region's tasks — it **declines**, and the caller (`Machine`
//! and the `combined` audit) runs it on the compiled front end instead, whose continuations live
//! on the tier's stacks (ADR 0044).
//!
//! The eval walk lives on [`Core`], which owns its per-run state as plain fields and borrows the
//! program tables. Both the `interp` [`Provider`] (over a `RefCell<Core>`, so its `Compiled`
//! methods stay `&self`) and [`crate::Machine`] (which embeds a `Core` and threads it by `&mut`,
//! so `cells()` can hand out `&Arena`) evaluate through the one `Core`.

use crate::backend::Counters;
use crate::code::{
    self, Arm, Captures, Clause, Code, Lowered, Lowering, NodeKind, Pat, ReturnArm, Stmt,
};
use crate::compiled::Entered;
use crate::semantics::{ctor_value, lit_matches, strict_binary};
use crate::value::{Closure, ClosureKind, Fields, Value};
use crate::{Arena, Builtin, TaskRegions};
use ply_core::CheckOutput;
use ply_core::ty::EffectAtom;
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::{Expr, Item, Program, QName};
use ply_syntax::resolve::{Namespace, Resolved};
use rustc_hash::FxHashMap;
use std::collections::BTreeSet;
type GlobalKey = (usize, Option<Symbol>, Symbol);

use std::rc::Rc;
use std::sync::Arc;

/// A body the interpreter can enter: its parameters and where its bare names resolve.
struct Def<'p> {
    params: Vec<Symbol>,
    body: &'p Expr,
    module: usize,
}

/// The interpreter's program tables: the definitions, tests, constructors and effect operations
/// of one program, with the name resolution behind them. Built either borrowing the program
/// (`borrow`, for an engine that lives as long as the borrow) or leaked to `'static` (`over`, for
/// a `Provider` a backend shares across threads).
pub struct Interpreter<'p> {
    origin: usize,
    program: &'p Program,
    resolved: &'p Resolved,
    defs: FxHashMap<Symbol, Def<'p>>,
    tests: FxHashMap<Symbol, (&'p Expr, usize)>,
    ctors: FxHashMap<Symbol, usize>,
    ops: crate::semantics::OpTable,
    members: BTreeSet<Symbol>,
    counters: Counters,
}

impl Interpreter<'static> {
    pub fn over(
        program: &Program,
        resolved: &Resolved,
        check: &CheckOutput,
    ) -> &'static Interpreter<'static> {
        let origin = std::ptr::from_ref(program) as usize;
        let program: &'static Program = Box::leak(Box::new(program.clone()));
        let resolved: &'static Resolved = Box::leak(Box::new(resolved.clone()));
        let _ = check;
        Box::leak(Box::new(Interpreter::build(origin, program, resolved)))
    }

    pub fn over_static(
        program: &'static Program,
        resolved: &'static Resolved,
        check: &'static CheckOutput,
    ) -> &'static Interpreter<'static> {
        { let _ = check; Box::leak(Box::new(Interpreter::build(std::ptr::from_ref(program) as usize, program, resolved))) }
    }
}

impl<'p> Interpreter<'p> {
    /// Build the tables borrowing the program, for an engine whose life is the borrow's.
    pub fn borrow(program: &'p Program, resolved: &'p Resolved) -> Interpreter<'p> {
        let origin = std::ptr::from_ref(program) as usize;
        Interpreter::build(origin, program, resolved)
    }

    /// The parameters, body and home module of a definition, for an engine entering it whole.
    pub fn def(&self, name: &Symbol) -> Option<(code::Params, &'p Expr, usize)> {
        self.defs
            .get(name)
            .map(|d| (Rc::new(d.params.clone()), d.body, d.module))
    }

    fn build(origin: usize, program: &'p Program, resolved: &'p Resolved) -> Interpreter<'p> {
        let mut defs = FxHashMap::default();
        let mut tests = FxHashMap::default();
        let mut ctors: FxHashMap<Symbol, usize> =
            ply_core::prelude::ctor_arities().into_iter().collect();
        let mut ops = crate::semantics::OpTable::default();
        let mut members = BTreeSet::new();
        for (module, m) in program.modules.iter().enumerate() {
            let mut ordinal = 0usize;
            for item in &m.items {
                match item {
                    Item::Fn(def) => {
                        let name = m.name.qualify(&def.name.name);
                        let params: Vec<Symbol> =
                            def.params.iter().map(|p| p.name.name.clone()).collect();
                        defs.insert(
                            name.clone(),
                            Def {
                                params,
                                body: &def.body,
                                module,
                            },
                        );
                        members.insert(name);
                    }
                    Item::Test(test) => {
                        let name = m.name.qualify(&Symbol::new(format!("test#{ordinal}")));
                        tests.insert(name.clone(), (&test.body, module));
                        members.insert(name);
                        ordinal += 1;
                    }
                    Item::Type(t) => {
                        if let ply_syntax::ast::TypeDefBody::Sum(variants) = &t.body {
                            for v in variants {
                                ctors.insert(m.name.qualify(&v.name.name), v.fields.len());
                            }
                        }
                    }
                    Item::Effect(e) => {
                        for op in &e.ops {
                            ops.insert(
                                (m.name.qualify(&e.name.name), op.name.name.clone()),
                                (op.resource_param, op.mode),
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
        Interpreter {
            origin,
            program,
            resolved,
            defs,
            tests,
            ctors,
            ops,
            members,
            counters: Counters::default(),
        }
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }
}

/// The eval walk and the per-run state it mutates: a cell/region arena reset per entry, the
/// caches a run fills, and the active handler stack. Borrows the program [`Interpreter`] tables.
pub struct Core<'p> {
    lowering: Rc<Lowering<'p>>,
    regions: TaskRegions,
    globals: FxHashMap<GlobalKey, Value>,
    lowered: FxHashMap<Symbol, Lowered>,
    handlers: Vec<HandlerFrame>,
    performed: Vec<EffectAtom>,
}

impl<'p> Core<'p> {
    pub fn new(program: &'p Program) -> Core<'p> {
        Core {
            lowering: Rc::new(Lowering::for_program(program)),
            regions: TaskRegions::new(),
            globals: FxHashMap::default(),
            lowered: FxHashMap::default(),
            handlers: Vec::new(),
            performed: Vec::new(),
        }
    }

    /// The lowering cache to hand a `Core` built next over the same program, so a body is lowered
    /// once for the program rather than once per engine.
    pub fn lowering(&self) -> Rc<Lowering<'p>> {
        Rc::clone(&self.lowering)
    }

    /// Lower into `lowering` rather than into a cache of this `Core`'s own.
    pub fn set_lowering(&mut self, lowering: Rc<Lowering<'p>>) {
        self.lowering = lowering;
    }

    pub fn regions(&self) -> &TaskRegions {
        &self.regions
    }

    pub fn cells(&self) -> &Arena {
        self.regions.arena()
    }

    pub fn cells_mut(&mut self) -> &mut Arena {
        self.regions.arena_mut()
    }

    pub fn set_regions(&mut self, regions: TaskRegions) {
        self.regions = regions;
    }

    pub fn take_performed(&mut self) -> Vec<EffectAtom> {
        std::mem::take(&mut self.performed)
    }
}

/// The eval walk over one entry point: the program tables and the mutable [`Core`] state as
/// disjoint borrows, so an engine can own both an [`Interpreter`] and its `Core` and still walk
/// without a self-referential struct.
pub struct Run<'p, 'x> {
    home: &'x Interpreter<'p>,
    st: &'x mut Core<'p>,
}

impl<'p, 'x> Run<'p, 'x> {
    pub fn new(home: &'x Interpreter<'p>, st: &'x mut Core<'p>) -> Run<'p, 'x> {
        Run { home, st }
    }

    /// Restore the world for a fresh entry point: the arena the entry resets to, and the handler
    /// stack and performed atoms it starts empty. The name and lowering caches are program-level
    /// and kept.
    fn reset_run(&mut self) {
        self.st.regions.reset();
        self.st.handlers.clear();
        self.st.performed.clear();
    }

    /// Enter a test or whole definition as an entry point: reset, seed the window with the
    /// arguments, and walk the lowered body under a fresh recursion budget.
    pub fn enter_root(
        &mut self,
        params: code::Params,
        body: &'p Expr,
        module: usize,
        args: Vec<Value>,
        budget: usize,
    ) -> Entered {
        let lowered = self.st.lowering.of(&params, body);
        self.enter_lowered(lowered, module, &args, budget)
    }

    /// Enter an expression from this program, with `bindings` written into its leading slots as a
    /// function's parameters would be — the path `eval_expr` takes for a law body or a const.
    pub fn enter_expr_in(
        &mut self,
        e: &'p Expr,
        bindings: &[(Symbol, Value)],
        module: usize,
        budget: usize,
    ) -> Entered {
        let params: code::Params = Rc::new(bindings.iter().map(|(n, _)| n.clone()).collect());
        let lowered = self.st.lowering.of(&params, e);
        let values: Vec<Value> = bindings.iter().map(|(_, v)| v.clone()).collect();
        self.enter_lowered(lowered, module, &values, budget)
    }

    /// Reset the world, seed the window with `bindings`, and walk a lowered body under a fresh
    /// recursion budget.
    pub fn enter_lowered(
        &mut self,
        lowered: Lowered,
        module: usize,
        bindings: &[Value],
        budget: usize,
    ) -> Entered {
        self.reset_run();
        let mut window = vec![None; lowered.size as usize];
        for (i, v) in bindings.iter().enumerate() {
            window[i] = Some(v.clone());
        }
        let calls = Calls {
            depth: 0,
            max: budget,
        };
        match self.eval(&lowered.code, &mut window, module, calls) {
            Ok(value) => Entered::Answered(value),
            Err(Bail::Fail(d)) => Entered::Raised(d),
            Err(Bail::Decline) => Entered::Declined,
        }
    }

    fn eval(
        &mut self,
        code: &Code,
        window: &mut Vec<Option<Value>>,
        module: usize,
        calls: Calls,
    ) -> Result<Value, Bail> {
        crate::limit::grow(|| self.eval_node(code, window, module, calls))
    }

    fn eval_node(
        &mut self,
        code: &Code,
        window: &mut Vec<Option<Value>>,
        module: usize,
        calls: Calls,
    ) -> Result<Value, Bail> {
        let span = code.span;
        match &code.kind {
            NodeKind::Lit(_, value) => Ok(value.clone()),

            NodeKind::Var { name, slot } => match slot {
                Some(s) => window[*s as usize]
                    .clone()
                    .ok_or_else(|| Bail::Fail(err_released(&name.name.name, span))),
                None => self.lookup(name, module),
            },

            NodeKind::Unary { op, operand } => {
                let v = self.eval(operand, window, module, calls)?;
                crate::semantics::apply_unary(*op, &v, operand.span, span).map_err(Bail::Fail)
            }

            NodeKind::Binary { op, lhs, rhs } => {
                use ply_syntax::ast::BinOp;
                if matches!(op, BinOp::And | BinOp::Or) {
                    let l = self.eval(lhs, window, module, calls)?;
                    let lb = l
                        .as_bool(lhs.span, "a Boolean operator")
                        .map_err(Bail::Fail)?;
                    if crate::semantics::short_circuits(*op, lb) {
                        return Ok(Value::Bool(lb));
                    }
                    let r = self.eval(rhs, window, module, calls)?;
                    let rb = r
                        .as_bool(rhs.span, "a Boolean operator")
                        .map_err(Bail::Fail)?;
                    return Ok(Value::Bool(rb));
                }
                let l = self.eval(lhs, window, module, calls)?;
                let r = self.eval(rhs, window, module, calls)?;
                strict_binary(*op, &l, &r, lhs.span, rhs.span, span).map_err(Bail::Fail)
            }

            NodeKind::Lambda {
                params,
                body,
                size,
                captures,
            } => {
                let captured = self.capture(captures, window)?;
                Ok(Value::Closure(Arc::new(Closure {
                    name: None,
                    kind: ClosureKind::Code {
                        params: params.clone(),
                        body: body.clone(),
                        size: *size,
                        captures: captures.clone(),
                        captured,
                        module,
                    },
                })))
            }

            NodeKind::App { func, args } => {
                let callee = self.eval(func, window, module, calls)?;
                let mut vals = Vec::with_capacity(args.len());
                for a in args.iter() {
                    vals.push(self.eval(a, window, module, calls)?);
                }
                self.apply(&callee, vals, span, calls)
            }

            NodeKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let c = self.eval(cond, window, module, calls)?;
                if c.as_bool(cond.span, "an `if` condition")
                    .map_err(Bail::Fail)?
                {
                    self.eval(then_branch, window, module, calls)
                } else {
                    self.eval(else_branch, window, module, calls)
                }
            }

            NodeKind::Match { scrutinee, arms } => {
                let v = self.eval(scrutinee, window, module, calls)?;
                self.match_arms(&v, arms, window, module, calls, scrutinee.span)
            }

            NodeKind::Block { stmts, tail } => {
                for stmt in stmts.iter() {
                    match stmt {
                        Stmt::Let {
                            pat,
                            value,
                            span: lspan,
                        } => {
                            let v = self.eval(value, window, module, calls)?;
                            if !self.bind(pat, &v, window, module)? {
                                return Err(Bail::Fail(crate::semantics::err_let_mismatch(
                                    *lspan, &v,
                                )));
                            }
                        }
                        Stmt::Expr { code } => {
                            self.eval(code, window, module, calls)?;
                        }
                    }
                }
                match tail {
                    Some(t) => self.eval(t, window, module, calls),
                    None => Ok(Value::Unit),
                }
            }

            NodeKind::Record { fields } => {
                let mut out = Fields::default();
                for (name, code) in fields.iter() {
                    let v = self.eval(code, window, module, calls)?;
                    out.insert(name.clone(), v);
                }
                Ok(Value::Record(Arc::new(out)))
            }

            NodeKind::RecordUpdate { base, copies, sets } => {
                let mut set_values = Vec::with_capacity(sets.len());
                for (name, code) in sets.iter() {
                    set_values.push((name.clone(), self.eval(code, window, module, calls)?));
                }
                let base_v = self.eval(base, window, module, calls)?;
                let Value::Record(fields) = &base_v else {
                    return Err(Bail::Fail(err_not_a_record(base.span, &base_v)));
                };
                let mut out = Fields::default();
                for copy in copies.iter() {
                    if let Some(v) = fields.get(&copy.name) {
                        out.insert(copy.name.clone(), v.clone());
                    }
                }
                for (name, v) in set_values {
                    out.insert(name, v);
                }
                Ok(Value::Record(Arc::new(out)))
            }

            NodeKind::Field { base, field } => {
                let v = self.eval(base, window, module, calls)?;
                match &v {
                    Value::Record(fields) => fields.get(&field.name).cloned().ok_or_else(|| {
                        Bail::Fail(crate::semantics::err_no_such_field(field, fields))
                    }),
                    _ => Err(Bail::Fail(err_not_a_record(base.span, &v))),
                }
            }

            NodeKind::List { items } => {
                let mut out = Vec::with_capacity(items.len());
                for item in items.iter() {
                    out.push(self.eval(item, window, module, calls)?);
                }
                Ok(Value::list(out))
            }

            NodeKind::Handle { body, clauses, ret } => {
                let frame = self.handler_frame(clauses, ret, window, module)?;
                self.st.handlers.push(frame);
                let outcome = self.eval(body, window, module, calls);
                let frame = self.st.handlers.pop().expect("the frame this handle pushed");
                let value = outcome?;
                match &frame.ret {
                    None => Ok(value),
                    Some(r) => self.run_clause(
                        &r.params,
                        &r.body,
                        r.size,
                        &r.captured,
                        &r.captures,
                        r.module,
                        vec![value],
                        calls,
                    ),
                }
            }

            NodeKind::Perform {
                effect,
                op,
                resource,
                args,
            } => {
                let effect_g = self.effect_name(module, effect);
                let mut vals = Vec::with_capacity(args.len());
                for a in args.iter() {
                    vals.push(self.eval(a, window, module, calls)?);
                }
                let decl = crate::semantics::op_decl(&self.home.ops, &effect_g, op);
                if let Some(atom) =
                    crate::handler::performed_atom(&effect_g, resource.as_ref(), decl)
                {
                    self.st.performed.push(atom);
                }
                self.perform(&effect_g, op, resource, vals, calls, span)
            }

            NodeKind::WithCell {
                init,
                binder,
                slot,
                body,
                ..
            } => {
                let initial = self.eval(init, window, module, calls)?;
                let cell = self.st.regions.alloc_cell(initial);
                let _ = binder;
                if let Some(s) = slot {
                    window[*s as usize] = Some(Value::Cell(cell));
                }
                self.eval(body, window, module, calls)
            }

            // A region's tasks interleave on the tier's stacks; the caller runs these on the
            // compiled front end.
            NodeKind::WithRegion { .. } | NodeKind::Simulate { .. } => Err(Bail::Decline),
        }
    }

    fn apply(
        &mut self,
        callee: &Value,
        args: Vec<Value>,
        span: Span,
        calls: Calls,
    ) -> Result<Value, Bail> {
        let Value::Closure(closure) = callee else {
            return Err(Bail::Fail(crate::semantics::err_not_a_function(
                span, callee,
            )));
        };
        match &closure.kind {
            ClosureKind::Code {
                params,
                body,
                size,
                captures,
                captured,
                module,
            } => {
                if params.len() != args.len() {
                    return Err(Bail::Fail(arity(span, closure, params.len(), args.len())));
                }
                let calls = calls.deeper(span)?;
                let mut window = vec![None; *size as usize];
                for (i, v) in args.into_iter().enumerate() {
                    window[i] = Some(v);
                }
                for (j, dst) in captures.dst.iter().enumerate() {
                    window[*dst as usize] = Some(captured[j].clone());
                }
                let body = body.clone();
                let module = *module;
                self.eval(&body, &mut window, module, calls)
            }
            ClosureKind::Fn {
                params,
                body,
                bindings,
                module,
            } => {
                if params.len() != args.len() {
                    return Err(Bail::Fail(arity(span, closure, params.len(), args.len())));
                }
                let calls = calls.deeper(span)?;
                // The bindings the closure carries are lowered as leading parameters, so their
                // occurrences resolve to slots ahead of the closure's own parameters.
                let combined: Vec<Symbol> = bindings
                    .iter()
                    .map(|(n, _)| n.clone())
                    .chain(params.iter().cloned())
                    .collect();
                let lowered = crate::code::lower_fn(&combined, body);
                let mut window = vec![None; lowered.size as usize];
                for (i, (_, v)) in bindings.iter().enumerate() {
                    window[i] = Some(v.clone());
                }
                for (i, v) in args.into_iter().enumerate() {
                    window[bindings.len() + i] = Some(v);
                }
                let module = *module;
                self.eval(&lowered.code, &mut window, module, calls)
            }
            ClosureKind::Ctor { name, arity: n } => {
                if *n != args.len() {
                    return Err(Bail::Fail(arity(span, closure, *n, args.len())));
                }
                Ok(Value::ctor(name.clone(), args))
            }
            ClosureKind::Builtin(b) => self.call_builtin(*b, args, span, calls),
            ClosureKind::Native { .. } => Err(Bail::Decline),
        }
    }

    fn call_builtin(
        &mut self,
        b: Builtin,
        args: Vec<Value>,
        span: Span,
        calls: Calls,
    ) -> Result<Value, Bail> {
        let mut step =
            crate::builtins::call(b, args, self.st.regions.arena_mut(), span).map_err(Bail::Fail)?;
        loop {
            match step {
                crate::builtins::Step::Done(v) => return Ok(v),
                crate::builtins::Step::Apply {
                    callee,
                    args,
                    frame,
                } => {
                    let answer = self.apply(&callee, args, span, calls)?;
                    step = crate::builtins::advance(frame, answer).map_err(Bail::Fail)?;
                }
            }
        }
    }

    fn match_arms(
        &mut self,
        scrutinee: &Value,
        arms: &Rc<Vec<Arm>>,
        window: &mut Vec<Option<Value>>,
        module: usize,
        calls: Calls,
        scrutinee_span: Span,
    ) -> Result<Value, Bail> {
        for arm in arms.iter() {
            if self.bind(&arm.pat, scrutinee, window, module)? {
                if let Some(guard) = &arm.guard {
                    let g = self.eval(guard, window, module, calls)?;
                    if !g.as_bool(guard.span, "a match guard").map_err(Bail::Fail)? {
                        continue;
                    }
                }
                return self.eval(&arm.body, window, module, calls);
            }
        }
        Err(Bail::Fail(crate::semantics::err_non_exhaustive(
            scrutinee_span,
            scrutinee,
        )))
    }

    /// Match a pattern, writing binders into the window.
    fn bind(
        &self,
        pat: &Pat,
        value: &Value,
        window: &mut Vec<Option<Value>>,
        module: usize,
    ) -> Result<bool, Bail> {
        Ok(match pat {
            Pat::Wildcard => true,
            Pat::Var { name, slot } => {
                let declared = self.ctor_name(module, &QName::bare(name.clone()));
                match declared.as_ref().and_then(|n| self.home.ctors.get(n)) {
                    Some(0) => {
                        let ctor = declared.expect("a hit came from a resolved name");
                        matches!(value, Value::Ctor { name, args } if *name == ctor && args.is_empty())
                    }
                    _ => {
                        if let Some(s) = slot {
                            window[*s as usize] = Some(value.clone());
                        }
                        true
                    }
                }
            }
            Pat::Lit(lit) => lit_matches(lit, value),
            Pat::Ctor { name, args } => match value {
                Value::Ctor {
                    name: vname,
                    args: vargs,
                } => {
                    let expected = self.ctor_name(module, name);
                    if expected.as_ref() != Some(vname) || vargs.len() != args.len() {
                        return Ok(false);
                    }
                    for (p, v) in args.iter().zip(vargs.iter()) {
                        if !self.bind(p, v, window, module)? {
                            return Ok(false);
                        }
                    }
                    true
                }
                _ => false,
            },
            Pat::Record { fields, rest } => match value {
                Value::Record(map) => {
                    if !*rest && map.len() != fields.len() {
                        return Ok(false);
                    }
                    for (name, p) in fields {
                        let Some(v) = map.get(&name.name).cloned() else {
                            return Ok(false);
                        };
                        if !self.bind(p, &v, window, module)? {
                            return Ok(false);
                        }
                    }
                    true
                }
                _ => false,
            },
            Pat::List { items, rest } => match value {
                Value::List(xs) => {
                    let fits = match rest {
                        Some(_) => xs.len() >= items.len(),
                        None => xs.len() == items.len(),
                    };
                    if !fits {
                        return Ok(false);
                    }
                    for (p, v) in items.iter().zip(xs.iter()) {
                        if !self.bind(p, v, window, module)? {
                            return Ok(false);
                        }
                    }
                    match rest {
                        Some(rest) => {
                            let tail = Value::List(xs.skip(items.len()));
                            self.bind(rest, &tail, window, module)?
                        }
                        None => true,
                    }
                }
                _ => false,
            },
        })
    }

    fn capture(
        &self,
        captures: &Rc<Captures>,
        window: &[Option<Value>],
    ) -> Result<Rc<[Value]>, Bail> {
        if captures.is_empty() {
            return Ok(code::no_captured());
        }
        let mut out = Vec::with_capacity(captures.src.len());
        for j in 0..captures.src.len() {
            let src = captures.src[j] as usize;
            let v = window[src]
                .clone()
                .ok_or_else(|| Bail::Fail(err_released(&captures.names[j], Span::DUMMY)))?;
            out.push(v);
        }
        Ok(out.into())
    }

    /// A handler's clauses and its `return` arm, with each body's free variables captured from the
    /// scope the `handle` was written in — the tail-resumptive subset. A clause that binds
    /// `resume` is carried but declined when performed, since its continuation lives on the tier's
    /// stacks.
    fn handler_frame(
        &self,
        clauses: &Rc<Vec<Clause>>,
        ret: &Option<Rc<ReturnArm>>,
        window: &[Option<Value>],
        module: usize,
    ) -> Result<HandlerFrame, Bail> {
        let mut cs = Vec::with_capacity(clauses.len());
        for c in clauses.iter() {
            cs.push(HandlerClause {
                effect: self.effect_name(module, &c.effect),
                op: c.op.clone(),
                resource: c.resource.clone(),
                params: c.params.clone(),
                resumes: c.resume.is_some(),
                body: c.body.clone(),
                size: c.size,
                captured: self.capture(&c.captures, window)?,
                captures: c.captures.clone(),
                module,
            });
        }
        let ret = match ret {
            None => None,
            Some(r) => Some(RetArm {
                params: Rc::new(vec![r.binder.clone()]),
                body: r.body.clone(),
                size: r.size,
                captured: self.capture(&r.captures, window)?,
                captures: r.captures.clone(),
                module,
            }),
        };
        Ok(HandlerFrame { clauses: cs, ret })
    }

    /// Search the active handlers from the innermost out for a clause that answers this operation,
    /// and run it below its own frame so a `perform` in the clause sees only the outer handlers.
    fn perform(
        &mut self,
        effect: &Symbol,
        op: &Symbol,
        resource: &Option<Symbol>,
        args: Vec<Value>,
        calls: Calls,
        span: Span,
    ) -> Result<Value, Bail> {
        let found = {
            let mut hit = None;
            'outer: for (i, frame) in self.st.handlers.iter().enumerate().rev() {
                for c in &frame.clauses {
                    if c.effect == *effect && c.op == *op && c.resource == *resource {
                        if c.resumes {
                            return Err(Bail::Decline);
                        }
                        hit = Some((i, c.clone()));
                        break 'outer;
                    }
                }
            }
            hit
        };
        let Some((depth, c)) = found else {
            return Err(Bail::Decline);
        };
        if c.params.len() != args.len() {
            return Err(Bail::Fail(crate::semantics::arity_error(
                span,
                &format!("the handler clause for `{effect}.{op}`"),
                c.params.len(),
                args.len(),
            )));
        }
        // Run the clause with the handlers below its own frame in scope.
        let saved: Vec<HandlerFrame> = self.st.handlers.split_off(depth);
        let out = self.run_clause(
            &c.params,
            &c.body,
            c.size,
            &c.captured,
            &c.captures,
            c.module,
            args,
            calls,
        );
        self.st.handlers.extend(saved);
        out
    }

    /// Evaluate a clause or `return` body in a fresh window: its parameters, then its captures.
    #[allow(clippy::too_many_arguments)]
    fn run_clause(
        &mut self,
        params: &[Symbol],
        body: &Code,
        size: u32,
        captured: &[Value],
        captures: &Rc<Captures>,
        module: usize,
        args: Vec<Value>,
        calls: Calls,
    ) -> Result<Value, Bail> {
        let _ = params;
        let mut window = vec![None; size as usize];
        for (i, v) in args.into_iter().enumerate() {
            window[i] = Some(v);
        }
        for (j, dst) in captures.dst.iter().enumerate() {
            window[*dst as usize] = Some(captured[j].clone());
        }
        self.eval(body, &mut window, module, calls)
    }

    fn effect_name(&self, module: usize, q: &QName) -> Symbol {
        self.global(module, Namespace::Effect, q)
            .unwrap_or_else(|| q.symbol().clone())
    }

    fn lookup(&mut self, q: &QName, module: usize) -> Result<Value, Bail> {
        let key = (
            module,
            q.module.as_ref().map(|m| m.name.clone()),
            q.name.name.clone(),
        );
        if let Some(v) = self.st.globals.get(&key) {
            return Ok(v.clone());
        }
        let value = self.resolve(q, module)?;
        self.st.globals.insert(key, value.clone());
        Ok(value)
    }

    fn resolve(&mut self, q: &QName, module: usize) -> Result<Value, Bail> {
        if let Some(name) = self.global(module, Namespace::Value, q)
            && self.home.defs.contains_key(&name)
        {
            let lowered = self.lowered_body(&name);
            let params = self.home.defs[&name].params.clone();
            let def_module = self.home.defs[&name].module;
            return Ok(Value::Closure(Arc::new(Closure {
                name: Some(name.clone()),
                kind: ClosureKind::Code {
                    params: Rc::new(params),
                    size: lowered.size,
                    body: lowered.code,
                    captures: code::no_captures(),
                    captured: code::no_captured(),
                    module: def_module,
                },
            })));
        }
        if let Some(name) = self.ctor_name(module, q)
            && let Some(&arity) = self.home.ctors.get(&name)
        {
            return Ok(ctor_value(&name, arity));
        }
        if q.is_bare()
            && let Some(b) = Builtin::from_name(q.symbol())
        {
            return Ok(Value::builtin(b));
        }
        Err(Bail::Fail(crate::semantics::err_unknown_name(q)))
    }

    fn lowered_body(&mut self, name: &Symbol) -> Lowered {
        if let Some(l) = self.st.lowered.get(name) {
            return l.clone();
        }
        let def = &self.home.defs[name];
        let params: code::Params = Rc::new(def.params.clone());
        let lowered = self.st.lowering.of(&params, def.body);
        self.st.lowered.insert(name.clone(), lowered.clone());
        lowered
    }

    fn global(&self, module: usize, ns: Namespace, q: &QName) -> Option<Symbol> {
        if q.is_bare() {
            return self
                .home
                .resolved
                .scopes
                .get(module)
                .and_then(|scope| scope.get(ns, q.symbol()))
                .map(|b| b.qualified.clone());
        }
        self.home
            .resolved
            .lookup(module, ns, q)
            .ok()
            .map(|b| b.qualified.clone())
    }

    fn ctor_name(&self, module: usize, q: &QName) -> Option<Symbol> {
        match self.global(module, Namespace::Value, q) {
            Some(name) => Some(name),
            None if q.is_bare() && self.home.ctors.contains_key(q.symbol()) => {
                Some(q.symbol().clone())
            }
            None => None,
        }
    }
}

/// The pure applier: an [`Interpreter`] and a [`Core`] over one program, used to evaluate an
/// ad-hoc expression that has no compiled body — a const the tooling reads, a `law` body, and the
/// generated function values higher-order property testing applies. It is the tier that runs the
/// language; this is the interpreter kept only for expressions the tier never compiled. It carries
/// the pure first-order language, local `with_cell`, and tail-resumptive `handle`/`perform` — never
/// `simulate`, regions, or multi-shot `resume`, which a law never uses.
pub struct Pure<'a> {
    interp: Interpreter<'a>,
    core: Core<'a>,
}

impl<'a> Pure<'a> {
    pub fn new(program: &'a Program, resolved: &'a Resolved) -> Pure<'a> {
        Pure {
            interp: Interpreter::borrow(program, resolved),
            core: Core::new(program),
        }
    }

    /// Evaluate `e` in `module` with `bindings` bound as its leading parameters — a law body over
    /// its generated binders.
    pub fn eval_expr_in(
        &mut self,
        e: &'a Expr,
        bindings: &[(Symbol, Value)],
        module: usize,
        budget: usize,
    ) -> Result<Value, Diagnostic> {
        answer(Run::new(&self.interp, &mut self.core).enter_expr_in(e, bindings, module, budget))
    }

    /// Evaluate an expression of unknown provenance, lowered afresh in module 0.
    pub fn eval_expr(&mut self, e: &Expr, budget: usize) -> Result<Value, Diagnostic> {
        let lowered = crate::code::lower(e);
        answer(Run::new(&self.interp, &mut self.core).enter_lowered(lowered, 0, &[], budget))
    }

    /// Enter a definition whole — the program-wide name, `store.orders.place` not `place` — for a
    /// const the tooling reads or a helper that applies a generated closure.
    pub fn call(
        &mut self,
        name: &str,
        args: Vec<Value>,
        span: Span,
        budget: usize,
    ) -> Result<Value, Diagnostic> {
        let sym = Symbol::new(name);
        let Some((params, body, module)) = self.interp.def(&sym) else {
            return Err(Diagnostic::error(
                codes::UNKNOWN_NAME,
                format!("no definition named `{name}`"),
            )
            .primary(span, "not defined in this program"));
        };
        answer(Run::new(&self.interp, &mut self.core).enter_root(params, body, module, args, budget))
    }
}

/// Turn an [`Entered`] into a `Result`, where a `Declined` is a pure-evaluator refusal rather than
/// a fall-through to a tier — the pure applier has no tier to fall to.
fn answer(entered: Entered) -> Result<Value, Diagnostic> {
    match entered {
        Entered::Answered(v) => Ok(v),
        Entered::Raised(d) => Err(d),
        Entered::Declined => Err(Diagnostic::error(
            codes::RUNTIME_ERROR,
            "this expression uses `simulate`, a region, or a multi-shot resume, which the pure \
             evaluator does not carry",
        )
        .note("law bodies and generated values are expected to be pure and first-order")),
    }
}

#[derive(Clone)]
struct HandlerFrame {
    clauses: Vec<HandlerClause>,
    ret: Option<RetArm>,
}

#[derive(Clone)]
struct HandlerClause {
    effect: Symbol,
    op: Symbol,
    resource: Option<Symbol>,
    params: code::Params,
    resumes: bool,
    body: Code,
    size: u32,
    captured: Rc<[Value]>,
    captures: Rc<Captures>,
    module: usize,
}

#[derive(Clone)]
struct RetArm {
    params: code::Params,
    body: Code,
    size: u32,
    captured: Rc<[Value]>,
    captures: Rc<Captures>,
    module: usize,
}

/// A per-entry recursion depth, capped as the tier caps nested calls: counted on each closure
/// application and unwound by the native stack, so a sequential loop of ten thousand calls stays
/// shallow while unbounded recursion is refused.
#[derive(Clone, Copy)]
struct Calls {
    depth: usize,
    max: usize,
}

impl Calls {
    fn deeper(self, span: Span) -> Result<Calls, Bail> {
        if self.depth + 1 > self.max {
            return Err(Bail::Fail(
                Diagnostic::error(
                    codes::RUNTIME_ERROR,
                    format!("recursion limit of {} nested calls exceeded", self.max),
                )
                .primary(span, "this call is too deeply nested"),
            ));
        }
        Ok(Calls {
            depth: self.depth + 1,
            max: self.max,
        })
    }
}

enum Bail {
    Decline,
    Fail(Diagnostic),
}

fn err_released(name: &Symbol, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`{name}` was read after its last use"),
    )
    .primary(
        span,
        "the liveness analysis called this binding dead and it was read anyway",
    )
}

fn err_not_a_record(span: Span, v: &Value) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("a record was expected, and this is {}", v.type_name()),
    )
    .primary(span, "not a record")
}

fn arity(span: Span, closure: &Closure, expected: usize, got: usize) -> Diagnostic {
    crate::semantics::arity_error(span, &closure.describe(), expected, got)
}
