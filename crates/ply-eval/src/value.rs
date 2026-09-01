use crate::arena::Slot;
use crate::builtins::Builtin;
use crate::code::Code;
use crate::cont::Continuation;
use crate::env::Env;
use crate::limit::{self, MAX_VALUE_DEPTH, grow};
use crate::sim::TaskId;
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::{Expr, render_float};
use rpds::RedBlackTreeMap;
pub use rust_decimal::Decimal;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::Arc;

/// Whole-list sharing rather than structural sharing, which costs no persistent-vector dependency:
/// `push` rewrites the array when its caller is the last owner of it, and copies the whole array
/// when anything else can still see it.
pub type Vector<T> = Arc<Vec<T>>;

/// The `Map` primitive's representation.
pub type Map = RedBlackTreeMap<Value, Value>;

const RENDER_MAX_ITEMS: usize = 32;
const RENDER_MAX_DEPTH: usize = 16;

thread_local! {
    /// Indexed by [`Builtin`]'s discriminant.
    static BUILTIN_VALUES: RefCell<Vec<Option<Value>>> = const { RefCell::new(Vec::new()) };
}

#[derive(Clone)]
pub enum Value {
    Int(i64),
    Bool(bool),
    /// IEEE-754 binary64, unmodified.
    Float(f64),
    /// Exact base ten: sign, a 96-bit mantissa and a scale of `0..=28`.
    Decimal(Decimal),
    Str(Arc<str>),
    /// Mirrors [`Value::Str`] exactly, deliberately: what `bytes::Bytes` buys is cheap slicing of a
    /// shared buffer, which W3's streaming bodies want and W1 does not, and it would put a type
    /// carrying its own refcount semantics into the enum the hygiene rules are written against.
    Bytes(Arc<[u8]>),
    Unit,
    List(Vector<Value>),
    /// Iterated in ascending key order by [`Value::cmp`], always.
    Map(Map),
    Record(Arc<BTreeMap<Symbol, Value>>),
    Ctor {
        name: Symbol,
        args: Arc<Vec<Value>>,
    },
    Closure(Arc<Closure>),
    /// A slot in the region that allocated it, not a pointer into it: an index and a generation, so
    /// a cell whose region has closed reads `None` rather than aliasing whatever was allocated in
    /// its place.
    Cell(Slot),
    /// A handle on a task, and a key into its region's scheduler for the same reason
    /// [`Value::Cell`] is one: a key cannot dangle, two keys cannot alias, and identity is integer
    /// comparison.
    Task(TaskId),
    /// A captured continuation.
    Continuation(Rc<Continuation>),
    /// A credential, and a **distinct variant** rather than a `Ctor { name: "Secret", .. }`, which
    /// is the single most important line of ADR 0015 §2: a `Ctor` is matchable, and `match s {
    /// Secret(plain) -> plain }` would be a one-line escape from every guarantee below.
    Secret(Arc<Value>),
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
        /// Index into `Program::modules`: the scope the body's bare names are resolved in, which
        /// travels with the closure rather than the caller.
        module: usize,
    },
    /// The tree-walker deep-clones an `Expr` per closure; the machine lowers once and every closure
    /// after that is a pointer.
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

    /// Later entries win, which is what makes this a fold of `map_insert` and therefore the same
    /// rule `map_of_entries` and `map_merge` follow.
    pub fn map(entries: impl IntoIterator<Item = (Value, Value)>) -> Value {
        let mut m = Map::new();
        for (k, v) in entries {
            insert_key(&mut m, k, v);
        }
        Value::Map(m)
    }

    pub fn ctor(name: impl Into<Symbol>, args: Vec<Value>) -> Value {
        Value::Ctor {
            name: name.into(),
            args: Arc::new(args),
        }
    }

    /// One `Value` per builtin per thread, built on first reference.
    pub fn builtin(b: Builtin) -> Value {
        let fresh = || {
            Value::Closure(Arc::new(Closure {
                name: Some(Symbol::new(b.name())),
                kind: ClosureKind::Builtin(b),
            }))
        };
        // `try_with`, because a value dropped during thread-local teardown can reach here after the
        // cache is gone, and building a fresh one is the right answer there rather than an abort.
        BUILTIN_VALUES
            .try_with(|cache| {
                let mut cache = cache.borrow_mut();
                let slot = b as usize;
                if slot >= cache.len() {
                    cache.resize(slot + 1, None);
                }
                cache[slot].get_or_insert_with(fresh).clone()
            })
            .unwrap_or_else(|_| fresh())
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
            Value::Secret(_) => "Secret",
        }
    }

    pub fn secret(inner: Value) -> Value {
        Value::Secret(Arc::new(inner))
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

    pub fn as_cell(&self, span: Span, what: &str) -> Result<Slot, Diagnostic> {
        match self {
            Value::Cell(slot) => Ok(*slot),
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
            // Never as an `Int`: a `Float` always shows a `.` or an exponent, so a rendered
            // expected/actual pair cannot make `1` and `1.0` look like one value.
            Value::Float(f) => out.push_str(&render_float(*f)),
            // The scale as stored, so `1.50m` renders `1.50`.
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
            // Key order, so two maps that are equal render identically and a failure's
            // expected/actual pair can be read side by side.
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
            Value::Cell(slot) => {
                let _ = write!(out, "<cell {slot}>");
            }
            Value::Task(id) => {
                let _ = write!(out, "<task {id}>");
            }
            Value::Continuation(k) => {
                let _ = write!(out, "<continuation {} frames>", k.frames());
            }
            // Before the depth guard would matter and with no recursion into the payload, so the
            // redaction is a property of this arm rather than of any bound: a `Secret` nested a
            // thousand deep still prints this.
            Value::Secret(_) => out.push_str(SECRET_REDACTED),
        }
    }
}

/// What a `Secret` renders as, everywhere, always.
pub const SECRET_REDACTED: &str = "Secret(****)";

/// Drop glue recurses once per level of nesting, so a value deeper than the host stack aborts the
/// process on the way *out* — the same hole [`values_equal`] closes on the way in, and the one hole
/// no bound can close, because a value has to be dropped whatever its depth.
const DISMANTLE_KEEP: usize = 256;

thread_local! {
    static DISMANTLE: std::cell::Cell<Vec<Value>> = const { std::cell::Cell::new(Vec::new()) };
}

impl Drop for Value {
    fn drop(&mut self) {
        if !nests(self) {
            return;
        }
        let mut pending: Vec<Value> = DISMANTLE
            .try_with(std::cell::Cell::take)
            .unwrap_or_default();
        take_children(self, &mut pending);
        while let Some(mut v) = pending.pop() {
            take_children(&mut v, &mut pending);
        }
        if pending.capacity() <= DISMANTLE_KEEP {
            let _ = DISMANTLE.try_with(|slot| slot.set(pending));
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
        Value::Secret(inner) => nests(inner),
        _ => false,
    }
}

/// Moves the children that can nest further onto `out`, leaving the value empty.
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
        // A map's entries cannot be moved onto the worklist: `rpds` hands out no owned iterator,
        // and cloning them there would leave the tree holding them too, so the glue would walk the
        // whole chain again at every level.
        Value::Map(m) => {
            let taken = std::mem::replace(m, Map::new());
            grow(move || drop(taken));
        }
        Value::Secret(inner) => {
            if let Some(v) = Arc::get_mut(inner) {
                let taken = std::mem::replace(v, Value::Unit);
                if nests(&taken) {
                    out.push(taken);
                }
            }
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

/// One byte, as `b"..."` writes it.
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
        Value::Secret(_) => 15,
    }
}

/// Structural, total and deterministic — the order `Map` keys are held in.
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
            // Total and deterministic where IEEE `<` is neither.
            (Value::Float(x), Value::Float(y)) => x.total_cmp(y),
            // By numeric value, so `1.50m` and `1.5m` are one key.
            (Value::Decimal(x), Value::Decimal(y)) => x.cmp(y),
            (Value::Str(x), Value::Str(y)) => x.cmp(y),
            (Value::Bytes(x), Value::Bytes(y)) => x.cmp(y),
            // Every compound arm grows the host stack rather than bounding the walk: `cmp` has no
            // way to report a refusal, and answering `Equal` past a depth would put two distinct
            // keys in one slot.
            (Value::List(x), Value::List(y)) => grow(|| x.iter().cmp(y.iter())),
            (Value::Map(x), Value::Map(y)) => grow(|| x.iter().cmp(y.iter())),
            (Value::Record(x), Value::Record(y)) => grow(|| x.iter().cmp(y.iter())),
            (Value::Ctor { name: n1, args: a1 }, Value::Ctor { name: n2, args: a2 }) => {
                grow(|| n1.cmp(n2).then_with(|| a1.iter().cmp(a2.iter())))
            }
            (Value::Cell(x), Value::Cell(y)) => x.cmp(y),
            (Value::Task(x), Value::Task(y)) => x.cmp(y),
            // Unreachable from a well-typed program: `Map<Secret<a>, v>` is `E0206` because a key
            // needs `derivable(ord, k)`, and `derive ord` and `compare_values` both refuse a
            // `Secret`.
            (Value::Secret(x), Value::Secret(y)) => grow(|| x.cmp(y)),
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
impl PartialEq for Value {
    fn eq(&self, other: &Value) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Value {}

/// The one place a key enters a [`Map`] from Rust, so that the canonical form below cannot be
/// bypassed by a seventh map builder.
pub(crate) fn insert_key(m: &mut Map, k: Value, v: Value) {
    m.insert_mut(canonical_key(&k).unwrap_or(k), v);
}

/// The canonical member of a key's equivalence class under [`Value::cmp`], or `None` when the key
/// already is one.
pub(crate) fn canonical_key(v: &Value) -> Option<Value> {
    if is_canonical(v) {
        return None;
    }
    Some(canonicalize(v))
}

fn is_canonical(v: &Value) -> bool {
    match v {
        // `normalize` is minimal scale, which is unique per numeric value, so it is a canonical
        // form rather than merely a smaller one.
        Value::Decimal(d) => d.serialize() == d.normalize().serialize(),
        Value::List(items) => grow(|| items.iter().all(is_canonical)),
        Value::Map(entries) => grow(|| {
            entries
                .iter()
                .all(|(k, val)| is_canonical(k) && is_canonical(val))
        }),
        Value::Record(fields) => grow(|| fields.values().all(is_canonical)),
        Value::Ctor { args, .. } => grow(|| args.iter().all(is_canonical)),
        _ => true,
    }
}

fn canonicalize(v: &Value) -> Value {
    match v {
        Value::Decimal(d) => Value::Decimal(d.normalize()),
        Value::List(items) => grow(|| Value::list(items.iter().map(canonicalize).collect())),
        // Rebuilt through `insert_key` rather than `insert_mut`, so that a nested map is canonical
        // by the same rule and by the same code.
        Value::Map(entries) => grow(|| {
            let mut out = Map::new();
            for (k, val) in entries.iter() {
                insert_key(&mut out, canonicalize(k), canonicalize(val));
            }
            Value::Map(out)
        }),
        Value::Record(fields) => grow(|| {
            Value::Record(Arc::new(
                fields
                    .iter()
                    .map(|(name, val)| (name.clone(), canonicalize(val)))
                    .collect(),
            ))
        }),
        Value::Ctor { name, args } => grow(|| Value::Ctor {
            name: name.clone(),
            args: Arc::new(args.iter().map(canonicalize).collect()),
        }),
        other => other.clone(),
    }
}

/// The refusal every path that would order a credential meets.
pub(crate) fn secret_has_no_order(v: &Value, what: &str, span: Span) -> Result<(), Diagnostic> {
    if !matches!(v, Value::Secret(_)) {
        return Ok(());
    }
    Err(Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`{what}` cannot order a `Secret`"),
    )
    .primary(span, "a credential has no order")
    .note(
        "an ordering over a credential leaks a bit of position per comparison and recovers the \
         value in calls proportional to its length",
    )
    .note("use `secret_verify` to check a candidate, or `==` to compare two secrets")
    .note(
        "reaching this is a defect in Ply: `derivable(ord, Secret<a>)` is false, so a `Map` key \
         and a `derive ord` over one are both `E0206` at compile time",
    ))
}

pub(crate) fn type_error(span: Span, what: &str, expected: &str, got: &Value) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("{what} expects {expected}, but got {}", got.type_name()),
    )
    .primary(span, format!("this is {}", got.render()))
}

/// Comparing functions is an error rather than a silently-false answer.
pub fn values_equal(a: &Value, b: &Value, span: Span) -> Result<bool, Diagnostic> {
    equal_at(a, b, span, 0)
}

/// One level down: refuses past the bound, and otherwise grows the host stack so that the bound is
/// what a program meets.
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
        // IEEE `==`, so `NaN != NaN` and `0.0 == -0.0`.
        (Value::Float(x), Value::Float(y)) => x == y,
        // By numeric value, so `1.50m == 1.5m` — which is why the two are one map key and why
        // `Decimal` may appear in a `proved` obligation as an uninterpreted term while `Float` may
        // not.
        (Value::Decimal(x), Value::Decimal(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        // Content, not identity, and never equal to a `Str`: the two have different types, and the
        // falling-through `_ => false` arm is what says so.
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
        // Length, then entries in key order.
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
        // The language's `==` over two credentials, and the reason `assert_eq` over a record
        // holding one works while printing nothing.
        (Value::Secret(x), Value::Secret(y)) => {
            return descend(span, depth, || match (&**x, &**y) {
                (Value::Str(p), Value::Str(q)) => Ok(constant_time_eq(p.as_bytes(), q.as_bytes())),
                (Value::Bytes(p), Value::Bytes(q)) => Ok(constant_time_eq(p, q)),
                // No payload but a `String` is constructible — `secret_of_string` is the only
                // introduction — so this arm is for a payload a later milestone adds, and it is
                // structural rather than absent so that adding one cannot silently make two secrets
                // unequal.
                (p, q) => equal_at(p, q, span, depth + 1),
            });
        }
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

/// Byte equality whose running time is a function of the lengths and not of where the first
/// difference is.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = (a.len() ^ b.len()) as u64;
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= u64::from(x ^ y);
        diff = std::hint::black_box(diff);
    }
    diff == 0
}

/// Bounded exactly as [`values_equal`] is, and for the same reason.
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
            // Only when the key sets agree, so a differing *shape* is reported as two whole maps
            // rather than as a misaligned entry-by-entry walk.
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
