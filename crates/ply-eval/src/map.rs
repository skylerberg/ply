//! The `Map` builtins.
//!
//! Every one of them is a pure function of its arguments except `map_fold`,
//! which threads its function's row, and every one of them that iterates does so
//! in **ascending key order**. That is not a convenience: `map_keys`,
//! `map_values`, `map_entries` and `map_fold` are the four places a program can
//! observe an order at all, and a value whose canonical form depended on
//! insertion history would break content addressing, the result cache, seeded
//! replay and `--engine both` at once — every one of them silently, and every
//! one of them as a green result or as a red result over correct code.
//!
//! The order itself is [`Value::cmp`](crate::value::Value#impl-Ord-for-Value)
//! and lives beside the value it orders, so there is one definition of it rather
//! than one per operation.

use crate::cont::Frame;
use crate::value::{Map, Value};
use ply_span::{Diagnostic, Span, Symbol, codes};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

/// `map_entries` and `map_of_entries` speak in records rather than tuples,
/// because Ply has no tuples. Spelled once so the two cannot disagree.
const KEY: &str = "key";
const VALUE: &str = "value";

fn some(v: Value) -> Value {
    Value::ctor("Some", vec![v])
}

fn none() -> Value {
    Value::ctor("None", Vec::new())
}

fn entry(k: Value, v: Value) -> Value {
    Value::Record(Arc::new(BTreeMap::from([
        (Symbol::new(KEY), k),
        (Symbol::new(VALUE), v),
    ])))
}

pub(crate) fn new() -> Value {
    Value::empty_map()
}

/// Replaces an equal key's entry, **key and value both** — the last write wins.
/// Visible only where two equal keys are distinguishable, which is `Decimal`:
/// inserting `1.5m` over `1.50m` leaves the key `1.5m`, so `map_keys` then
/// renders `1.5` where it rendered `1.50`. The alternative costs a lookup on
/// every insert to preserve a distinction nobody asked for.
pub(crate) fn insert(m: &Value, k: Value, v: Value, span: Span) -> Result<Value, Diagnostic> {
    Ok(Value::Map(m.as_map(span, "`map_insert`")?.insert(k, v)))
}

pub(crate) fn get(m: &Value, k: &Value, span: Span) -> Result<Value, Diagnostic> {
    Ok(match m.as_map(span, "`map_get`")?.get(k) {
        Some(v) => some(v.clone()),
        None => none(),
    })
}

pub(crate) fn contains(m: &Value, k: &Value, span: Span) -> Result<Value, Diagnostic> {
    Ok(Value::Bool(
        m.as_map(span, "`map_contains`")?.contains_key(k),
    ))
}

/// An absent key is a no-op rather than an error: removing what is not there
/// leaves a map with the property the caller asked for, and refusing would make
/// every caller write the guard.
pub(crate) fn remove(m: &Value, k: &Value, span: Span) -> Result<Value, Diagnostic> {
    Ok(Value::Map(m.as_map(span, "`map_remove`")?.remove(k)))
}

pub(crate) fn len(m: &Value, span: Span) -> Result<Value, Diagnostic> {
    Ok(Value::Int(m.as_map(span, "`map_len`")?.size() as i64))
}

pub(crate) fn keys(m: &Value, span: Span) -> Result<Value, Diagnostic> {
    let m = m.as_map(span, "`map_keys`")?;
    Ok(Value::list(m.keys().cloned().collect()))
}

pub(crate) fn values(m: &Value, span: Span) -> Result<Value, Diagnostic> {
    let m = m.as_map(span, "`map_values`")?;
    Ok(Value::list(m.values().cloned().collect()))
}

pub(crate) fn entries(m: &Value, span: Span) -> Result<Value, Diagnostic> {
    let m = m.as_map(span, "`map_entries`")?;
    Ok(Value::list(
        m.iter().map(|(k, v)| entry(k.clone(), v.clone())).collect(),
    ))
}

/// Later entries win, matching a fold of [`insert`] — so
/// `map_of_entries(map_entries(m))` is `m` for every `m`, which is the property
/// a derived codec's round trip rests on.
pub(crate) fn of_entries(list: &Value, span: Span) -> Result<Value, Diagnostic> {
    let items = list.as_list(span, "`map_of_entries`")?;
    let mut out = Map::new();
    for item in items.iter() {
        let (k, v) = pair(item, span)?;
        out.insert_mut(k, v);
    }
    Ok(Value::Map(out))
}

fn pair(item: &Value, span: Span) -> Result<(Value, Value), Diagnostic> {
    let Value::Record(fields) = item else {
        return Err(crate::value::type_error(
            span,
            "`map_of_entries`",
            "a list of `{key, value}` records",
            item,
        ));
    };
    match (
        fields.get(&Symbol::new(KEY)),
        fields.get(&Symbol::new(VALUE)),
    ) {
        (Some(k), Some(v)) => Ok((k.clone(), v.clone())),
        _ => Err(Diagnostic::error(
            codes::RUNTIME_ERROR,
            "`map_of_entries` needs each entry to have a `key` and a `value` field",
        )
        .primary(span, format!("this entry is {}", item.render()))),
    }
}

/// The right side wins a shared key, by the same last-write-wins rule
/// [`insert`] follows. Folding the right into the left is O(m log(n+m)) and
/// shares the left's structure, which is why the argument order is not
/// symmetric in cost.
pub(crate) fn merge(a: &Value, b: &Value, span: Span) -> Result<Value, Diagnostic> {
    let mut out = a.as_map(span, "`map_merge`")?.clone();
    for (k, v) in b.as_map(span, "`map_merge`")?.iter() {
        out.insert_mut(k.clone(), v.clone());
    }
    Ok(Value::Map(out))
}

/// The entries `map_fold` will visit, in ascending key order, snapshotted.
///
/// A snapshot rather than a live cursor because the frame it rides in is
/// `Clone`: a continuation captured inside the folded function may be resumed
/// more than once, and each resumption has to continue over the same entries in
/// the same order rather than over whatever the tree looks like by then.
pub type Entries = Rc<Vec<(Value, Value)>>;

pub(crate) fn fold_entries(m: &Value, span: Span) -> Result<Entries, Diagnostic> {
    let m = m.as_map(span, "`map_fold`")?;
    Ok(Rc::new(
        m.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
    ))
}

/// One step of `map_fold`: apply `f` to `(acc, key, value)`, or finish.
pub(crate) fn next_fold(
    f: Value,
    entries: Entries,
    next: usize,
    acc: Value,
    span: Span,
) -> crate::builtins::Step {
    let Some((k, v)) = entries.get(next).cloned() else {
        return crate::builtins::Step::Done(acc);
    };
    crate::builtins::Step::Apply {
        callee: f.clone(),
        args: vec![acc, k, v],
        frame: Frame::MapFoldStep {
            f,
            entries,
            next: next + 1,
            span,
        },
    }
}
