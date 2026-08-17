//! Escape enforcement at the boundaries the brand cannot see.
//!
//! ADR 0017 §2 makes escape a type error, and it is one: `brand_in` runs over
//! the **resolved** type, a function type's effect row included, so a closure
//! that captured a branded value is caught wherever a type still mentions the
//! brand. This module is about the places where no type does.
//!
//! Before regions, an escape here was harmless: the forkable world was monotone,
//! a `CellId` never dangled, and the worst outcome was a read of a cell some
//! other run allocated. Once a region's memory is genuinely reclaimed the same
//! escape is a read of a slot that has been handed to something else, so each of
//! these boundaries needs an answer, and the answer has to be one of three
//! things rather than silence.
//!
//! # The boundaries, and what each one's answer is
//!
//! **Closed here, by a runtime check.**
//!
//! - *A host handler*, which outlives every region (ADR 0008). Checked on the
//!   arguments, by [`Boundary::HostArgument`].
//! - *A value a host handler or runtime answers with*, which is the same
//!   boundary crossed the other way and the only route by which a handle the
//!   program never made can enter it. Checked by [`Boundary::HostAnswer`] on the
//!   inline and `block_on` routes and by [`Boundary::HostToken`] where the
//!   scheduler resolves a token a parked task is waiting on.
//! - *A value handed to an entry point from outside the program.* Checked by
//!   [`Boundary::EntryPoint`], and this one got sharper rather than softer under
//!   regions. `world_isolation_audit.rs` recorded the route as half-closed: a
//!   `CellId` the new world did not hold was named, and one it happened to hold
//!   was read. [`TaskRegions::reset`](crate::TaskRegions::reset) restores the
//!   fixture's values **and their generations**, deliberately and correctly, so
//!   a [`Slot`](crate::arena::Slot) carried out of one entry point *resolves* in
//!   the next one and reads the fixture. The half that used to be caught is no
//!   longer caught by anything downstream, and this check is what stands there.
//!
//! **Closed elsewhere, and tested from here.**
//!
//! - *A trace span attribute or a log line* (W5). `std.trace` is served by a
//!   host handler, so a value reaching a span attribute crosses
//!   [`Boundary::HostArgument`] and nothing further is needed. What a sink then
//!   writes is text: `Value::write` renders a handle as an opaque `<cell @3.0>`
//!   and never dereferences one.
//! - *A cached result.* `memo.rs` refuses to remember a value that is not
//!   independent of the run that produced it, and a `Cell`, a `Task` and a
//!   `Continuation` are each disqualifying there.
//! - *The content-addressed store and a failure artifact.* `ply-store` does not
//!   depend on `ply-eval` and holds no `Value`: an outcome is a `String` and a
//!   `Diagnostic`. The route is a rendered handle, which is opaque.
//! - *An M8 counterexample or a shrunk witness.* `E0418` refuses a `forall`
//!   binder whose type is not one the generator can inhabit, and `ungeneratable`
//!   walks a user type's declared fields — so a record or a variant holding a
//!   `Cell` is refused where the law is written.
//! - *A bisection hybrid* (M5). What crosses between two executions of a mixture
//!   is `ply_test::hybrid::Signature` — a code and a message — and never a
//!   `Value`.
//! - *A value crossing into a task.* ADR 0017 §2 excludes `task.spawn` from a
//!   bare `with_cell`'s escape rule on purpose, because a cell reaching a task
//!   is how tasks share memory, and §3 makes that safe rather than tolerated: a
//!   `task` operation anywhere in a region infers [`RegionKind::Shared`](crate::RegionKind)
//!   ([`Cause::Task`], [`Cause::Simulate`]), and a `shared` region's slots
//!   outlive its close. A runtime refusal here would refuse the landed shape.
//!   For `with_region` the stricter rule is static, and is `E0446`.
//!
//! **Open, and named.**
//!
//! - *A continuation parked in an enclosing region's cell*, where the brand is
//!   erased at a nominal constructor's field type. ADR 0017 §2 records this as
//!   the one route that stays open and it stays open: closing it needs the brand
//!   to survive a nominal declaration. What this module does about it is make
//!   its *consequences* checkable — the continuation cannot then leave through
//!   an entry point or a host operation, and a slot it reads after its region
//!   closed is a diagnostic rather than a wrong value.
//! - *A simulation record replayed from a seed* (M7). A `Trail` records
//!   `Access::Cell { id }` and compares ids across interleavings to decide
//!   dependence. Under the monotone world an id was never reused, so the
//!   comparison was exact. Under an arena a slot index **is** reused after a
//!   region closes, and the comparison stays exact only if what is recorded
//!   carries the generation — a whole [`Slot`](crate::arena::Slot) rather than
//!   its index. That is a requirement on the wiring rather than something this
//!   module can enforce, and it is written down here so it is not discovered
//!   later as a reduction that reports more than it explored.
//!
//! # What the walk is, and what it deliberately over-approximates
//!
//! [`carries`] finds the first region-bound handle a value can reach: a `Cell`
//! or a `Task`, which are keys into a region's store and into a scheduler that
//! dies with its region, and a `Continuation`, which reaches every region open
//! where it was captured. It descends through every data constructor, a
//! `Secret`'s payload included — a credential is not a laundering wrapper — and
//! through a closure's captured environment.
//!
//! The environment walk is the over-approximation, and it is deliberate: an
//! [`Env`](crate::Env) is the closure's whole defining scope rather than its
//! free set, so a closure built beside a cell carries that cell in its chain
//! whether or not its body can reach it. Nothing in the shipped trusted
//! computing base declares an operation with a function parameter, so no
//! program's meaning moves for it today.
//!
//! It is a backstop rather than the primary defence, and it is worth being exact
//! about which: the shape it looks like it is for — `Wrap(|| cell_get(c))`,
//! smuggling a cell through the same constructor erasure §2's open route uses —
//! does not compile. A field type is declared once for the whole program, so
//! `Wrap`'s is `() -> Int` with an empty row and the closure's `{cell.read[log]}`
//! has nothing to unify with at the constructor's *application*: `E0302`, before
//! any region check runs. §2's route is open specifically for a **continuation**,
//! because `ρ_κ` is a variable the `handle` solves. So the walk defends no route
//! that is reachable today, and it is kept because six of this project's found
//! defects were routes nobody had enumerated.
//! `the_constructor_erasure_does_not_also_launder_a_cell_inside_a_closure` in
//! `region_boundary_audit.rs` is where that is pinned, so a change that made the
//! shape compile would be caught rather than quietly widening the boundary.
//!
//! The walk terminates without a depth bound because a `Value` graph is acyclic:
//! a `Cell` is a slot and not a pointer, so the only cycle a program can build
//! goes through the arena, and the walk stops at the `Cell` rather than
//! following it. `grow` is what keeps a deep value from overflowing the host
//! stack, exactly as it does for the credential walk beside it.
//!
//! [`Cause::Task`]: crate::region_kind::Cause::Task
//! [`Cause::Simulate`]: crate::region_kind::Cause::Simulate

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
    /// A captured continuation, which reaches every region that was open where
    /// it was captured.
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
    /// Outermost first, e.g. `["item 2", "`Just`'s argument 1"]`. Empty when the
    /// value *is* the handle.
    pub route: Vec<String>,
}

impl Escapee {
    /// `, reached through item 2 → `Just`'s argument 1`, or the empty string
    /// when the value is the handle itself.
    fn reached(&self) -> String {
        if self.route.is_empty() {
            return String::new();
        }
        format!(", reached through {}", self.route.join(" → "))
    }
}

/// Which boundary a value was about to cross.
///
/// Borrowed rather than owned so that constructing one costs nothing: it is
/// built per host answer and per entry point, and a `String` there would put an
/// allocation on the request path this milestone exists to take them off.
#[derive(Clone, Copy, Debug)]
pub enum Boundary<'a> {
    /// An argument to a host operation. Positions are 0-based; the diagnostic
    /// prints them 1-based, as `E0439` does.
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
    /// The value a host runtime resolved a parked token to. A separate variant
    /// because the scheduler knows the token and not the registration: the task
    /// parked, and which handler minted the token is no longer on the path.
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

    /// What outlives the region, said once per boundary. This is the sentence
    /// the reader needs and the one a generic message would not have.
    fn outlives(&self) -> Cow<'static, str> {
        match self {
            Boundary::HostArgument { path, .. } => Cow::Owned(format!(
                "`{path}` is a host handler, and a host handler outlives every region the program \
                 opens (ADR 0008)"
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
///
/// Structural order — a list by index, a map by ascending key, a record by field
/// name — so two engines walking the same value name the same handle and print
/// the same diagnostic.
pub fn carries(value: &Value) -> Option<Escapee> {
    let mut route = Vec::new();
    let handle = find(value, &mut route)?;
    route.reverse();
    Some(Escapee { handle, route })
}

/// Refuses `value` at `boundary`, or lets it through.
///
/// The handle's *kind* and its route are named and its contents are not, which
/// is `E0439`'s discipline and holds here for the same reason: the route is
/// enough to find the value in the source, and the value may be a credential.
pub fn check(boundary: &Boundary<'_>, value: &Value, span: Span) -> Result<(), Diagnostic> {
    match carries(value) {
        None => Ok(()),
        Some(escapee) => Err(refuse(boundary, &escapee, span)),
    }
}

/// [`check`] over a run of arguments, naming the position of the first that
/// carries a handle.
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
        "ADR 0017 §2 makes this a type error wherever a type still mentions the brand; this is \
         the boundary where none does, so it is refused here instead of read later",
    )
    .note(boundary.remedy())
}

/// Innermost-first: each frame pushes its own segment as the `Some` unwinds, so
/// nothing is allocated for a value that carries no handle.
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
        // The route stops here rather than naming what is inside: everything
        // below a `Secret` is redacted, and a field name is part of the value's
        // shape.
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

    /// A real slot from a real region, because [`Slot`](crate::arena::Slot)
    /// carries a generation and has no constructor outside the allocator — which
    /// is the property that makes a stale one readable as stale.
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

    /// The erasure route ADR 0017 §2 leaves open, seen from the boundary: the
    /// constructor's field type mentions no brand, so only the value says so.
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

    /// A `Secret` is walked, because a wrapper nothing descends into is a
    /// wrapper a handle hides behind — and the route stops at it, because what
    /// is inside one is redacted everywhere else.
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

    /// The backstop the module doc describes, exercised where it can be: no
    /// source program builds this closure today (`E0302` refuses the shape at
    /// the constructor), so the environment is assembled directly.
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
