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
//! than one per operation. The order is not enough on its own: it is coarser
//! than what a program can print, so the *key* is reduced to the canonical
//! member of its class as well — [`value::canonical_key`](crate::value::canonical_key)
//! says which distinction that is and why the order cannot see it.

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

/// The one gate every key passes through before [`Value::cmp`] sees it.
///
/// A `Map` orders its keys, and ordering a `Secret` recovers its plaintext in a
/// number of comparisons proportional to its length — so this is where ADR 0015
/// §2.2's runtime backstop lives: once, under the operations, rather than once
/// per builtin at the call sites. The per-builtin version covered four of the
/// six, and the two it missed — `map_of_entries` and `map_merge`, which reach
/// `insert_mut` by a different route — were a total ordering oracle over a
/// credential.
fn key(k: &Value, what: &str, span: Span) -> Result<(), Diagnostic> {
    crate::value::secret_has_no_order(k, what, span)
}

/// The only insert in this module, so that adding a seventh map builder cannot
/// reintroduce the gap: there is one place a key enters a `Map`.
///
/// [`value::insert_key`](crate::value::insert_key) is what it delegates to, and
/// it puts the key in canonical form first — see [`value::canonical_key`](crate::value::canonical_key).
fn put(m: &mut Map, k: Value, v: Value, what: &str, span: Span) -> Result<(), Diagnostic> {
    key(&k, what, span)?;
    crate::value::insert_key(m, k, v);
    Ok(())
}

/// Replaces an equal key's entry, **key and value both** — the last write wins.
///
/// That rule is only observable where two equal keys are distinguishable, which
/// is `Decimal`, and it is not observable there either: a key is reduced to the
/// canonical member of its class on the way in, so `1.5m` and `1.50m` both
/// store `1.5` and `map_keys` answers the same list whichever was written last.
/// The alternative — keeping the first spelling instead of the last — is still
/// a function of insertion history and fixes nothing.
///
/// Takes the map **by value**, so a caller that was its last owner hands over
/// the tree rather than a second handle to it and `insert_mut` rewrites the
/// nodes on the path instead of copying them. Borrowing here and cloning would
/// guarantee two owners at the moment of the write, which is the shape that
/// makes reference counting cost something and buy nothing.
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

/// An absent key is a no-op rather than an error: removing what is not there
/// leaves a map with the property the caller asked for, and refusing would make
/// every caller write the guard.
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

/// The right side wins a shared key, by the same last-write-wins rule
/// [`insert`] follows. Folding the right into the left is O(m log(n+m)) and
/// shares the left's structure, which is why the argument order is not
/// symmetric in cost.
pub(crate) fn merge(a: &Value, b: &Value, span: Span) -> Result<Value, Diagnostic> {
    let mut out = a.as_map(span, "`map_merge`")?.clone();
    for (k, v) in b.as_map(span, "`map_merge`")?.iter() {
        put(&mut out, k.clone(), v.clone(), "map_merge", span)?;
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
