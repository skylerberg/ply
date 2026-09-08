//! The interpreted front end (ADR 0047): one evaluator walks the lowered `code` the compiled
//! front end lowers to C, over the shared runtime, with no C compiler in the path.
//!
//! It reuses the leaf semantics the machine and the tier already share — `strict_binary`,
//! `apply_unary`, `lit_matches`, `ctor_value`, and `builtins::{call, advance}` — so a body it
//! evaluates answers what the compiled code answers by construction. Where it reaches a node it
//! does not yet carry — a `perform`, a `handle`, a `simulate`, a `with cell`/`with region`, a
//! continuation resumed — it **declines**, exactly as the emitter port refuses a form it has not
//! reached, and `--audit-backend` compares only what it enters. The declined nodes are the ones
//! whose continuations live on the tier's stacks (ADR 0044); they are the next increment.

use crate::backend::{Counters, Offers, Policed, Provider, Spec, wrap};
use crate::code::{self, Arm, Captures, Code, Lowered, Lowering, NodeKind, Pat, Stmt};
use crate::compiled::{Compiled, Entered};
use crate::semantics::{ctor_value, lit_matches, strict_binary};
use crate::value::{Closure, ClosureKind, Fields, Value};
use crate::{Builtin, TaskRegions};
use ply_core::CheckOutput;
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::{Expr, Item, Program, QName};
use ply_syntax::resolve::{Namespace, Resolved};
use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::collections::BTreeSet;
type GlobalKey = (usize, Option<Symbol>, Symbol);

use std::rc::Rc;
use std::sync::Arc;

/// A body the interpreter can enter: its parameters and where its bare names resolve.
struct Def {
    params: Vec<Symbol>,
    body: &'static Expr,
    module: usize,
}

/// The interpreter over one program, built like `Fragment`: the program, its resolution and its
/// check, leaked to `'static` so a closure it makes can outlive the borrow that made it.
pub struct Interpreter {
    origin: usize,
    program: &'static Program,
    resolved: &'static Resolved,
    defs: FxHashMap<Symbol, Def>,
    tests: FxHashMap<Symbol, (&'static Expr, usize)>,
    ctors: FxHashMap<Symbol, usize>,
    members: BTreeSet<Symbol>,
    counters: Counters,
}

impl Interpreter {
    pub fn over(
        program: &Program,
        resolved: &Resolved,
        check: &CheckOutput,
    ) -> &'static Interpreter {
        let origin = std::ptr::from_ref(program) as usize;
        let program: &'static Program = Box::leak(Box::new(program.clone()));
        let resolved: &'static Resolved = Box::leak(Box::new(resolved.clone()));
        let check: &'static CheckOutput = Box::leak(Box::new(check.clone()));
        Interpreter::build(origin, program, resolved, check)
    }

    pub fn over_static(
        program: &'static Program,
        resolved: &'static Resolved,
        check: &'static CheckOutput,
    ) -> &'static Interpreter {
        Interpreter::build(
            std::ptr::from_ref(program) as usize,
            program,
            resolved,
            check,
        )
    }

    fn build(
        origin: usize,
        program: &'static Program,
        resolved: &'static Resolved,
        _check: &'static CheckOutput,
    ) -> &'static Interpreter {
        let mut defs = FxHashMap::default();
        let mut tests = FxHashMap::default();
        let mut ctors: FxHashMap<Symbol, usize> =
            ply_core::prelude::ctor_arities().into_iter().collect();
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
                    _ => {}
                }
            }
        }
        Box::leak(Box::new(Interpreter {
            origin,
            program,
            resolved,
            defs,
            tests,
            ctors,
            members,
            counters: Counters::default(),
        }))
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    pub fn attach(&'static self, spec: &Spec) -> Rc<dyn Compiled> {
        wrap(Rc::new(Interp::new(self)), spec)
    }
}

impl Provider for Interpreter {
    fn attach(&'static self, spec: &Spec) -> Rc<dyn Compiled> {
        Interpreter::attach(self, spec)
    }

    fn name(&self) -> &'static str {
        "interp"
    }

    fn len(&self) -> usize {
        Interpreter::len(self)
    }

    fn offers(&self) -> Offers {
        self.counters.offers()
    }
}

/// One attached interpreter: the shared program tables, a private cell/region arena reset per
/// entry, and the caches a run fills.
pub struct Interp {
    home: &'static Interpreter,
    lowering: Lowering<'static>,
    arena: RefCell<TaskRegions>,
    globals: RefCell<FxHashMap<GlobalKey, Value>>,
    lowered: RefCell<FxHashMap<Symbol, Lowered>>,
}

impl Interp {
    fn new(home: &'static Interpreter) -> Interp {
        Interp {
            home,
            lowering: Lowering::for_program(home.program),
            arena: RefCell::new(TaskRegions::new()),
            globals: RefCell::new(FxHashMap::default()),
            lowered: RefCell::new(FxHashMap::default()),
        }
    }

    fn reset(&self) {
        *self.arena.borrow_mut() = TaskRegions::new();
    }

    fn enter_root(
        &self,
        params: code::Params,
        body: &'static Expr,
        module: usize,
        args: Vec<Value>,
        budget: usize,
    ) -> Entered {
        self.reset();
        let lowered = self.lowering.of(&params, body);
        let mut window = vec![None; lowered.size as usize];
        for (i, v) in args.into_iter().enumerate() {
            window[i] = Some(v);
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
        &self,
        code: &Code,
        window: &mut Vec<Option<Value>>,
        module: usize,
        calls: Calls,
    ) -> Result<Value, Bail> {
        crate::limit::grow(|| self.eval_node(code, window, module, calls))
    }

    fn eval_node(
        &self,
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
                crate::machine::apply_unary(*op, &v, operand.span, span).map_err(Bail::Fail)
            }

            NodeKind::Binary { op, lhs, rhs } => {
                use ply_syntax::ast::BinOp;
                if matches!(op, BinOp::And | BinOp::Or) {
                    let l = self.eval(lhs, window, module, calls)?;
                    let lb = l
                        .as_bool(lhs.span, "a Boolean operator")
                        .map_err(Bail::Fail)?;
                    if crate::machine::short_circuits(*op, lb) {
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

            // The nodes whose continuations live on the tier's stacks: the next increment.
            NodeKind::Perform { .. }
            | NodeKind::Handle { .. }
            | NodeKind::WithCell { .. }
            | NodeKind::WithRegion { .. }
            | NodeKind::Simulate { .. } => Err(Bail::Decline),
        }
    }

    fn apply(
        &self,
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
                let lowered = Lowered {
                    code: body.clone(),
                    size: *size,
                };
                self.eval(&lowered.code, &mut window, *module, calls)
            }
            ClosureKind::Fn { .. } => Err(Bail::Decline),
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
        &self,
        b: Builtin,
        args: Vec<Value>,
        span: Span,
        calls: Calls,
    ) -> Result<Value, Bail> {
        let mut step = {
            let mut arena = self.arena.borrow_mut();
            crate::builtins::call(b, args, arena.arena_mut(), span).map_err(Bail::Fail)?
        };
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
        &self,
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

    /// The machine's `match_pattern`, writing binders into the interpreter's window.
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

    fn lookup(&self, q: &QName, module: usize) -> Result<Value, Bail> {
        let key = (
            module,
            q.module.as_ref().map(|m| m.name.clone()),
            q.name.name.clone(),
        );
        if let Some(v) = self.globals.borrow().get(&key) {
            return Ok(v.clone());
        }
        let value = self.resolve(q, module)?;
        self.globals.borrow_mut().insert(key, value.clone());
        Ok(value)
    }

    fn resolve(&self, q: &QName, module: usize) -> Result<Value, Bail> {
        if let Some(name) = self.global(module, Namespace::Value, q)
            && let Some(def) = self.home.defs.get(&name)
        {
            let lowered = self.lowered_body(&name, def);
            return Ok(Value::Closure(Arc::new(Closure {
                name: Some(name.clone()),
                kind: ClosureKind::Code {
                    params: Rc::new(def.params.clone()),
                    size: lowered.size,
                    body: lowered.code,
                    captures: code::no_captures(),
                    captured: code::no_captured(),
                    module: def.module,
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

    fn lowered_body(&self, name: &Symbol, def: &Def) -> Lowered {
        if let Some(l) = self.lowered.borrow().get(name) {
            return l.clone();
        }
        let params: code::Params = Rc::new(def.params.clone());
        let lowered = self.lowering.of(&params, def.body);
        self.lowered
            .borrow_mut()
            .insert(name.clone(), lowered.clone());
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

/// A per-entry recursion depth, capped as the machine caps nested calls (`stack.calls()` against
/// `max_calls`): counted on each closure application and unwound by the native stack, so a
/// sequential loop of ten thousand calls stays shallow while unbounded recursion is refused.
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

impl Policed for Interp {
    fn counters(&self) -> &'static Counters {
        &self.home.counters
    }

    fn holds(&self, name: &Symbol) -> bool {
        self.home.members.contains(name)
    }

    fn answer(&self, _name: &Symbol, _args: &[Value], _budget: usize) -> Option<Value> {
        None
    }

    fn run_with_fuel(&self, _name: &Symbol, _args: &[Value], _fuel: usize) -> Option<Value> {
        None
    }
}

impl Compiled for Interp {
    fn describes(&self, program: &Program) -> bool {
        self.home.origin == std::ptr::from_ref(program) as usize
    }

    fn enter(&self, _name: &Symbol, _args: &[Value], _budget: usize) -> Option<Value> {
        None
    }

    fn enter_test(&self, name: &Symbol, budget: usize) -> Entered {
        self.home.counters.note_offer(&[]);
        let Some((body, module)) = self.home.tests.get(name) else {
            return Entered::Declined;
        };
        self.enter_root(Rc::new(Vec::new()), body, *module, Vec::new(), budget)
    }

    fn enter_whole(&self, name: &Symbol, args: &[Value], budget: usize) -> Entered {
        self.home.counters.note_offer(args);
        if let Some((body, module)) = self.home.tests.get(name) {
            return self.enter_root(Rc::new(Vec::new()), body, *module, args.to_vec(), budget);
        }
        let Some(def) = self.home.defs.get(name) else {
            return Entered::Declined;
        };
        if def.params.len() != args.len() {
            return Entered::Declined;
        }
        let (params, body, module) = (Rc::new(def.params.clone()), def.body, def.module);
        self.enter_root(params, body, module, args.to_vec(), budget)
    }
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
