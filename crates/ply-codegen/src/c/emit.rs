//! The emitter: one `Code` body in, one C function out.
//!
//! It reuses the Cranelift tier's whole analysis — `Jit` holds the constant pool, the shapes, the
//! field and builtin tables and the published signatures, and `Jit::prepare` has already built
//! them — so this file is the *code* and nothing else, which is ADR 0037's "one code generator
//! serving both tiers" taken literally.
//!
//! **Ownership is deliberately conservative here and is the first thing to revisit.** A value
//! passed where a runtime helper takes one is duplicated first, and nothing is ever released: the
//! entry's arena is recycled whole when the entry ends (`Ctx::end`), so a body that never
//! decrements leaks only within one entry and can never free something still held. It costs the
//! token reuse the Cranelift tier has, and it cannot be wrong.

use crate::jit::{Kind, Refused};
use crate::source::Source;
use anyhow::Result;
use ply_eval::code::{Arm, Pat, Stmt};
use ply_eval::rc::Own;
use ply_eval::{Builtin, Code, NodeKind, Value};
use ply_span::Symbol;
use ply_syntax::ast::{BinOp, IntTy, Lit, QName, UnOp};

/// A value the emitted C holds: the C expression naming it, what it means, and what the checker
/// said its type is — which is how a width survives a record field, and without which `rotr` on a
/// `U32` read out of one dispatches at `Int` and turns sixty-four bits.
#[derive(Clone)]
pub struct V {
    pub k: Kind,
    pub c: String,
    pub ty: CTy,
}

impl V {
    fn boxed(c: impl Into<String>) -> V {
        V {
            k: Kind::Boxed,
            c: c.into(),
            ty: CTy::Unknown,
        }
    }
}

/// What the emitter keeps of a type the checker published. The Cranelift tier interns the same
/// thing; here it is carried by value, since a record's field list is short and there is no
/// module to hold a table for.
#[derive(Clone, PartialEq, Debug)]
pub enum CTy {
    Unknown,
    Int,
    Bool,
    Num(IntTy),
    /// Carried so that the builtins reading one need no kind test: the checker said it is a
    /// `Bytes`, the seam refuses anything a carried type does not denote, and a `Bytes` in
    /// compiled code is `KIND_BYTES` or it came from somewhere this fragment cannot reach.
    Bytes,
    List,
    Record(Vec<(Symbol, CTy)>),
    /// A type the checker fixed that this tier will not read as a number: a `Float`, a `Decimal`,
    /// and the two widths past the immediate.
    ///
    /// Distinct from `Unknown`, and the distinction is load-bearing. `Unknown` is the emitter
    /// having lost the type; this is the type being one the emitter must not guess `Int` for. An
    /// operator may refuse on this and must not refuse on that -- refusing on `Unknown` took down
    /// every body with a `match` arm binding a payload, which is most of them.
    Opaque,
}

impl CTy {
    /// The type the checker published, as much of it as the emitter uses.
    pub fn of(t: &ply_core::ty::Type) -> CTy {
        use ply_core::ty::Type;
        match t {
            Type::Con(name, args) if args.is_empty() => match name.as_str() {
                "Int" => CTy::Int,
                "Bool" => CTy::Bool,
                "Bytes" => CTy::Bytes,
                "Float" | "Decimal" => CTy::Opaque,
                // A width past sixty-two bits is not an immediate, so carrying one in a register
                // would need a heap object of its own kind and a test for it -- the cost the
                // family exists to remove. `jit::carried_width` draws the line in the same place
                // and for the same reason; a `U64` left in a register here loses its top bit to
                // the tag on the way into a record.
                other => match IntTy::from_name(other) {
                    Some(t) if t.bits() < 64 => CTy::Num(t),
                    Some(_) => CTy::Opaque,
                    None => CTy::Unknown,
                },
            },
            Type::Con(name, args) if name.as_str() == "List" && args.len() == 1 => CTy::List,
            Type::Record(fields) => CTy::Record(
                fields
                    .iter()
                    .map(|(n, t)| (n.clone(), CTy::of(t)))
                    .collect(),
            ),
            _ => CTy::Unknown,
        }
    }

    fn kind(&self) -> Kind {
        match self {
            CTy::Int => Kind::Int,
            CTy::Bool => Kind::Bool,
            CTy::Num(t) => Kind::Num(*t),
            _ => Kind::Boxed,
        }
    }

    fn field(&self, name: &Symbol) -> Option<&CTy> {
        match self {
            CTy::Record(fields) => fields.iter().find(|(n, _)| n == name).map(|(_, t)| t),
            _ => None,
        }
    }

    /// Where a field sits in the record, when the type says. `Type::Record` is a `BTreeMap`, so
    /// its order is the sorted name order the shape is interned in.
    fn offset(&self, name: &Symbol) -> Option<usize> {
        match self {
            CTy::Record(fields) => fields.iter().position(|(n, _)| n == name),
            _ => None,
        }
    }
}

/// The C type a kind is held in. A width is held in its own unsigned type and cast where the
/// signedness matters, because signed overflow is undefined in C and the wrapping builtins are
/// defined to wrap.
pub fn ctype(k: Kind) -> &'static str {
    match k {
        Kind::Boxed | Kind::Int => "int64_t",
        Kind::Bool => "int64_t",
        Kind::Num(t) => match (t.bits(), t.signed()) {
            (8, false) => "uint8_t",
            (16, false) => "uint16_t",
            (32, false) => "uint32_t",
            (8, true) => "int8_t",
            (16, true) => "int16_t",
            _ => "int32_t",
        },
    }
}

/// The unsigned C type of a width, which every operation defined to wrap is computed in.
/// Whether a value of this width is carried in a register at all. The same line
/// `jit::carried_width` draws, for the same reason: past sixty-two bits the tag has nowhere to go.
fn carried(t: IntTy) -> bool {
    t.bits() < 64
}

fn utype(t: IntTy) -> &'static str {
    match t.bits() {
        8 => "uint8_t",
        16 => "uint16_t",
        _ => "uint32_t",
    }
}

pub struct Emit<'a> {
    pub src: &'a Source,
    /// The function being emitted, for a refusal's message.
    pub function: String,
    pub module_index: usize,
    /// The body's statements, in order.
    pub out: String,
    tmp: usize,
    /// The bindings in scope, innermost last, as the Cranelift tier keeps them.
    scope: Vec<(Symbol, V)>,
    /// What the unit needs beside the code: constants, shapes, field names, builtins.
    pub unit: &'a mut Unit,
    depth: usize,
    /// The record widths this body builds or lets go of. Each gets a token: a record held once at
    /// its last use keeps its memory for the next literal of its width, which is Perceus's `reset`
    /// and what `rt_reset` answers. Without it the integer kernel takes sixteen fresh records per
    /// block and touches two megabytes of cold memory per hash.
    pub tokens: std::collections::BTreeSet<usize>,
    /// For each C local holding a record this body built, the values its fields were built from.
    /// A later read of one of those fields is that value, not a load: see `emit_record`.
    built: std::collections::HashMap<String, Vec<(Symbol, V)>>,
    /// Records built but not yet put in memory, by the local that will hold one when it is.
    ///
    /// A record of immediates whose every read is answered from `built` is never looked at, so
    /// building it is an allocation, sixteen tags and sixteen stores that nothing observes. These
    /// wait until something asks for the *word* -- a call, a return, a field of another record --
    /// and a record that is asked for its fields and then dies never becomes one at all.
    deferred: std::collections::HashMap<String, Deferred>,
    /// The locals the deferred records above will land in, declared once at the top so that
    /// materialising inside a branch still names something the whole body can see.
    record_locals: Vec<String>,
}

/// A record that has been described but not built: what `emit_record` would have emitted.
struct Deferred {
    shape: u32,
    n: usize,
    flags: i32,
    words: Vec<String>,
}

/// What an emitted unit accumulates that is not code. It is the Cranelift tier's `Jit` state,
/// named separately because only these four tables are the C tier's to fill.
pub struct Unit {
    pub consts: Vec<Value>,
    pub fields: Vec<Symbol>,
    pub builtins: Vec<Builtin>,
    /// Every compiled function's program-wide name: what a direct call is allowed to name.
    pub functions: Vec<String>,
    /// The shapes and constructor indices the runtime reads a record and a variant against.
    pub layouts: crate::heap::Layouts,
}

impl Unit {
    pub fn new(ctors: Vec<(Symbol, usize)>, functions: Vec<String>) -> Unit {
        Unit {
            consts: Vec::new(),
            fields: Vec::new(),
            builtins: Vec::new(),
            functions,
            layouts: crate::heap::Layouts::new(ctors),
        }
    }

    /// The shape a field set interns to, in the same table the runtime will read it against.
    pub fn shape(&mut self, names: &[Symbol]) -> u32 {
        self.layouts.shape(names.to_vec())
    }

    pub fn ctor_index(&self, name: &Symbol) -> Option<u32> {
        self.layouts.ctor_index(name)
    }
}

impl Unit {
    fn constant(&mut self, v: Value) -> usize {
        self.consts.push(v);
        self.consts.len() - 1
    }

    fn field(&mut self, name: &Symbol) -> usize {
        if let Some(i) = self.fields.iter().position(|f| f == name) {
            return i;
        }
        self.fields.push(name.clone());
        self.fields.len() - 1
    }

    fn builtin(&mut self, b: Builtin) -> usize {
        if let Some(i) = self.builtins.iter().position(|x| *x == b) {
            return i;
        }
        self.builtins.push(b);
        self.builtins.len() - 1
    }
}

/// A name the emitted C can carry: a Ply name holds dots, and a C identifier may not.
pub fn mangle(name: &str) -> String {
    let mut out = String::from("ply_");
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    out
}

impl<'a> Emit<'a> {
    pub fn new(
        src: &'a Source,
        unit: &'a mut Unit,
        function: &str,
        module_index: usize,
    ) -> Emit<'a> {
        Emit {
            src,
            function: function.to_string(),
            module_index,
            out: String::new(),
            tmp: 0,
            scope: Vec::new(),
            unit,
            depth: 0,
            tokens: std::collections::BTreeSet::new(),
            built: std::collections::HashMap::new(),
            deferred: std::collections::HashMap::new(),
            record_locals: Vec::new(),
        }
    }

    /// `Word tok16 = 0;` for each width this body touched, for the prologue.
    pub fn token_decls(&self) -> String {
        self.tokens
            .iter()
            .map(|n| format!("  Word tok{n} = 0;\n"))
            .collect()
    }

    fn refuse<T>(&self, what: impl Into<String>) -> Result<T> {
        Err(Refused {
            function: self.function.clone(),
            construct: what.into(),
        }
        .into())
    }

    fn fresh(&mut self) -> String {
        self.tmp += 1;
        format!("t{}", self.tmp)
    }

    fn line(&mut self, s: impl AsRef<str>) {
        for _ in 0..self.depth + 1 {
            self.out.push_str("  ");
        }
        self.out.push_str(s.as_ref());
        self.out.push('\n');
    }

    /// Bind an expression to a fresh local of its kind's C type, so that evaluation order is the
    /// order the statements are in and nothing is evaluated twice.
    fn bind(&mut self, k: Kind, expr: impl AsRef<str>) -> V {
        // A scalar kind *is* its type; only a word leaves the type open. Deriving it here rather
        // than at each of the several dozen call sites is what keeps a width attached to the value
        // carrying it: `wrap_add` bound its answer at `Kind::Num(U32)` with no type beside it, so
        // the record built from sixteen of them looked to have sixteen fields of unknown type.
        let ty = match k {
            Kind::Int => CTy::Int,
            Kind::Bool => CTy::Bool,
            Kind::Num(t) => CTy::Num(t),
            Kind::Boxed => CTy::Unknown,
        };
        self.bind_as(k, ty, expr)
    }

    fn bind_as(&mut self, k: Kind, ty: CTy, expr: impl AsRef<str>) -> V {
        let name = self.fresh();
        let ct = ctype(k);
        let e = expr.as_ref().to_string();
        // A record still waiting to be built is *not* renamed: the local it will land in holds a
        // zero until it is, and copying that zero into a second local is a null the next reader
        // walks into. Its local is declared for the whole body, so naming it again is safe and
        // naming it is all a rename needed to do.
        if self.record_locals.contains(&e) {
            return V { k, c: e, ty };
        }
        // A binding that is only a rename carries the fields the record was built from with it.
        // The inliner turns every argument into a `let`, so without this the knowledge is lost at
        // the first one -- which is immediately.
        if let Some(fields) = self.built.get(&e).cloned() {
            self.built.insert(name.clone(), fields);
        }
        self.line(format!("{ct} {name} = {e};"));
        V { k, c: name, ty }
    }

    /// The check after any helper that can raise: a body that failed answers nothing.
    fn check(&mut self) {
        self.line("if (ctx->failed) return 0;");
    }

    pub fn param(&mut self, name: &Symbol, c: String, ty: CTy) {
        let word = V {
            k: Kind::Boxed,
            c,
            ty: ty.clone(),
        };
        // A parameter whose checked type is a scalar arrives as a word and is then read many
        // times over -- `word_at` reads its offset three times and its limit twice -- and each
        // read was a test, a branch and a shift. Unbox it once, here where the prologue is, and
        // let every use downstream be the register.
        let v = match ty {
            CTy::Int => {
                let e = self.as_int(&word);
                self.bind_as(Kind::Int, CTy::Int, e)
            }
            CTy::Num(t) => {
                let e = self.as_num(&word, t);
                self.bind_as(Kind::Num(t), CTy::Num(t), e)
            }
            CTy::Bool => {
                let e = self.as_bool(&word);
                self.bind_as(Kind::Bool, CTy::Bool, e)
            }
            _ => word,
        };
        self.scope.push((name.clone(), v));
    }

    // --- conversions --------------------------------------------------------------------

    /// A value as a word: an `Int` that fits is tagged in place and one that does not is boxed by
    /// the runtime; a width always fits; a `Bool` is one of the two singletons.
    /// Build a record that was held back, here, because something is about to want the word.
    ///
    /// Guarded on the local still being zero, and the guard is not paranoia: emission follows the
    /// branch structure, so the *first* place that wants the word may be inside one arm of an `if`
    /// while another arm wants it too. Building under `if (!x)` is correct on every path and free
    /// on the one that already built it, where a build emitted once is right on one path and a
    /// null dereference on the rest.
    fn materialise(&mut self, name: &str) {
        let Some(d) = self.deferred.get(name) else {
            return;
        };
        let (shape, n, flags) = (d.shape, d.n, d.flags);
        let words = d.words.clone();
        self.tokens.insert(n);
        self.line(format!("if (!{name}) {{"));
        self.depth += 1;
        self.line(format!("if (tok{n}) {{"));
        self.depth += 1;
        self.line(format!("{name} = tok{n}; tok{n} = 0;"));
        self.line(format!(
            "ply_obj({name})->rc = 1; ply_obj({name})->flags = {flags}; ply_obj({name})->len = {n}; ply_obj({name})->layout = {shape};"
        ));
        self.depth -= 1;
        self.line("} else {");
        self.depth += 1;
        self.line(format!(
            "{name} = rt_alloc_p(ctx, 3, {n}, {shape}, {flags});"
        ));
        self.line("if (ctx->failed) return 0;");
        self.depth -= 1;
        self.line("}");
        for (at, w) in words.iter().enumerate() {
            self.line(format!("ply_words({name})[{at}] = {w};"));
        }
        self.depth -= 1;
        self.line("}");
    }

    /// The locals a deferred record lands in, declared at the top of the body so that building one
    /// inside a branch still names something every later statement can see.
    pub fn record_decls(&self) -> String {
        self.record_locals
            .iter()
            .map(|n| format!("  Word {n} = 0;\n"))
            .collect()
    }

    pub fn word(&mut self, v: &V) -> String {
        if v.k == Kind::Boxed && self.deferred.contains_key(&v.c) {
            self.materialise(&v.c.clone());
        }
        match v.k {
            Kind::Boxed => v.c.clone(),
            Kind::Int => format!(
                "(ply_fits_imm({0}) ? ply_imm({0}) : rt_box_int_p(ctx, {0}))",
                v.c
            ),
            Kind::Bool => format!("({} ? {} : {})", v.c, true_word(), false_word()),
            // Either way it widens to the word the immediate carries; the C type the value is
            // held in is what decides whether that is a sign or a zero extension.
            Kind::Num(_) => format!("ply_imm((int64_t){})", v.c),
        }
    }

    fn as_int(&mut self, v: &V) -> String {
        match v.k {
            Kind::Int => v.c.clone(),
            Kind::Num(_) => format!("(int64_t)({})", v.c),
            Kind::Bool => v.c.clone(),
            Kind::Boxed => format!(
                "(ply_is_imm({0}) ? ply_imm_value({0}) : rt_unbox_int_p(ctx, {0}))",
                v.c
            ),
        }
    }

    fn as_num(&mut self, v: &V, t: IntTy) -> String {
        let ty = ctype(Kind::Num(t));
        match v.k {
            Kind::Num(have) if have == t => v.c.clone(),
            Kind::Num(_) | Kind::Int | Kind::Bool => format!("({ty})({})", v.c),
            Kind::Boxed => format!("({ty})ply_imm_value({})", v.c),
        }
    }

    /// Whether a value may be read as an `Int` at all. A raw register is one by construction; a
    /// word is one only if the checker said so.
    ///
    /// `rt_unbox_int` raises on a `Float`, a `Decimal` and on the two widths this tier does not
    /// carry, so an operator reaching for `as_int` on one of those is a body that would answer
    /// with a diagnostic where the interpreter answers with a number. Refusing is the tier's
    /// answer to that, and it is the same answer it gives a lambda.
    ///
    /// A word whose type the emitter merely lost is not one of those: the checker has already
    /// agreed the operands of a `+` are numbers of one type, so an unknown word under an operator
    /// is an `Int` unless the type says otherwise -- and `CTy::Opaque` is the type saying so.
    fn int_like(v: &V) -> bool {
        v.k != Kind::Boxed || v.ty != CTy::Opaque
    }

    fn refuse_unless_int(&self, l: &V, r: &V, what: &str) -> Result<()> {
        if Self::int_like(l) && Self::int_like(r) {
            return Ok(());
        }
        self.refuse(format!(
            "`{what}` over a value whose type the fragment does not fix"
        ))
    }

    fn as_bool(&mut self, v: &V) -> String {
        match v.k {
            Kind::Bool => v.c.clone(),
            Kind::Int | Kind::Num(_) => format!("(({}) != 0)", v.c),
            Kind::Boxed => format!("rt_unbox_bool_p(ctx, {})", v.c),
        }
    }

    /// A word a helper is about to take. Duplicated first, because nothing here is ever released
    /// and a helper that takes will release: the duplicate is what puts the count back.
    fn owned(&mut self, v: &V) -> String {
        let w = self.word(v);
        let t = self.fresh();
        self.line(format!("Word {t} = {w};"));
        // A scalar is an immediate and holds no count, so there is nothing to take: the kernel
        // builds sixteen-field records of them and the increments were the whole of the cost.
        if !matches!(v.k, Kind::Num(_) | Kind::Int | Kind::Bool) {
            self.line(format!("ply_inc({t});"));
        }
        t
    }

    // --- the walk -----------------------------------------------------------------------

    pub fn expr(&mut self, code: &Code) -> Result<V> {
        match &code.kind {
            NodeKind::Lit(lit, value) => self.literal(lit, value),
            NodeKind::Var { name, .. } => self.var(name, code.own),
            NodeKind::Unary { op, operand } => self.unary(*op, operand),
            NodeKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs),
            NodeKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.if_expr(cond, then_branch, else_branch),
            NodeKind::Block { stmts, tail } => self.block(stmts, tail.as_ref()),
            NodeKind::Field { base, field } => self.field(base, &field.name),
            NodeKind::Record { fields } => self.record(fields),
            NodeKind::List { items } => self.list(items),
            NodeKind::App { func, args } => self.app(func, args),
            NodeKind::Match { scrutinee, arms } => self.match_expr(scrutinee, arms),
            NodeKind::Lambda { .. } => self.refuse("a lambda, which this tier does not carry yet"),
            NodeKind::RecordUpdate { base, copies, sets } => self.record_update(base, copies, sets),
            other => self.refuse(describe(other).to_string()),
        }
    }

    fn literal(&mut self, lit: &Lit, value: &Value) -> Result<V> {
        match lit {
            Lit::Int(i) => Ok(V {
                k: Kind::Int,
                c: format!("INT64_C({i})"),
                ty: CTy::Int,
            }),
            Lit::Fixed { ty, bits } if ty.bits() < 64 => Ok(V {
                k: Kind::Num(*ty),
                c: format!("(({}){})", ctype(Kind::Num(*ty)), *bits as i64),
                ty: CTy::Num(*ty),
            }),
            // A sixty-four bit literal is a constant like any other: it goes in the pool, where
            // the value keeps every bit, rather than into a register the tag would clip.
            Lit::Fixed { .. } => {
                let index = self.unit.constant(value.clone());
                let v = self.bind(Kind::Boxed, format!("rt_lit_p(ctx, {index})"));
                self.check();
                Ok(v)
            }
            Lit::Bool(b) => Ok(V {
                k: Kind::Bool,
                c: (if *b { "1" } else { "0" }).to_string(),
                ty: CTy::Bool,
            }),
            Lit::Float(_) | Lit::Decimal { .. } => {
                self.refuse("a `Float` or `Decimal` literal, which the fragment has no path for")
            }
            Lit::Str(_) | Lit::Bytes(_) | Lit::Unit => {
                let index = self.unit.constant(value.clone());
                let v = self.bind(Kind::Boxed, format!("rt_lit_p(ctx, {index})"));
                self.check();
                Ok(v)
            }
        }
    }

    fn var(&mut self, q: &QName, _own: Own) -> Result<V> {
        if q.is_bare()
            && let Some((_, v)) = self.scope.iter().rev().find(|(n, _)| n == q.symbol())
        {
            return Ok(v.clone());
        }
        let name = q.symbol().as_str();
        // A nullary compiled function used as a value is its call.
        if let Some(full) = self.resolve_q(q)
            && self
                .src
                .definition(&full)
                .is_some_and(|(d, _)| d.params.is_empty())
            && self.unit.functions.contains(&full)
        {
            let v = self.bind(Kind::Boxed, format!("{}(ctx)", mangle(&full)));
            self.check();
            return Ok(v);
        }
        if q.is_bare()
            && let Some(b) = Builtin::from_name(q.symbol())
        {
            let index = self.unit.builtin(b);
            let v = self.bind(Kind::Boxed, format!("rt_builtin_value_p(ctx, {index})"));
            self.check();
            return Ok(v);
        }
        // A constructor named as a value is two different things by arity, and the tier asked for
        // the wrong one: `None` is the singleton the tables hold, and `Some` is a function value.
        // Asking for the function in both cases put a closure where a variant belonged, so every
        // body that answered `None` answered wrongly -- which four tests in `examples/` had been
        // saying since this tier's first commit, to nothing that was listening.
        if let Some(i) = self.unit.ctor_index(q.symbol()) {
            let nullary = self
                .unit
                .layouts
                .ctors
                .get(i as usize)
                .is_some_and(|(_, arity)| *arity == 0);
            let call = if nullary {
                format!("rt_nullary_p(ctx, {i})")
            } else {
                format!("rt_ctor_value_p(ctx, {i})")
            };
            let v = self.bind(Kind::Boxed, call);
            self.check();
            return Ok(v);
        }
        self.refuse(format!(
            "the name `{}` denotes nothing this tier knows",
            name
        ))
    }

    /// The type the checker published for a definition's answer.
    pub fn declared_ret(&self, full: &str) -> CTy {
        use ply_core::ty::Type;
        match self
            .src
            .check
            .defs
            .get(&Symbol::new(full))
            .map(|d| &d.scheme.ty)
        {
            Some(Type::Fn { ret, .. }) => CTy::of(ret),
            Some(other) => CTy::of(other),
            None => CTy::Unknown,
        }
    }

    /// The program-wide name a `QName` denotes: its own module's for a bare name, and the module
    /// the import named for a qualified one.
    fn resolve_q(&self, q: &QName) -> Option<String> {
        if q.is_bare() {
            let module = &self.src.program.modules[self.module_index].name;
            let full = format!("{module}.{}", q.symbol());
            return self.src.definition(&full).map(|_| full);
        }
        // A qualified name is `alias::name`; the resolver has already put the module behind the
        // alias, so every module that defines the simple name and ends with the alias is tried.
        let simple = q.symbol().as_str().to_string();
        let alias = q.module.as_ref()?.name.as_str().to_string();
        for module in &self.src.program.modules {
            let m = module.name.to_string();
            if (m == alias || m.ends_with(&format!(".{alias}")))
                && self.src.definition(&format!("{m}.{simple}")).is_some()
            {
                return Some(format!("{m}.{simple}"));
            }
        }
        None
    }

    fn unary(&mut self, op: UnOp, operand: &Code) -> Result<V> {
        let v = self.expr(operand)?;
        match op {
            UnOp::Not => {
                let b = self.as_bool(&v);
                Ok(self.bind(Kind::Bool, format!("!({b})")))
            }
            UnOp::BitNot => match v.k {
                Kind::Num(t) => {
                    let a = self.as_num(&v, t);
                    Ok(self.bind(Kind::Num(t), format!("({}) ~({a})", "")))
                }
                _ => {
                    let a = self.as_int(&v);
                    Ok(self.bind(Kind::Int, format!("~({a})")))
                }
            },
            UnOp::Neg => match v.k {
                Kind::Num(t) => {
                    let a = self.as_num(&v, t);
                    let wide = self.bind(Kind::Int, format!("-(int64_t)({a})"));
                    self.narrow(&wide, t, true)
                }
                _ => {
                    let a = self.as_int(&v);
                    self.line(format!(
                        "if ({a} == INT64_MIN) {{ rt_overflow_p(ctx, 2); return 0; }}"
                    ));
                    Ok(self.bind(Kind::Int, format!("-({a})")))
                }
            },
        }
    }

    /// `wide`, refused unless it is one of `t`'s values: the check `Int` gets from the machine, at
    /// a width.
    fn narrow(&mut self, wide: &V, t: IntTy, sub: bool) -> Result<V> {
        let a = wide.c.clone();
        self.line(format!(
            "if ({a} < INT64_C({}) || {a} > INT64_C({})) {{ rt_overflow_p(ctx, {}); return 0; }}",
            t.min(),
            t.max(),
            i64::from(sub)
        ));
        Ok(self.bind(Kind::Num(t), format!("({}){a}", ctype(Kind::Num(t)))))
    }

    fn binary(&mut self, op: BinOp, lhs: &Code, rhs: &Code) -> Result<V> {
        // `&&` and `||` short-circuit, so the right operand is emitted inside the branch.
        if matches!(op, BinOp::And | BinOp::Or) {
            let l = self.expr(lhs)?;
            let lb = self.as_bool(&l);
            let out = self.fresh();
            self.line(format!("int64_t {out} = {lb};"));
            let test = if matches!(op, BinOp::And) {
                format!("if ({out})")
            } else {
                format!("if (!{out})")
            };
            self.line(format!("{test} {{"));
            self.depth += 1;
            let r = self.expr(rhs)?;
            let rb = self.as_bool(&r);
            self.line(format!("{out} = {rb};"));
            self.depth -= 1;
            self.line("}");
            return Ok(V {
                k: Kind::Bool,
                c: out,
                ty: CTy::Bool,
            });
        }
        let l = self.expr(lhs)?;
        let r = self.expr(rhs)?;
        let width = match (l.k, r.k) {
            (Kind::Num(t), _) | (_, Kind::Num(t)) => Some(t),
            _ => None,
        };
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                self.arithmetic(op, &l, &r, width)
            }
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                let c = match op {
                    BinOp::BitAnd => "&",
                    BinOp::BitOr => "|",
                    _ => "^",
                };
                match width {
                    Some(t) => {
                        let a = self.as_num(&l, t);
                        let b = self.as_num(&r, t);
                        Ok(self.bind(Kind::Num(t), format!("({a}) {c} ({b})")))
                    }
                    None => {
                        self.refuse_unless_int(&l, &r, c)?;
                        let a = self.as_int(&l);
                        let b = self.as_int(&r);
                        Ok(self.bind(Kind::Int, format!("({a}) {c} ({b})")))
                    }
                }
            }
            BinOp::Shl | BinOp::Shr | BinOp::Ushr => self.shift(op, &l, &r, width),
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let c = match op {
                    BinOp::Lt => "<",
                    BinOp::Le => "<=",
                    BinOp::Gt => ">",
                    _ => ">=",
                };
                match width {
                    Some(t) => {
                        let a = self.as_num(&l, t);
                        let b = self.as_num(&r, t);
                        Ok(self.bind(Kind::Bool, format!("({a}) {c} ({b})")))
                    }
                    None => {
                        self.refuse_unless_int(&l, &r, c)?;
                        let a = self.as_int(&l);
                        let b = self.as_int(&r);
                        Ok(self.bind(Kind::Bool, format!("({a}) {c} ({b})")))
                    }
                }
            }
            BinOp::Eq | BinOp::Ne => {
                // Equality has a runtime path that is right for every value, so this one only
                // has to decide when the cheap comparison is *also* right -- no refusal needed.
                let native = width.is_some()
                    || (l.k == Kind::Int && r.k == Kind::Int)
                    || (l.k == Kind::Bool && r.k == Kind::Bool);
                let e = if native {
                    let (a, b) = match width {
                        Some(t) => (self.as_num(&l, t), self.as_num(&r, t)),
                        None if l.k == Kind::Bool => (self.as_bool(&l), self.as_bool(&r)),
                        None => (self.as_int(&l), self.as_int(&r)),
                    };
                    format!("({a}) == ({b})")
                } else {
                    let a = self.owned(&l);
                    let b = self.owned(&r);
                    let out = self.bind(Kind::Int, format!("rt_equal_p(ctx, {a}, {b})"));
                    self.check();
                    out.c
                };
                let e = if matches!(op, BinOp::Ne) {
                    format!("!({e})")
                } else {
                    e
                };
                Ok(self.bind(Kind::Bool, e))
            }
            BinOp::Concat => {
                let a = self.owned(&l);
                let b = self.owned(&r);
                let v = self.bind(Kind::Boxed, format!("rt_concat_p(ctx, {a}, {b})"));
                self.check();
                Ok(v)
            }
            BinOp::And | BinOp::Or => unreachable!("handled above"),
        }
    }

    fn arithmetic(&mut self, op: BinOp, l: &V, r: &V, width: Option<IntTy>) -> Result<V> {
        let code = match op {
            BinOp::Mul => 0,
            BinOp::Div => 1,
            _ => 2,
        };
        match width {
            Some(t) => {
                let a = self.as_num(l, t);
                let b = self.as_num(r, t);
                let wide = match op {
                    BinOp::Add => self.bind(Kind::Int, format!("(int64_t)({a}) + (int64_t)({b})")),
                    BinOp::Sub => self.bind(Kind::Int, format!("(int64_t)({a}) - (int64_t)({b})")),
                    _ => {
                        let w = self.bind(
                            Kind::Int,
                            format!("rt_arith_p(ctx, {code}, (int64_t)({a}), (int64_t)({b}))"),
                        );
                        self.check();
                        w
                    }
                };
                self.narrow(&wide, t, matches!(op, BinOp::Sub))
            }
            None => {
                self.refuse_unless_int(l, r, "arithmetic")?;
                let a = self.as_int(l);
                let b = self.as_int(r);
                match op {
                    BinOp::Add | BinOp::Sub => {
                        let sign = if matches!(op, BinOp::Add) {
                            "add"
                        } else {
                            "sub"
                        };
                        let out = self.fresh();
                        self.line(format!("int64_t {out};"));
                        self.line(format!(
                            "if (__builtin_{sign}_overflow({a}, {b}, &{out})) {{ rt_overflow_p(ctx, {}); return 0; }}",
                            i64::from(matches!(op, BinOp::Sub))
                        ));
                        Ok(V {
                            k: Kind::Int,
                            c: out,
                            ty: CTy::Int,
                        })
                    }
                    _ => {
                        let v = self.bind(Kind::Int, format!("rt_arith_p(ctx, {code}, {a}, {b})"));
                        self.check();
                        Ok(v)
                    }
                }
            }
        }
    }

    fn shift(&mut self, op: BinOp, l: &V, r: &V, width: Option<IntTy>) -> Result<V> {
        let n = self.as_int(r);
        let bound = width.map_or(64, |t| i64::from(t.bits()));
        let count = self.bind(Kind::Int, n);
        self.line(format!(
            "if ((uint64_t){} >= (uint64_t){bound}) {{ rt_shift_count_p(ctx, {}); return 0; }}",
            count.c, count.c
        ));
        match width {
            Some(t) => {
                let a = self.as_num(l, t);
                let u = utype(t);
                let e = match op {
                    BinOp::Shl => format!("({u})(({u})({a}) << {})", count.c),
                    // Arithmetic where the type is signed, logical where it is not, which at an
                    // unsigned type are the same shift.
                    BinOp::Shr if t.signed() => format!("({a}) >> {}", count.c),
                    _ => format!("({u})(({u})({a}) >> {})", count.c),
                };
                Ok(self.bind(Kind::Num(t), e))
            }
            None => {
                self.refuse_unless_int(l, l, "a shift")?;
                let a = self.as_int(l);
                let e = match op {
                    BinOp::Shl => format!("(int64_t)((uint64_t)({a}) << {})", count.c),
                    BinOp::Shr => format!("({a}) >> {}", count.c),
                    _ => format!("(int64_t)((uint64_t)({a}) >> {})", count.c),
                };
                Ok(self.bind(Kind::Int, e))
            }
        }
    }

    fn if_expr(&mut self, cond: &Code, then_branch: &Code, else_branch: &Code) -> Result<V> {
        let c = self.expr(cond)?;
        let cb = self.as_bool(&c);
        let out = self.fresh();
        // Both arms are emitted into a buffer of their own before anything is written, because the
        // local the join lands in is typed by what the arms turn out to be. Two arms that agree on
        // a scalar keep it raw: `word_at` boxed a byte on the way out of a bounds test and unboxed
        // it one line later, four times per word, and that round trip was most of the body.
        self.depth += 1;
        let (t, t_text) = self.buffered(|s| s.expr(then_branch))?;
        let (e, e_text) = self.buffered(|s| s.expr(else_branch))?;
        self.depth -= 1;
        let join = match (t.k, e.k) {
            (Kind::Num(a), Kind::Num(b)) if a == b => Kind::Num(a),
            (Kind::Int, Kind::Int) => Kind::Int,
            (Kind::Bool, Kind::Bool) => Kind::Bool,
            _ => Kind::Boxed,
        };
        self.line(format!("{} {out} = 0;", ctype(join)));
        self.line(format!("if ({cb}) {{"));
        self.out.push_str(&t_text);
        self.depth += 1;
        let tw = self.as_kind(&t, join);
        self.line(format!("{out} = {tw};"));
        self.depth -= 1;
        self.line("} else {");
        self.out.push_str(&e_text);
        self.depth += 1;
        let ew = self.as_kind(&e, join);
        self.line(format!("{out} = {ew};"));
        self.depth -= 1;
        self.line("}");
        Ok(V {
            k: join,
            c: out,
            ty: if t.ty == e.ty { t.ty } else { CTy::Unknown },
        })
    }

    /// Run `f` with the output diverted, and hand back what it wrote alongside its answer. The
    /// statements come out in the order they were made either way; what this buys is the chance to
    /// decide the enclosing declaration after seeing them.
    fn buffered(&mut self, f: impl FnOnce(&mut Self) -> Result<V>) -> Result<(V, String)> {
        let saved = std::mem::take(&mut self.out);
        let answer = f(self);
        let text = std::mem::replace(&mut self.out, saved);
        Ok((answer?, text))
    }

    /// A value read at some other kind: the one conversion the several below are chosen by.
    fn as_kind(&mut self, v: &V, k: Kind) -> String {
        match k {
            Kind::Boxed => self.word(v),
            Kind::Int => self.as_int(v),
            Kind::Bool => self.as_bool(v),
            Kind::Num(t) => self.as_num(v, t),
        }
    }

    fn block(&mut self, stmts: &[Stmt], tail: Option<&Code>) -> Result<V> {
        let mark = self.block_stmts(stmts)?;
        let answer = match tail {
            Some(t) => self.expr(t)?,
            None => V::boxed(unit_word()),
        };
        // The answer is bound before the scope closes, since a name it reads goes out of scope.
        let held = self.bind_as(answer.k, answer.ty.clone(), answer.c);
        self.scope.truncate(mark);
        Ok(held)
    }

    /// A block's statements, with the scope mark to close it at.
    fn block_stmts(&mut self, stmts: &[Stmt]) -> Result<usize> {
        let mark = self.scope.len();
        for s in stmts {
            match s {
                Stmt::Let { pat, value, .. } => {
                    let v = self.expr(value)?;
                    match pat {
                        Pat::Var { name, .. } => {
                            // Bound to a local of the value's own kind **and its type**: the
                            // inliner turns every argument into a `let`, so a `let` that dropped
                            // the type would lose every width and every known record one call
                            // deep — which is most of a body after inlining.
                            let held = self.bind_as(v.k, v.ty.clone(), v.c.clone());
                            self.scope.push((name.name.clone(), held));
                        }
                        Pat::Wildcard => {}
                        _ => {
                            let w = self.word(&v);
                            let held = self.bind(Kind::Boxed, w);
                            self.bind_pattern(pat, &held)?;
                        }
                    }
                }
                Stmt::Expr { code } => {
                    let v = self.expr(code)?;
                    let w = self.word(&v);
                    self.line(format!("(void)({w});"));
                }
            }
        }
        Ok(mark)
    }

    fn field(&mut self, base: &Code, name: &Symbol) -> Result<V> {
        let b = self.expr(base)?;
        let field_ty = b.ty.field(name).cloned().unwrap_or(CTy::Unknown);
        let at = b.ty.offset(name);
        // Built in this body: the value is already in a register, and asking for the word here
        // would be what forces a record into memory that nothing else ever looks at.
        if let Some(v) = self
            .built
            .get(&b.c)
            .and_then(|fs| fs.iter().find(|(n, _)| n == name))
            .map(|(_, v)| v.clone())
        {
            self.release_base(base, &b);
            return Ok(v);
        }
        let bw = self.word(&b);
        let held = self.bind(Kind::Boxed, bw);
        // A field whose record type the checker fixed is a load at its offset, and nothing else.
        // The shape's field order is the sorted name order and `Type::Record` is a `BTreeMap`, so
        // the position in the declared type *is* the offset. Going through the runtime instead —
        // which is what this emitter did first — costs a call per word, and the integer kernel
        // reads thirty-two per round.
        let w = match at {
            Some(at) => self.bind(Kind::Boxed, format!("ply_words({})[{at}]", held.c)),
            None => {
                let index = self.unit.field(name);
                let v = self.bind(
                    Kind::Boxed,
                    format!("rt_field_p(ctx, {}, {index}, 0)", held.c),
                );
                self.check();
                v
            }
        };
        // A scalar field is read into a register of its own kind: this is what keeps a `U32` a
        // `U32` on the way out of a record, and without it every width in the kernel is lost at
        // the first field read.
        let kind = field_ty.kind();
        if kind == Kind::Boxed {
            return Ok(V {
                k: Kind::Boxed,
                c: w.c,
                ty: field_ty,
            });
        }
        let e = match kind {
            Kind::Num(t) => self.as_num(&w, t),
            Kind::Int => self.as_int(&w),
            _ => self.as_bool(&w),
        };
        let held = self.bind_as(kind, field_ty, e);
        // The one release this tier makes, and the reason it makes it: a record read to its last
        // field is dead, and letting it go here puts its memory on the heap's free list, where the
        // *next* record of its size class takes it instead of fresh memory. Without it the integer
        // kernel touches two megabytes per hash and every record is cold.
        //
        // Narrow on purpose. The base must be a variable at its last use — which is what the
        // lowering's `Own::Owned` says and what the interpreter reads too — and the field must be
        // a scalar, so the value just read cannot be inside the memory being let go.
        self.release_base(base, &b);
        Ok(held)
    }

    /// A record read to its last field is dead; letting it go here puts its memory on the free
    /// list, where the next record of its size class takes it instead of fresh memory. Without it
    /// the integer kernel touches two megabytes per hash and every record is cold.
    ///
    /// Narrow on purpose. The base must be a variable at its last use -- which is what the
    /// lowering's `Own::Owned` says and what the interpreter reads too.
    fn release_base(&mut self, base: &Code, b: &V) {
        if !(matches!(base.own, Own::Owned) && matches!(base.kind, NodeKind::Var { .. })) {
            return;
        }
        // A record held back dies without ever having been built, and holds no counts -- only a
        // flat record is ever held back -- so there is nothing to let go of. Guarded rather than
        // skipped, because another path may have wanted the word and built it.
        if self.deferred.contains_key(&b.c) {
            let name = b.c.clone();
            let Some(n) = record_width(&b.ty) else {
                return;
            };
            self.tokens.insert(n);
            self.line(format!(
                "if ({name}) {{ if (tok{n} == 0) {{ tok{n} = ply_reset_flat({name}); if (!tok{n}) tok{n} = rt_reset_p(ctx, {name}); }} else {{ rt_dec_p(ctx, {name}); }} {name} = 0; }}"
            ));
            return;
        }
        let base_local = &self.word(b);
        match record_width(&b.ty) {
            Some(n) => {
                self.tokens.insert(n);
                self.line(format!(
                    "if (tok{n} == 0) {{ tok{n} = ply_reset_flat({base_local}); if (!tok{n}) tok{n} = rt_reset_p(ctx, {base_local}); }} else {{ rt_dec_p(ctx, {base_local}); }}"
                ));
            }
            None => self.line(format!("rt_dec_p(ctx, {base_local});")),
        }
    }

    fn record(&mut self, fields: &[(Symbol, Code)]) -> Result<V> {
        let mut names: Vec<Symbol> = fields.iter().map(|(n, _)| n.clone()).collect();
        names.sort();
        let mut words = Vec::with_capacity(fields.len());
        let mut kinds = Vec::with_capacity(fields.len());
        let mut vals = Vec::with_capacity(fields.len());
        for name in &names {
            let (_, code) = fields
                .iter()
                .find(|(n, _)| n == name)
                .expect("a field of the shape");
            let v = self.expr(code)?;
            kinds.push(v.ty.clone());
            words.push(self.owned(&v));
            vals.push(v);
        }
        Ok(self.emit_record(&names, words, kinds, Some(vals)))
    }

    /// The tail both record forms share: a Perceus token if one is in hand, a fresh allocation
    /// otherwise, then the words written straight into it.
    ///
    /// `{..b, f: e}` used to go through `rt_record` instead, which cost a call, and — worse —
    /// answered with no type at all. In the integer kernel the permuted message word *is* a record
    /// update, so one untyped record put the next round's thirty-two field reads back on the
    /// runtime, and the round after that, for a hundred and sixty calls per compression.
    fn emit_record(
        &mut self,
        names: &[Symbol],
        words: Vec<String>,
        kinds: Vec<CTy>,
        built_from: Option<Vec<V>>,
    ) -> V {
        let shape = self.unit.shape(names);
        // A record of nothing but immediates holds no counts, and saying so is what lets the
        // runtime skip walking its fields when it dies or is freed. The kernel's records are
        // sixteen scalars each and this tier was leaving the flag clear, so every death walked
        // sixteen children to decide there was nothing there.
        //
        // `Int` does not count, and the distinction is the whole of the correctness here: a width
        // this tier carries is under sixty-three bits and always an immediate, but an `Int` past
        // `2^62` is a heap object, and a record marked flat never lets its children go.
        let flat = !kinds.is_empty() && kinds.iter().all(|k| matches!(k, CTy::Num(_) | CTy::Bool));
        let flags = i32::from(flat);
        let ty = CTy::Record(names.iter().cloned().zip(kinds).collect());
        let n = words.len();
        // Flat and fully known: hold it back. Nothing here can be wrong if it is never built --
        // no count was taken, because a record of immediates holds none -- and if something does
        // ask for the word later, `materialise` emits exactly what this would have.
        if flat && let Some(vals) = built_from.clone() {
            let name = self.fresh();
            self.record_locals.push(name.clone());
            // Cleared where the record is *described*, not only where it is built. A description
            // inside a loop runs once per iteration and must describe a new record each time; the
            // guard in `materialise` would otherwise hand back the one the first iteration built,
            // with the first iteration's contents. That is a wrong answer rather than a crash, and
            // it showed as a wrong digest on an input long enough to loop.
            self.line(format!("{name} = 0;"));
            self.deferred.insert(
                name.clone(),
                Deferred {
                    shape,
                    n,
                    flags,
                    words,
                },
            );
            self.built
                .insert(name.clone(), names.iter().cloned().zip(vals).collect());
            return V {
                k: Kind::Boxed,
                c: name,
                ty,
            };
        }
        self.tokens.insert(n);
        let r = self.bind_as(Kind::Boxed, ty, "0");
        self.line(format!("if (tok{n}) {{"));
        self.depth += 1;
        self.line(format!("{0} = tok{n}; tok{n} = 0;", r.c));
        self.line(format!(
            "ply_obj({0})->rc = 1; ply_obj({0})->flags = {flags}; ply_obj({0})->len = {n}; ply_obj({0})->layout = {shape};",
            r.c
        ));
        self.depth -= 1;
        self.line("} else {");
        self.depth += 1;
        self.line(format!(
            "{0} = rt_alloc_p(ctx, 3, {n}, {shape}, {flags});",
            r.c
        ));
        self.line("if (ctx->failed) return 0;");
        self.depth -= 1;
        self.line("}");
        for (at, w) in words.iter().enumerate() {
            self.line(format!("ply_words({})[{at}] = {w};", r.c));
        }
        // What went in is what will come out: a record is immutable once built, so a field read of
        // it later in this body is the value already in a register. Remembering them here is what
        // lets that read skip the store, the load and the tag -- which is 32 of the 182
        // instructions `round` spends above the Rust bar, and all of `compress`'s own work once
        // the rounds are inlined into it.
        //
        // Sound because the values are C locals of this body, and a record that leaves the body
        // leaves through a name this table has no entry for.
        if let Some(vals) = built_from {
            self.built
                .insert(r.c.clone(), names.iter().cloned().zip(vals).collect());
        }
        r
    }

    /// `{..b, f: e}`: the written fields, then the copied ones read out of the base. Built fresh
    /// rather than updated in place — the in-place path is ADR 0034's and this tier does not have
    /// it yet, which costs an allocation and cannot be wrong.
    fn record_update(
        &mut self,
        base: &Code,
        copies: &[ply_syntax::ast::Ident],
        sets: &[(Symbol, Code)],
    ) -> Result<V> {
        let mut names: Vec<Symbol> = sets.iter().map(|(n, _)| n.clone()).collect();
        names.extend(copies.iter().map(|c| c.name.clone()));
        names.sort();
        let mut written: Vec<(Symbol, V)> = Vec::with_capacity(sets.len());
        for (name, code) in sets {
            let v = self.expr(code)?;
            let held = self.bind(v.k, v.c.clone());
            written.push((name.clone(), held));
        }
        let b = self.expr(base)?;
        let base_ty = b.ty.clone();
        let bw = self.word(&b);
        let held_base = self.bind(Kind::Boxed, bw);
        let mut words = Vec::with_capacity(names.len());
        let mut kinds = Vec::with_capacity(names.len());
        let mut vals: Option<Vec<V>> = Some(Vec::with_capacity(names.len()));
        for name in &names {
            match written.iter().find(|(n, _)| n == name) {
                Some((_, v)) => {
                    let v = v.clone();
                    kinds.push(v.ty.clone());
                    words.push(self.owned(&v));
                    if let Some(vs) = vals.as_mut() {
                        vs.push(v);
                    }
                }
                None => {
                    let ft = base_ty.field(name).cloned().unwrap_or(CTy::Unknown);
                    kinds.push(ft.clone());
                    // A field the base itself remembered is the value, not a load. This is what
                    // carries the knowledge along a chain of updates: BLAKE3's message schedule is
                    // six permutations one after another, and one link read from memory puts the
                    // next round's thirty-two reads back there too.
                    let known = self
                        .built
                        .get(&held_base.c)
                        .and_then(|fs| fs.iter().find(|(n, _)| n == name))
                        .map(|(_, v)| v.clone());
                    if let Some(v) = known {
                        words.push(self.owned(&v));
                        if let Some(vs) = vals.as_mut() {
                            vs.push(v);
                        }
                        continue;
                    }
                    match base_ty.offset(name) {
                        // A copied field of a shape the checker fixed is a load, as in `field`.
                        // The count has to go up by hand here: the runtime's reader takes one on
                        // the way out and a load does not.
                        Some(at) => {
                            let t = self.fresh();
                            self.line(format!("Word {t} = ply_words({})[{at}];", held_base.c));
                            self.line(format!("ply_inc({t});"));
                            let w = V {
                                k: Kind::Boxed,
                                c: t.clone(),
                                ty: ft.clone(),
                            };
                            words.push(t);
                            match ft.kind() {
                                Kind::Boxed => vals = None,
                                kind => {
                                    let e = self.as_kind(&w, kind);
                                    let held = self.bind_as(kind, ft, e);
                                    if let Some(vs) = vals.as_mut() {
                                        vs.push(held);
                                    }
                                }
                            }
                        }
                        None => {
                            let index = self.unit.field(name);
                            let f = self.bind(
                                Kind::Boxed,
                                format!("rt_field_p(ctx, {}, {index}, 0)", held_base.c),
                            );
                            self.check();
                            words.push(f.c);
                            vals = None;
                        }
                    }
                }
            }
        }
        // The base is dead once its copies are in hand, and letting it go here is the difference
        // between an update and a leak. `field` makes the same release for the same reason, under
        // the same condition -- a variable at its last use, which is what `Own::Owned` says.
        //
        // Every copy was counted above before this runs, so the walk that lets the base's children
        // go leaves the ones this record keeps alone, and drops exactly the ones it replaced.
        if matches!(base.own, Own::Owned) && matches!(base.kind, NodeKind::Var { .. }) {
            let n = names.len();
            self.tokens.insert(n);
            self.line(format!(
                "if (tok{n} == 0) {{ tok{n} = rt_reset_p(ctx, {0}); }} else {{ rt_dec_p(ctx, {0}); }}",
                held_base.c
            ));
        }
        Ok(self.emit_record(&names, words, kinds, vals))
    }

    fn list(&mut self, items: &[Code]) -> Result<V> {
        let mut words = Vec::with_capacity(items.len());
        for item in items {
            let v = self.expr(item)?;
            words.push(self.owned(&v));
        }
        let arr = self.fresh();
        self.line(format!(
            "Word {arr}[] = {{{}}};",
            if words.is_empty() {
                "0".to_string()
            } else {
                words.join(", ")
            }
        ));
        let v = self.bind(
            Kind::Boxed,
            format!("rt_list_p(ctx, (Word)(intptr_t){arr}, {})", items.len()),
        );
        self.check();
        Ok(v)
    }

    fn app(&mut self, func: &Code, args: &[Code]) -> Result<V> {
        if let NodeKind::Var { name: q, .. } = &func.kind {
            let bare = q.symbol().as_str().to_string();
            if let Some(full) = self.resolve_q(q)
                && self.unit.functions.contains(&full)
            {
                let (def, _) = self.src.definition(&full).expect("resolved");
                if def.params.len() != args.len() {
                    return self.refuse(format!("`{bare}` called with {} arguments", args.len()));
                }
                let mut ws = Vec::with_capacity(args.len());
                for a in args {
                    let v = self.expr(a)?;
                    ws.push(self.owned(&v));
                }
                let call = format!(
                    "{}(ctx{}{})",
                    mangle(&full),
                    if ws.is_empty() { "" } else { ", " },
                    ws.join(", ")
                );
                let ret = self.declared_ret(&full);
                let held = self.bind(Kind::Boxed, call);
                self.check();
                let kind = ret.kind();
                if kind == Kind::Boxed {
                    return Ok(V {
                        k: Kind::Boxed,
                        c: held.c,
                        ty: ret,
                    });
                }
                let e = match kind {
                    Kind::Num(t) => self.as_num(&held, t),
                    Kind::Int => self.as_int(&held),
                    _ => self.as_bool(&held),
                };
                return Ok(self.bind_as(kind, ret, e));
            }
            if q.is_bare()
                && let Some(b) = Builtin::from_name(q.symbol())
            {
                return self.builtin_call(b, args);
            }
            // A constructor applied to arguments.
            if let Some(i) = self.unit.ctor_index(q.symbol()) {
                let mut ws = Vec::with_capacity(args.len());
                for a in args {
                    let v = self.expr(a)?;
                    ws.push(self.owned(&v));
                }
                let arr = self.fresh();
                self.line(format!(
                    "Word {arr}[] = {{{}}};",
                    if ws.is_empty() {
                        "0".to_string()
                    } else {
                        ws.join(", ")
                    }
                ));
                let v = self.bind(
                    Kind::Boxed,
                    format!("rt_ctor_p(ctx, {i}, (Word)(intptr_t){arr}, {})", args.len()),
                );
                self.check();
                return Ok(v);
            }
            let _ = bare;
        }
        self.refuse("a call through a value, which this tier does not carry yet")
    }

    fn builtin_call(&mut self, b: Builtin, args: &[Code]) -> Result<V> {
        let (lo, hi) = b.arity();
        if args.len() < lo || args.len() > hi {
            return self.refuse(format!(
                "`{}` called with {} arguments",
                b.name(),
                args.len()
            ));
        }
        // `iterate` over a lambda literal is the loop, emitted in the body rather than called
        // through the runtime: `iterate` *is* the loop in this language (ADR 0022), so a tier that
        // sent it through a callback would be sending every loop through one.
        if b == Builtin::Iterate
            && args.len() == 3
            && matches!(&args[2].kind, NodeKind::Lambda { params, .. } if params.len() == 1)
        {
            return self.fused_iterate(&args[0], &args[1], &args[2]);
        }
        if b.higher_order() {
            return self.refuse(format!("`{}`, a builtin that calls user code", b.name()));
        }
        let mut vals = Vec::with_capacity(args.len());
        for a in args {
            vals.push(self.expr(a)?);
        }
        // The scalar family, inline: this is why the tier is worth having.
        if args.len() == 2
            && matches!(
                b,
                Builtin::WrapAdd | Builtin::WrapSub | Builtin::WrapMul | Builtin::Rotr
            )
            && let Kind::Num(t) = vals[0].k
            && carried(t)
        {
            let u = utype(t);
            let a = self.as_num(&vals[0], t);
            let e = match b {
                Builtin::Rotr => {
                    let n = self.as_int(&vals[1]);
                    let k = self.bind(Kind::Int, format!("(({n}) % {0} + {0}) % {0}", t.bits()));
                    format!(
                        "({u})({k} == 0 ? ({u})({a}) : (({u})({a}) >> {k}) | (({u})({a}) << ({} - {k})))",
                        t.bits(),
                        k = k.c
                    )
                }
                _ => {
                    let n = self.as_num(&vals[1], t);
                    let op = match b {
                        Builtin::WrapAdd => "+",
                        Builtin::WrapSub => "-",
                        _ => "*",
                    };
                    format!("({u})(({u})({a}) {op} ({u})({n}))")
                }
            };
            return Ok(self.bind(Kind::Num(t), e));
        }
        if args.len() == 1
            && let Some(t) = b.converts_into()
            && carried(t)
        {
            let n = self.as_int(&vals[0]);
            let held = self.bind(Kind::Int, n);
            self.line(format!(
                "if ({0} < INT64_C({1}) || {0} > INT64_C({2})) {{ rt_not_that_width_p(ctx, {3}, {0}); return 0; }}",
                held.c,
                t.min(),
                t.max(),
                t as i64
            ));
            return Ok(self.bind(Kind::Num(t), format!("({}){}", ctype(Kind::Num(t)), held.c)));
        }
        if args.len() == 1
            && let Some(t) = b.converts_from()
            && carried(t)
        {
            let e = self.as_int(&vals[0]);
            return Ok(self.bind(Kind::Int, e));
        }
        // The three the integer kernel reads its input through, inline with a slow path. A hash
        // asks `bytes_at` once per byte --- sixty-five thousand times over this kernel's input ---
        // and each one through the runtime is an argument array, a duplicate, and a dispatch on a
        // builtin index. The Cranelift tier inlines the same three for the same reason.
        if let Some(v) = self.inline_bytes(b, &vals)? {
            return Ok(v);
        }
        // Everything else goes through the runtime, which is the interpreter's own path. It
        // answers with a word of no known type -- except that a width this tier does not carry
        // stays uncarried through it, so that an operator downstream refuses rather than reading
        // a `U64` as an `Int`.
        let opaque = b.converts_into().is_some_and(|t| !carried(t))
            || vals.iter().any(|v| v.ty == CTy::Opaque);
        let mut ws = Vec::with_capacity(vals.len());
        for v in &vals.clone() {
            ws.push(self.owned(v));
        }
        let arr = self.fresh();
        self.line(format!(
            "Word {arr}[] = {{{}}};",
            if ws.is_empty() {
                "0".to_string()
            } else {
                ws.join(", ")
            }
        ));
        let index = self.unit.builtin(b);
        let v = self.bind_as(
            Kind::Boxed,
            if opaque { CTy::Opaque } else { CTy::Unknown },
            format!(
                "rt_builtin_p(ctx, {index}, (Word)(intptr_t){arr}, {})",
                ws.len()
            ),
        );
        self.check();
        Ok(v)
    }

    /// `iterate(seed, budget, |s| ..)` as a `for(;;)`: the step's body inlined, its parameter the
    /// loop's state, and `Stop`/`Continue` read off the answer's header rather than matched.
    fn fused_iterate(&mut self, seed: &Code, budget: &Code, step: &Code) -> Result<V> {
        let (Some(stop), Some(go)) = (self.unit.layouts.stop, self.unit.layouts.go) else {
            return self.refuse("`iterate` with no `Stop` and `Continue` in the program");
        };
        let NodeKind::Lambda { params, body, .. } = &step.kind else {
            unreachable!("checked by the caller")
        };
        let s = self.expr(seed)?;
        // The loop's state has the seed's type: `iterate` answers `Continue(state)` of the same
        // type it was handed, so the step's parameter is the seed's. Without this every read of
        // the state goes through the runtime by name, which for a fold over a record is a call
        // per field per iteration.
        let state_ty = s.ty.clone();
        let sw = self.word(&s);
        let state = self.fresh();
        self.line(format!("Word {state} = {sw};"));
        let b = self.expr(budget)?;
        let bud = self.as_int(&b);
        let left = self.fresh();
        self.line(format!("int64_t {left} = {bud};"));
        let answer = self.fresh();
        self.line(format!("Word {answer} = 0;"));
        self.line("for (;;) {");
        self.depth += 1;
        self.line(format!(
            "if ({left} <= 0) {{ rt_iterate_bad_p(ctx, 0, {bud}); return 0; }}"
        ));
        self.line(format!("{left} -= 1;"));
        let mark = self.scope.len();
        let held = self.bind_as(Kind::Boxed, state_ty.clone(), state.clone());
        self.scope.push((params[0].clone(), held));
        // The step's answer is a `Stop` or a `Continue` that this loop takes apart one line later.
        // When its shape says so all the way down, write straight into the loop's own control
        // instead: no constructor built, none taken apart, and one fewer object to dismantle per
        // iteration -- which over a hash is one per 64-byte block.
        if self.fusable_step(body) {
            self.emit_step(body, &state, &answer, stop, go)?;
            self.scope.truncate(mark);
            self.depth -= 1;
            self.line("}");
            return Ok(V::boxed(answer));
        }
        let r = self.expr(body)?;
        let rw = self.word(&r);
        let step_answer = self.bind(Kind::Boxed, rw);
        self.scope.truncate(mark);
        let k = self.fresh();
        self.line(format!(
            "uint32_t {k} = (!ply_is_imm({0}) && {0} != 0 && ply_obj({0})->kind == 4 && ply_obj({0})->len == 1) ? ply_obj({0})->layout : 0xFFFFFFFFu;",
            step_answer.c
        ));
        self.line(format!("if ({k} == {stop}u) {{"));
        self.depth += 1;
        self.line(format!("{answer} = ply_words({})[0];", step_answer.c));
        self.line(format!("ply_inc({answer});"));
        self.line("break;");
        self.depth -= 1;
        self.line(format!("}} else if ({k} == {go}u) {{"));
        self.depth += 1;
        self.line(format!("{state} = ply_words({})[0];", step_answer.c));
        self.line(format!("ply_inc({state});"));
        self.depth -= 1;
        self.line("} else {");
        self.depth += 1;
        self.line(format!(
            "rt_iterate_bad_p(ctx, 2, {}); return 0;",
            step_answer.c
        ));
        self.depth -= 1;
        self.line("}");
        self.depth -= 1;
        self.line("}");
        Ok(V::boxed(answer))
    }

    /// Whether every way out of this step is a `Stop` or a `Continue` written here, so that the
    /// loop can be given the payload rather than a constructor holding it.
    fn fusable_step(&self, code: &Code) -> bool {
        match &code.kind {
            NodeKind::App { func, args } if args.len() == 1 => {
                matches!(&func.kind, NodeKind::Var { name, .. }
                if self.ctor_index(name).is_some_and(|i| {
                    Some(i) == self.unit.layouts.stop || Some(i) == self.unit.layouts.go
                }))
            }
            NodeKind::If {
                then_branch,
                else_branch,
                ..
            } => self.fusable_step(then_branch) && self.fusable_step(else_branch),
            NodeKind::Block {
                tail: Some(tail), ..
            } => self.fusable_step(tail),
            _ => false,
        }
    }

    /// The step, with `Stop` and `Continue` written into the loop instead of built.
    fn emit_step(
        &mut self,
        code: &Code,
        state: &str,
        answer: &str,
        stop: u32,
        go: u32,
    ) -> Result<()> {
        match &code.kind {
            NodeKind::App { func, args } => {
                let NodeKind::Var { name, .. } = &func.kind else {
                    unreachable!("checked by `fusable_step`")
                };
                let which = self.ctor_index(name).expect("checked by `fusable_step`");
                let v = self.expr(&args[0])?;
                let w = self.word(&v);
                if Some(which) == self.unit.layouts.stop {
                    debug_assert_eq!(which, stop);
                    self.line(format!("{answer} = {w};"));
                    self.line("break;");
                } else {
                    debug_assert_eq!(which, go);
                    self.line(format!("{state} = {w};"));
                }
                Ok(())
            }
            NodeKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let c = self.expr(cond)?;
                let cb = self.as_bool(&c);
                self.line(format!("if ({cb}) {{"));
                self.depth += 1;
                self.emit_step(then_branch, state, answer, stop, go)?;
                self.depth -= 1;
                self.line("} else {");
                self.depth += 1;
                self.emit_step(else_branch, state, answer, stop, go)?;
                self.depth -= 1;
                self.line("}");
                Ok(())
            }
            NodeKind::Block { stmts, tail } => {
                let mark = self.block_stmts(stmts)?;
                self.emit_step(
                    tail.as_ref().expect("checked by `fusable_step`"),
                    state,
                    answer,
                    stop,
                    go,
                )?;
                self.scope.truncate(mark);
                Ok(())
            }
            _ => unreachable!("checked by `fusable_step`"),
        }
    }

    /// `bytes_at`, `bytes_len` and `len`, read straight off the object's header and payload when
    /// it is the kind they want, and left to the runtime when it is not.
    fn inline_bytes(&mut self, b: Builtin, vals: &[V]) -> Result<Option<V>> {
        let (kind, want, known) = match (b, vals.len()) {
            (Builtin::BytesAt, 2) => (crate::heap::KIND_BYTES, 2, vals[0].ty == CTy::Bytes),
            (Builtin::BytesU32Le, 2) => (crate::heap::KIND_BYTES, 2, vals[0].ty == CTy::Bytes),
            (Builtin::BytesLen, 1) => (crate::heap::KIND_BYTES, 1, vals[0].ty == CTy::Bytes),
            (Builtin::Len, 1) => (crate::heap::KIND_LIST, 1, vals[0].ty == CTy::List),
            _ => return Ok(None),
        };
        // With the type known there is no kind to test and no slow path to keep: an index out of
        // range raises, as the builtin does, and everything else is a load. This is the input path
        // of every byte-oriented kernel — `block_words` reads sixty-four bytes per block — and the
        // difference between a test with a cold call beside it and a load is the whole cost.
        if known {
            let t = {
                let w = self.word(&vals[0]);
                self.bind(Kind::Boxed, w)
            };
            // One bounds test and one four-byte load, where the same answer assembled a byte at a
            // time cost four of each plus the shifts. `memcpy` of four bytes is how a C compiler
            // is told to emit an unaligned load: it lowers to a single `ldur` and never a call.
            if matches!(b, Builtin::BytesU32Le) {
                let i = self.as_int(&vals[1]);
                let idx = self.bind(Kind::Int, i);
                let index = self.unit.builtin(b);
                self.line(format!(
                    "if ((uint64_t){0} + 4 > (uint64_t)ply_obj({1})->len) {{ Word a[2]; a[0] = {1}; ply_inc(a[0]); a[1] = ply_imm({0}); rt_builtin_p(ctx, {index}, (Word)(intptr_t)a, 2); return 0; }}",
                    idx.c, t.c
                ));
                let w = self.fresh();
                self.line(format!(
                    "uint32_t {w}; memcpy(&{w}, (unsigned char *)ply_words({}) + {}, 4);",
                    t.c, idx.c
                ));
                return Ok(Some(V {
                    k: Kind::Num(IntTy::U32),
                    c: format!("ply_le32({w})"),
                    ty: CTy::Num(IntTy::U32),
                }));
            }
            if want == 2 {
                let i = self.as_int(&vals[1]);
                let idx = self.bind(Kind::Int, i);
                let index = self.unit.builtin(b);
                self.line(format!(
                    "if ((uint64_t){0} >= (uint64_t)ply_obj({1})->len) {{ Word a[2]; a[0] = {1}; ply_inc(a[0]); a[1] = ply_imm({0}); rt_builtin_p(ctx, {index}, (Word)(intptr_t)a, 2); return 0; }}",
                    idx.c, t.c
                ));
                return Ok(Some(self.bind_as(
                    Kind::Int,
                    CTy::Int,
                    format!("(int64_t)((unsigned char *)ply_words({}))[{}]", t.c, idx.c),
                )));
            }
            return Ok(Some(self.bind_as(
                Kind::Int,
                CTy::Int,
                format!("(int64_t)ply_obj({})->len", t.c),
            )));
        }
        let target = self.word(&vals[0]);
        let t = self.bind(Kind::Boxed, target);
        let out = self.fresh();
        self.line(format!("int64_t {out} = 0;"));
        let ok = self.fresh();
        self.line(format!(
            "int {ok} = (!ply_is_imm({0}) && {0} != 0 && ply_obj({0})->kind == {kind});",
            t.c
        ));
        if want == 2 {
            let i = self.as_int(&vals[1]);
            let idx = self.bind(Kind::Int, i);
            self.line(format!(
                "if ({ok} && (uint64_t){0} < (uint64_t)ply_obj({1})->len) {{ {out} = (int64_t)((unsigned char *)ply_words({1}))[{0}]; }} else {{",
                idx.c, t.c
            ));
            self.depth += 1;
            let index = self.unit.builtin(b);
            let a0 = self.fresh();
            self.line(format!("Word {a0}[2];"));
            self.line(format!("{a0}[0] = {}; ply_inc({a0}[0]);", t.c));
            self.line(format!(
                "{a0}[1] = (ply_fits_imm({0}) ? ply_imm({0}) : rt_box_int_p(ctx, {0}));",
                idx.c
            ));
            self.line(format!(
                "Word r = rt_builtin_p(ctx, {index}, (Word)(intptr_t){a0}, 2);"
            ));
            self.line("if (ctx->failed) return 0;");
            self.line(format!("{out} = ply_imm_value(r);"));
            self.depth -= 1;
            self.line("}");
        } else {
            self.line(format!(
                "if ({ok}) {{ {out} = (int64_t)ply_obj({0})->len; }} else {{",
                t.c
            ));
            self.depth += 1;
            let index = self.unit.builtin(b);
            let a0 = self.fresh();
            self.line(format!("Word {a0}[1];"));
            self.line(format!("{a0}[0] = {}; ply_inc({a0}[0]);", t.c));
            self.line(format!(
                "Word r = rt_builtin_p(ctx, {index}, (Word)(intptr_t){a0}, 1);"
            ));
            self.line("if (ctx->failed) return 0;");
            self.line(format!("{out} = ply_imm_value(r);"));
            self.depth -= 1;
            self.line("}");
        }
        Ok(Some(V {
            k: Kind::Int,
            c: out,
            ty: CTy::Int,
        }))
    }

    fn match_expr(&mut self, scrutinee: &Code, arms: &[Arm]) -> Result<V> {
        let s = self.expr(scrutinee)?;
        let sw = self.word(&s);
        let held = self.bind(Kind::Boxed, sw);
        let out = self.fresh();
        self.line(format!("Word {out} = 0;"));
        let done = self.fresh();
        self.line(format!("int {done} = 0;"));
        for arm in arms {
            if arm.guard.is_some() {
                return self.refuse("a `match` arm with a guard");
            }
            self.line(format!("if (!{done}) {{"));
            self.depth += 1;
            let test = self.test(&arm.pat, &held)?;
            self.line(format!("if ({test}) {{"));
            self.depth += 1;
            let mark = self.scope.len();
            self.bind_pattern(&arm.pat, &held)?;
            let body = self.expr(&arm.body)?;
            let bw = self.word(&body);
            self.line(format!("{out} = {bw};"));
            self.line(format!("{done} = 1;"));
            self.scope.truncate(mark);
            self.depth -= 1;
            self.line("}");
            self.depth -= 1;
            self.line("}");
        }
        self.line(format!("if (!{done}) {{ rt_no_match_p(ctx); return 0; }}"));
        Ok(V::boxed(out))
    }

    /// A C expression that is true when `v` matches `pat`. Binding is separate, so a test that
    /// fails has bound nothing.
    fn test(&mut self, pat: &Pat, v: &V) -> Result<String> {
        match pat {
            Pat::Wildcard => Ok("1".to_string()),
            Pat::Var { slot: Some(_), .. } => Ok("1".to_string()),
            Pat::Var { name, .. } => {
                // A nullary constructor wearing a variable's shape.
                match self.ctor_index(&QName::bare(name.clone())) {
                    Some(i) => Ok(format!(
                        "(!ply_is_imm({0}) && ply_obj({0})->kind == 4 && ply_obj({0})->layout == {i})",
                        v.c
                    )),
                    None => Ok("1".to_string()),
                }
            }
            Pat::Lit(Lit::Int(k)) => Ok(format!(
                "(ply_is_imm({0}) ? ply_imm_value({0}) == INT64_C({k}) : 0)",
                v.c
            )),
            Pat::Lit(Lit::Fixed { bits, .. }) => Ok(format!(
                "(ply_is_imm({0}) ? ply_imm_value({0}) == INT64_C({1}) : 0)",
                v.c, *bits as i64
            )),
            Pat::Lit(Lit::Bool(b)) => Ok(format!(
                "({0} == {1})",
                v.c,
                if *b { true_word() } else { false_word() }
            )),
            Pat::Ctor { name, args } => {
                let Some(i) = self.ctor_index(name) else {
                    return self.refuse(format!("the constructor `{}`", name.symbol()));
                };
                let mut test = format!(
                    "(!ply_is_imm({0}) && ply_obj({0})->kind == 4 && ply_obj({0})->layout == {i} && ply_obj({0})->len == {1})",
                    v.c,
                    args.len()
                );
                for (k, sub) in args.iter().enumerate() {
                    if matches!(sub, Pat::Wildcard | Pat::Var { .. }) {
                        continue;
                    }
                    let field = self.bind(
                        Kind::Boxed,
                        format!("({0} ? ply_words({1})[{k}] : 0)", test.clone(), v.c),
                    );
                    let inner = self.test(sub, &field)?;
                    test = format!("({test} && {inner})");
                }
                Ok(test)
            }
            Pat::Record { .. } | Pat::List { .. } | Pat::Lit(_) => {
                self.refuse("a pattern this tier does not carry yet")
            }
        }
    }

    fn ctor_index(&self, name: &QName) -> Option<u32> {
        self.unit.ctor_index(name.symbol())
    }

    fn bind_pattern(&mut self, pat: &Pat, v: &V) -> Result<()> {
        match pat {
            Pat::Wildcard => Ok(()),
            Pat::Var {
                name,
                slot: Some(_),
            } => {
                let held = self.bind(Kind::Boxed, v.c.clone());
                self.scope.push((name.name.clone(), held));
                Ok(())
            }
            Pat::Var { .. } | Pat::Lit(_) => Ok(()),
            Pat::Ctor { args, .. } => {
                for (k, sub) in args.iter().enumerate() {
                    let field = self.bind(Kind::Boxed, format!("ply_words({})[{k}]", v.c));
                    self.line(format!("ply_inc({});", field.c));
                    self.bind_pattern(sub, &field)?;
                }
                Ok(())
            }
            _ => self.refuse("a pattern this tier does not carry yet"),
        }
    }
}

/// How many fields a record type has, when the type says.
fn record_width(ty: &CTy) -> Option<usize> {
    match ty {
        CTy::Record(fields) => Some(fields.len()),
        _ => None,
    }
}

fn describe(kind: &NodeKind) -> &'static str {
    match kind {
        NodeKind::Perform { .. } => "a `perform`",
        NodeKind::Handle { .. } => "a `handle`",
        NodeKind::WithCell { .. } => "a `with cell`",
        NodeKind::Simulate { .. } => "a `simulate`",
        _ => "a construct this tier does not carry yet",
    }
}

/// The two `Bool` singletons and `Unit`, as the addresses they are in this process. The unit is
/// built and loaded by the process that will call it, so an address is a constant here in a way it
/// could never be in an artefact meant to outlive the run.
fn true_word() -> String {
    format!("((Word)INT64_C({}))", crate::heap::bool(true))
}
fn false_word() -> String {
    format!("((Word)INT64_C({}))", crate::heap::bool(false))
}
fn unit_word() -> String {
    format!("((Word)INT64_C({}))", crate::heap::unit())
}
