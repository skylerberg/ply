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
        Value::Map(out) => put(out, k, v, "map_insert", span)?,
        other => return Err(crate::value::type_error(span, "`map_insert`", "Map", other)),
    }
    Ok(m)
}

/// Removes `k`'s entry for `map_update`, so a map held once lends its function the value held once.
pub fn take(mut m: Value, k: &Value, span: Span) -> Result<(Value, Option<Value>), Diagnostic> {
    key(k, "map_update", span)?;
    let taken = match &mut m {
        Value::Map(out) => out.remove(k),
        other => return Err(crate::value::type_error(span, "`map_update`", "Map", other)),
    };
    Ok((m, taken))
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

/// An absent key is a no-op, not an error.
pub(crate) fn remove(mut m: Value, k: &Value, span: Span) -> Result<Value, Diagnostic> {
    key(k, "map_remove", span)?;
    match &mut m {
        Value::Map(out) => {
            out.remove(k);
        }
        other => return Err(crate::value::type_error(span, "`map_remove`", "Map", other)),
    }
    Ok(m)
}

pub(crate) fn len(m: &Value, span: Span) -> Result<Value, Diagnostic> {
    Ok(Value::Int(m.as_map(span, "`map_len`")?.len() as i64))
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

/// Later entries win, so `map_of_entries(map_entries(m))` is `m`; derived codecs rely on it.
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
        .primary(span, format!("this entry is {}", crate::slot(0)))
        .showing(vec![crate::Plain::shown(item)])),
    }
}

/// The right side wins a shared key.
pub(crate) fn merge(a: &Value, b: &Value, span: Span) -> Result<Value, Diagnostic> {
    let mut out = a.as_map(span, "`map_merge`")?.clone();
    for (k, v) in b.as_map(span, "`map_merge`")?.iter() {
        put(&mut out, k.clone(), v.clone(), "map_merge", span)?;
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
        Builtin::MapFirst => "`map_first`",
        Builtin::MapLast => "`map_last`",
        Builtin::MapFloor => "`map_floor`",
        Builtin::MapCeiling => "`map_ceiling`",
        Builtin::MapBelow => "`map_below`",
        Builtin::MapAbove => "`map_above`",
        Builtin::MapPopFirst => "`map_pop_first`",
        _ => "`map_pop_last`",
    }
}

/// `map_first` and `map_last`.
pub(crate) fn end(m: &Value, which: Builtin, span: Span) -> Result<Value, Diagnostic> {
    let m = m.as_map(span, quoted(which))?;
    Ok(entry_of(match which {
        Builtin::MapLast => m.iter().next_back(),
        _ => m.iter().next(),
    }))
}

/// `map_floor`, `map_ceiling`, `map_below` and `map_above`.
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

/// `map_pop_first` and `map_pop_last`: the entry, beside the map it left.
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

/// One end of `map_range`: `Some(key)` with whether the key itself is admitted, or `None`.
fn bound<'a>(k: &'a Value, inclusive: &Value, span: Span) -> Result<Bound<&'a Value>, Diagnostic> {
    let inclusive = inclusive.as_bool(span, "`map_range`")?;
    match k {
        Value::Ctor { name, args, .. } if name.as_str() == "Some" && args.len() == 1 => {
            key(&args[0], "map_range", span)?;
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
            "`map_range`",
            "an Option",
            other,
        )),
    }
}

/// `map_range(m, lo, lo_inclusive, hi, hi_inclusive, limit)`.
pub(crate) fn range(args: &[Value], span: Span) -> Result<Value, Diagnostic> {
    let [m, lo, lo_inclusive, hi, hi_inclusive, limit] = args else {
        unreachable!("arity checked");
    };
    let m = m.as_map(span, "`map_range`")?;
    let lo = bound(lo, lo_inclusive, span)?;
    let hi = bound(hi, hi_inclusive, span)?;
    let limit = usize::try_from(limit.as_int(span, "`map_range`")?).unwrap_or(0);
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

/// `map_split`: the entries below `k`, the value at it, and the entries above it.
pub(crate) fn split(mut m: Value, k: &Value, span: Span) -> Result<Value, Diagnostic> {
    key(k, "map_split", span)?;
    let (at, above) = match &mut m {
        Value::Map(below) => below.split(k),
        other => return Err(crate::value::type_error(span, "`map_split`", "Map", other)),
    };
    Ok(Value::Record(Arc::new(Fields::from_iter([
        (Symbol::new("below"), m),
        (Symbol::new("at"), option(at)),
        (Symbol::new("above"), Value::Map(above)),
    ]))))
}
