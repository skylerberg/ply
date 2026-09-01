//! Escape enforcement at the boundaries the brand cannot see.

use crate::limit::grow;
use crate::value::{ClosureKind, Value};
use ply_span::{Diagnostic, Span, codes};
use std::borrow::Cow;

/// What kind of region-bound handle was found.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Handle {
    /// A key into the region that allocated it.
    Cell,
    /// A key into a scheduler, which dies with its region.
    Task,
    /// A captured continuation, which reaches every region that was open where it was captured.
    Continuation,
}

impl Handle {
    pub fn as_str(self) -> &'static str {
        match self {
            Handle::Cell => "Cell",
            Handle::Task => "Task",
            Handle::Continuation => "continuation",
        }
    }

    /// Why this handle is bound to a region, in the diagnostic's own voice.
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
            Handle::Continuation => {
                "a continuation holds the frames and scopes it was captured over, so it reaches \
                 every region that was open at the capture"
            }
        }
    }
}

/// A handle a value can reach, and the route to it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Escapee {
    pub handle: Handle,
    /// Outermost first, e.g. `["item 2", "`Just`'s argument 1"]`.
    pub route: Vec<String>,
}

impl Escapee {
    /// `, reached through item 2 → `Just`'s argument 1`, or the empty string when the value is the
    /// handle itself.
    fn reached(&self) -> String {
        if self.route.is_empty() {
            return String::new();
        }
        format!(", reached through {}", self.route.join(" → "))
    }
}

/// Which boundary a value was about to cross.
#[derive(Clone, Copy, Debug)]
pub enum Boundary<'a> {
    /// An argument to a host operation.
    HostArgument {
        operation: &'a str,
        path: &'static str,
        position: usize,
    },
    /// The value a host handler answered with, inline or through
    /// [`HostRuntime::block_on`](crate::HostRuntime::block_on).
    HostAnswer {
        operation: &'a str,
        path: &'static str,
    },
    /// The value a host runtime resolved a parked token to.
    HostToken { label: &'static str, token: u64 },
    /// An argument handed to an entry point from outside the program.
    EntryPoint { name: &'a str },
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

    /// What outlives the region, said once per boundary.
    fn outlives(&self) -> Cow<'static, str> {
        match self {
            Boundary::HostArgument { path, .. } => Cow::Owned(format!(
                "`{path}` is a host handler, and a host handler outlives every region the program \
                 opens (the host boundary)"
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

/// Refuses `value` at `boundary`, or lets it through.
pub fn check(boundary: &Boundary<'_>, value: &Value, span: Span) -> Result<(), Diagnostic> {
    match carries(value) {
        None => Ok(()),
        Some(escapee) => Err(refuse(boundary, &escapee, span)),
    }
}

/// [`check`] over a run of arguments, naming the position of the first that carries a handle.
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

/// Innermost-first: each frame pushes its own segment as the `Some` unwinds, so nothing is
/// allocated for a value that carries no handle.
fn find(value: &Value, route: &mut Vec<String>) -> Option<Handle> {
    match value {
        Value::Cell(_) => Some(Handle::Cell),
        Value::Task(_) => Some(Handle::Task),
        Value::Continuation(_) => Some(Handle::Continuation),

        Value::Int(_)
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
            ClosureKind::Fn { env, .. } | ClosureKind::Code { env, .. } => {
                env.bindings().find_map(|(bound, v)| {
                    let handle = find(v, route)?;
                    let what = match &closure.name {
                        Some(n) => format!("`{bound}`, captured by `{n}`"),
                        None => format!("`{bound}`, captured by a closure"),
                    };
                    route.push(what);
                    Some(handle)
                })
            }
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::{Arena, RegionKind};
    use ply_span::Symbol;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    /// A real slot from a real region, because [`Slot`](crate::arena::Slot) carries a generation
    /// and has no constructor outside the allocator — which is the property that makes a stale one
    /// readable as stale.
    fn cell() -> Value {
        let mut arena = Arena::new();
        arena.open(RegionKind::Shared, Span::DUMMY);
        Value::Cell(arena.alloc(Value::Int(0)).expect("the region is open"))
    }

    #[test]
    fn a_bare_handle_is_found_with_an_empty_route() {
        let found = carries(&cell()).expect("a cell is a handle");
        assert_eq!(found.handle, Handle::Cell);
        assert!(found.route.is_empty());
        assert_eq!(found.reached(), "");
    }

    #[test]
    fn data_without_a_handle_crosses() {
        let value = Value::list(vec![
            Value::Int(1),
            Value::str("two"),
            Value::Map(Default::default()),
        ]);
        assert_eq!(carries(&value), None);
    }

    /// The erasure route the escape brand leaves open, seen from the boundary: the constructor's field
    /// type mentions no brand, so only the value says so.
    #[test]
    fn a_handle_inside_a_constructor_is_found_and_the_route_names_it() {
        let value = Value::Ctor {
            name: Symbol::new("m.Just"),
            args: Arc::new(vec![cell()]),
        };
        let found = carries(&value).expect("the constructor carries it");
        assert_eq!(found.handle, Handle::Cell);
        assert_eq!(found.route, vec!["`m.Just`'s argument 1"]);
    }

    #[test]
    fn the_route_reads_outermost_first() {
        let inner = Value::Ctor {
            name: Symbol::new("m.Just"),
            args: Arc::new(vec![cell()]),
        };
        let mut fields = BTreeMap::new();
        fields.insert(Symbol::new("saved"), inner);
        let value = Value::list(vec![Value::Int(0), Value::Record(Arc::new(fields))]);

        let found = carries(&value).expect("the record carries it");
        assert_eq!(
            found.route,
            vec!["item 1", "field `saved`", "`m.Just`'s argument 1"]
        );
        assert_eq!(
            found.reached(),
            ", reached through item 1 → field `saved` → `m.Just`'s argument 1"
        );
    }

    /// A `Secret` is walked, because a wrapper nothing descends into is a wrapper a handle hides
    /// behind — and the route stops at it, because what is inside one is redacted everywhere else.
    #[test]
    fn a_secret_is_not_a_place_to_hide_a_handle_and_its_shape_stays_redacted() {
        let value = Value::Secret(Arc::new(Value::Ctor {
            name: Symbol::new("m.Just"),
            args: Arc::new(vec![cell()]),
        }));
        let found = carries(&value).expect("the payload carries it");
        assert_eq!(found.handle, Handle::Cell);
        assert_eq!(found.route, vec!["a `Secret`'s payload"]);
        assert!(
            !found.reached().contains("Just"),
            "the payload's shape stays redacted: {}",
            found.reached()
        );
    }

    #[test]
    fn a_task_and_a_continuation_are_handles_too() {
        assert_eq!(
            carries(&Value::Task(crate::sim::TaskId(0)))
                .expect("a task is a handle")
                .handle,
            Handle::Task
        );
    }

    #[test]
    fn a_builtin_closure_carries_nothing() {
        let value = Value::builtin(crate::Builtin::CellGet);
        assert_eq!(carries(&value), None);
    }

    /// The backstop the module doc describes, exercised where it can be: no source program builds
    /// this closure today (`E0302` refuses the shape at the constructor), so the environment is
    /// assembled directly.
    #[test]
    fn a_closure_whose_scope_reaches_a_handle_is_found_and_the_binding_named() {
        use crate::env::Env;
        use crate::value::Closure;
        use ply_syntax::ast::{Expr, ExprKind, Lit};

        let env = Env::empty()
            .bind(Symbol::new("c"), cell())
            .bind(Symbol::new("n"), Value::Int(1));
        let closure = Value::Closure(Arc::new(Closure {
            name: Some(Symbol::new("m.later")),
            kind: ClosureKind::Fn {
                params: Vec::new(),
                body: Arc::new(Expr {
                    kind: ExprKind::Lit(Lit::Int(0)),
                    span: Span::DUMMY,
                }),
                env,
                module: 0,
            },
        }));

        let found = carries(&closure).expect("the scope reaches the cell");
        assert_eq!(found.handle, Handle::Cell);
        assert_eq!(found.route, vec!["`c`, captured by `m.later`"]);
    }

    #[test]
    fn the_diagnostic_names_the_boundary_the_handle_and_the_route_and_no_value() {
        let value = Value::Ctor {
            name: Symbol::new("m.Just"),
            args: Arc::new(vec![cell()]),
        };
        let boundary = Boundary::HostArgument {
            operation: "db.query[users]",
            path: "ply_host::db",
            position: 1,
        };
        let d = check(&boundary, &value, Span::DUMMY).expect_err("it is refused");

        assert_eq!(d.code, codes::REGION_ESCAPE_AT_BOUNDARY);
        assert!(d.message.contains("argument 2"), "{}", d.message);
        assert!(d.message.contains("`Cell`"), "{}", d.message);
        assert!(d.message.contains("`m.Just`'s argument 1"), "{}", d.message);
        assert!(
            d.notes.iter().any(|n| n.contains("ply_host::db")),
            "the handler is named: {:#?}",
            d.notes
        );
        assert!(
            !d.message.contains('@'),
            "the slot's identity is not printed: {}",
            d.message
        );
    }

    #[test]
    fn check_arguments_names_the_first_position_that_carries_one() {
        let args = vec![Value::Int(1), Value::Int(2), cell(), cell()];
        let d = check_arguments("net.send[s]", "ply_host::tcp", &args, Span::DUMMY)
            .expect_err("argument 3 carries a cell");
        assert!(d.message.contains("argument 3"), "{}", d.message);
    }

    #[test]
    fn an_answer_and_an_entry_point_each_say_what_outlives_the_region() {
        let answer = check(
            &Boundary::HostAnswer {
                operation: "net.recv[s]",
                path: "ply_host::tcp",
            },
            &cell(),
            Span::DUMMY,
        )
        .expect_err("a forged handle is refused");
        assert!(
            answer
                .notes
                .iter()
                .any(|n| n.contains("outside the program")),
            "{:#?}",
            answer.notes
        );

        let entry = check(
            &Boundary::EntryPoint {
                name: "m.resume_it",
            },
            &cell(),
            Span::DUMMY,
        )
        .expect_err("a smuggled handle is refused");
        assert!(
            entry
                .notes
                .iter()
                .any(|n| n.contains("resets its region stack to the fixture")),
            "{:#?}",
            entry.notes
        );
    }
}
