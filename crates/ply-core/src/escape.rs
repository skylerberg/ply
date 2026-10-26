//! Escape enforcement at the boundaries the brand cannot see.

use crate::limit::grow;
use crate::value::{ClosureKind, Value};
use crate::{Diagnostic, Span, codes};

/// What may not cross a boundary: a continuation, which no type records as one, and a task or a
/// channel, which only the region that made it can hold.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Handle {
    Continuation,
    Task,
    Channel,
}

impl Handle {
    /// A continuation has no type of its own to name: its type is a function's.
    fn noun(self) -> &'static str {
        match self {
            Handle::Continuation => "a continuation",
            Handle::Task => "a `Task`",
            Handle::Channel => "a `Chan`",
        }
    }

    fn why(self) -> &'static str {
        match self {
            Handle::Continuation => {
                "a continuation resumes a body that lives only as long as the entry that captured \
                 it"
            }
            Handle::Task | Handle::Channel => {
                "a task or a channel is a key into the region that made it, which ends before the \
                 entry does"
            }
        }
    }

    /// Why no type refused it before the boundary did.
    fn unseen(self) -> &'static str {
        match self {
            Handle::Continuation => {
                "no type records that a function is a continuation, so it is refused at the \
                 boundary instead of wherever it would be resumed"
            }
            Handle::Task | Handle::Channel => {
                "a value may carry it out of its region, which no check stops, so it is refused \
                 at the boundary instead of wherever it would be joined"
            }
        }
    }

    /// What the program does instead, at an entry's answer.
    fn instead(self) -> &'static str {
        match self {
            Handle::Continuation => {
                "resume the continuation before the entry answers, and answer with what it \
                 answers"
            }
            Handle::Task | Handle::Channel => {
                "join the task, or drain the channel, inside the region that made it, and answer \
                 with what that gives"
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
    EntryAnswer { name: &'a str },
}

impl Boundary<'_> {
    fn headline(&self, handle: Handle, reached: &str) -> String {
        let what = handle.noun();
        match self {
            Boundary::EntryAnswer { name } => format!("`{name}` answered {what}{reached}"),
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Boundary::EntryAnswer { .. } => "answered here",
        }
    }

    fn outlives(&self) -> &'static str {
        match self {
            Boundary::EntryAnswer { .. } => {
                "an entry point's answer goes to its caller, which keeps it after the entry has \
                 ended and closed every region it opened"
            }
        }
    }

    fn remedy(&self, handle: Handle) -> &'static str {
        match self {
            Boundary::EntryAnswer { .. } => handle.instead(),
        }
    }
}

/// The first continuation `value` can reach, with the route to it.
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

/// `handle`, which no value stands for, refused at `boundary`.
pub fn refused(boundary: &Boundary<'_>, handle: Handle, span: Span) -> Diagnostic {
    let escapee = Escapee {
        handle,
        route: Vec::new(),
    };
    refuse(boundary, &escapee, span)
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
    .note(escapee.handle.unseen())
    .note(boundary.remedy(escapee.handle))
}

/// Builds `route` innermost-first as the `Some` unwinds, so a clean value allocates nothing.
fn find(value: &Value, route: &mut Vec<String>) -> Option<Handle> {
    match value {
        Value::Int(_)
        | Value::Fixed(_)
        | Value::Char(_)
        | Value::Bool(_)
        | Value::Float(_)
        | Value::Decimal(_)
        | Value::Str(_)
        | Value::Bytes(_)
        | Value::Secret(_)
        | Value::Unit => None,

        Value::List(items) => grow(|| {
            items.iter().enumerate().find_map(|(i, v)| {
                let handle = find(v, route)?;
                route.push(format!("item {i}"));
                Some(handle)
            })
        }),

        Value::Array(items) => grow(|| {
            items.iter().enumerate().find_map(|(i, v)| {
                let handle = find(v, route)?;
                route.push(format!("element {i}"));
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

        Value::Closure(closure) => grow(|| match &closure.kind {
            ClosureKind::Continuation { .. } => Some(Handle::Continuation),
            ClosureKind::Synth { rule, .. } => {
                rule.values().into_iter().find_map(|v| find(v, route))
            }
            ClosureKind::Hold { held, .. } => held.as_ref().and_then(|v| find(v, route)),
        }),
    }
}
