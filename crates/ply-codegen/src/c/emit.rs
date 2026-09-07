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
use ply_eval::code::{Arm, Captures, Pat, Stmt};
use ply_eval::rc::Own;
use ply_eval::{Builtin, Code, NodeKind, Value};
use ply_span::Symbol;
use ply_syntax::ast::{BinOp, IntTy, Lit, QName, UnOp};
use ply_syntax::resolve::Namespace;

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
    /// Locals holding a count on an object **this body just made** and nothing else can name.
    ///
    /// A helper that allocates hands back a word with a count of one, and the emitter then took a
    /// second one at every use -- so `{a: f(x)}` built the field, counted it once for the record
    /// and once more for nobody, and the second count was never released. That is the leak the
    /// state kernel showed at 56 bytes an iteration *per temporary*, and it is `ply_inc` sitting at
    /// 11.3% of the compiled front end's own profile.
    ///
    /// Only the fresh ones. A word read out of a record or a list is aliased by the thing it came
    /// from, and moving a count out of an alias is how a record gets freed under a later read of
    /// it -- ADR 0034's take, deleted twice. An allocation a helper just made is aliased by
    /// nothing, so handing its count on is unambiguous.
    made: std::collections::HashSet<String>,
    /// Bases already let go of, so that a second read marked as a last use does not let go again.
    released: std::collections::HashSet<String>,
    /// A local that is only another local's name, to the one it renames. The inliner binds a `let`
    /// per argument, so one object commonly wears several names and a rule about *the object* has
    /// to see through them.
    alias: std::collections::HashMap<String, String>,
    /// How many times each name is read in this body. A release is only safe where the answer is
    /// one: see `release_base`.
    reads: std::collections::HashMap<Symbol, usize>,
    /// For each object -- named by the local at the end of its rename chain -- how many times the
    /// tree reads *any* name for it. Accumulated as names are bound rather than searched for at
    /// each release, which on the self-hosted front end is the difference between 190ms of emit
    /// and 800ms. Never decremented: a name counted after its scope ended suppresses a release,
    /// which leaks, where missing one frees a live object.
    reads_by_root: std::collections::HashMap<String, usize>,
    /// The (object, name) pairs already added to `reads_by_root`, so that rebinding a name to the
    /// same object does not count its reads twice.
    counted: std::collections::HashSet<(String, Symbol)>,
    /// Whether `count_reads` has run. `bind_name` reads the table it fills.
    counted_reads: bool,
    /// The nodes in the body's tail position, by identity. Filled by `mark_tails` before anything
    /// is emitted, and the one place an update is allowed to let its base go without asking how
    /// many other names read it.
    tails: std::collections::HashSet<*const Code>,
    /// The lambda bodies this definition contains, as whole C functions, emitted beside it rather
    /// than inside it. Its length is how the next one is numbered; which *row* of the unit's code
    /// table each takes is `tables.lambdas`, which also holds the entries of definitions this body
    /// uses as values and so is not parallel to this.
    lambda_defs: Vec<String>,
    /// The unit-wide things this body names, in the order it met them.
    ///
    /// The text says `@@c3@@` where a constant's index belongs and `assemble` rewrites it, so a
    /// body's C is a function of *the body* rather than of what else happened to be emitted
    /// beside it. That is what lets one be kept and reused in another unit -- and the emitter's
    /// time is dominated by the inliner that produces the body, so keeping the text is keeping
    /// nearly all of it.
    pub tables: Tables,
}

/// What one body names of the unit around it, by the positions its own text uses.
#[derive(Default, Clone)]
pub struct Tables {
    pub consts: Vec<Value>,
    pub builtins: Vec<Builtin>,
    pub fields: Vec<Symbol>,
    pub shapes: Vec<Vec<Symbol>>,
    /// Every definition this body calls, so that a body restored from a cache can be checked
    /// against the set the fixpoint took rather than trusted.
    pub calls: Vec<String>,
    /// The C symbols of the lambda entries this body defines, in the order it met them. A closure
    /// names its code by an index into the unit's table of them, and the body's own text writes
    /// its own position, which `resolve` rewrites into the unit's.
    pub lambdas: Vec<String>,
}

impl Tables {
    fn at<T: PartialEq>(xs: &mut Vec<T>, x: T) -> usize {
        match xs.iter().position(|y| *y == x) {
            Some(i) => i,
            None => {
                xs.push(x);
                xs.len() - 1
            }
        }
    }
}

/// Where a release is being made from. A field read and a record update both see the lowering's
/// last use, and the two are not equally safe to act on.
#[derive(PartialEq, Eq, Clone, Copy)]
enum Site {
    /// A field read: the base may be read again through a name this one does not know about, at a
    /// point the emitter reaches later than the lowering marked it.
    Field,
    /// A record update, which has read every field it copies before it lets go.
    Update,
    /// The same, in the body's tail position: nothing in this function is emitted after it, so
    /// there is no later read to be wrong about. See `mark_tails`.
    TailUpdate,
}

/// One function's worth of emitter state, so that a lambda can be written as a function of its
/// own without losing its owner's place. Every field here is one of [`Emit`]'s.
#[derive(Default)]
struct Frame {
    out: String,
    tmp: usize,
    scope: Vec<(Symbol, V)>,
    depth: usize,
    tokens: std::collections::BTreeSet<usize>,
    built: std::collections::HashMap<String, Vec<(Symbol, V)>>,
    deferred: std::collections::HashMap<String, Deferred>,
    record_locals: Vec<String>,
    made: std::collections::HashSet<String>,
    released: std::collections::HashSet<String>,
    alias: std::collections::HashMap<String, String>,
    reads: std::collections::HashMap<Symbol, usize>,
    reads_by_root: std::collections::HashMap<String, usize>,
    counted: std::collections::HashSet<(String, Symbol)>,
    counted_reads: bool,
}

/// A record that has been described but not built: what `emit_record` would have emitted.
struct Deferred {
    shape: String,
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
    /// The C symbol of every lambda entry in the unit. `rt_closure` is handed a position in this,
    /// and `Tables::functions` holds the address `dlsym` found for each.
    pub lambdas: Vec<String>,
}

impl Unit {
    pub fn new(ctors: Vec<(Symbol, usize)>, functions: Vec<String>) -> Unit {
        Unit {
            consts: Vec::new(),
            fields: Vec::new(),
            builtins: Vec::new(),
            functions,
            layouts: crate::heap::Layouts::new(ctors),
            lambdas: Vec::new(),
        }
    }

    /// The shape a field set interns to, in the same table the runtime will read it against.
    pub fn shape(&mut self, names: &[Symbol]) -> u32 {
        self.layouts.shape(names.to_vec())
    }
}

impl Unit {
    pub(super) fn constant(&mut self, v: Value) -> usize {
        self.consts.push(v);
        self.consts.len() - 1
    }

    pub(super) fn field(&mut self, name: &Symbol) -> usize {
        if let Some(i) = self.fields.iter().position(|f| f == name) {
            return i;
        }
        self.fields.push(name.clone());
        self.fields.len() - 1
    }

    pub(super) fn lambda(&mut self, symbol: &str) -> usize {
        if let Some(i) = self.lambdas.iter().position(|l| l == symbol) {
            return i;
        }
        self.lambdas.push(symbol.to_string());
        self.lambdas.len() - 1
    }

    pub(super) fn builtin(&mut self, b: Builtin) -> usize {
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
            made: std::collections::HashSet::new(),
            released: std::collections::HashSet::new(),
            alias: std::collections::HashMap::new(),
            reads: std::collections::HashMap::new(),
            reads_by_root: std::collections::HashMap::new(),
            counted: std::collections::HashSet::new(),
            counted_reads: false,
            tails: std::collections::HashSet::new(),
            lambda_defs: Vec::new(),
            tables: Tables::default(),
        }
    }

    /// `Word tok16 = 0;` for each width this body touched, for the prologue.
    pub fn token_decls(&self) -> String {
        self.tokens
            .iter()
            .map(|n| format!("  Word tok{n} = 0;\n"))
            .collect()
    }

    /// The nodes nothing in this function is emitted after.
    ///
    /// `release_base` cannot ask "is this the last read" of the tree, because the emitter does not
    /// walk it in the order the lowering marked -- a record's fields go in the shape's order, not
    /// the source's -- and two attempts to answer it by counting reads as they are emitted both
    /// failed the self-hosted front end. This asks a smaller question that has an answer: *is
    /// anything emitted after this at all*. Down a block's tail, and into both arms of an `if` and
    /// every arm of a `match`, because only one of those runs.
    ///
    /// Deliberately not into a call's arguments, which is what keeps a fused `fold` out: its
    /// lambda body is emitted once and runs once per iteration, so "nothing after it" is true of
    /// the text and false of the execution.
    pub fn mark_tails(&mut self, code: &Code) {
        self.tails.insert(code as *const Code);
        match &code.kind {
            NodeKind::Block { tail: Some(t), .. } => self.mark_tails(t),
            NodeKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                self.mark_tails(then_branch);
                self.mark_tails(else_branch);
            }
            NodeKind::Match { arms, .. } => {
                for arm in arms.iter() {
                    self.mark_tails(&arm.body);
                }
            }
            _ => {}
        }
    }

    /// Count every bare name the body reads, once, before anything is emitted.
    ///
    /// `release_base` needs to know whether a binding is read anywhere else, and the emitter walks
    /// the tree in an order the tree does not fix -- a record's fields are emitted in the shape's
    /// order, not the source's -- so "is this the last read" cannot be answered by counting as it
    /// goes. One read is the answer it can trust.
    pub fn count_reads(&mut self, code: &Code) {
        self.counted_reads = true;
        if let NodeKind::Var { name, .. } = &code.kind
            && name.is_bare()
        {
            *self.reads.entry(name.symbol().clone()).or_insert(0) += 1;
        }
        let mut go = |c: &Code| self.count_reads(c);
        match &code.kind {
            NodeKind::Unary { operand, .. } => go(operand),
            NodeKind::Binary { lhs, rhs, .. } => {
                go(lhs);
                go(rhs);
            }
            NodeKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                go(cond);
                go(then_branch);
                go(else_branch);
            }
            NodeKind::Block { stmts, tail } => {
                for st in stmts.iter() {
                    go(st.code());
                }
                if let Some(t) = tail {
                    go(t);
                }
            }
            NodeKind::Field { base, .. } => go(base),
            NodeKind::Record { fields } => fields.iter().for_each(|(_, c)| go(c)),
            NodeKind::RecordUpdate { base, sets, .. } => {
                go(base);
                sets.iter().for_each(|(_, c)| go(c));
            }
            NodeKind::List { items } => items.iter().for_each(&mut go),
            NodeKind::App { func, args } => {
                go(func);
                args.iter().for_each(&mut go);
            }
            NodeKind::Match { scrutinee, arms } => {
                go(scrutinee);
                for a in arms.iter() {
                    if let Some(g) = &a.guard {
                        go(g);
                    }
                    go(&a.body);
                }
            }
            NodeKind::Lambda { body, .. } => go(body),
            _ => {}
        }
    }

    /// A constant, a builtin, a field or a shape as this body's own position in it, written as the
    /// placeholder `assemble` rewrites.
    fn local_const(&mut self, v: Value) -> String {
        // `Value` has no equality that means "the same literal", so constants are not deduplicated
        // here; the unit's own pool does that when the placeholder is resolved.
        self.tables.consts.push(v);
        format!("@@c{}@@", self.tables.consts.len() - 1)
    }
    fn local_builtin(&mut self, b: Builtin) -> String {
        format!("@@b{}@@", Tables::at(&mut self.tables.builtins, b))
    }
    fn local_field(&mut self, name: &Symbol) -> String {
        format!("@@f{}@@", Tables::at(&mut self.tables.fields, name.clone()))
    }
    /// The position of a code address in the unit's table, as this body's own.
    fn local_lambda(&mut self, symbol: &str) -> String {
        format!(
            "@@l{}@@",
            Tables::at(&mut self.tables.lambdas, symbol.to_string())
        )
    }
    fn local_shape(&mut self, names: &[Symbol]) -> String {
        format!(
            "@@s{}@@",
            Tables::at(&mut self.tables.shapes, names.to_vec())
        )
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

    /// Whether `expr` is a call to a runtime helper that answers a word.
    ///
    /// Read off the expression text rather than passed at each of the two dozen call sites,
    /// because the property belongs to the *helper* and the table in `prelude.rs` is where helpers
    /// are declared. A helper added there is covered here without anyone remembering to.
    ///
    /// Every such helper answers an **owned** word: `rt_field` increments what it reads out,
    /// `rt_ctor` allocates, `rt_concat` builds a new string, and each of them releases the word
    /// arguments it was given -- which is what `owned` duplicates for on the way in. So the count
    /// this body receives is its own to hand on, and taking a second one is the leak.
    fn answers_owned(expr: &str) -> bool {
        let Some(open) = expr.find('(') else {
            return false;
        };
        let name = &expr[..open];
        super::prelude::HELPERS
            .iter()
            .any(|h| h.answers && super::prelude::pointer_name(h.name) == name)
    }

    fn bind_as(&mut self, k: Kind, ty: CTy, expr: impl AsRef<str>) -> V {
        let name = self.fresh();
        let ct = ctype(k);
        let e = expr.as_ref().to_string();
        // A binding that is only another local's name is recorded as such, so that a rule about the
        // object it names sees one thing rather than two.
        if e.len() > 1
            && (e.starts_with('t') || e.starts_with('p'))
            && e[1..].bytes().all(|c| c.is_ascii_digit())
        {
            let root = self.root(&e);
            self.alias.insert(name.clone(), root);
        }
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
        if k == Kind::Boxed && Emit::answers_owned(&e) {
            self.made.insert(name.clone());
        }
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
        self.bind_name(name.clone(), v);
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
        let (shape, n, flags) = (d.shape.clone(), d.n, d.flags);
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

    /// A word a helper is about to take. Duplicated first, because a helper that takes will
    /// release: the duplicate is what puts the count back.
    ///
    /// Unless this body already holds a count on something nothing else can name, in which case it
    /// hands *that* one on. Removed as it is spent, so a second use of the same local takes one of
    /// its own -- which is what makes handing on safe where a plain "it was freshly made" flag
    /// would not be.
    fn owned(&mut self, v: &V) -> String {
        let already = self.made.remove(&v.c);
        let w = self.word(v);
        let t = self.fresh();
        self.line(format!("Word {t} = {w};"));
        // A scalar is an immediate and holds no count, so there is nothing to take: the kernel
        // builds sixteen-field records of them and the increments were the whole of the cost.
        if !already && !matches!(v.k, Kind::Num(_) | Kind::Int | Kind::Bool) {
            self.line(format!("ply_inc({t});"));
        }
        t
    }

    /// Records that `v` holds a count on an object this body just made and nothing else can name.
    fn made_here(&mut self, v: V) -> V {
        self.made.insert(v.c.clone());
        v
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
            NodeKind::Field { base, field } => self.field(base, &field.name, code.own),
            NodeKind::Record { fields } => self.record(fields),
            NodeKind::List { items } => self.list(items),
            NodeKind::App { func, args } => self.app(func, args),
            NodeKind::Match { scrutinee, arms } => self.match_expr(scrutinee, arms),
            NodeKind::Lambda {
                params,
                body,
                captures,
                ..
            } => self.lambda(params, body, captures),
            NodeKind::RecordUpdate { base, copies, sets } => {
                let tail = self.tails.contains(&(code as *const Code));
                self.record_update(base, copies, sets, tail)
            }
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
                let index = self.local_const(value.clone());
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
                let index = self.local_const(value.clone());
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
            && self.took(&full)
        {
            self.tables.calls.push(full.clone());
            let v = self.bind(Kind::Boxed, format!("{}(ctx)", mangle(&full)));
            self.check();
            return Ok(v);
        }
        // A compiled function used as a value is a closure over nothing, through the same
        // handle-ABI entry the seam enters it by. Passing one to `map` is how most of the
        // self-hosted front end spells a callback, so without this the closure work above buys
        // the tier the lambda literals and none of the named functions.
        if let Some(full) = self.resolve_q(q) {
            // A name the program defines shadows a builtin spelled the same way, so a definition
            // this unit did not take has to refuse rather than fall through to the builtin behind
            // it. `std.hash` defines `min`, and there is a builtin `min`.
            if !self.took(&full) {
                return self.refuse(format!("`{full}`, which is not in this compiled unit"));
            }
            let Some((def, _)) = self.src.definition(&full) else {
                return self.refuse(format!("`{full}`, which resolves to no definition"));
            };
            let arity = def.params.len();
            self.tables.calls.push(full.clone());
            let index = self.local_lambda(&format!("{}_entry", mangle(&full)));
            let arr = self.fresh();
            self.line(format!("Word {arr}[] = {{0}};"));
            let v = self.bind(
                Kind::Boxed,
                format!("rt_closure_p(ctx, {index}, {arity}, (Word)(intptr_t){arr}, 0)"),
            );
            self.check();
            return Ok(self.made_here(v));
        }
        if q.is_bare()
            && let Some(b) = Builtin::from_name(q.symbol())
        {
            let index = self.local_builtin(b);
            let v = self.bind(Kind::Boxed, format!("rt_builtin_value_p(ctx, {index})"));
            self.check();
            return Ok(v);
        }
        // A constructor named as a value is two different things by arity, and the tier asked for
        // the wrong one: `None` is the singleton the tables hold, and `Some` is a function value.
        // Asking for the function in both cases put a closure where a variant belonged, so every
        // body that answered `None` answered wrongly -- which four tests in `examples/` had been
        // saying since this tier's first commit, to nothing that was listening.
        if let Some((i, arity)) = self.ctor_of(q) {
            let call = if arity == 0 {
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
        // The resolver first, which is the only thing that knows about `import spine (start, ..)`:
        // a selective import puts another module's name in this one's scope *bare*, and guessing
        // `{this module}.{name}` finds nothing for it. The self-hosted front end imports that way
        // throughout, so without this the tier refuses every body that calls across a module.
        let scoped = if q.is_bare() {
            self.src
                .resolved
                .scopes
                .get(self.module_index)
                .and_then(|s| s.get(Namespace::Value, q.symbol()))
                .map(|b| b.qualified.to_string())
        } else {
            self.src
                .resolved
                .lookup(self.module_index, Namespace::Value, q)
                .ok()
                .map(|b| b.qualified.to_string())
        };
        if let Some(full) = scoped
            && self.src.definition(&full).is_some()
        {
            return Some(full);
        }
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
                            "if (ply_{sign}_ov({a}, {b}, &{out})) {{ rt_overflow_p(ctx, {}); return 0; }}",
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
        // Both arms built a record of the same shape: join field by field rather than record by
        // record, so that a record whose only difference is *which branch made it* stays in
        // registers. Without this an `if` is where elision stops.
        if let (Some(tf), Some(ef)) = (self.built.get(&t.c).cloned(), self.built.get(&e.c).cloned())
            && tf.len() == ef.len()
            && tf
                .iter()
                .zip(&ef)
                .all(|((n1, v1), (n2, v2))| n1 == n2 && v1.k == v2.k && v1.k != Kind::Boxed)
        {
            let mut locals = Vec::with_capacity(tf.len());
            for (_, v) in &tf {
                let name = self.fresh();
                self.line(format!("{} {name} = 0;", ctype(v.k)));
                locals.push(V {
                    k: v.k,
                    c: name,
                    ty: v.ty.clone(),
                });
            }
            self.line(format!("if ({cb}) {{"));
            self.out.push_str(&t_text);
            self.depth += 1;
            for (i, (_, v)) in tf.iter().enumerate() {
                let x = self.as_kind(v, locals[i].k);
                self.line(format!("{} = {x};", locals[i].c));
            }
            self.depth -= 1;
            self.line("} else {");
            self.out.push_str(&e_text);
            self.depth += 1;
            for (i, (_, v)) in ef.iter().enumerate() {
                let x = self.as_kind(v, locals[i].k);
                self.line(format!("{} = {x};", locals[i].c));
            }
            self.depth -= 1;
            self.line("}");
            let names: Vec<Symbol> = tf.iter().map(|(n, _)| n.clone()).collect();
            let kinds: Vec<CTy> = locals.iter().map(|v| v.ty.clone()).collect();
            let mut words = Vec::with_capacity(locals.len());
            for v in &locals.clone() {
                words.push(self.word(v));
            }
            return Ok(self.emit_record(&names, words, kinds, Some(locals)));
        }
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
                            self.bind_name(name.name.clone(), held);
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

    fn field(&mut self, base: &Code, name: &Symbol, own: Own) -> Result<V> {
        let _ = own;
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
                let index = self.local_field(name);
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
            // ADR 0034's in-place field take is *not* here, and the reason is recorded rather
            // than left to be rediscovered.
            //
            // It read a field marked `Own::OwnedField`, moved its count out of the record and
            // wrote `unit` where it had been, so the next record of that shape could reuse the
            // memory. That is correct only where nothing reads the field again, and the emitter
            // cannot tell: the lowering marks the last use *in its own order*, and a field is read
            // by more than `Field` nodes -- a record update copies the fields it does not write, a
            // record pattern binds the ones it names, and the seam walks every field of a record
            // it converts. Counting the readers it can see admitted a take in `infer.dump_outcome`
            // whose `unit` reached `fold` as its list. Five tests of the self-hosted front end
            // said so; `examples/` and every smaller corpus passed throughout.
            //
            // What it was worth, measured on the kernel ADR 0034 cites -- a map read out of a
            // record and handed to `map_insert` -- is 30.75ms against 30.90ms, three runs each,
            // minimum. That is nothing, and it is the whole reason this is a deletion rather than
            // a puzzle to solve: a correct version needs the last read in *emission* order, which
            // is a real piece of work to buy half a percent.
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
    /// The local a name ultimately renames.
    fn root(&self, name: &str) -> String {
        let mut cur = name.to_string();
        let mut hops = 0;
        while let Some(next) = self.alias.get(&cur) {
            if *next == cur || hops > 64 {
                break;
            }
            cur = next.clone();
            hops += 1;
        }
        cur
    }

    /// Swap in a fresh function's worth of state, and hand back what was there.
    ///
    /// A lambda becomes a C function beside its owner rather than a block inside it, so every
    /// "where am I in this body" -- the statements, the temporaries, the scope, the reuse tokens,
    /// the ownership tables -- has to step aside while it is written and come back afterwards.
    fn swap(&mut self, f: &mut Frame) {
        std::mem::swap(&mut self.out, &mut f.out);
        std::mem::swap(&mut self.tmp, &mut f.tmp);
        std::mem::swap(&mut self.scope, &mut f.scope);
        std::mem::swap(&mut self.depth, &mut f.depth);
        std::mem::swap(&mut self.tokens, &mut f.tokens);
        std::mem::swap(&mut self.built, &mut f.built);
        std::mem::swap(&mut self.deferred, &mut f.deferred);
        std::mem::swap(&mut self.record_locals, &mut f.record_locals);
        std::mem::swap(&mut self.made, &mut f.made);
        std::mem::swap(&mut self.released, &mut f.released);
        std::mem::swap(&mut self.alias, &mut f.alias);
        std::mem::swap(&mut self.reads, &mut f.reads);
        std::mem::swap(&mut self.reads_by_root, &mut f.reads_by_root);
        std::mem::swap(&mut self.counted, &mut f.counted);
        std::mem::swap(&mut self.counted_reads, &mut f.counted_reads);
    }

    /// A lambda: the captured words as a closure over a compiled function.
    ///
    /// The same shape the in-process tier uses, because the runtime is shared: the body becomes a
    /// function whose leading parameters are the captures, and the closure object holds their
    /// words beside the code's address. `rt_call` enters it, and so does the interpreter through
    /// the seam, so a closure this tier makes is callable from everywhere one made by the other
    /// is.
    fn lambda(&mut self, params: &[Symbol], body: &Code, captures: &Captures) -> Result<V> {
        // The environment, in the order the lowering named it. `rt_closure` takes these, so each
        // is handed over owned.
        let mut env = Vec::with_capacity(captures.names.len());
        for name in &captures.names {
            let Some((_, v)) = self.scope.iter().rev().find(|(s, _)| s == name).cloned() else {
                return self.refuse(format!(
                    "a lambda capturing `{name}`, which is not a local of its body"
                ));
            };
            env.push(self.owned(&v));
        }
        let index = self.lambda_defs.len();
        let symbol = format!("{}_lambda{index}", mangle(&self.function));
        // Reserved before the body is emitted, so that a lambda nested inside this one numbers
        // itself after this one rather than over it.
        self.lambda_defs.push(String::new());
        let slot = self.local_lambda(&format!("{symbol}_entry"));

        let mut frame = Frame::default();
        self.swap(&mut frame);
        let emitted = self.lambda_body(&symbol, params, body, captures);
        self.swap(&mut frame);
        self.lambda_defs[index] = emitted?;

        let arr = self.fresh();
        self.line(format!(
            "Word {arr}[] = {{{}}};",
            if env.is_empty() {
                "0".to_string()
            } else {
                env.join(", ")
            }
        ));
        let v = self.bind(
            Kind::Boxed,
            format!(
                "rt_closure_p(ctx, {slot}, {}, (Word)(intptr_t){arr}, {})",
                params.len(),
                env.len()
            ),
        );
        self.check();
        Ok(self.made_here(v))
    }

    /// The lambda's own C function, emitted into a frame of its own.
    fn lambda_body(
        &mut self,
        symbol: &str,
        params: &[Symbol],
        body: &Code,
        captures: &Captures,
    ) -> Result<String> {
        self.count_reads(body);
        let names: Vec<&Symbol> = captures.names.iter().chain(params.iter()).collect();
        let mut head = format!("Word {symbol}(PlyCtx *ctx");
        for (i, name) in names.iter().enumerate() {
            head.push_str(&format!(", Word q{i}"));
            // No declared type: the checker publishes a scheme per definition, not per lambda, so
            // every parameter arrives boxed. Widths are lost at a lambda boundary, which is a
            // reason to fuse a loop rather than close over one wherever the shape allows.
            self.param(name, format!("q{i}"), CTy::Unknown);
        }
        head.push_str(") {\n");
        head.push_str(
            "  if (ctx->fuel <= 0) { rt_no_fuel_p(ctx); return 0; }\n  ctx->fuel -= 1;\n",
        );
        let answer = self.expr(body)?;
        let word = self.word(&answer);
        let mut out = head;
        out.push_str(&self.token_decls());
        out.push_str(&self.record_decls());
        out.push_str(&self.out);
        out.push_str(&format!("  ctx->fuel += 1;\n  return {word};\n}}\n"));
        out.push_str(&format!(
            "Word {symbol}_entry(PlyCtx *ctx, const Word *args) {{\n  return {symbol}(ctx{});\n}}\n",
            (0..names.len())
                .map(|i| format!(", args[{i}]"))
                .collect::<Vec<_>>()
                .join("")
        ));
        Ok(out)
    }

    /// The C functions this definition's lambdas became, to go out beside it.
    pub fn lambda_defs(&self) -> String {
        self.lambda_defs.concat()
    }

    /// Bind a tree name to a local, charging that name's reads to the object the local holds.
    fn bind_name(&mut self, name: Symbol, v: V) {
        // A real assert, not a `debug_assert`: the suite runs in release, and the failure this
        // guards is a use-after-free in emitted C rather than a wrong number.
        assert!(
            self.counted_reads,
            "`count_reads` has to run before any name is bound, or the binding charges zero reads \
             to the object it holds and `release_base` frees something still live"
        );
        let root = self.root(&v.c);
        if self.counted.insert((root.clone(), name.clone())) {
            let n = self.reads.get(&name).copied().unwrap_or(0);
            *self.reads_by_root.entry(root).or_insert(0) += n;
        }
        self.scope.push((name, v));
    }

    /// How many times the tree reads *any* name for the object `local` holds.
    ///
    /// Not the same question as how many times one name is read. The emitter renames freely --
    /// the inliner turns every argument into a `let`, and `bind_as` records a rename as an alias
    /// rather than a second object -- so one object can wear several tree names, each read once,
    /// with no increment between them. Counting a single name there says "one read, safe to
    /// release" about an object three other names still hold.
    fn shared_reads(&self, local: &str) -> usize {
        self.reads_by_root
            .get(&self.root(local))
            .copied()
            .unwrap_or(0)
    }

    fn release_base(&mut self, base: &Code, b: &V) {
        self.release_from(base, b, Site::Field)
    }

    fn release_from(&mut self, base: &Code, b: &V, from: Site) {
        if !(matches!(base.own, Own::Owned) && matches!(base.kind, NodeKind::Var { .. })) {
            return;
        }
        // At most once per binding. `Own::Owned` marks a *use* as the last one, and a caller that
        // hands a record to a function and then reads a field of it for a later argument has two
        // reads the lowering marks that way -- which is the shape every parser state setter in the
        // spike has. Releasing at both freed a record the first release had already given back,
        // and the seam then read a dead word out of the answer. The in-process tier avoids it by
        // marking the local moved; this is the same rule, keyed on the local the binding holds.
        //
        // Deliberately conservative in the other direction: two releases on two arms of an `if`
        // are both legitimate and only one survives here, which leaks rather than frees twice.
        if !self.released.insert(self.root(&b.c)) {
            return;
        }
        // And only where the binding is read *once*. `Own::Owned` marks a use as the last one, but
        // the emitter does not visit reads in the order the lowering marked them -- a record's
        // fields are emitted in the shape's order -- so a body that hands a record on and also
        // reads a field of it releases at whichever read the emitter reached second and then reads
        // the freed record through the other. That freed a parser's whole state under the seam's
        // feet, and `spine.shallower` -- `with_depth(p, p.depth - 1)` -- is three words of it.
        //
        // One read means no other name for the object is live, whatever the order. It costs the
        // release wherever a record is read field by field, which is where `record_update`'s own
        // release takes over: that one reads every copy before it lets go, by construction.
        //
        // Counted over every *name* for the object, not over the one the base happens to wear.
        // `spine.advance_n` is the case: `advance(c, p).p` inlines to a rename of `p`, read once
        // under its inlined name and three times more under `p`, so the one-read rule said yes
        // and the reset freed the parser state that `cur_span(c, p)` then read a field of.
        if !matches!(&base.kind, NodeKind::Var { name, .. } if name.is_bare()) {
            return;
        }
        // The rule applies at both sites, and the state kernel pays for it.
        //
        // An update reads every field it copies before it lets go, so it looked exempt: what the
        // rule guards against is a *field* read emitted after the release, and an update has none
        // left. Exempting it put the reuse back and cost correctness --
        // `let c1 = {..acc.cx, ty_params: map_new()}` in `infer.collect_effects` takes its base
        // from a field read whose object `duplicate(acc.cx, ..)` reads again, and the reclaim
        // freed it. The rule is about every later read of the *object*, and an update knows no
        // more about those than a field read does.
        //
        // Counting reads *as they are emitted* and releasing when none is left does not work
        // either, and it is the attractive one: emission order is execution order in straight-line
        // code, and a branch is safe both ways round. It still failed the front end, with and
        // without a guard for the fused loops -- where a body's reads are emitted once and run once
        // per iteration -- so something else the count does not see reads the object again. Do not
        // retry it without a smaller failing case in hand than thirteen thousand lines.
        //
        // The exception below is the answer, and it is position rather than counting. At the
        // body's *tail* nothing is emitted after the update, so the later read this guard exists
        // to protect cannot be there to protect -- which is a syntactic fact about the text rather
        // than an inference about order, and that is the whole difference from the two attempts
        // above. `mark_tails` says which nodes those are, and deliberately does not follow a
        // call's arguments: a fused `fold`'s lambda body is emitted once and runs once per
        // iteration, so "nothing after it" is true of the text and false of the execution.
        //
        // What it recovers, measured: the state kernel's `{..s, ..}` reads `s` seven times, so
        // every one of two hundred thousand iterations used to allocate a five-field record and
        // keep it. The kernel's resident memory falls 167MB to 156MB -- 56 bytes an iteration,
        // which is exactly the record. Its *clock* does not move, and that is the honest reading:
        // the allocation was already cheap. k2 stands at 3.5x against a bar of 3.0 either way.
        //
        // What is left is larger than this and is not a guard. This tier's return ABI hands back
        // a borrow -- a caller `ply_inc`s a call's result to keep it -- and nothing releases an
        // owned temporary, so `let k = key_of(x)` leaks the bytes it built whatever this rule
        // says. That is why the kernel is still 3.3GB over twenty repeats where the in-process
        // tier is 240MB. Fixing it means owned returns and a release for every temporary, which
        // is a discipline rather than a rule, and it is the open item on this tier.
        // A tail update is the exception, and the only one: nothing in this function is emitted
        // after it, so the read this guard exists to protect cannot be there to protect. Without
        // it a body that reads its record parameter more than once -- which is every accumulator
        // this tier compiles -- never lets the record go at all. The state kernel allocates two
        // hundred thousand five-field records and frees none: 3.7GB of resident memory against
        // the in-process tier's 240MB, and `{..s, a: s.a + x}` over a boxed field grows a run's
        // memory linearly in its iterations.
        if from != Site::TailUpdate && self.shared_reads(&b.c) != 1 {
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
                "if ({name}) {{ if (tok{n} == 0) {{ tok{n} = ply_reset_flat({name}); if (!tok{n}) tok{n} = rt_reset_p(ctx, {name}); }} else {{ ply_dec(ctx, {name}); }} {name} = 0; }}"
            ));
            return;
        }
        let base_local = &self.word(b);
        match record_width(&b.ty) {
            Some(n) => {
                self.tokens.insert(n);
                self.line(format!(
                    "if (tok{n} == 0) {{ tok{n} = ply_reset_flat({base_local}); if (!tok{n}) tok{n} = rt_reset_p(ctx, {base_local}); }} else {{ ply_dec(ctx, {base_local}); }}"
                ));
            }
            None => self.line(format!("ply_dec(ctx, {base_local});")),
        }
    }

    fn record(&mut self, fields: &[(Symbol, Code)]) -> Result<V> {
        // Evaluated in the order they are written, assembled in the order the shape holds them.
        //
        // These are not the same order -- a shape is interned under its sorted field names -- and
        // evaluating in the shape's order is a reordering the *lowering* does not know about. Its
        // ownership marks say which read of a name is the last one in its own order, so a body
        // that hands a record to one field's expression and reads a field of it for another
        // released at whichever the emitter reached second and then read the freed record through
        // the other. `spine.advance_n` is three lines of it: `advance(c, p).p` builds `{p: .., node:
        // cur_span(c, p)}`, and `node` sorts first while `p` consumes.
        let mut names: Vec<Symbol> = fields.iter().map(|(n, _)| n.clone()).collect();
        names.sort();
        let mut written: Vec<(Symbol, String, CTy, V)> = Vec::with_capacity(fields.len());
        for (name, code) in fields {
            let v = self.expr(code)?;
            let w = self.owned(&v);
            written.push((name.clone(), w, v.ty.clone(), v));
        }
        let mut words = Vec::with_capacity(fields.len());
        let mut kinds = Vec::with_capacity(fields.len());
        let mut vals = Vec::with_capacity(fields.len());
        for name in &names {
            let (_, w, ty, v) = written
                .iter()
                .find(|(n, ..)| n == name)
                .expect("a field of the shape");
            words.push(w.clone());
            kinds.push(ty.clone());
            vals.push(v.clone());
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
        let shape = self.local_shape(names);
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
        self.made_here(r)
    }

    /// `{..b, f: e}`: the written fields, then the copied ones read out of the base. Built fresh
    /// rather than updated in place — the in-place path is ADR 0034's and this tier does not have
    /// it yet, which costs an allocation and cannot be wrong.
    fn record_update(
        &mut self,
        base: &Code,
        copies: &[ply_syntax::ast::Ident],
        sets: &[(Symbol, Code)],
        tail: bool,
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
                            let index = self.local_field(name);
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
        // between an update and a leak. Through `release_base` rather than beside it: an update and
        // a field read of the same variable both see a last use, and two releases of one record is
        // what freed it under the seam's feet.
        //
        // Every copy was counted above before this runs, so the walk that lets the base's children
        // go leaves the ones this record keeps alone, and drops exactly the ones it replaced.
        self.release_from(base, &b, if tail { Site::TailUpdate } else { Site::Update });
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
        Ok(self.made_here(v))
    }

    fn app(&mut self, func: &Code, args: &[Code]) -> Result<V> {
        if let NodeKind::Var { name: q, .. } = &func.kind {
            let bare = q.symbol().as_str().to_string();
            if let Some(full) = self.resolve_q(q) {
                if !self.took(&full) {
                    return self.refuse(format!("`{full}`, which is not in this compiled unit"));
                }
                let (def, _) = self.src.definition(&full).expect("resolved");
                if def.params.len() != args.len() {
                    return self.refuse(format!("`{bare}` called with {} arguments", args.len()));
                }
                let ret = self.declared_ret(&full);
                // A pure nullary root whose answer is a handle: ask the runtime for the answer it
                // already has rather than building it again. Without this the gate's k1 rebuilds a
                // sixty-five-kilobyte byte literal per call, and a two-hundred-element list built
                // in a twenty-thousand-iteration fold costs 56ms here against 0.1ms in process --
                // not because the code is worse, but because the in-process tier remembers and
                // this one did not. One that answers a register is cheaper to call than to look up,
                // which is why the test is on the declared return.
                if args.is_empty()
                    && ret.kind() == Kind::Boxed
                    && ply_eval::memo::pure_by_published_row(
                        Some(self.src.check),
                        &Symbol::new(&full),
                    )
                {
                    self.tables.calls.push(full.clone());
                    let index = self.local_lambda(&format!("{}_entry", mangle(&full)));
                    let held = self.bind(Kind::Boxed, format!("rt_constant_p(ctx, {index})"));
                    self.check();
                    return Ok(V {
                        k: Kind::Boxed,
                        c: held.c,
                        ty: ret,
                    });
                }
                let mut ws = Vec::with_capacity(args.len());
                for a in args {
                    let v = self.expr(a)?;
                    ws.push(self.owned(&v));
                }
                self.tables.calls.push(full.clone());
                let call = format!(
                    "{}(ctx{}{})",
                    mangle(&full),
                    if ws.is_empty() { "" } else { ", " },
                    ws.join(", ")
                );
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
            if let Some((i, arity)) = self.ctor_of(q) {
                if arity != args.len() {
                    return self.refuse(format!(
                        "the constructor `{}` takes {arity} fields and was given {}",
                        q.symbol(),
                        args.len()
                    ));
                }
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
                return Ok(self.made_here(v));
            }
            let _ = bare;
        }
        // A call through a value: a local holding a closure, a parameter, a field, or an
        // expression that answers one. `rt_call` sorts out what it is -- a compiled closure it
        // enters directly, a builtin or a constructor through the interpreter's own -- which is
        // the same helper the in-process tier reaches for and the same object either tier builds.
        let f = self.expr(func)?;
        let callee = self.owned(&f);
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
            format!(
                "rt_call_p(ctx, {callee}, (Word)(intptr_t){arr}, {})",
                args.len()
            ),
        );
        self.check();
        Ok(v)
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
        // `fold` over a list is the other loop this language writes, and a tier that refuses it
        // does not merely fall back: the fold runs interpreted and crosses the seam into whatever
        // it calls, once per element, deep-converting the accumulator each way. Over a list of
        // two hundred thousand that is quadratic, and it is why the record kernel took ninety
        // seconds on this tier while the interpreter alone took less than one.
        if b == Builtin::Fold && args.len() == 3 {
            return self.fused_fold(&args[0], &args[1], &args[2]);
        }
        // `bytes_concat_all([a, b, ..])` joins the pieces without building the list. The runtime
        // has `rt_bytes_join` for exactly this and this tier declared it, bound it and never
        // called it -- so the state kernel's `key_of`, which is this shape, allocated a
        // two-element list and went through the generic builtin dispatch two hundred thousand
        // times. The in-process tier has taken this path since it was written.
        if b == Builtin::BytesConcatAll
            && args.len() == 1
            && let NodeKind::List { items } = &args[0].kind
        {
            let mut ws = Vec::with_capacity(items.len());
            for item in items.iter() {
                let v = self.expr(item)?;
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
                format!(
                    "rt_bytes_join_p(ctx, (Word)(intptr_t){arr}, {})",
                    items.len()
                ),
            );
            self.check();
            return Ok(v);
        }
        // The callback family, through the helpers the in-process tier uses. Those helpers walk
        // the list themselves and enter each call through `call_value`, so a compiled closure is
        // entered directly and an interpreted one goes back over the seam -- which is what makes
        // this the same answer either tier gives. The fused forms above are still preferred where
        // the shape allows: they keep the loop in the body, with no closure object per element.
        if b.higher_order() {
            let helper = match (b, args.len()) {
                (Builtin::Map, 2) => Some("rt_map_p"),
                (Builtin::Filter, 2) => Some("rt_filter_p"),
                (Builtin::Fold, 3) => Some("rt_fold_p"),
                (Builtin::MapFold, 3) => Some("rt_map_fold_p"),
                (Builtin::Iterate, 3) => Some("rt_iterate_p"),
                _ => None,
            };
            let Some(helper) = helper else {
                return self.refuse(format!("`{}`, a builtin that calls user code", b.name()));
            };
            let mut ws = Vec::with_capacity(args.len());
            for a in args {
                let v = self.expr(a)?;
                ws.push(self.owned(&v));
            }
            let v = self.bind(Kind::Boxed, format!("{helper}(ctx, {})", ws.join(", ")));
            self.check();
            return Ok(v);
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
        // The map, list and bytes family, called by name rather than dispatched. The generic path
        // below costs an argument array, a builtin index and a match on it for every call, and k2
        // makes four of those per element over two hundred thousand elements. The in-process tier
        // has called these directly since it was written, and the gate says what the difference
        // is: on the value kernel that tier was within the bar at 1.9x where this one was over it
        // at 4.2x, on the same runtime and the same data structures.
        if let Some(helper) = direct_helper(b, vals.len()) {
            let mut ws = Vec::with_capacity(vals.len());
            for v in &vals.clone() {
                ws.push(self.owned(v));
            }
            let v = self.bind(Kind::Boxed, format!("{helper}(ctx, {})", ws.join(", ")));
            self.check();
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
        let index = self.local_builtin(b);
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
    /// `fold(xs, init, f)`: the list walked here, with `f` called on each element.
    ///
    /// The accumulator and the element are both owned by this frame and both consumed by the
    /// call, so neither is duplicated on the way in -- which is what `owned` would do and what
    /// would leak one count per element.
    /// **`map` and `filter` are not fused here, and one attempt at it is recorded rather than
    /// left to be repeated.** The in-process tier fuses all three -- `jit.rs`'s `fused_loop`, with
    /// a `Step::Inline` that lowers a lambda literal in the loop's own body -- and this tier sends
    /// `map` and `filter` through `rt_map`/`rt_filter`, which walk the list themselves and enter
    /// each element through `call_value`. Over the self-hosted front end that is 273 `map` sites
    /// and 19 `filter` sites, and `call_value` is the largest item in its profile.
    ///
    /// The attempt fused both, inlining the lambda the way `fused_iterate` already does, and took
    /// the sites from 273 and 19 down to 5 and 2. The whole Rust suite passed, both corpora passed
    /// under `--audit-backend`, and the self-hosted front end's *check* phase went from 0.52s to
    /// 2.4s -- because it was aborting partway with `a word of kind 255 was read after its object
    /// died`, and no corpus in this tree reaches the shape that does it.
    ///
    /// What went wrong is the ownership question, in the form `release_from` above records two
    /// other answers to. A fused loop owns the element it reads, and an inlined body either spends
    /// that count or does not; the attempt asked the `made` set which, and the set is keyed on the
    /// local the value arrived in, while the body spends it through the *alias* `bind_as` makes
    /// when the parameter is bound. So it released a count the body had already handed on.
    ///
    /// A next attempt needs the release keyed on the object rather than the name -- `root` is the
    /// existing spelling of that -- and a corpus that reaches it, which is the harder half: the
    /// front end and `examples/` both pass with the bug in.
    fn fused_fold(&mut self, items: &Code, init: &Code, f: &Code) -> Result<V> {
        let xs = self.expr(items)?;
        // A count of the loop's own, because the loop releases at the end. `rt_list_at` reads the
        // list rather than taking it, so one count covers every element -- but the fold had been
        // releasing a count it never took, and a caller that reads the list *again* then walked
        // freed memory. `len(walk(xs).types) + len(walk(xs).aliases)` is three lines of that.
        let list = V {
            k: Kind::Boxed,
            c: self.owned(&xs),
            ty: xs.ty.clone(),
        };
        // Through the runtime, once, rather than off the header: the list usually comes from
        // `range`, whose answer the fragment has no type for, and `len` is where a value that is
        // not a list is caught -- with the diagnostic the interpreter would have given.
        let len = self.local_builtin(Builtin::Len);
        let arr = self.fresh();
        self.line(format!(
            "Word {arr}[1] = {{{}}}; ply_inc({arr}[0]);",
            list.c
        ));
        let n_word = self.bind(
            Kind::Boxed,
            format!("rt_builtin_p(ctx, {len}, (Word)(intptr_t){arr}, 1)"),
        );
        self.check();
        let n_e = self.as_int(&n_word);
        let n = self.bind(Kind::Int, n_e);
        // A function this tier cannot name is held as a value, evaluated once outside the loop
        // rather than per element. `rt_call` takes its callee, so each iteration hands it a
        // count of its own and the loop lets go of the last one at the end.
        let held = match self.nameable_step(f, 2) {
            Some(_) => None,
            None => {
                let v = self.expr(f)?;
                Some(self.owned(&v))
            }
        };
        let seed = self.expr(init)?;
        let sw = self.word(&seed);
        let acc = self.fresh();
        self.line(format!("Word {acc} = {sw};"));
        let i = self.fresh();
        self.line(format!("int64_t {i} = 0;"));
        self.line(format!("for (; {i} < {}; {i} += 1) {{", n.c));
        self.depth += 1;
        let x = self.bind(Kind::Boxed, format!("rt_list_at_p(ctx, {}, {i})", list.c));
        self.check();
        let call = match &held {
            Some(callee) => {
                let arr = self.fresh();
                self.line(format!("Word {arr}[] = {{{acc}, {}}};", x.c));
                self.line(format!("ply_inc({callee});"));
                format!("rt_call_p(ctx, {callee}, (Word)(intptr_t){arr}, 2)")
            }
            None => self.step_call(f, acc.clone(), x.c.clone())?,
        };
        self.line(format!("{acc} = {call};"));
        self.check();
        self.depth -= 1;
        self.line("}");
        if let Some(callee) = &held {
            self.line(format!("ply_dec(ctx, {callee});"));
        }
        self.line(format!("ply_dec(ctx, {});", list.c));
        Ok(V {
            k: Kind::Boxed,
            c: acc,
            ty: CTy::Unknown,
        })
    }

    /// The program-wide name of a definition of `arity` that `f` denotes directly, if it does.
    fn nameable_step(&self, f: &Code, arity: usize) -> Option<String> {
        let NodeKind::Var { name, .. } = &f.kind else {
            return None;
        };
        let full = self.resolve_q(name)?;
        (self.took(&full)
            && self
                .src
                .definition(&full)
                .is_some_and(|(d, _)| d.params.len() == arity))
        .then_some(full)
    }

    /// Whether the unit carries `full`. A no ends in a refusal at every caller, and a refusal is
    /// cached against the digest of what was offered, so nothing more has to travel with it.
    fn took(&self, full: &str) -> bool {
        self.unit.functions.iter().any(|f| f == full)
    }

    /// The call a fold makes per element, where the function is one this unit compiled: straight
    /// to its typed body, with no closure object and no dispatch.
    fn step_call(&mut self, f: &Code, acc: String, x: String) -> Result<String> {
        match self.nameable_step(f, 2) {
            Some(full) => {
                self.tables.calls.push(full.clone());
                Ok(format!("{}(ctx, {acc}, {x})", mangle(&full)))
            }
            None => self.refuse("`fold` over a function this tier cannot name"),
        }
    }

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
        self.bind_name(params[0].clone(), held);
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
                let index = self.local_builtin(b);
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
                let index = self.local_builtin(b);
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
            let index = self.local_builtin(b);
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
            let index = self.local_builtin(b);
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
                match self.ctor_of(&QName::bare(name.clone())) {
                    Some((i, 0)) => Ok(format!(
                        "(!ply_is_imm({0}) && ply_obj({0})->kind == 4 && ply_obj({0})->layout == {i})",
                        v.c
                    )),
                    _ => Ok("1".to_string()),
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
                let Some((i, arity)) = self.ctor_of(name) else {
                    return self.refuse(format!("the constructor `{}`", name.symbol()));
                };
                if arity != args.len() {
                    return self.refuse(format!(
                        "the constructor pattern `{}` binds {} of its {arity} fields",
                        name.symbol(),
                        args.len()
                    ));
                }
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
            // Refutable despite the shape, and for three reasons: a record pattern fails on a
            // value that is not a record, on a field count when there is no `..`, and on a field
            // the value does not have. The field read is guarded by the test built so far,
            // because `rt_field` raises on a non-record rather than answering.
            Pat::Record { fields, rest } => {
                let mut test = format!(
                    "rt_record_fits_p(ctx, {}, {}, {})",
                    v.c,
                    fields.len(),
                    i64::from(!*rest)
                );
                for (name, sub) in fields {
                    let index = self.local_field(&name.name);
                    test = format!("({test} && rt_record_has_p(ctx, {}, {index}))", v.c);
                    if self.irrefutable_pat(sub) {
                        continue;
                    }
                    let field = self.bind(
                        Kind::Boxed,
                        format!("({test} ? rt_field_p(ctx, {}, {index}, 0) : 0)", v.c),
                    );
                    self.check();
                    let inner = self.test(sub, &field)?;
                    test = format!("({test} && {inner})");
                }
                Ok(test)
            }
            // A refutable `..rest` would need the tail built before it could be tested, which is a
            // list allocated inside a test that may fail. The in-process tier draws the same line.
            Pat::List { items, rest } => {
                if let Some(bad) = rest.iter().find(|p| !self.irrefutable_pat(p)) {
                    return self.refuse(format!(
                        "a {} pattern as a list pattern's rest",
                        pattern_name(bad)
                    ));
                }
                let mut test = format!(
                    "rt_list_fits_p(ctx, {}, {}, {})",
                    v.c,
                    items.len(),
                    i64::from(rest.is_none())
                );
                for (i, item) in items.iter().enumerate() {
                    if self.irrefutable_pat(item) {
                        continue;
                    }
                    let at = self.bind(
                        Kind::Boxed,
                        format!("({test} ? rt_list_at_p(ctx, {}, {i}) : 0)", v.c),
                    );
                    self.check();
                    let inner = self.test(item, &at)?;
                    test = format!("({test} && {inner})");
                }
                Ok(test)
            }
            Pat::Lit(_) => self.refuse(format!(
                "a {} pattern, which this tier does not carry yet",
                pattern_name(pat)
            )),
        }
    }

    fn ctor_index(&self, name: &QName) -> Option<u32> {
        self.ctor_of(name).map(|(i, _)| i)
    }

    /// The unit's index for the constructor `q` names, and its arity.
    ///
    /// A user constructor is interned under the program-wide name its module qualifies it with,
    /// and a body names it bare, so the resolver is what stands between the two. Reading the
    /// unit's table with the bare symbol finds only the prelude's constructors, which is why this
    /// tier refused every `type` a program declared.
    ///
    /// The nullary case in `Pat::Var` is defensive rather than reached: the parser makes every
    /// upper-case bare name in pattern position a `Ctor`, so a constructor does not arrive
    /// wearing a variable's shape from source. It is kept because the arity is what separates the
    /// two, and reading it wrongly is a match that succeeds rather than a refusal.
    fn ctor_of(&self, q: &QName) -> Option<(u32, usize)> {
        let global = if q.is_bare() {
            self.src
                .resolved
                .scopes
                .get(self.module_index)
                .and_then(|s| s.get(Namespace::Value, q.symbol()))
                .map(|b| b.qualified.clone())
        } else {
            self.src
                .resolved
                .lookup(self.module_index, Namespace::Value, q)
                .ok()
                .map(|b| b.qualified.clone())
        };
        let name = global.or_else(|| {
            (q.is_bare() && self.unit.layouts.ctors.iter().any(|(n, _)| n == q.symbol()))
                .then(|| q.symbol().clone())
        })?;
        let index = self
            .unit
            .layouts
            .ctors
            .iter()
            .position(|(n, _)| *n == name)?;
        Some((index as u32, self.unit.layouts.ctors[index].1))
    }

    fn bind_pattern(&mut self, pat: &Pat, v: &V) -> Result<()> {
        match pat {
            Pat::Wildcard => Ok(()),
            Pat::Var {
                name,
                slot: Some(_),
            } => {
                let held = self.bind(Kind::Boxed, v.c.clone());
                self.bind_name(name.name.clone(), held);
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
            Pat::Record { fields, .. } => {
                for (name, sub) in fields {
                    let index = self.local_field(&name.name);
                    // `rt_field` at mode 0 answers the field held once more, so unlike the
                    // constructor case above there is no increment to add here.
                    let field =
                        self.bind(Kind::Boxed, format!("rt_field_p(ctx, {}, {index}, 0)", v.c));
                    self.check();
                    self.bind_pattern(sub, &field)?;
                }
                Ok(())
            }
            Pat::List { items, rest } => {
                for (i, item) in items.iter().enumerate() {
                    let at = self.bind(Kind::Boxed, format!("rt_list_at_p(ctx, {}, {i})", v.c));
                    self.check();
                    self.bind_pattern(item, &at)?;
                }
                if let Some(r) = rest {
                    let tail = self.bind(
                        Kind::Boxed,
                        format!("rt_list_rest_p(ctx, {}, {})", v.c, items.len()),
                    );
                    self.check();
                    self.bind_pattern(r, &tail)?;
                }
                Ok(())
            }
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

/// The two `Bool` singletons and `Unit`, as the names `ply_bind_singletons` fills in.
///
/// They were their addresses once, which is a constant in a process and nothing at all in a file:
/// the emitted source differed on every run, so no two runs could share an object. Named, the
/// source is a function of the program and an object outlives the run that built it.
fn true_word() -> String {
    "ply_true".to_string()
}
fn false_word() -> String {
    "ply_false".to_string()
}
fn unit_word() -> String {
    "ply_unit".to_string()
}

/// The helper that answers a builtin directly, where one does. Each takes its arguments exactly as
/// the generic path does -- they fall back to the same `direct` over values -- so the call is a
/// swap and nothing about ownership changes.
fn direct_helper(b: Builtin, args: usize) -> Option<&'static str> {
    Some(match (b, args) {
        (Builtin::Push, 2) => "rt_push_p",
        (Builtin::MapInsert, 3) => "rt_map_insert_p",
        (Builtin::MapContains, 2) => "rt_map_contains_p",
        (Builtin::MapGet, 2) => "rt_map_get_p",
        (Builtin::Compare, 2) => "rt_compare_p",
        (Builtin::ByteOfInt, 1) => "rt_byte_of_int_p",
        (Builtin::BytesConcat, 2) => "rt_bytes_concat_p",
        (Builtin::BytesSlice, 3) => "rt_bytes_slice_p",
        (Builtin::BytesScan, 4) => "rt_bytes_scan_p",
        (Builtin::BytesScanUntil, 4) => "rt_bytes_scan_until_p",
        (Builtin::ListAt, 2) => "rt_list_index_p",
        _ => return None,
    })
}

/// What to call a pattern in a refusal.
fn pattern_name(pat: &Pat) -> &'static str {
    match pat {
        Pat::Wildcard => "wildcard",
        Pat::Var { .. } => "binding",
        Pat::Lit(Lit::Str(_)) => "string literal",
        Pat::Lit(Lit::Bytes(_)) => "bytes literal",
        Pat::Lit(_) => "literal",
        Pat::Ctor { .. } => "constructor",
        Pat::Record { .. } => "record",
        Pat::List { .. } => "list",
    }
}

impl Emit<'_> {
    /// Whether a pattern can fail. A binding and a wildcard cannot; a bare name that is a nullary
    /// constructor can, which is why this asks rather than matching on the shape alone.
    fn irrefutable_pat(&self, pat: &Pat) -> bool {
        match pat {
            Pat::Wildcard => true,
            Pat::Var { name, .. } => self.ctor_of(&QName::bare(name.clone())).is_none(),
            _ => false,
        }
    }
}
