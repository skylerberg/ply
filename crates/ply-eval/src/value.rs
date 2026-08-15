use crate::builtins::Builtin;
use crate::code::Code;
use crate::cont::Continuation;
use crate::env::Env;
use crate::limit::{self, MAX_VALUE_DEPTH, grow};
use crate::sim::TaskId;
use crate::world::CellId;
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::{Expr, render_float};
use rpds::RedBlackTreeMap;
pub use rust_decimal::Decimal;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::Arc;

/// Whole-list sharing rather than structural sharing: `push` copies, which is
/// fine at v0 sizes and costs no persistent-vector dependency.
pub type Vector<T> = Arc<Vec<T>>;

/// The `Map` primitive's representation.
///
/// A search tree rather than a hash table, and that is the whole design: a
/// hash-ordered map makes `map_keys` a function of a hasher's seed and of
/// insertion history, and four separate guarantees rest on a value having one
/// canonical form — a derived encoding that is stable run to run, `assert_eq`
/// over two maps built in different orders, a seeded replay that takes the same
/// branch on a `map_fold`, and `--engine both` reporting no divergence. Every
/// one of those failures is a green result over unexplored space or a red one
/// over correct code, so the order is not an implementation detail to be
/// documented as unspecified. It is fixed by [`Value::cmp`], and an unordered
/// implementation is not reachable from here: `rpds` keeps the entries sorted
/// and hands out no other iteration order.
///
/// `RcK`, the same shared-pointer kind [`World`](crate::world::World) uses, so a
/// `Value` stays thread-confined exactly as it was.
pub type Map = RedBlackTreeMap<Value, Value>;

const RENDER_MAX_ITEMS: usize = 32;
const RENDER_MAX_DEPTH: usize = 16;

#[derive(Clone)]
pub enum Value {
    Int(i64),
    Bool(bool),
    /// IEEE-754 binary64, unmodified. `1.0 / 0.0` is `Infinity` and `0.0 / 0.0`
    /// is `NaN`; refusing them would make this a worse [`Value::Decimal`] rather
    /// than a different type. Its `==` is IEEE's, so it is **not** an
    /// equivalence relation and nothing about it may be `proved`.
    Float(f64),
    /// Exact base ten: sign, a 96-bit mantissa and a scale of `0..=28`. Bounded
    /// rather than arbitrary-precision, because a value that enters a hash and a
    /// cache key needs a size that does not depend on the operations performed
    /// on it.
    Decimal(Decimal),
    Str(Arc<str>),
    /// Mirrors [`Value::Str`] exactly, deliberately: what `bytes::Bytes` buys is
    /// cheap slicing of a shared buffer, which W3's streaming bodies want and
    /// W1 does not, and it would put a type carrying its own refcount semantics
    /// into the enum the hygiene rules are written against. Slicing copies.
    Bytes(Arc<[u8]>),
    Unit,
    List(Vector<Value>),
    /// Iterated in ascending key order by [`Value::cmp`], always. See [`Map`].
    Map(Map),
    Record(Arc<BTreeMap<Symbol, Value>>),
    Ctor {
        name: Symbol,
        args: Arc<Vec<Value>>,
    },
    Closure(Arc<Closure>),
    /// A key into the [`World`](crate::world::World), not a pointer into it, so
    /// that state is a value the machine threads rather than a location values
    /// alias.
    Cell(CellId),
    /// A handle on a task, and a key into its region's scheduler for the same
    /// reason [`Value::Cell`] is one: a key cannot dangle, two keys cannot alias
    /// across a fork, and identity is integer comparison. The scheduler dies
    /// with its region, so a handle that outlives it is `E0413` rather than a
    /// wrong answer.
    Task(TaskId),
    /// A captured continuation. Callable with exactly one argument — the value
    /// the `perform` it was captured at should have returned.
    Continuation(Rc<Continuation>),
}

pub struct Closure {
    pub name: Option<Symbol>,
    pub kind: ClosureKind,
}

pub enum ClosureKind {
    Fn {
        params: Vec<Symbol>,
        body: Arc<Expr>,
        env: Env,
        /// Index into `Program::modules`: the scope the body's bare names are
        /// resolved in, which travels with the closure rather than the caller.
        module: usize,
    },
    /// The tree-walker deep-clones an `Expr` per closure; the machine lowers
    /// once and every closure after that is a pointer.
    Code {
        params: Rc<Vec<Symbol>>,
        body: Code,
        env: Env,
        module: usize,
    },
    Ctor {
        name: Symbol,
        arity: usize,
    },
    Builtin(Builtin),
}

impl Closure {
    pub fn arity(&self) -> usize {
        match &self.kind {
            ClosureKind::Fn { params, .. } => params.len(),
            ClosureKind::Code { params, .. } => params.len(),
            ClosureKind::Ctor { arity, .. } => *arity,
            ClosureKind::Builtin(b) => b.arity().0,
        }
    }

    pub fn describe(&self) -> String {
        match (&self.name, &self.kind) {
            (Some(n), _) => format!("`{n}`"),
            (None, ClosureKind::Ctor { name, .. }) => format!("`{name}`"),
            (None, ClosureKind::Builtin(b)) => format!("`{}`", b.name()),
            (None, ClosureKind::Fn { .. } | ClosureKind::Code { .. }) => {
                "an anonymous function".to_string()
            }
        }
    }
}

impl Value {
    pub fn str(s: impl AsRef<str>) -> Value {
        Value::Str(Arc::from(s.as_ref()))
    }

    pub fn bytes(b: impl AsRef<[u8]>) -> Value {
        Value::Bytes(Arc::from(b.as_ref()))
    }

    pub fn list(items: Vec<Value>) -> Value {
        Value::List(Arc::new(items))
    }

    pub fn empty_map() -> Value {
        Value::Map(Map::new())
    }

    /// Later entries win, which is what makes this a fold of `map_insert` and
    /// therefore the same rule `map_of_entries` and `map_merge` follow.
    pub fn map(entries: impl IntoIterator<Item = (Value, Value)>) -> Value {
        let mut m = Map::new();
        for (k, v) in entries {
            m.insert_mut(k, v);
        }
        Value::Map(m)
    }

    pub fn ctor(name: impl Into<Symbol>, args: Vec<Value>) -> Value {
        Value::Ctor {
            name: name.into(),
            args: Arc::new(args),
        }
    }

    pub fn builtin(b: Builtin) -> Value {
        Value::Closure(Arc::new(Closure {
            name: Some(Symbol::new(b.name())),
            kind: ClosureKind::Builtin(b),
        }))
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "Int",
            Value::Bool(_) => "Bool",
            Value::Float(_) => "Float",
            Value::Decimal(_) => "Decimal",
            Value::Str(_) => "String",
            Value::Bytes(_) => "Bytes",
            Value::Unit => "Unit",
            Value::List(_) => "List",
            Value::Map(_) => "Map",
            Value::Record(_) => "record",
            Value::Ctor { .. } => "variant",
            Value::Closure(_) => "function",
            Value::Cell(_) => "Cell",
            Value::Task(_) => "Task",
            Value::Continuation(_) => "continuation",
        }
    }

    pub fn as_int(&self, span: Span, what: &str) -> Result<i64, Diagnostic> {
        match self {
            Value::Int(i) => Ok(*i),
            other => Err(type_error(span, what, "Int", other)),
        }
    }

    pub fn as_bool(&self, span: Span, what: &str) -> Result<bool, Diagnostic> {
        match self {
            Value::Bool(b) => Ok(*b),
            other => Err(type_error(span, what, "Bool", other)),
        }
    }

    pub fn as_float(&self, span: Span, what: &str) -> Result<f64, Diagnostic> {
        match self {
            Value::Float(f) => Ok(*f),
            other => Err(type_error(span, what, "Float", other)),
        }
    }

    pub fn as_decimal(&self, span: Span, what: &str) -> Result<Decimal, Diagnostic> {
        match self {
            Value::Decimal(d) => Ok(*d),
            other => Err(type_error(span, what, "Decimal", other)),
        }
    }

    pub fn as_str(&self, span: Span, what: &str) -> Result<&str, Diagnostic> {
        match self {
            Value::Str(s) => Ok(s),
            other => Err(type_error(span, what, "String", other)),
        }
    }

    pub fn as_bytes(&self, span: Span, what: &str) -> Result<&Arc<[u8]>, Diagnostic> {
        match self {
            Value::Bytes(b) => Ok(b),
            other => Err(type_error(span, what, "Bytes", other)),
        }
    }

    pub fn as_list(&self, span: Span, what: &str) -> Result<&Vector<Value>, Diagnostic> {
        match self {
            Value::List(xs) => Ok(xs),
            other => Err(type_error(span, what, "List", other)),
        }
    }

    pub fn as_map(&self, span: Span, what: &str) -> Result<&Map, Diagnostic> {
        match self {
            Value::Map(m) => Ok(m),
            other => Err(type_error(span, what, "Map", other)),
        }
    }

    pub fn as_cell(&self, span: Span, what: &str) -> Result<CellId, Diagnostic> {
        match self {
            Value::Cell(id) => Ok(*id),
            other => Err(type_error(span, what, "Cell", other)),
        }
    }

    pub fn as_task(&self, span: Span, what: &str) -> Result<TaskId, Diagnostic> {
        match self {
            Value::Task(id) => Ok(*id),
            other => Err(type_error(span, what, "Task", other)),
        }
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out
    }

    fn write(&self, out: &mut String, depth: usize) {
        if depth > RENDER_MAX_DEPTH {
            out.push('…');
            return;
        }
        match self {
            Value::Int(i) => {
                let _ = write!(out, "{i}");
            }
            Value::Bool(b) => {
                let _ = write!(out, "{b}");
            }
            // Never as an `Int`: a `Float` always shows a `.` or an exponent, so
            // a rendered expected/actual pair cannot make `1` and `1.0` look
            // like one value.
            Value::Float(f) => out.push_str(&render_float(*f)),
            // The scale as stored, so `1.50m` renders `1.50`. That is the digit
            // count the value carries, and rounding it away in a diff would hide
            // the very distinction `Decimal` is for.
            Value::Decimal(d) => {
                let _ = write!(out, "{d}");
            }
            Value::Str(s) => {
                out.push('"');
                out.push_str(&escape(s));
                out.push('"');
            }
            Value::Bytes(b) => {
                out.push_str("b\"");
                for byte in b.iter().take(RENDER_MAX_ITEMS) {
                    out.push_str(&escape_byte(*byte));
                }
                out.push('"');
                if b.len() > RENDER_MAX_ITEMS {
                    let _ = write!(out, " … {} more", b.len() - RENDER_MAX_ITEMS);
                }
            }
            Value::Unit => out.push_str("()"),
            Value::List(items) => {
                out.push('[');
                for (i, item) in items.iter().take(RENDER_MAX_ITEMS).enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    item.write(out, depth + 1);
                }
                if items.len() > RENDER_MAX_ITEMS {
                    let _ = write!(out, ", … {} more", items.len() - RENDER_MAX_ITEMS);
                }
                out.push(']');
            }
            // Key order, so two maps that are equal render identically and a
            // failure's expected/actual pair can be read side by side.
            Value::Map(entries) => {
                out.push('{');
                for (i, (k, v)) in entries.iter().take(RENDER_MAX_ITEMS).enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    k.write(out, depth + 1);
                    out.push_str(": ");
                    v.write(out, depth + 1);
                }
                if entries.size() > RENDER_MAX_ITEMS {
                    let _ = write!(out, ", … {} more", entries.size() - RENDER_MAX_ITEMS);
                }
                out.push('}');
            }
            Value::Record(fields) => {
                out.push('{');
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    let _ = write!(out, "{k}: ");
                    v.write(out, depth + 1);
                }
                out.push('}');
            }
            Value::Ctor { name, args } => {
                let _ = write!(out, "{name}");
                if !args.is_empty() {
                    out.push('(');
                    for (i, a) in args.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        a.write(out, depth + 1);
                    }
                    out.push(')');
                }
            }
            Value::Closure(c) => {
                let _ = match &c.name {
                    Some(n) => write!(out, "<fn {n}>"),
                    None => write!(out, "<fn>"),
                };
            }
            Value::Cell(id) => {
                let _ = write!(out, "<cell {id}>");
            }
            Value::Task(id) => {
                let _ = write!(out, "<task {id}>");
            }
            Value::Continuation(k) => {
                let _ = write!(out, "<continuation {} frames>", k.frames());
            }
        }
    }
}

/// Drop glue recurses once per level of nesting, so a value deeper than the host
/// stack aborts the process on the way *out* — the same hole [`values_equal`]
/// closes on the way in, and the one hole no bound can close, because a value
/// has to be dropped whatever its depth.
///
/// Dismantling is iterative: a uniquely-owned compound hands its compound
/// children to an explicit worklist before the glue reaches them, so the glue
/// only ever sees an emptied node. Scalars are left to the glue and cost
/// nothing, which is why a list of integers still drops without allocating.
impl Drop for Value {
    fn drop(&mut self) {
        if !nests(self) {
            return;
        }
        let mut pending: Vec<Value> = Vec::new();
        take_children(self, &mut pending);
        while let Some(mut v) = pending.pop() {
            take_children(&mut v, &mut pending);
        }
    }
}

/// Whether dropping this value can reach another one.
fn nests(v: &Value) -> bool {
    match v {
        Value::List(xs) => !xs.is_empty(),
        Value::Map(m) => !m.is_empty(),
        Value::Record(fields) => !fields.is_empty(),
        Value::Ctor { args, .. } => !args.is_empty(),
        _ => false,
    }
}

/// Moves the children that can nest further onto `out`, leaving the value empty.
///
/// A shared `Arc` is left alone: it is not being freed here, and whichever owner
/// does free it takes this path itself.
fn take_children(v: &mut Value, out: &mut Vec<Value>) {
    match v {
        Value::List(xs) | Value::Ctor { args: xs, .. } => {
            if let Some(items) = Arc::get_mut(xs) {
                out.extend(items.drain(..).filter(nests));
            }
        }
        Value::Record(fields) => {
            if let Some(map) = Arc::get_mut(fields) {
                out.extend(std::mem::take(map).into_values().filter(nests));
            }
        }
        // A map's entries cannot be moved onto the worklist: `rpds` hands out no
        // owned iterator, and cloning them there would leave the tree holding
        // them too, so the glue would walk the whole chain again at every level.
        // So a map is freed by the glue, on a stack `grow` extends — which turns
        // an abort into an allocation, and leaves the residual hole the module
        // comment already records for drop rather than opening a new one.
        Value::Map(m) => {
            let taken = std::mem::replace(m, Map::new());
            grow(move || drop(taken));
        }
        _ => {}
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render())
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render())
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c => out.push(c),
        }
    }
    out
}

/// One byte, as `b"..."` writes it. Everything outside printable ASCII is
/// `\xNN`, so a rendered literal is copy-pasteable back into source.
fn escape_byte(b: u8) -> String {
    match b {
        b'\n' => "\\n".to_string(),
        b'\t' => "\\t".to_string(),
        b'\r' => "\\r".to_string(),
        b'\\' => "\\\\".to_string(),
        b'"' => "\\\"".to_string(),
        0x20..=0x7e => (b as char).to_string(),
        _ => format!("\\x{b:02x}"),
    }
}

/// A variant's position in the total order below.
///
/// **Pinned.** Append a new variant; never insert one and never swap two. The
/// numbers are only observable where one map holds keys of two different
/// variants, which the type system refuses, so nothing a well-typed program can
/// print depends on them — but a store, a replay and a `--engine both` audit are
/// all comparisons across processes, and a number that moved between two builds
/// of the same source is the one way that could produce a difference nobody
/// wrote.
fn discriminant(v: &Value) -> u8 {
    match v {
        Value::Unit => 0,
        Value::Bool(_) => 1,
        Value::Int(_) => 2,
        Value::Float(_) => 3,
        Value::Decimal(_) => 4,
        Value::Str(_) => 5,
        Value::Bytes(_) => 6,
        Value::List(_) => 7,
        Value::Map(_) => 8,
        Value::Record(_) => 9,
        Value::Ctor { .. } => 10,
        Value::Closure(_) => 11,
        Value::Cell(_) => 12,
        Value::Task(_) => 13,
        Value::Continuation(_) => 14,
    }
}

/// Structural, total and deterministic — the order `Map` keys are held in, so
/// `map_keys` is a function of the values and of nothing else.
///
/// Two things about it are load-bearing rather than incidental:
///
/// - It is **total on every `Value`**, including the ones no key type admits.
///   `Closure`, `Cell`, `Task` and `Continuation` compare by discriminant alone
///   — every closure equal to every closure. Not a panic, which is banned on a
///   path a program can reach, and not a pointer comparison, which is not
///   deterministic. Those cases are unreachable from a well-typed program, and
///   the definition is here so that a defect elsewhere produces a wrong answer
///   that reproduces rather than an abort or a different answer per run.
/// - It is **not** the language's equality. [`values_equal`] stays that, is not
///   rewritten in terms of this, and the two are checked against each other —
///   which is what catches a divergence rather than hiding it. The one place
///   they part is a `Float` NaN, where `cmp` is `Equal` and `values_equal` is
///   false, and that is asserted explicitly in the tests rather than excluded
///   from them.
impl Ord for Value {
    fn cmp(&self, other: &Value) -> Ordering {
        let (a, b) = (discriminant(self), discriminant(other));
        if a != b {
            return a.cmp(&b);
        }
        match (self, other) {
            (Value::Unit, Value::Unit) => Ordering::Equal,
            (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
            (Value::Int(x), Value::Int(y)) => x.cmp(y),
            // Total and deterministic where IEEE `<` is neither. It orders NaN,
            // and it separates `0.0` from `-0.0` — which is why `Float` is not
            // an ordered key type: this makes the *Rust* comparison total, and
            // says nothing about the language's `==` being an equivalence
            // relation, which is what a map's contract is stated in terms of.
            (Value::Float(x), Value::Float(y)) => x.total_cmp(y),
            // By numeric value, so `1.50m` and `1.5m` are one key.
            (Value::Decimal(x), Value::Decimal(y)) => x.cmp(y),
            (Value::Str(x), Value::Str(y)) => x.cmp(y),
            (Value::Bytes(x), Value::Bytes(y)) => x.cmp(y),
            // Every compound arm grows the host stack rather than bounding the
            // walk: `cmp` has no way to report a refusal, and answering `Equal`
            // past a depth would put two distinct keys in one slot.
            (Value::List(x), Value::List(y)) => grow(|| x.iter().cmp(y.iter())),
            (Value::Map(x), Value::Map(y)) => grow(|| x.iter().cmp(y.iter())),
            (Value::Record(x), Value::Record(y)) => grow(|| x.iter().cmp(y.iter())),
            (Value::Ctor { name: n1, args: a1 }, Value::Ctor { name: n2, args: a2 }) => {
                grow(|| n1.cmp(n2).then_with(|| a1.iter().cmp(a2.iter())))
            }
            (Value::Cell(x), Value::Cell(y)) => x.cmp(y),
            (Value::Task(x), Value::Task(y)) => x.cmp(y),
            (Value::Closure(_), Value::Closure(_))
            | (Value::Continuation(_), Value::Continuation(_)) => Ordering::Equal,
            // Unreachable: the discriminants matched, so the variants did.
            _ => Ordering::Equal,
        }
    }
}

impl PartialOrd for Value {
    fn partial_cmp(&self, other: &Value) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Rust's `==`, which is [`Ord`] and therefore **not** the language's equality.
/// A call site that means the language's must say so by calling
/// [`values_equal`].
impl PartialEq for Value {
    fn eq(&self, other: &Value) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Value {}

pub(crate) fn type_error(span: Span, what: &str, expected: &str, got: &Value) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("{what} expects {expected}, but got {}", got.type_name()),
    )
    .primary(span, format!("this is {}", got.render()))
}

/// Comparing functions is an error rather than a silently-false answer.
///
/// Bounded, because a walk over a value is host recursion that no count of
/// *calls* reaches: a deep enough value would otherwise abort the process from
/// inside a worker, losing every sibling test's result rather than failing one.
pub fn values_equal(a: &Value, b: &Value, span: Span) -> Result<bool, Diagnostic> {
    equal_at(a, b, span, 0)
}

/// One level down: refuses past the bound, and otherwise grows the host stack so
/// that the bound is what a program meets. Only the compound arms pay for it —
/// comparing two integers costs exactly what it did.
fn descend(
    span: Span,
    depth: usize,
    f: impl FnOnce() -> Result<bool, Diagnostic>,
) -> Result<bool, Diagnostic> {
    if depth >= MAX_VALUE_DEPTH {
        return Err(limit::err_value_depth(span, MAX_VALUE_DEPTH));
    }
    grow(f)
}

fn equal_at(a: &Value, b: &Value, span: Span, depth: usize) -> Result<bool, Diagnostic> {
    Ok(match (a, b) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        // IEEE `==`, so `NaN != NaN` and `0.0 == -0.0`. Deliberately **not**
        // `total_cmp`, which [`Value::cmp`] uses: the two disagree at exactly
        // those two points, and this is the one the language's `==` means. Every
        // restriction on `Float` — not an ordered key type, not derivable for
        // `ord`, never inside a `proved` obligation — follows from this line.
        (Value::Float(x), Value::Float(y)) => x == y,
        // By numeric value, so `1.50m == 1.5m` — which is why the two are one
        // map key and why `Decimal` may appear in a `proved` obligation as an
        // uninterpreted term while `Float` may not.
        (Value::Decimal(x), Value::Decimal(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        // Content, not identity, and never equal to a `Str`: the two have
        // different types, and the falling-through `_ => false` arm is what says
        // so.
        (Value::Bytes(x), Value::Bytes(y)) => x == y,
        (Value::Unit, Value::Unit) => true,
        (Value::List(x), Value::List(y)) => {
            if x.len() != y.len() {
                return Ok(false);
            }
            return descend(span, depth, || {
                for (p, q) in x.iter().zip(y.iter()) {
                    if !equal_at(p, q, span, depth + 1)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            });
        }
        // Length, then entries in key order. Both sides iterate ascending, so
        // two maps built by different insertion orders zip up entry for entry —
        // which is the whole point of the order being canonical, and is what
        // lets a passing test be cached under one order and read from cache
        // under the other.
        //
        // Keys are compared with the *language's* equality rather than with the
        // ordering that placed them, so the two are checked against each other
        // here on every comparison rather than only in the test that asserts it.
        (Value::Map(x), Value::Map(y)) => {
            if x.size() != y.size() {
                return Ok(false);
            }
            return descend(span, depth, || {
                for ((k1, v1), (k2, v2)) in x.iter().zip(y.iter()) {
                    if !equal_at(k1, k2, span, depth + 1)? || !equal_at(v1, v2, span, depth + 1)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            });
        }
        (Value::Record(x), Value::Record(y)) => {
            if x.len() != y.len() || x.keys().ne(y.keys()) {
                return Ok(false);
            }
            return descend(span, depth, || {
                for (p, q) in x.values().zip(y.values()) {
                    if !equal_at(p, q, span, depth + 1)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            });
        }
        (Value::Ctor { name: n1, args: a1 }, Value::Ctor { name: n2, args: a2 }) => {
            if n1 != n2 || a1.len() != a2.len() {
                return Ok(false);
            }
            return descend(span, depth, || {
                for (p, q) in a1.iter().zip(a2.iter()) {
                    if !equal_at(p, q, span, depth + 1)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            });
        }
        (Value::Cell(x), Value::Cell(y)) => x == y,
        (Value::Task(x), Value::Task(y)) => x == y,
        (Value::Closure(_) | Value::Continuation(_), _)
        | (_, Value::Closure(_) | Value::Continuation(_)) => {
            return Err(Diagnostic::error(
                codes::RUNTIME_ERROR,
                "cannot compare functions for equality",
            )
            .primary(span, "functions have no equality")
            .note("compare the results of calling them instead"));
        }
        _ => false,
    })
}

/// Bounded exactly as [`values_equal`] is, and for the same reason. Past the
/// bound no difference can be located, which costs a failure one note rather
/// than costing the run every result it held.
pub fn first_difference(actual: &Value, expected: &Value) -> Option<(String, String, String)> {
    fn go(
        actual: &Value,
        expected: &Value,
        path: &mut String,
        depth: usize,
    ) -> Option<(String, String, String)> {
        if depth >= MAX_VALUE_DEPTH {
            return None;
        }
        match (actual, expected) {
            (Value::List(a), Value::List(e)) if a.len() == e.len() => grow(|| {
                for (i, (x, y)) in a.iter().zip(e.iter()).enumerate() {
                    let mark = path.len();
                    let _ = write!(path, "[{i}]");
                    if let Some(found) = go(x, y, path, depth + 1) {
                        return Some(found);
                    }
                    path.truncate(mark);
                }
                None
            }),
            // Only when the key sets agree, so a differing *shape* is reported
            // as two whole maps rather than as a misaligned entry-by-entry walk.
            // `keys()` is ascending on both sides, so `eq` here is set equality.
            (Value::Map(a), Value::Map(e)) if a.size() == e.size() && a.keys().eq(e.keys()) => {
                grow(|| {
                    for ((k, x), y) in a.iter().zip(e.values()) {
                        let mark = path.len();
                        let _ = write!(path, "[{}]", k.render());
                        if let Some(found) = go(x, y, path, depth + 1) {
                            return Some(found);
                        }
                        path.truncate(mark);
                    }
                    None
                })
            }
            (Value::Record(a), Value::Record(e)) if a.keys().eq(e.keys()) => grow(|| {
                for ((k, x), y) in a.iter().zip(e.values()) {
                    let mark = path.len();
                    let _ = write!(path, ".{k}");
                    if let Some(found) = go(x, y, path, depth + 1) {
                        return Some(found);
                    }
                    path.truncate(mark);
                }
                None
            }),
            (Value::Ctor { name: n1, args: a1 }, Value::Ctor { name: n2, args: a2 })
                if n1 == n2 && a1.len() == a2.len() =>
            {
                grow(|| {
                    for (i, (x, y)) in a1.iter().zip(a2.iter()).enumerate() {
                        let mark = path.len();
                        let _ = write!(path, ".{n1}.{i}");
                        if let Some(found) = go(x, y, path, depth + 1) {
                            return Some(found);
                        }
                        path.truncate(mark);
                    }
                    None
                })
            }
            (a, e) => {
                if equal_at(a, e, Span::DUMMY, depth).unwrap_or(false) || path.is_empty() {
                    None
                } else {
                    Some((path.clone(), e.render(), a.render()))
                }
            }
        }
    }
    let mut path = String::new();
    go(actual, expected, &mut path, 0)
}
