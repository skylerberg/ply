//! Normalized serialization of a definition.

use ply_span::Symbol;
use ply_syntax::ast::*;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::DefHash;
use crate::graph::{NodeBody, NodeId, ProgramIndex, ValueTarget};

fn qualifier(q: &QName) -> Option<&Symbol> {
    q.module.as_ref().map(|m| &m.name)
}

/// The parser's nesting limit does not bound this walk: a left-leaning operator chain is parsed
/// iteratively at constant depth but is still an arbitrarily deep tree.
pub(crate) fn grow<R>(f: impl FnOnce() -> R) -> R {
    const RED_ZONE: usize = 256 * 1024;
    const NEW_SEGMENT: usize = 2 * 1024 * 1024;
    stacker::maybe_grow(RED_ZONE, NEW_SEGMENT, f)
}

pub(crate) mod tag {
    pub const LOCAL: u8 = 1;
    pub const REF_HASH: u8 = 2;
    pub const REF_SELF: u8 = 3;
    pub const REF_INDEX: u8 = 4;
    pub const FREE: u8 = 5;
    pub const FREE_QUALIFIED: u8 = 9;
    /// Only ever written by a [`super::Normalizer::by_name`] encoding, which no definition hash is
    /// derived from.
    pub const REF_NAME: u8 = 12;
    pub const CTOR: u8 = 6;
    pub const TY_PARAM: u8 = 7;
    pub const ROW_PARAM: u8 = 8;

    pub const NONE: u8 = 10;
    pub const SOME: u8 = 11;
    /// A `fn` parameter carrying a default, written *in place of* the [`NONE`]/[`SOME`] its type
    /// annotation would otherwise open with, and followed by that annotation and the default
    /// expression.
    pub const PARAM_DEFAULT: u8 = 13;

    pub const FN: u8 = 20;
    pub const TYPE_ALIAS: u8 = 21;
    pub const TYPE_SUM: u8 = 22;
    pub const EFFECT: u8 = 23;
    pub const TEST: u8 = 24;
    pub const VARIANT: u8 = 25;
    pub const OP: u8 = 26;
    pub const TYPE: u8 = 27;
    pub const LAW: u8 = 28;
    pub const SPEC: u8 = 29;

    pub const TY_CON: u8 = 30;
    pub const TY_FN: u8 = 31;
    pub const TY_RECORD: u8 = 32;
    pub const TY_UNIT: u8 = 33;

    pub const LIT_INT: u8 = 40;
    pub const LIT_BOOL: u8 = 41;
    pub const LIT_STR: u8 = 42;
    pub const LIT_UNIT: u8 = 43;
    /// A distinct tag from [`LIT_STR`], so `b"ab"` and `"ab"` are different definitions.
    pub const LIT_BYTES: u8 = 44;
    /// The IEEE **bit pattern**, eight bytes.
    pub const LIT_FLOAT: u8 = 45;
    /// Mantissa then scale.
    pub const LIT_DECIMAL: u8 = 46;

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
    pub const E_SIMULATE: u8 = 65;
    pub const E_WITH_REGION: u8 = 66;

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
    /// A `where derivable(D, a)`.
    pub const CONSTRAINT: u8 = 95;
}

/// An explicit table rather than a discriminant cast: appending a row is free, renumbering one
/// invalidates every cached result in every store.
pub(crate) fn binop_byte(op: BinOp) -> u8 {
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
        BinOp::BitAnd => 15,
        BinOp::BitOr => 16,
        BinOp::BitXor => 17,
        BinOp::Shl => 18,
        BinOp::Shr => 19,
        BinOp::Ushr => 20,
    }
}

pub(crate) fn unop_byte(op: UnOp) -> u8 {
    match op {
        UnOp::Neg => 1,
        UnOp::Not => 2,
        UnOp::BitNot => 3,
    }
}

pub(crate) fn mode_byte(mode: Mode) -> u8 {
    match mode {
        Mode::Read => 0,
        Mode::Write => 1,
    }
}

/// How a reference to a member of the component currently being hashed is written.
pub type ComponentIndices = FxHashMap<usize, u32>;
pub type HashTable = FxHashMap<usize, DefHash>;

/// Effect node -> its slot in the enumeration built for the component being encoded.
pub type EffectIndex = FxHashMap<usize, u32>;

/// `'a` is the AST's lifetime and `'t` the tables'.
pub struct Normalizer<'a, 't> {
    index: &'a ProgramIndex<'a>,
    /// Which module's bodies are being walked.
    module: usize,
    hashes: &'t HashTable,
    component: &'t ComponentIndices,
    effects: &'t EffectIndex,
    out: Vec<u8>,
    refs: Vec<NodeId>,
    seen: Vec<bool>,
    values: Vec<&'a Symbol>,
    ty_params: Vec<&'a Symbol>,
    row_params: Vec<&'a Symbol>,
    /// Encode a reference as the referent's program-wide **name** rather than its hash.
    refs_by_name: bool,
    /// While a row's atoms are being encoded for sorting, the references they mention are held here
    /// instead of joining [`Self::refs`].
    deferred: Option<Vec<NodeId>>,
}

impl<'a, 't> Normalizer<'a, 't> {
    pub fn new(
        index: &'a ProgramIndex<'a>,
        module: usize,
        hashes: &'t HashTable,
        component: &'t ComponentIndices,
        effects: &'t EffectIndex,
    ) -> Self {
        Normalizer {
            index,
            module,
            hashes,
            component,
            effects,
            out: Vec::new(),
            refs: Vec::new(),
            seen: vec![false; index.nodes.len()],
            values: Vec::new(),
            ty_params: Vec::new(),
            row_params: Vec::new(),
            refs_by_name: false,
            deferred: None,
        }
    }

    /// Encode references by name instead of by hash.
    pub fn by_name(mut self) -> Self {
        self.refs_by_name = true;
        self
    }

    /// The normalized bytes, and the definitions they reference in first-mention order.
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

    /// Records a reference in first-mention order, unless a row is holding its mentions back until
    /// it knows the order its atoms sort into.
    fn mention(&mut self, node: NodeId) {
        if let Some(held) = &mut self.deferred {
            held.push(node);
            return;
        }
        if !self.seen[node.0] {
            self.seen[node.0] = true;
            self.refs.push(node);
        }
    }

    fn node_ref(&mut self, node: NodeId) {
        self.mention(node);
        if self.refs_by_name {
            self.tag(tag::REF_NAME);
            let name = self.index.nodes[node.0].name.clone();
            self.strv(&name);
            return;
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

    /// A name nothing in the index answers to.
    fn free_ref(&mut self, qual: Option<&Symbol>, name: &Symbol) {
        match qual {
            None => {
                self.tag(tag::FREE);
                self.strv(name);
            }
            Some(module) => {
                self.tag(tag::FREE_QUALIFIED);
                self.strv(module);
                self.strv(name);
            }
        }
    }

    fn ctor_bytes(&mut self, owner: NodeId, name: &Symbol) {
        self.tag(tag::CTOR);
        self.node_ref(owner);
        self.strv(name);
    }

    /// A local binder wins over everything, and only a bare name can be one: a module binder is a
    /// fourth namespace reachable only through `::`.
    fn value_ref(&mut self, q: &QName) {
        if q.is_bare()
            && let Some(level) = self.values.iter().rposition(|s| **s == q.name.name)
        {
            self.tag(tag::LOCAL);
            self.u32v(level as u32);
            return;
        }
        match self.index.value(self.module, q) {
            Some(ValueTarget::Fn(node)) => self.node_ref(node),
            Some(ValueTarget::Ctor { owner, name }) => self.ctor_bytes(owner, &name),
            None => self.free_ref(qualifier(q), &q.name.name),
        }
    }

    fn type_ref(&mut self, qual: Option<&Symbol>, name: &Symbol) {
        if qual.is_none()
            && let Some(level) = self.ty_params.iter().rposition(|s| **s == *name)
        {
            self.tag(tag::TY_PARAM);
            self.u32v(level as u32);
        } else if let Some(node) = self.index.ty(self.module, qual, name) {
            self.node_ref(node);
        } else {
            self.free_ref(qual, name);
        }
    }

    /// Effects are nominal: `db` and `audit` may declare byte-identical operations and are still
    /// different capabilities, performed as different atoms and discharged by different handlers.
    fn effect_ref(&mut self, q: &QName) {
        if let Some(node) = self.index.effect(self.module, q) {
            self.node_ref(node);
            self.u32v(self.effects.get(&node.0).copied().unwrap_or(0));
        } else {
            self.free_ref(qualifier(q), &q.name.name);
        }
    }

    fn ctor_ref(&mut self, q: &QName) {
        match self.index.ctor(self.module, q) {
            Some(ValueTarget::Ctor { owner, name }) => self.ctor_bytes(owner, &name),
            _ => self.free_ref(qualifier(q), &q.name.name),
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
        // Before the parameter names reach `self.values`, which is what a default must not see: it
        // is closed, and a mention of a sibling parameter is `E0121` rather than a de Bruijn level.
        for p in &d.params {
            if let Some(default) = &p.default {
                self.tag(tag::PARAM_DEFAULT);
                self.opt(p.ty.as_ref(), Self::type_expr);
                self.expr(default);
            } else {
                self.opt(p.ty.as_ref(), Self::type_expr);
            }
        }
        self.opt(d.ret.as_ref(), Self::type_expr);
        self.opt(d.effects.as_ref(), Self::row);
        self.constraints(&d.constraints);
        for p in &d.params {
            self.values.push(&p.name.name);
        }
        self.expr(&d.body);
    }

    /// Sorted and deduplicated, so that writing the same two constraints in the other order is the
    /// same definition.
    fn constraints(&mut self, cs: &'a [Constraint]) {
        let mut levels: Vec<(u32, u8)> = cs
            .iter()
            .filter_map(|c| {
                let level = self.ty_params.iter().rposition(|p| **p == c.param.name)?;
                Some((level as u32, c.deriver.tag()))
            })
            .collect();
        levels.sort_unstable();
        levels.dedup();
        self.len(levels.len());
        for (level, deriver) in levels {
            self.tag(tag::CONSTRAINT);
            self.u32v(level);
            self.tag(deriver);
        }
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

    /// Operations are looked up by name, never by position, so their declaration order carries no
    /// meaning and is sorted away.
    fn effect_def(&mut self, d: &'a EffectDef) {
        self.tag(tag::EFFECT);
        self.boolv(d.nondet);
        let mut ops: Vec<Vec<u8>> = d
            .ops
            .iter()
            .map(|op| self.capture(|s| s.op_def(op)))
            .collect();
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

    /// A law's binder *types* are part of its identity — they are what the claim quantifies over —
    /// while their names are levels like any other binder, and the label is erased exactly as a
    /// test's is.
    pub fn law_def(&mut self, d: &'a LawDef) {
        self.tag(tag::LAW);
        self.boolv(d.host);
        self.len(d.binders.len());
        for b in &d.binders {
            self.type_expr(&b.ty);
        }
        for b in &d.binders {
            self.values.push(&b.name.name);
        }
        self.opt(d.guard.as_ref(), Self::expr);
        self.expr(&d.body);
    }

    /// One `requires` or `ensures`, in the scope its owner gives it: the owner's generics and
    /// parameters, plus `result` for an `ensures` — introduced **beside** the parameters, which is
    /// what makes a parameter named `result` a duplicate rather than a shadow.
    pub fn spec_clause(&mut self, owner: &'a FnDef, clause: &'a SpecClause) {
        let index = self.index;
        self.tag(tag::SPEC);
        self.out.push(clause.kind.tag());
        self.len(owner.generics.types.len());
        self.len(owner.generics.effects.len());
        for g in &owner.generics.types {
            self.ty_params.push(&g.name);
        }
        for g in &owner.generics.effects {
            self.row_params.push(&g.name);
        }
        // A parameter's default is not written here, and that is the claim: an obligation is about
        // what the body promises, and a default changes what *callers* pass rather than what the
        // promise says.
        self.len(owner.params.len());
        for p in &owner.params {
            self.values.push(&p.name.name);
        }
        if clause.kind == SpecKind::Ensures {
            self.values.push(&index.result);
        }
        self.expr(&clause.expr);
    }

    fn type_expr(&mut self, t: &'a TypeExpr) {
        grow(|| self.type_expr_inner(t))
    }

    fn type_expr_inner(&mut self, t: &'a TypeExpr) {
        match t {
            TypeExpr::Var(name) => {
                self.tag(tag::TY_CON);
                self.type_ref(None, &name.name);
                self.len(0);
            }
            TypeExpr::Con { name, args, .. } => {
                self.tag(tag::TY_CON);
                let qual = name.module.as_ref().map(|m| m.name.clone());
                self.type_ref(qual.as_ref(), &name.name.name);
                self.len(args.len());
                for a in args {
                    self.type_expr(a);
                }
            }
            TypeExpr::Fn {
                params,
                ret,
                effects,
                ..
            } => {
                self.tag(tag::TY_FN);
                self.len(params.len());
                for p in params {
                    self.type_expr(p);
                }
                self.type_expr(ret);
                self.opt(effects.as_ref(), Self::row);
            }
            // A record type is a map from label to type — `{a: Int, b: String}` and `{b: String, a:
            // Int}` are the same type — so field order is sorted away here.
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

    /// A row is a set, so its atoms are sorted by their own encoding and deduplicated before being
    /// written: reordering an annotation is as free as reformatting it.
    fn row(&mut self, r: &'a RowExpr) {
        self.tag(tag::ROW);
        let mut atoms: Vec<(Vec<u8>, &[u8], Vec<NodeId>)> = Vec::with_capacity(r.atoms.len());
        for a in &r.atoms {
            let outer = self.deferred.replace(Vec::new());
            let bytes = self.capture(|s| s.atom(a));
            let held = self.deferred.take().unwrap_or_default();
            self.deferred = outer;
            let tie = self
                .index
                .effect(self.module, &a.effect)
                .and_then(|node| self.index.effect_sketch(node))
                .unwrap_or_default();
            atoms.push((bytes, tie, held));
        }
        // By the bytes, then by the declaration the atom's effect names.
        atoms.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));
        let distinct =
            usize::from(!atoms.is_empty()) + atoms.windows(2).filter(|w| w[0].0 != w[1].0).count();
        self.len(distinct);
        // Every atom is mentioned, including one the dedup drops.
        let mut previous: Option<Vec<u8>> = None;
        for (bytes, _, held) in atoms {
            for node in held {
                self.mention(node);
            }
            if previous.as_deref() == Some(bytes.as_slice()) {
                continue;
            }
            self.out.extend_from_slice(&bytes);
            previous = Some(bytes);
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
        self.effect_ref(&a.effect);
        self.out.push(mode_byte(a.mode));
        self.opt(a.resource.as_ref(), |s, r| s.strv(&r.name));
    }

    fn expr(&mut self, e: &'a Expr) {
        grow(|| self.expr_inner(e))
    }

    fn expr_inner(&mut self, e: &'a Expr) {
        match &e.kind {
            // `{ e }` and `e` are the same computation; treating them alike is what makes wrapping
            // a body in braces a formatting change.
            ExprKind::Block {
                stmts,
                tail: Some(tail),
            } if stmts.is_empty() => self.expr(tail),
            ExprKind::Lit(l) => {
                self.tag(tag::E_LIT);
                self.lit(l);
            }
            ExprKind::Var(name) => {
                self.tag(tag::E_VAR);
                self.value_ref(name);
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
            ExprKind::App { func, args, .. } => {
                self.tag(tag::E_APP);
                self.expr(func);
                self.len(args.len());
                for a in args {
                    self.expr(a);
                }
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
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
            // The one arm that must never guess.
            ExprKind::RecordUpdate { .. } => unreachable!(
                "`{{..b, f: e}}` is expanded away by `ply_syntax::parse_module`; the guard is \
                 `no_record_update_survives_parse_module_anywhere_in_the_tree`"
            ),
            // The other arm that must never guess.
            ExprKind::Try { .. } => unreachable!(
                "`e?` is expanded away by `ply_syntax::parse_module`; the guard is \
                 `no_try_survives_parse_module_anywhere_in_the_tree`"
            ),
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
            ExprKind::Perform {
                effect,
                op,
                resource,
                args,
            } => {
                self.tag(tag::E_PERFORM);
                self.effect_ref(effect);
                self.strv(&op.name);
                self.opt(resource.as_ref(), |s, r| s.strv(&r.name));
                self.len(args.len());
                for a in args {
                    self.expr(a);
                }
            }
            ExprKind::Handle {
                body,
                clauses,
                return_clause,
            } => {
                self.tag(tag::E_HANDLE);
                self.expr(body);
                self.len(clauses.len());
                for c in clauses {
                    self.tag(tag::CLAUSE);
                    self.effect_ref(&c.effect);
                    self.strv(&c.op.name);
                    self.opt(c.resource.as_ref(), |s, r| s.strv(&r.name));
                    self.len(c.params.len());
                    let mark = self.values.len();
                    for p in &c.params {
                        self.values.push(&p.name);
                    }
                    // Whether a clause binds a continuation is part of what the definition *is* —
                    // the two forms have different typing and different semantics — so the marker
                    // is hashed.
                    match &c.resume {
                        None => self.tag(tag::NONE),
                        Some(binder) => {
                            self.tag(tag::SOME);
                            self.values.push(&binder.name);
                        }
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
            ExprKind::WithCell {
                resource,
                init,
                binder,
                body,
            } => {
                self.tag(tag::E_WITH_CELL);
                self.strv(&resource.name);
                self.expr(init);
                let mark = self.values.len();
                self.values.push(&binder.name);
                self.expr(body);
                self.values.truncate(mark);
            }
            ExprKind::WithRegion { region, body } => {
                self.tag(tag::E_WITH_REGION);
                self.strv(&region.name);
                self.expr(body);
            }
            ExprKind::Simulate { body } => {
                self.tag(tag::E_SIMULATE);
                self.expr(body);
            }
        }
    }

    /// Consecutive `let`s that commute are emitted in the order their own encodings sort in, so
    /// which one the author typed first is not part of the definition's identity.
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

    /// The members of the run, ordered by their encodings.
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
            Lit::Bytes(b) => {
                self.tag(tag::LIT_BYTES);
                self.len(b.len());
                self.out.extend_from_slice(b);
            }
            Lit::Float(f) => {
                self.tag(tag::LIT_FLOAT);
                self.out.extend_from_slice(&f.to_bits().to_le_bytes());
            }
            Lit::Decimal { mantissa, scale } => {
                self.tag(tag::LIT_DECIMAL);
                self.out.extend_from_slice(&mantissa.to_le_bytes());
                self.out.extend_from_slice(&scale.to_le_bytes());
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
                self.ctor_ref(name);
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

/// How many statements from `from` are `let`s that may be written in any order without changing
/// what the block does.
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
        PatternKind::Record { fields, .. } => fields
            .iter()
            .map(|(_, pat)| pattern_binders(pat, out))
            .sum(),
        PatternKind::List { items, rest } => {
            let mut n: usize = items.iter().map(|i| pattern_binders(i, out)).sum();
            if let Some(rest) = rest {
                n += pattern_binders(rest, out);
            }
            n
        }
    })
}

// `is_pure` used to live here.

/// Deliberately blind to scope: any occurrence of the name counts, even one that a nested binder
/// would have shadowed.
fn mentions(e: &Expr, names: &FxHashSet<Symbol>) -> bool {
    grow(|| match &e.kind {
        ExprKind::Lit(_) => false,
        ExprKind::Var(name) => name.is_bare() && names.contains(&name.name.name),
        ExprKind::Binary { lhs, rhs, .. } => mentions(lhs, names) || mentions(rhs, names),
        ExprKind::Unary { operand, .. } => mentions(operand, names),
        ExprKind::Lambda { params, body } => {
            params.iter().any(|p| names.contains(&p.name.name)) || mentions(body, names)
        }
        ExprKind::App { func, args, .. } => {
            mentions(func, names) || args.iter().any(|a| mentions(a, names))
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => mentions(cond, names) || mentions(then_branch, names) || mentions(else_branch, names),
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
        ExprKind::RecordUpdate { base, fields } => {
            mentions(base, names) || fields.iter().any(|(_, v)| mentions(v, names))
        }
        ExprKind::Field { base, .. } => mentions(base, names),
        ExprKind::Try { operand } => mentions(operand, names),
        ExprKind::List { items } => items.iter().any(|i| mentions(i, names)),
        ExprKind::Perform { args, .. } => args.iter().any(|a| mentions(a, names)),
        ExprKind::Handle {
            body,
            clauses,
            return_clause,
        } => {
            mentions(body, names)
                || clauses.iter().any(|c| {
                    c.params.iter().any(|p| names.contains(&p.name)) || mentions(&c.body, names)
                })
                || return_clause
                    .as_deref()
                    .is_some_and(|r| names.contains(&r.binder.name) || mentions(&r.body, names))
        }
        ExprKind::WithCell {
            init, binder, body, ..
        } => names.contains(&binder.name) || mentions(init, names) || mentions(body, names),
        ExprKind::WithRegion { body, .. } => mentions(body, names),
        ExprKind::Simulate { body } => mentions(body, names),
    })
}

fn pattern_mentions(p: &Pattern, names: &FxHashSet<Symbol>) -> bool {
    grow(|| match &p.kind {
        PatternKind::Wildcard | PatternKind::Lit(_) => false,
        PatternKind::Var(name) => names.contains(&name.name),
        PatternKind::Ctor { name, args } => {
            (name.is_bare() && names.contains(&name.name.name))
                || args.iter().any(|a| pattern_mentions(a, names))
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
