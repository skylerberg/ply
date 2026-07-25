//! Normalized serialization of a definition.
//!
//! The bytes produced here are the definition's identity. Two rules do the real
//! work: a local binder is replaced by the de Bruijn *level* at which it was
//! bound, and a reference to another top-level definition is replaced by that
//! definition's hash. Neither a local's name nor a referent's name can reach the
//! byte stream, which is what makes renaming free.
//!
//! Every node starts with a distinct tag byte and every variable-length field
//! carries a `u32` length prefix, so the encoding is uniquely decodable and two
//! differently-shaped trees cannot produce the same bytes.

use ply_span::Symbol;
use ply_syntax::ast::*;
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::BTreeMap;

use crate::DefHash;
use crate::graph::{ModuleIndex, NodeBody, NodeId};

/// The parser's nesting limit does not bound this walk: a left-leaning operator
/// chain is parsed iteratively at constant depth but is still an arbitrarily
/// deep tree. Overflowing here aborts the process, taking every unrelated test
/// sharing it down too.
fn grow<R>(f: impl FnOnce() -> R) -> R {
    const RED_ZONE: usize = 256 * 1024;
    const NEW_SEGMENT: usize = 2 * 1024 * 1024;
    stacker::maybe_grow(RED_ZONE, NEW_SEGMENT, f)
}

mod tag {
    pub const LOCAL: u8 = 1;
    pub const REF_HASH: u8 = 2;
    pub const REF_SELF: u8 = 3;
    pub const REF_INDEX: u8 = 4;
    pub const FREE: u8 = 5;
    pub const CTOR: u8 = 6;
    pub const TY_PARAM: u8 = 7;
    pub const ROW_PARAM: u8 = 8;

    pub const NONE: u8 = 10;
    pub const SOME: u8 = 11;

    pub const FN: u8 = 20;
    pub const TYPE_ALIAS: u8 = 21;
    pub const TYPE_SUM: u8 = 22;
    pub const EFFECT: u8 = 23;
    pub const TEST: u8 = 24;
    pub const VARIANT: u8 = 25;
    pub const OP: u8 = 26;
    pub const TYPE: u8 = 27;

    pub const TY_CON: u8 = 30;
    pub const TY_FN: u8 = 31;
    pub const TY_RECORD: u8 = 32;
    pub const TY_UNIT: u8 = 33;

    pub const LIT_INT: u8 = 40;
    pub const LIT_BOOL: u8 = 41;
    pub const LIT_STR: u8 = 42;
    pub const LIT_UNIT: u8 = 43;

    pub const E_LIT: u8 = 50;
    pub const E_VAR: u8 = 51;
    pub const E_BINARY: u8 = 52;
    pub const E_UNARY: u8 = 53;
    pub const E_LAMBDA: u8 = 54;
    pub const E_APP: u8 = 55;
    pub const E_IF: u8 = 56;
    pub const E_MATCH: u8 = 57;
    pub const E_BLOCK: u8 = 58;
    pub const E_RECORD: u8 = 59;
    pub const E_FIELD: u8 = 60;
    pub const E_LIST: u8 = 61;
    pub const E_PERFORM: u8 = 62;
    pub const E_HANDLE: u8 = 63;
    pub const E_WITH_CELL: u8 = 64;

    pub const S_LET: u8 = 70;
    pub const S_EXPR: u8 = 71;

    pub const P_WILDCARD: u8 = 80;
    pub const P_VAR: u8 = 81;
    pub const P_LIT: u8 = 82;
    pub const P_CTOR: u8 = 83;
    pub const P_RECORD: u8 = 84;
    pub const P_LIST: u8 = 85;

    pub const ROW: u8 = 90;
    pub const ATOM: u8 = 91;
    pub const ARM: u8 = 92;
    pub const CLAUSE: u8 = 93;
    pub const RETURN_CLAUSE: u8 = 94;
}

fn binop_byte(op: BinOp) -> u8 {
    match op {
        BinOp::Add => 1,
        BinOp::Sub => 2,
        BinOp::Mul => 3,
        BinOp::Div => 4,
        BinOp::Rem => 5,
        BinOp::Eq => 6,
        BinOp::Ne => 7,
        BinOp::Lt => 8,
        BinOp::Le => 9,
        BinOp::Gt => 10,
        BinOp::Ge => 11,
        BinOp::And => 12,
        BinOp::Or => 13,
        BinOp::Concat => 14,
    }
}

fn unop_byte(op: UnOp) -> u8 {
    match op {
        UnOp::Neg => 1,
        UnOp::Not => 2,
    }
}

fn mode_byte(mode: Mode) -> u8 {
    match mode {
        Mode::Read => 0,
        Mode::Write => 1,
    }
}

/// How a reference to a member of the component currently being hashed is
/// written. A node that is in neither this map nor the hash table — a reference
/// made before anything about the component is known — falls back to a bare
/// marker that says only "one of us".
pub type ComponentIndices = FxHashMap<usize, u32>;
pub type HashTable = FxHashMap<usize, DefHash>;

pub struct Normalizer<'a> {
    index: &'a ModuleIndex<'a>,
    hashes: &'a HashTable,
    component: &'a ComponentIndices,
    out: Vec<u8>,
    refs: Vec<NodeId>,
    seen: Vec<bool>,
    values: Vec<&'a Symbol>,
    ty_params: Vec<&'a Symbol>,
    row_params: Vec<&'a Symbol>,
}

impl<'a> Normalizer<'a> {
    pub fn new(
        index: &'a ModuleIndex<'a>,
        hashes: &'a HashTable,
        component: &'a ComponentIndices,
    ) -> Self {
        Normalizer {
            index,
            hashes,
            component,
            out: Vec::new(),
            refs: Vec::new(),
            seen: vec![false; index.nodes.len()],
            values: Vec::new(),
            ty_params: Vec::new(),
            row_params: Vec::new(),
        }
    }

    /// The normalized bytes, and the definitions they reference in first-mention
    /// order.
    pub fn finish(self) -> (Vec<u8>, Vec<NodeId>) {
        (self.out, self.refs)
    }

    pub fn node(&mut self, body: NodeBody<'a>) {
        match body {
            NodeBody::Fn(d) => self.fn_def(d),
            NodeBody::Type(d) => self.type_def(d),
            NodeBody::Effect(d) => self.effect_def(d),
        }
    }

    fn tag(&mut self, t: u8) {
        self.out.push(t);
    }

    fn len(&mut self, n: usize) {
        self.out.extend_from_slice(&(n as u32).to_le_bytes());
    }

    fn u32v(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    fn boolv(&mut self, b: bool) {
        self.out.push(u8::from(b));
    }

    fn strv(&mut self, s: &str) {
        self.len(s.len());
        self.out.extend_from_slice(s.as_bytes());
    }

    fn capture(&mut self, f: impl FnOnce(&mut Self)) -> Vec<u8> {
        let saved = std::mem::take(&mut self.out);
        f(self);
        std::mem::replace(&mut self.out, saved)
    }

    fn opt<T: ?Sized>(&mut self, value: Option<&'a T>, f: impl FnOnce(&mut Self, &'a T)) {
        match value {
            None => self.tag(tag::NONE),
            Some(v) => {
                self.tag(tag::SOME);
                f(self, v);
            }
        }
    }

    fn node_ref(&mut self, node: NodeId) {
        if !self.seen[node.0] {
            self.seen[node.0] = true;
            self.refs.push(node);
        }
        if let Some(&ix) = self.component.get(&node.0) {
            self.tag(tag::REF_INDEX);
            self.u32v(ix);
        } else if let Some(hash) = self.hashes.get(&node.0) {
            self.tag(tag::REF_HASH);
            self.out.extend_from_slice(&hash.0);
        } else {
            self.tag(tag::REF_SELF);
        }
    }

    fn value_ref(&mut self, name: &Symbol) {
        if let Some(level) = self.values.iter().rposition(|s| *s == name) {
            self.tag(tag::LOCAL);
            self.u32v(level as u32);
        } else if let Some(&node) = self.index.fns.get(name) {
            self.node_ref(node);
        } else if let Some(&owner) = self.index.ctors.get(name) {
            self.tag(tag::CTOR);
            self.node_ref(owner);
            self.strv(name);
        } else {
            self.tag(tag::FREE);
            self.strv(name);
        }
    }

    fn type_ref(&mut self, name: &Symbol) {
        if let Some(level) = self.ty_params.iter().rposition(|s| *s == name) {
            self.tag(tag::TY_PARAM);
            self.u32v(level as u32);
        } else if let Some(&node) = self.index.types.get(name) {
            self.node_ref(node);
        } else {
            self.tag(tag::FREE);
            self.strv(name);
        }
    }

    /// Effects are nominal: `db` and `audit` may declare byte-identical
    /// operations and are still different capabilities, performed as different
    /// atoms and discharged by different handlers. A declaration alone therefore
    /// cannot identify one, so a reference carries a discriminator too.
    fn effect_ref(&mut self, name: &Symbol) {
        if let Some(&node) = self.index.effects.get(name) {
            self.node_ref(node);
            self.u32v(self.index.effect_ids.get(&node.0).copied().unwrap_or(0));
        } else {
            self.tag(tag::FREE);
            self.strv(name);
        }
    }

    fn ctor_ref(&mut self, name: &Symbol) {
        if let Some(&owner) = self.index.ctors.get(name) {
            self.tag(tag::CTOR);
            self.node_ref(owner);
            self.strv(name);
        } else {
            self.tag(tag::FREE);
            self.strv(name);
        }
    }

    fn fn_def(&mut self, d: &'a FnDef) {
        self.tag(tag::FN);
        self.len(d.generics.types.len());
        self.len(d.generics.effects.len());
        for g in &d.generics.types {
            self.ty_params.push(&g.name);
        }
        for g in &d.generics.effects {
            self.row_params.push(&g.name);
        }
        self.len(d.params.len());
        for p in &d.params {
            self.opt(p.ty.as_ref(), Self::type_expr);
        }
        self.opt(d.ret.as_ref(), Self::type_expr);
        self.opt(d.effects.as_ref(), Self::row);
        for p in &d.params {
            self.values.push(&p.name.name);
        }
        self.expr(&d.body);
    }

    fn type_def(&mut self, d: &'a TypeDef) {
        self.tag(tag::TYPE);
        for p in &d.params {
            self.ty_params.push(&p.name);
        }
        self.len(d.params.len());
        match &d.body {
            TypeDefBody::Alias(t) => {
                self.tag(tag::TYPE_ALIAS);
                self.type_expr(t);
            }
            TypeDefBody::Sum(variants) => {
                self.tag(tag::TYPE_SUM);
                self.len(variants.len());
                for v in variants {
                    self.tag(tag::VARIANT);
                    self.strv(&v.name.name);
                    self.len(v.fields.len());
                    for f in &v.fields {
                        self.type_expr(f);
                    }
                }
            }
        }
    }

    /// Operations are looked up by name, never by position, so their declaration
    /// order carries no meaning and is sorted away.
    fn effect_def(&mut self, d: &'a EffectDef) {
        self.tag(tag::EFFECT);
        self.boolv(d.nondet);
        let mut ops: Vec<Vec<u8>> = d.ops.iter().map(|op| self.capture(|s| s.op_def(op))).collect();
        ops.sort_unstable();
        self.len(ops.len());
        for bytes in ops {
            self.out.extend_from_slice(&bytes);
        }
    }

    fn op_def(&mut self, op: &'a OpDef) {
        self.tag(tag::OP);
        self.strv(&op.name.name);
        self.out.push(mode_byte(op.mode));
        self.boolv(op.resource_param);
        self.len(op.params.len());
        for p in &op.params {
            self.type_expr(p);
        }
        self.type_expr(&op.ret);
    }

    pub fn test_def(&mut self, d: &'a TestDef) {
        self.tag(tag::TEST);
        self.boolv(d.nondet);
        self.expr(&d.body);
    }

    fn type_expr(&mut self, t: &'a TypeExpr) {
        grow(|| self.type_expr_inner(t))
    }

    fn type_expr_inner(&mut self, t: &'a TypeExpr) {
        match t {
            TypeExpr::Var(name) => {
                self.tag(tag::TY_CON);
                self.type_ref(&name.name);
                self.len(0);
            }
            TypeExpr::Con { name, args, .. } => {
                self.tag(tag::TY_CON);
                self.type_ref(&name.name);
                self.len(args.len());
                for a in args {
                    self.type_expr(a);
                }
            }
            TypeExpr::Fn { params, ret, effects, .. } => {
                self.tag(tag::TY_FN);
                self.len(params.len());
                for p in params {
                    self.type_expr(p);
                }
                self.type_expr(ret);
                self.opt(effects.as_ref(), Self::row);
            }
            // A record type is a map from label to type — `{a: Int, b: String}`
            // and `{b: String, a: Int}` are the same type — so field order is
            // sorted away here. A record *expression* keeps its order: its
            // fields are evaluated left to right.
            TypeExpr::Record { fields, .. } => {
                self.tag(tag::TY_RECORD);
                let mut sorted: Vec<Vec<u8>> = fields
                    .iter()
                    .map(|(name, ty)| {
                        self.capture(|s| {
                            s.strv(&name.name);
                            s.type_expr(ty);
                        })
                    })
                    .collect();
                sorted.sort_unstable();
                self.len(sorted.len());
                for bytes in sorted {
                    self.out.extend_from_slice(&bytes);
                }
            }
            TypeExpr::Unit { .. } => self.tag(tag::TY_UNIT),
        }
    }

    /// A row is a set, so its atoms are sorted by their own encoding and
    /// deduplicated before being written: reordering an annotation is as free as
    /// reformatting it.
    fn row(&mut self, r: &'a RowExpr) {
        self.tag(tag::ROW);
        let mut atoms = Vec::with_capacity(r.atoms.len());
        for a in &r.atoms {
            atoms.push(self.capture(|s| s.atom(a)));
        }
        atoms.sort_unstable();
        atoms.dedup();
        self.len(atoms.len());
        for bytes in atoms {
            self.out.extend_from_slice(&bytes);
        }
        match &r.tail {
            None => self.tag(tag::NONE),
            Some(tail) => {
                self.tag(tag::SOME);
                match self.row_params.iter().rposition(|s| **s == tail.name) {
                    Some(level) => {
                        self.tag(tag::ROW_PARAM);
                        self.u32v(level as u32);
                    }
                    None => {
                        self.tag(tag::FREE);
                        self.strv(&tail.name);
                    }
                }
            }
        }
    }

    fn atom(&mut self, a: &'a AtomExpr) {
        self.tag(tag::ATOM);
        self.effect_ref(&a.effect.name);
        self.out.push(mode_byte(a.mode));
        self.opt(a.resource.as_ref(), |s, r| s.strv(&r.name));
    }

    fn expr(&mut self, e: &'a Expr) {
        grow(|| self.expr_inner(e))
    }

    fn expr_inner(&mut self, e: &'a Expr) {
        match &e.kind {
            // `{ e }` and `e` are the same computation; treating them alike is
            // what makes wrapping a body in braces a formatting change.
            ExprKind::Block { stmts, tail: Some(tail) } if stmts.is_empty() => self.expr(tail),
            ExprKind::Lit(l) => {
                self.tag(tag::E_LIT);
                self.lit(l);
            }
            ExprKind::Var(name) => {
                self.tag(tag::E_VAR);
                self.value_ref(&name.name);
            }
            ExprKind::Binary { op, lhs, rhs } => {
                self.tag(tag::E_BINARY);
                self.out.push(binop_byte(*op));
                self.expr(lhs);
                self.expr(rhs);
            }
            ExprKind::Unary { op, operand } => {
                self.tag(tag::E_UNARY);
                self.out.push(unop_byte(*op));
                self.expr(operand);
            }
            ExprKind::Lambda { params, body } => {
                self.tag(tag::E_LAMBDA);
                self.len(params.len());
                for p in params {
                    self.opt(p.ty.as_ref(), Self::type_expr);
                }
                let mark = self.values.len();
                for p in params {
                    self.values.push(&p.name.name);
                }
                self.expr(body);
                self.values.truncate(mark);
            }
            ExprKind::App { func, args } => {
                self.tag(tag::E_APP);
                self.expr(func);
                self.len(args.len());
                for a in args {
                    self.expr(a);
                }
            }
            ExprKind::If { cond, then_branch, else_branch } => {
                self.tag(tag::E_IF);
                self.expr(cond);
                self.expr(then_branch);
                self.expr(else_branch);
            }
            ExprKind::Match { scrutinee, arms } => {
                self.tag(tag::E_MATCH);
                self.expr(scrutinee);
                self.len(arms.len());
                for arm in arms {
                    self.tag(tag::ARM);
                    let mark = self.values.len();
                    self.pattern(&arm.pat);
                    self.opt(arm.guard.as_ref(), Self::expr);
                    self.expr(&arm.body);
                    self.values.truncate(mark);
                }
            }
            ExprKind::Block { stmts, tail } => {
                self.tag(tag::E_BLOCK);
                let mark = self.values.len();
                self.len(stmts.len());
                for i in self.stmt_order(stmts) {
                    self.stmt(&stmts[i]);
                }
                self.opt(tail.as_deref(), Self::expr);
                self.values.truncate(mark);
            }
            ExprKind::Record { fields } => {
                self.tag(tag::E_RECORD);
                self.len(fields.len());
                for (name, value) in fields {
                    self.strv(&name.name);
                    self.expr(value);
                }
            }
            ExprKind::Field { base, field } => {
                self.tag(tag::E_FIELD);
                self.expr(base);
                self.strv(&field.name);
            }
            ExprKind::List { items } => {
                self.tag(tag::E_LIST);
                self.len(items.len());
                for i in items {
                    self.expr(i);
                }
            }
            ExprKind::Perform { effect, op, resource, args } => {
                self.tag(tag::E_PERFORM);
                self.effect_ref(&effect.name);
                self.strv(&op.name);
                self.opt(resource.as_ref(), |s, r| s.strv(&r.name));
                self.len(args.len());
                for a in args {
                    self.expr(a);
                }
            }
            ExprKind::Handle { body, clauses, return_clause } => {
                self.tag(tag::E_HANDLE);
                self.expr(body);
                self.len(clauses.len());
                for c in clauses {
                    self.tag(tag::CLAUSE);
                    self.effect_ref(&c.effect.name);
                    self.strv(&c.op.name);
                    self.opt(c.resource.as_ref(), |s, r| s.strv(&r.name));
                    self.len(c.params.len());
                    let mark = self.values.len();
                    for p in &c.params {
                        self.values.push(&p.name);
                    }
                    self.expr(&c.body);
                    self.values.truncate(mark);
                }
                match return_clause.as_deref() {
                    None => self.tag(tag::NONE),
                    Some(rc) => {
                        self.tag(tag::SOME);
                        self.tag(tag::RETURN_CLAUSE);
                        let mark = self.values.len();
                        self.values.push(&rc.binder.name);
                        self.expr(&rc.body);
                        self.values.truncate(mark);
                    }
                }
            }
            ExprKind::WithCell { resource, init, binder, body } => {
                self.tag(tag::E_WITH_CELL);
                self.strv(&resource.name);
                self.expr(init);
                let mark = self.values.len();
                self.values.push(&binder.name);
                self.expr(body);
                self.values.truncate(mark);
            }
        }
    }

    /// Consecutive `let`s that commute are emitted in the order their own
    /// encodings sort in, so which one the author typed first is not part of the
    /// definition's identity. Everything else stays where it is.
    fn stmt_order(&mut self, stmts: &'a [Stmt]) -> Vec<usize> {
        let mut order = Vec::with_capacity(stmts.len());
        let mut i = 0;
        while i < stmts.len() {
            let run = commutable_run(stmts, i);
            if run < 2 {
                order.push(i);
                i += 1;
            } else {
                order.extend(self.sorted_run(stmts, i, i + run));
                i += run;
            }
        }
        order
    }

    /// The members of the run, ordered by their encodings. Every member encodes
    /// the same way wherever it lands in the run, because none of them can see a
    /// binder the run introduces — that is what [`commutable_run`] establishes.
    fn sorted_run(&mut self, stmts: &'a [Stmt], from: usize, to: usize) -> Vec<usize> {
        let mark = self.values.len();
        let mut keyed: Vec<(Vec<u8>, usize)> = (from..to)
            .map(|i| {
                let key = self.capture(|s| s.stmt(&stmts[i]));
                self.values.truncate(mark);
                (key, i)
            })
            .collect();
        keyed.sort_unstable();
        keyed.into_iter().map(|(_, i)| i).collect()
    }

    fn stmt(&mut self, s: &'a Stmt) {
        match s {
            Stmt::Let { pat, ty, value, .. } => {
                self.tag(tag::S_LET);
                self.opt(ty.as_ref(), Self::type_expr);
                self.expr(value);
                self.pattern(pat);
            }
            Stmt::Expr(e) => {
                self.tag(tag::S_EXPR);
                self.expr(e);
            }
        }
    }

    fn lit(&mut self, l: &Lit) {
        match l {
            Lit::Int(i) => {
                self.tag(tag::LIT_INT);
                self.out.extend_from_slice(&i.to_le_bytes());
            }
            Lit::Bool(b) => {
                self.tag(tag::LIT_BOOL);
                self.boolv(*b);
            }
            Lit::Str(s) => {
                self.tag(tag::LIT_STR);
                self.strv(s);
            }
            Lit::Unit => self.tag(tag::LIT_UNIT),
        }
    }

    fn pattern(&mut self, p: &'a Pattern) {
        grow(|| self.pattern_inner(p))
    }

    fn pattern_inner(&mut self, p: &'a Pattern) {
        match &p.kind {
            PatternKind::Wildcard => self.tag(tag::P_WILDCARD),
            PatternKind::Var(name) => {
                self.tag(tag::P_VAR);
                self.values.push(&name.name);
            }
            PatternKind::Lit(l) => {
                self.tag(tag::P_LIT);
                self.lit(l);
            }
            PatternKind::Ctor { name, args } => {
                self.tag(tag::P_CTOR);
                self.ctor_ref(&name.name);
                self.len(args.len());
                for a in args {
                    self.pattern(a);
                }
            }
            PatternKind::Record { fields, rest } => {
                self.tag(tag::P_RECORD);
                self.len(fields.len());
                for (name, pat) in fields {
                    self.strv(&name.name);
                    self.pattern(pat);
                }
                self.boolv(*rest);
            }
            PatternKind::List { items, rest } => {
                self.tag(tag::P_LIST);
                self.len(items.len());
                for i in items {
                    self.pattern(i);
                }
                self.opt(rest.as_deref(), Self::pattern);
            }
        }
    }
}

/// Effects that declare exactly the same operations are told apart by their rank
/// in name order among that group of look-alikes. Nothing else can separate
/// them: their declarations are byte-identical by construction, and source
/// position would make moving an item change hashes. Almost every effect is the
/// only one of its shape, gets rank 0, and so stays free to rename.
pub fn effect_disambiguators(index: &ModuleIndex<'_>) -> FxHashMap<usize, u32> {
    let no_hashes = HashTable::default();
    let no_component = ComponentIndices::default();
    let mut groups: BTreeMap<Vec<u8>, Vec<(&Symbol, usize)>> = BTreeMap::new();
    for (v, node) in index.nodes.iter().enumerate() {
        if matches!(node.body, NodeBody::Effect(_)) {
            let mut nz = Normalizer::new(index, &no_hashes, &no_component);
            nz.node(node.body);
            groups.entry(nz.finish().0).or_default().push((node.name, v));
        }
    }

    let mut out = FxHashMap::default();
    for members in groups.values_mut() {
        members.sort_unstable();
        for (rank, (_, v)) in members.iter().enumerate() {
            out.insert(*v, rank as u32);
        }
    }
    out
}

/// How many statements from `from` are `let`s that may be written in any order
/// without changing what the block does. Two conditions make that safe: no value
/// can call or perform, so every one of them is evaluated whatever the order and
/// none can observe another's effect; and no value mentions a name the run
/// binds, so no reordering can change what any of them resolves to.
fn commutable_run(stmts: &[Stmt], from: usize) -> usize {
    let mut to = from;
    while let Some(Stmt::Let { value, .. }) = stmts.get(to) {
        if !is_pure(value) {
            break;
        }
        to += 1;
    }
    if to - from < 2 {
        return 0;
    }

    let mut bound: FxHashSet<Symbol> = FxHashSet::default();
    let mut count = 0;
    for s in &stmts[from..to] {
        if let Stmt::Let { pat, .. } = s {
            count += pattern_binders(pat, &mut bound);
        }
    }
    if count != bound.len() {
        return 0;
    }
    for s in &stmts[from..to] {
        if let Stmt::Let { value, .. } = s
            && mentions(value, &bound)
        {
            return 0;
        }
    }
    to - from
}

fn pattern_binders(p: &Pattern, out: &mut FxHashSet<Symbol>) -> usize {
    grow(|| match &p.kind {
        PatternKind::Var(name) => {
            out.insert(name.name.clone());
            1
        }
        PatternKind::Wildcard | PatternKind::Lit(_) => 0,
        PatternKind::Ctor { args, .. } => args.iter().map(|a| pattern_binders(a, out)).sum(),
        PatternKind::Record { fields, .. } => {
            fields.iter().map(|(_, pat)| pattern_binders(pat, out)).sum()
        }
        PatternKind::List { items, rest } => {
            let mut n: usize = items.iter().map(|i| pattern_binders(i, out)).sum();
            if let Some(rest) = rest {
                n += pattern_binders(rest, out);
            }
            n
        }
    })
}

/// Evaluates without calling anything and without performing anything, so it
/// cannot diverge and cannot be observed by, or observe, its neighbours. It can
/// still fail — on overflow, on a divisor of zero, on an unmatched scrutinee —
/// but a failure that happens in one order happens in every order, because a
/// block evaluates all of its `let`s regardless.
fn is_pure(e: &Expr) -> bool {
    grow(|| match &e.kind {
        ExprKind::App { .. }
        | ExprKind::Perform { .. }
        | ExprKind::Handle { .. }
        | ExprKind::WithCell { .. } => false,
        ExprKind::Lit(_) | ExprKind::Var(_) => true,
        ExprKind::Binary { lhs, rhs, .. } => is_pure(lhs) && is_pure(rhs),
        ExprKind::Unary { operand, .. } => is_pure(operand),
        ExprKind::Lambda { body, .. } => is_pure(body),
        ExprKind::If { cond, then_branch, else_branch } => {
            is_pure(cond) && is_pure(then_branch) && is_pure(else_branch)
        }
        ExprKind::Match { scrutinee, arms } => {
            is_pure(scrutinee)
                && arms
                    .iter()
                    .all(|a| is_pure(&a.body) && a.guard.as_ref().is_none_or(is_pure))
        }
        ExprKind::Block { stmts, tail } => {
            stmts.iter().all(|s| match s {
                Stmt::Let { value, .. } => is_pure(value),
                Stmt::Expr(e) => is_pure(e),
            }) && tail.as_deref().is_none_or(is_pure)
        }
        ExprKind::Record { fields } => fields.iter().all(|(_, v)| is_pure(v)),
        ExprKind::Field { base, .. } => is_pure(base),
        ExprKind::List { items } => items.iter().all(is_pure),
    })
}

/// Deliberately blind to scope: any occurrence of the name counts, even one that
/// a nested binder would have shadowed. Over-reporting only costs a reordering
/// that would have been safe.
fn mentions(e: &Expr, names: &FxHashSet<Symbol>) -> bool {
    grow(|| match &e.kind {
        ExprKind::Lit(_) => false,
        ExprKind::Var(name) => names.contains(&name.name),
        ExprKind::Binary { lhs, rhs, .. } => mentions(lhs, names) || mentions(rhs, names),
        ExprKind::Unary { operand, .. } => mentions(operand, names),
        ExprKind::Lambda { params, body } => {
            params.iter().any(|p| names.contains(&p.name.name)) || mentions(body, names)
        }
        ExprKind::App { func, args } => {
            mentions(func, names) || args.iter().any(|a| mentions(a, names))
        }
        ExprKind::If { cond, then_branch, else_branch } => {
            mentions(cond, names) || mentions(then_branch, names) || mentions(else_branch, names)
        }
        ExprKind::Match { scrutinee, arms } => {
            mentions(scrutinee, names)
                || arms.iter().any(|a| {
                    pattern_mentions(&a.pat, names)
                        || a.guard.as_ref().is_some_and(|g| mentions(g, names))
                        || mentions(&a.body, names)
                })
        }
        ExprKind::Block { stmts, tail } => {
            stmts.iter().any(|s| match s {
                Stmt::Let { pat, value, .. } => {
                    pattern_mentions(pat, names) || mentions(value, names)
                }
                Stmt::Expr(e) => mentions(e, names),
            }) || tail.as_deref().is_some_and(|t| mentions(t, names))
        }
        ExprKind::Record { fields } => fields.iter().any(|(_, v)| mentions(v, names)),
        ExprKind::Field { base, .. } => mentions(base, names),
        ExprKind::List { items } => items.iter().any(|i| mentions(i, names)),
        ExprKind::Perform { args, .. } => args.iter().any(|a| mentions(a, names)),
        ExprKind::Handle { body, clauses, return_clause } => {
            mentions(body, names)
                || clauses.iter().any(|c| {
                    c.params.iter().any(|p| names.contains(&p.name)) || mentions(&c.body, names)
                })
                || return_clause.as_deref().is_some_and(|r| {
                    names.contains(&r.binder.name) || mentions(&r.body, names)
                })
        }
        ExprKind::WithCell { init, binder, body, .. } => {
            names.contains(&binder.name) || mentions(init, names) || mentions(body, names)
        }
    })
}

fn pattern_mentions(p: &Pattern, names: &FxHashSet<Symbol>) -> bool {
    grow(|| match &p.kind {
        PatternKind::Wildcard | PatternKind::Lit(_) => false,
        PatternKind::Var(name) => names.contains(&name.name),
        PatternKind::Ctor { name, args } => {
            names.contains(&name.name) || args.iter().any(|a| pattern_mentions(a, names))
        }
        PatternKind::Record { fields, .. } => {
            fields.iter().any(|(_, pat)| pattern_mentions(pat, names))
        }
        PatternKind::List { items, rest } => {
            items.iter().any(|i| pattern_mentions(i, names))
                || rest.as_deref().is_some_and(|r| pattern_mentions(r, names))
        }
    })
}
