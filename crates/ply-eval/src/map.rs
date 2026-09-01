//! The `Map` builtins.

use crate::cont::Frame;
use crate::value::{Fields, Map, Value};
use ply_span::{Diagnostic, Span, Symbol, codes};
use std::rc::Rc;
use std::sync::Arc;

/// `map_entries` and `map_of_entries` speak in records rather than tuples, because Ply has no
/// tuples.
const KEY: &str = "key";
const VALUE: &str = "value";

fn some(v: Value) -> Value {
    Value::ctor("Some", vec![v])
}

fn none() -> Value {
    Value::ctor("None", Vec::new())
}

fn entry(k: Value, v: Value) -> Value {
    Value::Record(Arc::new(Fields::from_iter([
        (Symbol::new(KEY), k),
        (Symbol::new(VALUE), v),
    ])))
}

pub(crate) fn new() -> Value {
    Value::empty_map()
}

/// The one gate every key passes through before [`Value::cmp`] sees it.
fn key(k: &Value, what: &str, span: Span) -> Result<(), Diagnostic> {
    crate::value::secret_has_no_order(k, what, span)
}

/// The only insert in this module, so that adding a seventh map builder cannot reintroduce the gap:
/// there is one place a key enters a `Map`.
fn put(m: &mut Map, k: Value, v: Value, what: &str, span: Span) -> Result<(), Diagnostic> {
    key(&k, what, span)?;
    crate::value::insert_key(m, k, v);
    Ok(())
}

/// Replaces an equal key's entry, **key and value both** — the last write wins.
pub(crate) fn insert(mut m: Value, k: Value, v: Value, span: Span) -> Result<Value, Diagnostic> {
    match &mut m {
        Value::Map(out) => put(out, k, v, "map_insert", span)?,
        other => return Err(crate::value::type_error(span, "`map_insert`", "Map", other)),
    }
    Ok(m)
}

pub(crate) fn get(m: &Value, k: &Value, span: Span) -> Result<Value, Diagnostic> {
    key(k, "map_get", span)?;
    Ok(match m.as_map(span, "`map_get`")?.get(k) {
        Some(v) => some(v.clone()),
        None => none(),
    })
}

pub(crate) fn contains(m: &Value, k: &Value, span: Span) -> Result<Value, Diagnostic> {
    key(k, "map_contains", span)?;
    Ok(Value::Bool(
        m.as_map(span, "`map_contains`")?.contains_key(k),
    ))
}

/// An absent key is a no-op rather than an error: removing what is not there leaves a map with the
/// property the caller asked for, and refusing would make every caller write the guard.
pub(crate) fn remove(mut m: Value, k: &Value, span: Span) -> Result<Value, Diagnostic> {
    key(k, "map_remove", span)?;
    match &mut m {
        Value::Map(out) => {
            out.remove_mut(k);
        }
        other => return Err(crate::value::type_error(span, "`map_remove`", "Map", other)),
    }
    Ok(m)
}

/// Takes the entry's value out of the map for a `map_update`: the map no longer holds it, so
/// the function it is handed to sees the value at one owner when nothing else does.
pub(crate) fn take(
    mut m: Value,
    k: &Value,
    span: Span,
) -> Result<(Value, Option<Value>), Diagnostic> {
    key(k, "map_update", span)?;
    let taken = match &mut m {
        Value::Map(out) => match out.get(k).cloned() {
            Some(v) => {
                out.remove_mut(k);
                Some(v)
            }
            None => None,
        },
        other => return Err(crate::value::type_error(span, "`map_update`", "Map", other)),
    };
    Ok((m, taken))
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

/// Later entries win, matching a fold of [`insert`] — so `map_of_entries(map_entries(m))` is `m`
/// for every `m`, which is the property a derived codec's round trip rests on.
pub(crate) fn of_entries(list: &Value, span: Span) -> Result<Value, Diagnostic> {
    let items = list.as_list(span, "`map_of_entries`")?;
    let mut out = Map::new();
    for item in items.iter() {
        let (k, v) = pair(item, span)?;
        put(&mut out, k, v, "map_of_entries", span)?;
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

/// The right side wins a shared key, by the same last-write-wins rule [`insert`] follows.
pub(crate) fn merge(a: &Value, b: &Value, span: Span) -> Result<Value, Diagnostic> {
    let mut out = a.as_map(span, "`map_merge`")?.clone();
    for (k, v) in b.as_map(span, "`map_merge`")?.iter() {
        put(&mut out, k.clone(), v.clone(), "map_merge", span)?;
    }
    Ok(Value::Map(out))
}

/// The entries `map_fold` will visit, in ascending key order, snapshotted.
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
        args: crate::argv::of([acc, k, v]),
        frame: Frame::MapFoldStep {
            f,
            entries,
            next: next + 1,
            span,
        },
    }
}
