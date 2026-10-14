//! The `Map` builtins.

use crate::builtins::Builtin;
use crate::value::{Fields, Map, Value};
use crate::{Diagnostic, Span, Symbol, codes};
use std::ops::Bound;
use std::sync::Arc;

/// An entry is a `{key, value}` record.
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

/// The gate every key passes before [`Value::cmp`] sees it.
fn key(k: &Value, what: &str, span: Span) -> Result<(), Diagnostic> {
    crate::value::secret_has_no_order(k, what, span)
}

/// The only place a key enters a `Map`, so no builder can skip `key`.
fn put(m: &mut Map, k: Value, v: Value, what: &str, span: Span) -> Result<(), Diagnostic> {
    key(&k, what, span)?;
    crate::value::insert_key(m, k, v);
    Ok(())
}

/// Replaces an equal key's entry, key and value both.
pub(crate) fn insert(mut m: Value, k: Value, v: Value, span: Span) -> Result<Value, Diagnostic> {
    match &mut m {
        Value::Map(out) => put(out, k, v, "Map::insert", span)?,
        other => {
            return Err(crate::value::type_error(
                span,
                "`Map::insert`",
                "Map",
                other,
            ));
        }
    }
    Ok(m)
}

/// Removes `k`'s entry for `Map::update`, so a map held once lends its function the value held once.
pub fn take(mut m: Value, k: &Value, span: Span) -> Result<(Value, Option<Value>), Diagnostic> {
    key(k, "Map::update", span)?;
    let taken = match &mut m {
        Value::Map(out) => out.remove(k),
        other => {
            return Err(crate::value::type_error(
                span,
                "`Map::update`",
                "Map",
                other,
            ));
        }
    };
    Ok((m, taken))
}

pub(crate) fn get(m: &Value, k: &Value, span: Span) -> Result<Value, Diagnostic> {
    key(k, "Map::get", span)?;
    Ok(match m.as_map(span, "`Map::get`")?.get(k) {
        Some(v) => some(v.clone()),
        None => none(),
    })
}

pub(crate) fn contains(m: &Value, k: &Value, span: Span) -> Result<Value, Diagnostic> {
    key(k, "Map::contains", span)?;
    Ok(Value::Bool(
        m.as_map(span, "`Map::contains`")?.contains_key(k),
    ))
}

/// An absent key is a no-op, not an error.
pub(crate) fn remove(mut m: Value, k: &Value, span: Span) -> Result<Value, Diagnostic> {
    key(k, "Map::remove", span)?;
    match &mut m {
        Value::Map(out) => {
            out.remove(k);
        }
        other => {
            return Err(crate::value::type_error(
                span,
                "`Map::remove`",
                "Map",
                other,
            ));
        }
    }
    Ok(m)
}

pub(crate) fn len(m: &Value, span: Span) -> Result<Value, Diagnostic> {
    Ok(Value::Int(m.as_map(span, "`Map::len`")?.len() as i64))
}

pub(crate) fn keys(m: &Value, span: Span) -> Result<Value, Diagnostic> {
    let m = m.as_map(span, "`Map::keys`")?;
    Ok(Value::list(m.keys().cloned().collect()))
}

pub(crate) fn values(m: &Value, span: Span) -> Result<Value, Diagnostic> {
    let m = m.as_map(span, "`Map::values`")?;
    Ok(Value::list(m.values().cloned().collect()))
}

pub(crate) fn entries(m: &Value, span: Span) -> Result<Value, Diagnostic> {
    let m = m.as_map(span, "`Map::entries`")?;
    Ok(Value::list(
        m.iter().map(|(k, v)| entry(k.clone(), v.clone())).collect(),
    ))
}

/// Later entries win, so `Map::of_entries(map_entries(m))` is `m`; derived codecs rely on it.
pub(crate) fn of_entries(list: &Value, span: Span) -> Result<Value, Diagnostic> {
    let items = list.as_list(span, "`Map::of_entries`")?;
    let mut out = Map::new();
    for item in items.iter() {
        let (k, v) = pair(item, span)?;
        put(&mut out, k, v, "Map::of_entries", span)?;
    }
    Ok(Value::Map(out))
}

fn pair(item: &Value, span: Span) -> Result<(Value, Value), Diagnostic> {
    let Value::Record(fields) = item else {
        return Err(crate::value::type_error(
            span,
            "`Map::of_entries`",
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
            "`Map::of_entries` needs each entry to have a `key` and a `value` field",
        )
        .primary(span, format!("this entry is {}", crate::slot(0)))
        .showing(vec![crate::Plain::shown(item)])),
    }
}

/// The right side wins a shared key.
pub(crate) fn merge(a: &Value, b: &Value, span: Span) -> Result<Value, Diagnostic> {
    let mut out = a.as_map(span, "`Map::merge`")?.clone();
    for (k, v) in b.as_map(span, "`Map::merge`")?.iter() {
        put(&mut out, k.clone(), v.clone(), "Map::merge", span)?;
    }
    Ok(Value::Map(out))
}

fn option(v: Option<Value>) -> Value {
    v.map_or_else(none, some)
}

fn entry_of(found: Option<(&Value, &Value)>) -> Value {
    option(found.map(|(k, v)| entry(k.clone(), v.clone())))
}

/// The builtin as a type error names it.
fn quoted(which: Builtin) -> &'static str {
    match which {
        Builtin::MapFirst => "`Map::first`",
        Builtin::MapLast => "`Map::last`",
        Builtin::MapFloor => "`Map::floor`",
        Builtin::MapCeiling => "`Map::ceiling`",
        Builtin::MapBelow => "`Map::below`",
        Builtin::MapAbove => "`Map::above`",
        Builtin::MapPopFirst => "`Map::pop_first`",
        _ => "`Map::pop_last`",
    }
}

/// `Map::first` and `Map::last`.
pub(crate) fn end(m: &Value, which: Builtin, span: Span) -> Result<Value, Diagnostic> {
    let m = m.as_map(span, quoted(which))?;
    Ok(entry_of(match which {
        Builtin::MapLast => m.iter().next_back(),
        _ => m.iter().next(),
    }))
}

/// `Map::floor`, `Map::ceiling`, `Map::below` and `Map::above`.
pub(crate) fn beside(
    m: &Value,
    k: &Value,
    which: Builtin,
    span: Span,
) -> Result<Value, Diagnostic> {
    key(k, which.name(), span)?;
    let m = m.as_map(span, quoted(which))?;
    Ok(entry_of(match which {
        Builtin::MapFloor => m.range((Bound::Unbounded, Bound::Included(k))).next_back(),
        Builtin::MapBelow => m.range((Bound::Unbounded, Bound::Excluded(k))).next_back(),
        Builtin::MapCeiling => m.range((Bound::Included(k), Bound::Unbounded)).next(),
        _ => m.range((Bound::Excluded(k), Bound::Unbounded)).next(),
    }))
}

/// `Map::pop_first` and `Map::pop_last`: the entry, beside the map it left.
pub(crate) fn pop(mut m: Value, which: Builtin, span: Span) -> Result<Value, Diagnostic> {
    let taken = match &mut m {
        Value::Map(out) => out.pop(which == Builtin::MapPopLast),
        other => return Err(crate::value::type_error(span, quoted(which), "Map", other)),
    };
    Ok(option(taken.map(|(k, v)| {
        Value::Record(Arc::new(Fields::from_iter([
            (Symbol::new(KEY), k),
            (Symbol::new(VALUE), v),
            (Symbol::new("rest"), m),
        ])))
    })))
}

/// One end of `Map::range`: `Some(key)` with whether the key itself is admitted, or `None`.
fn bound<'a>(k: &'a Value, inclusive: &Value, span: Span) -> Result<Bound<&'a Value>, Diagnostic> {
    let inclusive = inclusive.as_bool(span, "`Map::range`")?;
    match k {
        Value::Ctor { name, args, .. } if name.as_str() == "Some" && args.len() == 1 => {
            key(&args[0], "Map::range", span)?;
            Ok(if inclusive {
                Bound::Included(&args[0])
            } else {
                Bound::Excluded(&args[0])
            })
        }
        Value::Ctor { name, args, .. } if name.as_str() == "None" && args.is_empty() => {
            Ok(Bound::Unbounded)
        }
        other => Err(crate::value::type_error(
            span,
            "`Map::range`",
            "an Option",
            other,
        )),
    }
}

/// `Map::range(m, lo, lo_inclusive, hi, hi_inclusive, limit)`.
pub(crate) fn range(args: &[Value], span: Span) -> Result<Value, Diagnostic> {
    let [m, lo, lo_inclusive, hi, hi_inclusive, limit] = args else {
        unreachable!("arity checked");
    };
    let m = m.as_map(span, "`Map::range`")?;
    let lo = bound(lo, lo_inclusive, span)?;
    let hi = bound(hi, hi_inclusive, span)?;
    let limit = usize::try_from(limit.as_int(span, "`Map::range`")?).unwrap_or(0);
    // Bounds that cross admit nothing, and `BTreeMap::range` panics on them.
    let crossed = match (lo, hi) {
        (Bound::Included(a), Bound::Included(b)) => a > b,
        (Bound::Included(a) | Bound::Excluded(a), Bound::Included(b) | Bound::Excluded(b)) => {
            a >= b
        }
        _ => false,
    };
    if crossed {
        return Ok(Value::list(Vec::new()));
    }
    Ok(Value::list(
        m.range((lo, hi))
            .take(limit)
            .map(|(k, v)| entry(k.clone(), v.clone()))
            .collect(),
    ))
}

/// `Map::split`: the entries below `k`, the value at it, and the entries above it.
pub(crate) fn split(mut m: Value, k: &Value, span: Span) -> Result<Value, Diagnostic> {
    key(k, "Map::split", span)?;
    let (at, above) = match &mut m {
        Value::Map(below) => below.split(k),
        other => return Err(crate::value::type_error(span, "`Map::split`", "Map", other)),
    };
    Ok(Value::Record(Arc::new(Fields::from_iter([
        (Symbol::new("below"), m),
        (Symbol::new("at"), option(at)),
        (Symbol::new("above"), Value::Map(above)),
    ]))))
}
