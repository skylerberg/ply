//! How a value's words read back out of compiled code. A width below 64 bits is a tagged `Int`
//! there, so only the type tells a `U8` from an `Int`; every other word says what it is. The
//! compiler publishes each root's, constructor's and operation's types as carries, so the runtime
//! reads no type.

use crate::{ClosureKind, IntTy, Symbol, Synth, Value};
use std::collections::BTreeMap;

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Carry {
    /// No fixed width sits anywhere below, so every word reads as itself.
    Plain,
    Width(IntTy),
    /// A list's elements, or an array's.
    List(Box<Carry>),
    Map(Box<Carry>, Box<Carry>),
    /// Sorted by name.
    Record(Vec<(Symbol, Carry)>),
    /// A sum type at its parameters: a constructor's fields read as its declaration types them.
    Sum(Vec<Carry>),
    /// A function's parameters and answer. Its words are never read, but a function the prover
    /// generated shows what its variables are.
    Fn(Vec<Carry>, Box<Carry>),
    /// The `n`th type variable of the scheme around it, or parameter of the declaration.
    Var(usize),
    /// A type nothing here fixes: a variable no argument bound, or a root the compiler published
    /// no carries for.
    Open,
}

/// Each constructor's fields over its type's parameters, by program-wide name; ordered, so a
/// front end reads and prints alike every time.
pub type CtorCarries = BTreeMap<Symbol, Vec<Carry>>;

impl Carry {
    /// Each variable `n` replaced by `args[n]`, and by [`Carry::Open`] past them.
    pub fn instantiate(&self, args: &[Carry]) -> Carry {
        match self {
            Carry::Var(n) => args.get(*n).cloned().unwrap_or(Carry::Open),
            Carry::List(item) => Carry::List(Box::new(item.instantiate(args))),
            Carry::Map(key, value) => Carry::Map(
                Box::new(key.instantiate(args)),
                Box::new(value.instantiate(args)),
            ),
            Carry::Record(fields) => Carry::Record(
                fields
                    .iter()
                    .map(|(name, c)| (name.clone(), c.instantiate(args)))
                    .collect(),
            ),
            Carry::Sum(params) => Carry::Sum(params.iter().map(|c| c.instantiate(args)).collect()),
            Carry::Fn(params, ret) => Carry::Fn(
                params.iter().map(|c| c.instantiate(args)).collect(),
                Box::new(ret.instantiate(args)),
            ),
            Carry::Plain | Carry::Width(_) | Carry::Open => self.clone(),
        }
    }

    pub fn mentions_var(&self) -> bool {
        match self {
            Carry::Var(_) => true,
            Carry::List(item) => item.mentions_var(),
            Carry::Map(key, value) => key.mentions_var() || value.mentions_var(),
            Carry::Record(fields) => fields.iter().any(|(_, c)| c.mentions_var()),
            Carry::Sum(params) => params.iter().any(Carry::mentions_var),
            Carry::Fn(params, ret) => params.iter().any(Carry::mentions_var) || ret.mentions_var(),
            Carry::Plain | Carry::Width(_) | Carry::Open => false,
        }
    }

    /// An element's carry.
    pub fn item(&self) -> Carry {
        match self {
            Carry::List(item) => (**item).clone(),
            Carry::Plain => Carry::Plain,
            _ => Carry::Open,
        }
    }

    /// A key's carry and a value's.
    pub fn entry(&self) -> (Carry, Carry) {
        match self {
            Carry::Map(key, value) => ((**key).clone(), (**value).clone()),
            Carry::Plain => (Carry::Plain, Carry::Plain),
            _ => (Carry::Open, Carry::Open),
        }
    }

    pub fn field(&self, name: &Symbol) -> Carry {
        match self {
            Carry::Record(fields) => fields
                .iter()
                .find(|(n, _)| n == name)
                .map_or(Carry::Open, |(_, c)| c.clone()),
            Carry::Plain => Carry::Plain,
            _ => Carry::Open,
        }
    }

    /// Field `i` of constructor `ctor`, of the sum type `self` stands for. Where `self` gives no
    /// parameters, the declaration's own types still read.
    pub fn ctor_field(&self, ctor: &Symbol, i: usize, ctors: &CtorCarries) -> Carry {
        let params: &[Carry] = match self {
            Carry::Plain => return Carry::Plain,
            Carry::Sum(params) => params,
            _ => &[],
        };
        ctors
            .get(ctor)
            .and_then(|fields| fields.get(i))
            .map_or(Carry::Open, |field| field.instantiate(params))
    }

    /// Binds the variables `self` names to what `value`, a value of the type `self` stands for,
    /// shows at them. A binding is only ever widened: whatever one value leaves open, another of
    /// the same type may fix.
    pub fn bind(&self, value: &Value, vars: &mut Vec<Carry>, ctors: &CtorCarries) {
        if self.settled(vars) {
            return;
        }
        match (self, value) {
            (Carry::Var(n), _) => {
                if vars.len() <= *n {
                    vars.resize(n + 1, Carry::Open);
                }
                let held = std::mem::replace(&mut vars[*n], Carry::Open);
                vars[*n] = held.join(Carry::of(value, ctors));
            }
            (Carry::List(item), Value::List(items)) => {
                for x in items.iter() {
                    item.bind(x, vars, ctors);
                    if item.settled(vars) {
                        break;
                    }
                }
            }
            (Carry::List(item), Value::Array(items)) => {
                for x in items.iter() {
                    item.bind(x, vars, ctors);
                    if item.settled(vars) {
                        break;
                    }
                }
            }
            (Carry::Map(key, val), Value::Map(entries)) => {
                for (k, v) in entries.iter() {
                    key.bind(k, vars, ctors);
                    val.bind(v, vars, ctors);
                    if self.settled(vars) {
                        break;
                    }
                }
            }
            (Carry::Record(fields), Value::Record(record)) => {
                for (name, c) in fields {
                    if let Some(x) = record.get(name) {
                        c.bind(x, vars, ctors);
                    }
                }
            }
            (Carry::Sum(params), Value::Ctor { name, args }) => {
                if let Some(fields) = ctors.get(name) {
                    for (field, x) in fields.iter().zip(args.iter()) {
                        field.instantiate(params).bind(x, vars, ctors);
                    }
                }
            }
            (Carry::Fn(params, ret), Value::Closure(closure)) => {
                if let ClosureKind::Synth { rule, .. } = &closure.kind {
                    match rule {
                        Synth::Const(answer) => ret.bind(answer, vars, ctors),
                        Synth::Table { entries, default } => {
                            for (key, answer) in entries {
                                if let Some(first) = params.first() {
                                    first.bind(key, vars, ctors);
                                }
                                ret.bind(answer, vars, ctors);
                            }
                            ret.bind(default, vars, ctors);
                        }
                        Synth::Project(_) => {}
                    }
                }
            }
            _ => {}
        }
    }

    /// The first value `value`, a value of the type `self` stands for, holds where `self` names
    /// variable `var`.
    pub fn value_at<'v>(
        &self,
        var: usize,
        value: &'v Value,
        ctors: &CtorCarries,
    ) -> Option<&'v Value> {
        match (self, value) {
            (Carry::Var(n), _) => (*n == var).then_some(value),
            (Carry::List(item), Value::List(items)) => {
                items.iter().find_map(|x| item.value_at(var, x, ctors))
            }
            (Carry::List(item), Value::Array(items)) => {
                items.iter().find_map(|x| item.value_at(var, x, ctors))
            }
            (Carry::Map(key, val), Value::Map(entries)) => entries.iter().find_map(|(k, v)| {
                key.value_at(var, k, ctors)
                    .or_else(|| val.value_at(var, v, ctors))
            }),
            (Carry::Record(fields), Value::Record(record)) => fields
                .iter()
                .find_map(|(name, c)| record.get(name).and_then(|x| c.value_at(var, x, ctors))),
            (Carry::Sum(params), Value::Ctor { name, args }) => {
                ctors.get(name).and_then(|fields| {
                    fields
                        .iter()
                        .zip(args.iter())
                        .find_map(|(field, x)| field.instantiate(params).value_at(var, x, ctors))
                })
            }
            _ => None,
        }
    }

    /// What `value` shows of its own type, as far as it holds values to show it.
    pub fn of(value: &Value, ctors: &CtorCarries) -> Carry {
        match value {
            Value::Fixed(f) => Carry::Width(f.ty),
            Value::List(items) => Carry::items(items.iter(), ctors),
            Value::Array(items) => Carry::items(items.iter(), ctors),
            Value::Map(entries) => {
                let (mut key, mut val) = (Carry::Open, Carry::Open);
                for (k, v) in entries.iter() {
                    if key.complete() && val.complete() {
                        break;
                    }
                    key = key.join(Carry::of(k, ctors));
                    val = val.join(Carry::of(v, ctors));
                }
                Carry::Map(Box::new(key), Box::new(val))
            }
            Value::Record(fields) => Carry::Record(
                fields
                    .iter()
                    .map(|(name, x)| (name.clone(), Carry::of(x, ctors)))
                    .collect(),
            ),
            Value::Ctor { name, args } => {
                let mut params = Vec::new();
                if let Some(fields) = ctors.get(name) {
                    for (field, x) in fields.iter().zip(args.iter()) {
                        field.bind(x, &mut params, ctors);
                    }
                }
                Carry::Sum(params)
            }
            _ => Carry::Plain,
        }
    }

    fn items<'a>(items: impl Iterator<Item = &'a Value>, ctors: &CtorCarries) -> Carry {
        let mut item = Carry::Open;
        for x in items {
            if item.complete() {
                break;
            }
            item = item.join(Carry::of(x, ctors));
        }
        Carry::List(Box::new(item))
    }

    /// What two readings of one type say together: an open part gives way to the other's.
    fn join(self, other: Carry) -> Carry {
        match (self, other) {
            (Carry::Open, c) | (c, Carry::Open) => c,
            (Carry::List(a), Carry::List(b)) => Carry::List(Box::new(a.join(*b))),
            (Carry::Map(a, b), Carry::Map(c, d)) => {
                Carry::Map(Box::new(a.join(*c)), Box::new(b.join(*d)))
            }
            (Carry::Record(a), Carry::Record(b)) if a.len() == b.len() => Carry::Record(
                a.into_iter()
                    .zip(b)
                    .map(|((name, x), (_, y))| (name, x.join(y)))
                    .collect(),
            ),
            (Carry::Sum(a), Carry::Sum(b)) => {
                let mut b = b.into_iter();
                let mut out: Vec<Carry> = a
                    .into_iter()
                    .map(|x| x.join(b.next().unwrap_or(Carry::Open)))
                    .collect();
                out.extend(b);
                Carry::Sum(out)
            }
            (c, _) => c,
        }
    }

    /// Whether nothing below is open, so no further value could fix more of it.
    fn complete(&self) -> bool {
        match self {
            Carry::Open | Carry::Var(_) => false,
            Carry::List(item) => item.complete(),
            Carry::Map(key, value) => key.complete() && value.complete(),
            Carry::Record(fields) => fields.iter().all(|(_, c)| c.complete()),
            Carry::Sum(params) => params.iter().all(Carry::complete),
            Carry::Fn(params, ret) => params.iter().all(Carry::complete) && ret.complete(),
            Carry::Plain | Carry::Width(_) => true,
        }
    }

    /// Whether every variable `self` names is already bound completely in `vars`.
    fn settled(&self, vars: &[Carry]) -> bool {
        match self {
            Carry::Var(n) => vars.get(*n).is_some_and(Carry::complete),
            Carry::List(item) => item.settled(vars),
            Carry::Map(key, value) => key.settled(vars) && value.settled(vars),
            Carry::Record(fields) => fields.iter().all(|(_, c)| c.settled(vars)),
            Carry::Sum(params) => params.iter().all(|c| c.settled(vars)),
            Carry::Fn(params, ret) => params.iter().all(|c| c.settled(vars)) && ret.settled(vars),
            Carry::Plain | Carry::Width(_) | Carry::Open => true,
        }
    }
}
