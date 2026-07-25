use crate::builtins::Builtin;
use crate::env::Env;
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::Expr;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::Arc;

/// Whole-list sharing rather than structural sharing: `push` copies, which is
/// fine at v0 sizes and costs no persistent-vector dependency.
pub type Vector<T> = Arc<Vec<T>>;

const RENDER_MAX_ITEMS: usize = 32;
const RENDER_MAX_DEPTH: usize = 16;

#[derive(Clone)]
pub enum Value {
    Int(i64),
    Bool(bool),
    Str(Arc<str>),
    Unit,
    List(Vector<Value>),
    Record(Arc<BTreeMap<Symbol, Value>>),
    Ctor { name: Symbol, args: Arc<Vec<Value>> },
    Closure(Arc<Closure>),
    Cell(Rc<RefCell<Value>>),
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
            ClosureKind::Ctor { arity, .. } => *arity,
            ClosureKind::Builtin(b) => b.arity().0,
        }
    }

    pub fn describe(&self) -> String {
        match (&self.name, &self.kind) {
            (Some(n), _) => format!("`{n}`"),
            (None, ClosureKind::Ctor { name, .. }) => format!("`{name}`"),
            (None, ClosureKind::Builtin(b)) => format!("`{}`", b.name()),
            (None, ClosureKind::Fn { .. }) => "an anonymous function".to_string(),
        }
    }
}

impl Value {
    pub fn str(s: impl AsRef<str>) -> Value {
        Value::Str(Arc::from(s.as_ref()))
    }

    pub fn list(items: Vec<Value>) -> Value {
        Value::List(Arc::new(items))
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
            Value::Str(_) => "String",
            Value::Unit => "Unit",
            Value::List(_) => "List",
            Value::Record(_) => "record",
            Value::Ctor { .. } => "variant",
            Value::Closure(_) => "function",
            Value::Cell(_) => "Cell",
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

    pub fn as_str(&self, span: Span, what: &str) -> Result<&str, Diagnostic> {
        match self {
            Value::Str(s) => Ok(s),
            other => Err(type_error(span, what, "String", other)),
        }
    }

    pub fn as_list(&self, span: Span, what: &str) -> Result<&Vector<Value>, Diagnostic> {
        match self {
            Value::List(xs) => Ok(xs),
            other => Err(type_error(span, what, "List", other)),
        }
    }

    pub fn as_cell(&self, span: Span, what: &str) -> Result<&Rc<RefCell<Value>>, Diagnostic> {
        match self {
            Value::Cell(c) => Ok(c),
            other => Err(type_error(span, what, "Cell", other)),
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
            Value::Str(s) => {
                out.push('"');
                out.push_str(&escape(s));
                out.push('"');
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
            Value::Cell(c) => match c.try_borrow() {
                Ok(v) => {
                    out.push_str("<cell ");
                    v.write(out, depth + 1);
                    out.push('>');
                }
                Err(_) => out.push_str("<cell>"),
            },
        }
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

pub(crate) fn type_error(span: Span, what: &str, expected: &str, got: &Value) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("{what} expects {expected}, but got {}", got.type_name()),
    )
    .primary(span, format!("this is {}", got.render()))
}

/// Comparing functions is an error rather than a silently-false answer.
pub fn values_equal(a: &Value, b: &Value, span: Span) -> Result<bool, Diagnostic> {
    Ok(match (a, b) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Unit, Value::Unit) => true,
        (Value::List(x), Value::List(y)) => {
            if x.len() != y.len() {
                return Ok(false);
            }
            for (p, q) in x.iter().zip(y.iter()) {
                if !values_equal(p, q, span)? {
                    return Ok(false);
                }
            }
            true
        }
        (Value::Record(x), Value::Record(y)) => {
            if x.len() != y.len() || x.keys().ne(y.keys()) {
                return Ok(false);
            }
            for (p, q) in x.values().zip(y.values()) {
                if !values_equal(p, q, span)? {
                    return Ok(false);
                }
            }
            true
        }
        (Value::Ctor { name: n1, args: a1 }, Value::Ctor { name: n2, args: a2 }) => {
            if n1 != n2 || a1.len() != a2.len() {
                return Ok(false);
            }
            for (p, q) in a1.iter().zip(a2.iter()) {
                if !values_equal(p, q, span)? {
                    return Ok(false);
                }
            }
            true
        }
        (Value::Cell(x), Value::Cell(y)) => Rc::ptr_eq(x, y),
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

pub fn first_difference(actual: &Value, expected: &Value) -> Option<(String, String, String)> {
    fn go(actual: &Value, expected: &Value, path: &mut String) -> Option<(String, String, String)> {
        match (actual, expected) {
            (Value::List(a), Value::List(e)) if a.len() == e.len() => {
                for (i, (x, y)) in a.iter().zip(e.iter()).enumerate() {
                    let mark = path.len();
                    let _ = write!(path, "[{i}]");
                    if let Some(found) = go(x, y, path) {
                        return Some(found);
                    }
                    path.truncate(mark);
                }
                None
            }
            (Value::Record(a), Value::Record(e)) if a.keys().eq(e.keys()) => {
                for ((k, x), y) in a.iter().zip(e.values()) {
                    let mark = path.len();
                    let _ = write!(path, ".{k}");
                    if let Some(found) = go(x, y, path) {
                        return Some(found);
                    }
                    path.truncate(mark);
                }
                None
            }
            (Value::Ctor { name: n1, args: a1 }, Value::Ctor { name: n2, args: a2 })
                if n1 == n2 && a1.len() == a2.len() =>
            {
                for (i, (x, y)) in a1.iter().zip(a2.iter()).enumerate() {
                    let mark = path.len();
                    let _ = write!(path, ".{n1}.{i}");
                    if let Some(found) = go(x, y, path) {
                        return Some(found);
                    }
                    path.truncate(mark);
                }
                None
            }
            (a, e) => {
                if values_equal(a, e, Span::DUMMY).unwrap_or(false) || path.is_empty() {
                    None
                } else {
                    Some((path.clone(), e.render(), a.render()))
                }
            }
        }
    }
    let mut path = String::new();
    go(actual, expected, &mut path)
}
