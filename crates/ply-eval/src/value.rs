use crate::arena::Slot;
use crate::builtins::Builtin;
use crate::limit::{self, MAX_VALUE_DEPTH, grow};
use crate::sim::TaskId;
use ply_span::{Diagnostic, Span, Symbol, codes};
pub use ply_ty::IntTy;
use ply_ty::render_float;
use rpds::RedBlackTreeMap;
pub use rust_decimal::Decimal;
use std::cell::RefCell;

thread_local! {
    static NO_ARGS: Arc<Vec<Value>> = Arc::new(Vec::new());
}
use std::cmp::Ordering;
use std::fmt;
use std::fmt::Write as _;
use std::sync::Arc;

pub use crate::list::List;

pub type Map = RedBlackTreeMap<Value, Value>;

const RENDER_MAX_ITEMS: usize = 32;
const RENDER_MAX_DEPTH: usize = 16;

thread_local! {
    /// Indexed by [`Builtin`]'s discriminant.
    static BUILTIN_VALUES: RefCell<Vec<Option<Value>>> = const { RefCell::new(Vec::new()) };
}

/// A record's fields, sorted by name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fields(Vec<(Symbol, Value)>);

impl Fields {
    pub fn get(&self, name: &Symbol) -> Option<&Value> {
        self.0
            .binary_search_by(|(k, _)| k.cmp(name))
            .ok()
            .map(|i| &self.0[i].1)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Symbol, &Value)> {
        self.0.iter().map(|(k, v)| (k, v))
    }

    pub fn contains_key(&self, name: &Symbol) -> bool {
        self.get(name).is_some()
    }

    pub fn keys(&self) -> impl Iterator<Item = &Symbol> {
        self.0.iter().map(|(k, _)| k)
    }

    pub fn values(&self) -> impl Iterator<Item = &Value> {
        self.0.iter().map(|(_, v)| v)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn insert(&mut self, name: Symbol, value: Value) -> Option<Value> {
        match self.0.binary_search_by(|(k, _)| k.cmp(&name)) {
            Ok(i) => Some(std::mem::replace(&mut self.0[i].1, value)),
            Err(i) => {
                self.0.insert(i, (name, value));
                None
            }
        }
    }

    pub fn into_values(self) -> impl Iterator<Item = Value> {
        self.0.into_iter().map(|(_, v)| v)
    }

    /// Replaces an existing field's value; `None` when the name is absent.
    pub fn set(&mut self, name: &Symbol, value: Value) -> Option<Value> {
        let i = self.0.binary_search_by(|(k, _)| k.cmp(name)).ok()?;
        Some(std::mem::replace(&mut self.0[i].1, value))
    }
}

impl std::ops::Index<&Symbol> for Fields {
    type Output = Value;
    fn index(&self, name: &Symbol) -> &Value {
        self.get(name).expect("no such field")
    }
}

impl Fields {
    /// Sorts `v` in place to become the record's storage; later duplicates win.
    pub fn from_unsorted(mut v: Vec<(Symbol, Value)>) -> Fields {
        v.sort_by(|(a, _), (b, _)| a.cmp(b));
        v.dedup_by(|later, earlier| {
            if later.0 == earlier.0 {
                *earlier = later.clone();
                true
            } else {
                false
            }
        });
        Fields(v)
    }
}

impl FromIterator<(Symbol, Value)> for Fields {
    fn from_iter<I: IntoIterator<Item = (Symbol, Value)>>(iter: I) -> Fields {
        Fields::from_unsorted(iter.into_iter().collect())
    }
}

impl<'a> IntoIterator for &'a Fields {
    type Item = (&'a Symbol, &'a Value);
    type IntoIter = std::vec::IntoIter<(&'a Symbol, &'a Value)>;
    fn into_iter(self) -> Self::IntoIter {
        self.0
            .iter()
            .map(|(k, v)| (k, v))
            .collect::<Vec<_>>()
            .into_iter()
    }
}

#[derive(Clone, Default)]
pub enum Value {
    Int(i64),
    Fixed(Fixed),
    Bool(bool),
    Float(f64),
    Decimal(Decimal),
    Str(Arc<str>),
    Bytes(Arc<[u8]>),
    #[default]
    Unit,
    List(List),
    /// Iterated in ascending key order by [`Value::cmp`], always.
    Map(Map),
    Record(Arc<Fields>),
    Ctor {
        name: Symbol,
        args: Arc<Vec<Value>>,
    },
    Closure(Arc<Closure>),
    /// An index and generation, so a cell of a closed region reads `None` instead of aliasing.
    Cell(Slot),
    Task(TaskId),
    /// A credential; a distinct variant rather than a `Ctor`, so no pattern match can unwrap it.
    Secret(Arc<Value>),
}

pub struct Closure {
    pub name: Option<Symbol>,
    pub kind: ClosureKind,
}

pub enum ClosureKind {
    Ctor {
        name: Symbol,
        arity: usize,
    },
    Builtin(Builtin),
    /// A compiled function taking `captured` as leading arguments; the machine never enters one.
    Native {
        code: usize,
        arity: usize,
        captured: Vec<Value>,
    },
    /// A function the prover generated: data rather than a body, so the compiled tier applies it.
    Synth {
        arity: usize,
        rule: Synth,
    },
}

pub enum Synth {
    Const(Value),
    /// The argument at this position.
    Project(usize),
    /// The first entry whose key equals the first argument, else `default`.
    Table {
        entries: Vec<(Value, Value)>,
        default: Value,
    },
}

impl Synth {
    /// `args` is as long as the closure's arity, which the caller checks.
    pub fn apply(&self, args: &[Value]) -> Result<Value, Diagnostic> {
        match self {
            Synth::Const(value) => Ok(value.clone()),
            Synth::Project(index) => Ok(args[*index].clone()),
            Synth::Table { entries, default } => {
                for (key, value) in entries {
                    if values_equal(&args[0], key, Span::DUMMY)? {
                        return Ok(value.clone());
                    }
                }
                Ok(default.clone())
            }
        }
    }

    pub fn values(&self) -> Vec<&Value> {
        match self {
            Synth::Const(value) => vec![value],
            Synth::Project(_) => Vec::new(),
            Synth::Table { entries, default } => entries
                .iter()
                .flat_map(|(key, value)| [key, value])
                .chain([default])
                .collect(),
        }
    }
}

impl Closure {
    pub fn arity(&self) -> usize {
        match &self.kind {
            ClosureKind::Builtin(b) => b.arity().0,
            ClosureKind::Ctor { arity, .. }
            | ClosureKind::Native { arity, .. }
            | ClosureKind::Synth { arity, .. } => *arity,
        }
    }
}

impl Value {
    // Never inlined, so allocation attribution by backtrace sees these frames.
    #[inline(never)]
    pub fn str(s: impl AsRef<str>) -> Value {
        Value::Str(Arc::from(s.as_ref()))
    }

    #[inline(never)]
    pub fn bytes(b: impl AsRef<[u8]>) -> Value {
        Value::Bytes(Arc::from(b.as_ref()))
    }

    #[inline(never)]
    pub fn list(items: Vec<Value>) -> Value {
        Value::List(List::from(items))
    }

    pub fn empty_map() -> Value {
        Value::Map(Map::new())
    }

    /// Later entries win, as in a fold of `map_insert`.
    pub fn map(entries: impl IntoIterator<Item = (Value, Value)>) -> Value {
        let mut m = Map::new();
        for (k, v) in entries {
            insert_key(&mut m, k, v);
        }
        Value::Map(m)
    }

    #[inline(never)]
    pub fn ctor(name: impl Into<Symbol>, args: Vec<Value>) -> Value {
        let args = if args.is_empty() {
            NO_ARGS.with(Arc::clone)
        } else {
            Arc::new(args)
        };
        Value::Ctor {
            name: name.into(),
            args,
        }
    }

    /// `ctor` for a pooled buffer: copies into an exact payload and returns the buffer.
    pub fn ctor_pooled(name: impl Into<Symbol>, mut args: Vec<Value>) -> Value {
        let mut payload = Vec::with_capacity(args.len());
        payload.append(&mut args);
        crate::argv::give(args);
        Value::ctor(name, payload)
    }

    pub fn builtin(b: Builtin) -> Value {
        let fresh = || {
            Value::Closure(Arc::new(Closure {
                name: Some(Symbol::new(b.name())),
                kind: ClosureKind::Builtin(b),
            }))
        };
        // `try_with`: thread-local teardown can drop a value after the cache is gone.
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
            Value::Fixed(f) => f.ty.name(),
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

    pub fn as_fixed(&self, span: Span, what: &str) -> Result<Fixed, Diagnostic> {
        match self {
            Value::Fixed(f) => Ok(*f),
            other => Err(type_error(span, what, "a fixed-width integer", other)),
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

    pub fn as_list(&self, span: Span, what: &str) -> Result<&List, Diagnostic> {
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
            Value::Fixed(f) => {
                let _ = write!(out, "{}", f.value());
            }
            Value::Bool(b) => {
                let _ = write!(out, "{b}");
            }
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
                // A tuple is the record `{_0: a, _1: b}` and renders as one.
                let tuple = fields.len() >= 2
                    && (0..fields.len())
                        .all(|i| fields.get(&Symbol::new(format!("_{i}"))).is_some());
                if tuple {
                    out.push('(');
                    for i in 0..fields.len() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        if let Some(v) = fields.get(&Symbol::new(format!("_{i}"))) {
                            v.write(out, depth + 1);
                        }
                    }
                    out.push(')');
                    return;
                }
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
            // No recursion into the payload, so the redaction holds at any depth.
            Value::Secret(_) => out.push_str(SECRET_REDACTED),
        }
    }
}

pub const SECRET_REDACTED: &str = "Secret(****)";

// Drop glue recurses per nesting level, so a deep value would overflow the stack when dropped.
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

fn take_children(v: &mut Value, out: &mut Vec<Value>) {
    match v {
        Value::List(xs) => {
            let mut items = Vec::new();
            xs.drain_unique(&mut items);
            out.extend(items.into_iter().filter(nests));
        }
        Value::Ctor { args: xs, .. } => {
            if let Some(items) = Arc::get_mut(xs) {
                out.extend(items.drain(..).filter(nests));
            }
        }
        Value::Record(fields) => {
            if let Some(map) = Arc::get_mut(fields) {
                out.extend(std::mem::take(map).into_values().filter(nests));
            }
        }
        // `rpds` has no owned iterator, and cloning entries out would rewalk the tree per level.
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

/// A fixed-width integer; `bits` is always normalized, so derived `Eq` and `Hash` are by value.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Fixed {
    pub ty: IntTy,
    bits: u64,
}

impl Fixed {
    pub fn new(ty: IntTy, bits: u64) -> Fixed {
        Fixed {
            ty,
            bits: ty.normalize(bits),
        }
    }

    pub fn of(ty: IntTy, v: i128) -> Option<Fixed> {
        ty.holds(v).then(|| Fixed::new(ty, v as u64))
    }

    pub fn bits(self) -> u64 {
        self.bits
    }

    /// The mathematical value, not the bits.
    pub fn value(self) -> i128 {
        self.ty.value(self.bits)
    }

    /// The bits with nothing above this type's width, for shifts, masks and rotates.
    pub fn raw(self) -> u64 {
        if self.ty.bits() == 64 {
            self.bits
        } else {
            self.bits & (u64::MAX >> (64 - self.ty.bits()))
        }
    }

    /// `f` over values in `i128`; `f` must check itself, since a `U64` product overflows `i128`.
    pub fn checked(self, other: Fixed, f: impl Fn(i128, i128) -> Option<i128>) -> Option<Fixed> {
        let v = f(self.value(), other.value())?;
        Fixed::of(self.ty, v)
    }

    /// `f` over the bits, cut to the width; two's complement makes this right when signed too.
    pub fn wrapping(self, other: Fixed, f: impl Fn(u128, u128) -> u128) -> Fixed {
        Fixed::new(self.ty, f(self.raw() as u128, other.raw() as u128) as u64)
    }
}

impl fmt::Display for Fixed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.value())
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
        Value::Secret(_) => 14,
        Value::Fixed(_) => 15,
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
            // By value rather than by bits, so `I8` orders `-1` below `0`.
            (Value::Fixed(x), Value::Fixed(y)) => {
                x.ty.cmp(&y.ty).then_with(|| x.value().cmp(&y.value()))
            }
            (Value::Float(x), Value::Float(y)) => x.total_cmp(y),
            // By numeric value, so `1.50m` and `1.5m` are one key.
            (Value::Decimal(x), Value::Decimal(y)) => x.cmp(y),
            (Value::Str(x), Value::Str(y)) => x.cmp(y),
            (Value::Bytes(x), Value::Bytes(y)) => x.cmp(y),
            // Grows the stack instead of bounding: `cmp` cannot refuse, and `Equal` merges keys.
            (Value::List(x), Value::List(y)) => grow(|| x.iter().cmp(y.iter())),
            (Value::Map(x), Value::Map(y)) => grow(|| x.iter().cmp(y.iter())),
            (Value::Record(x), Value::Record(y)) => grow(|| x.iter().cmp(y.iter())),
            (Value::Ctor { name: n1, args: a1 }, Value::Ctor { name: n2, args: a2 }) => {
                grow(|| n1.cmp(n2).then_with(|| a1.iter().cmp(a2.iter())))
            }
            (Value::Cell(x), Value::Cell(y)) => x.cmp(y),
            (Value::Task(x), Value::Task(y)) => x.cmp(y),
            // Unreachable from a well-typed program: a `Secret` has no order.
            (Value::Secret(x), Value::Secret(y)) => grow(|| x.cmp(y)),
            (Value::Closure(_), Value::Closure(_)) => Ordering::Equal,
            _ => Ordering::Equal,
        }
    }
}

impl PartialOrd for Value {
    fn partial_cmp(&self, other: &Value) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Rust's `==`, which is [`Ord`] and not the language's equality.
impl PartialEq for Value {
    fn eq(&self, other: &Value) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Value {}

/// The one place a key enters a [`Map`] from Rust, so keys are always canonical.
pub(crate) fn insert_key(m: &mut Map, k: Value, v: Value) {
    m.insert_mut(canonical_key(&k).unwrap_or(k), v);
}

/// The canonical member of `v`'s class under [`Value::cmp`], or `None` when `v` already is.
pub(crate) fn canonical_key(v: &Value) -> Option<Value> {
    if is_canonical(v) {
        return None;
    }
    Some(canonicalize(v))
}

fn is_canonical(v: &Value) -> bool {
    match v {
        // Minimal scale is unique per numeric value, so it is canonical.
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

/// Refuses past the bound; otherwise grows the stack so the bound is what a program meets.
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
        (Value::Fixed(x), Value::Fixed(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        // IEEE `==`, so `NaN != NaN` and `0.0 == -0.0`.
        (Value::Float(x), Value::Float(y)) => x == y,
        // By numeric value, so `1.50m == 1.5m`.
        (Value::Decimal(x), Value::Decimal(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
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
        (Value::Secret(x), Value::Secret(y)) => {
            return descend(span, depth, || match (&**x, &**y) {
                (Value::Str(p), Value::Str(q)) => Ok(constant_time_eq(p.as_bytes(), q.as_bytes())),
                (Value::Bytes(p), Value::Bytes(q)) => Ok(constant_time_eq(p, q)),
                // Structural, so a new payload type cannot silently make two secrets unequal.
                (p, q) => equal_at(p, q, span, depth + 1),
            });
        }
        (Value::Closure(_), _) | (_, Value::Closure(_)) => {
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

/// Byte equality whose time depends on the lengths, not on where they first differ.
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
            // Only when key sets agree, so a differing shape reports the whole maps.
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
