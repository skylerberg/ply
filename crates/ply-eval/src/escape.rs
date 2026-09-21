//! Escape enforcement at the boundaries the brand cannot see.

use crate::limit::grow;
use crate::value::{ClosureKind, Value};
use ply_span::{Diagnostic, Span, codes};
use std::borrow::Cow;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Handle {
    Cell,
    Task,
}

impl Handle {
    pub fn as_str(self) -> &'static str {
        match self {
            Handle::Cell => "Cell",
            Handle::Task => "Task",
        }
    }

    fn why(self) -> &'static str {
        match self {
            Handle::Cell => {
                "a `Cell` is a key into the region that allocated it, and the region frees its \
                 slots at its `}`"
            }
            Handle::Task => {
                "a `Task` is a key into a scheduler, and the scheduler dies with the region that \
                 opened it"
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Escapee {
    pub handle: Handle,
    /// Outermost first, e.g. `["item 2", "`Just`'s argument 1"]`.
    pub route: Vec<String>,
}

impl Escapee {
    pub fn reached(&self) -> String {
        if self.route.is_empty() {
            return String::new();
        }
        format!(", reached through {}", self.route.join(" → "))
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Boundary<'a> {
    HostArgument {
        operation: &'a str,
        path: &'static str,
        position: usize,
    },
    /// A host handler's answer, inline or through `HostRuntime::block_on`.
    HostAnswer {
        operation: &'a str,
        path: &'static str,
    },
    /// The value a host runtime resolved a parked token to.
    HostToken {
        label: &'static str,
        token: u64,
    },
    EntryPoint {
        name: &'a str,
    },
}

impl Boundary<'_> {
    fn headline(&self, handle: Handle, reached: &str) -> String {
        let what = handle.as_str();
        match self {
            Boundary::HostArgument {
                operation,
                position,
                ..
            } => format!(
                "`{operation}` was handed a `{what}` in argument {}{reached}",
                position + 1
            ),
            Boundary::HostAnswer { operation, .. } => {
                format!("`{operation}` answered with a `{what}`{reached}")
            }
            Boundary::HostToken { label, token } => {
                format!("the host runtime resolved `{label}` (#{token}) to a `{what}`{reached}")
            }
            Boundary::EntryPoint { name } => {
                format!("`{name}` was called with a `{what}`{reached}")
            }
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Boundary::HostArgument { .. } => "performed here",
            Boundary::HostAnswer { .. } | Boundary::HostToken { .. } => "the answer to this",
            Boundary::EntryPoint { .. } => "entered here",
        }
    }

    fn outlives(&self) -> Cow<'static, str> {
        match self {
            Boundary::HostArgument { path, .. } => Cow::Owned(format!(
                "`{path}` is a host handler, and a host handler outlives every region the program \
                 opens"
            )),
            Boundary::HostAnswer { path, .. } => Cow::Owned(format!(
                "`{path}` is outside the program, so a handle it produced names no region this \
                 run allocated"
            )),
            Boundary::HostToken { .. } => Cow::Borrowed(
                "a host runtime is outside the program, so a handle it produced names no region \
                 this run allocated",
            ),
            Boundary::EntryPoint { .. } => Cow::Borrowed(
                "an entry point resets its region stack to the fixture before it runs, restoring \
                 the fixture's generations — so a slot carried out of an earlier run resolves \
                 here and reads whatever this run put at that position",
            ),
        }
    }

    fn remedy(&self) -> &'static str {
        match self {
            Boundary::HostArgument { .. } => {
                "read the value inside the region and perform the operation with something that \
                 does not reach a region"
            }
            Boundary::HostAnswer { .. } | Boundary::HostToken { .. } => {
                "a handler answers with data; a handle into the program's memory is not data it \
                 is in a position to have"
            }
            Boundary::EntryPoint { .. } => {
                "call the entry point with data, and let the program allocate its own cells"
            }
        }
    }
}

/// The first region-bound handle `value` can reach, with the route to it.
pub fn carries(value: &Value) -> Option<Escapee> {
    let mut route = Vec::new();
    let handle = find(value, &mut route)?;
    route.reverse();
    Some(Escapee { handle, route })
}

pub fn check(boundary: &Boundary<'_>, value: &Value, span: Span) -> Result<(), Diagnostic> {
    match carries(value) {
        None => Ok(()),
        Some(escapee) => Err(refuse(boundary, &escapee, span)),
    }
}

pub fn check_arguments(
    operation: &str,
    path: &'static str,
    args: &[Value],
    span: Span,
) -> Result<(), Diagnostic> {
    for (position, arg) in args.iter().enumerate() {
        if let Some(escapee) = carries(arg) {
            let boundary = Boundary::HostArgument {
                operation,
                path,
                position,
            };
            return Err(refuse(&boundary, &escapee, span));
        }
    }
    Ok(())
}

#[cold]
#[inline(never)]
fn refuse(boundary: &Boundary<'_>, escapee: &Escapee, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::REGION_ESCAPE_AT_BOUNDARY,
        boundary.headline(escapee.handle, &escapee.reached()),
    )
    .primary(span, boundary.label())
    .note(escapee.handle.why())
    .note(boundary.outlives())
    .note(
        "the escape brand makes this a type error wherever a type still mentions the brand; this is \
         the boundary where none does, so it is refused here instead of read later",
    )
    .note(boundary.remedy())
}

/// Builds `route` innermost-first as the `Some` unwinds, so a clean value allocates nothing.
fn find(value: &Value, route: &mut Vec<String>) -> Option<Handle> {
    match value {
        Value::Cell(_) => Some(Handle::Cell),
        Value::Task(_) => Some(Handle::Task),

        Value::Int(_)
        | Value::Fixed(_)
        | Value::Bool(_)
        | Value::Float(_)
        | Value::Decimal(_)
        | Value::Str(_)
        | Value::Bytes(_)
        | Value::Unit => None,

        Value::List(items) => grow(|| {
            items.iter().enumerate().find_map(|(i, v)| {
                let handle = find(v, route)?;
                route.push(format!("item {i}"));
                Some(handle)
            })
        }),

        Value::Map(entries) => grow(|| {
            entries.iter().find_map(|(k, v)| {
                if let Some(handle) = find(k, route) {
                    route.push("a key".to_string());
                    return Some(handle);
                }
                let handle = find(v, route)?;
                route.push("a map entry".to_string());
                Some(handle)
            })
        }),

        Value::Record(fields) => grow(|| {
            fields.iter().find_map(|(name, v)| {
                let handle = find(v, route)?;
                route.push(format!("field `{name}`"));
                Some(handle)
            })
        }),

        Value::Ctor { name, args } => grow(|| {
            args.iter().enumerate().find_map(|(i, v)| {
                let handle = find(v, route)?;
                route.push(format!("`{name}`'s argument {}", i + 1));
                Some(handle)
            })
        }),

        // Descended into, because a credential is not a place to hide a handle.
        Value::Secret(inner) => grow(|| {
            let handle = find(inner, route)?;
            route.clear();
            route.push("a `Secret`'s payload".to_string());
            Some(handle)
        }),

        Value::Closure(closure) => grow(|| match &closure.kind {
            ClosureKind::Ctor { .. } | ClosureKind::Builtin(_) => None,
            ClosureKind::Native { captured, .. } => captured.iter().find_map(|v| find(v, route)),
            ClosureKind::Synth { rule, .. } => {
                rule.values().into_iter().find_map(|v| find(v, route))
            }
        }),
    }
}
