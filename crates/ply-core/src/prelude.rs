//! The effects the language declares rather than a module.
//!
//! Concurrency is an effect, so the scheduler is a test double like any other:
//! the signature is written once, and a production handler, a sequential one
//! written in Ply and the seeded one `simulate { .. }` installs are all checked
//! against it. That is what stops the three from drifting.
//!
//! ```text
//! nondet effect task   { write spawn<a | e>(body: () -> a / e) -> Task<a> / e
//!                        write join<a>(t: Task<a>) -> a
//!                        write yield() -> Unit }
//! nondet effect clock  { read  now() -> Int
//!                        write sleep(nanos: Int) -> Unit }
//! nondet effect random { write next() -> Int
//!                        write below(bound: Int) -> Int }
//! effect sim           { read  seed() -> Int }
//! ```
//!
//! Every operation is singleton-resource: there is one scheduler, one clock and
//! one random stream per simulated region, so `[r]` would name a distinction
//! that does not exist.

use crate::ty::{EffectAtom, Resource, Row, RowVar, Scheme, TyVar, Type};
use crate::{CtorInfo, EffectInfo, OpInfo};
use indexmap::IndexMap;
use ply_span::{Span, Symbol};
use ply_syntax::ast::{Mode, ModuleName};
use std::collections::BTreeSet;

pub const TASK: &str = "task";
pub const CLOCK: &str = "clock";
pub const RANDOM: &str = "random";
pub const SIM: &str = "sim";

/// The program-wide names the prelude occupies. A declaration that claims one
/// is `DUPLICATE_DEFINITION`, which only an anonymous module can produce: an
/// `effect clock` in module `clock` is `clock.clock` and shadows the prelude by
/// the ordinary resolution order.
pub const NAMES: &[&str] = &[TASK, CLOCK, RANDOM, SIM];

/// The effects `simulate { .. }` discharges. A user's own `nondet effect http`
/// inside a region still trips `E0412`: the language does not get to claim it
/// simulated an effect it has never heard of.
pub const SIMULATED: &[&str] = &[TASK, CLOCK, RANDOM];

/// The handle `task.spawn` answers with. A key into the region's scheduler, and
/// the scheduler dies with the region, so a `Task` in a region's result type is
/// `E0413`.
pub const TASK_TYPE: &str = "Task";

/// The operation that hands a value to another task. Named here because the
/// region escape check has to recognize it: everything else a region can leak
/// through shows up in a type, and this one shows up in an argument.
pub const SPAWN_OP: &str = "spawn";

pub fn is_prelude_effect(name: &Symbol) -> bool {
    NAMES.contains(&name.as_str())
}

/// An ADT the language declares rather than a module.
///
/// `map_get` returns an `Option`, `decode` a `Result`, `compare` an `Ordering`
/// and `decimal_div` takes a `Rounding`. A **builtin** whose type mentions a
/// type the user has to import first is incoherent, so these are in scope
/// everywhere and a user declaration of one is `E0105`.
pub struct Adt {
    pub name: &'static str,
    pub params: &'static [&'static str],
    /// `(constructor, the parameters its fields have)`. Every prelude field is
    /// a bare parameter, so this table needs no notion of a type expression and
    /// no declaration order to resolve one against.
    pub variants: &'static [(&'static str, &'static [&'static str])],
}

/// Read top to bottom, like `ply-host`'s registry: the whole of what the
/// language declares without a file. Order is the declaration order, and a
/// constructor's position in it is its `index`, so appending is free and
/// reordering is not.
pub const ADTS: &[Adt] = &[
    Adt {
        name: "Option",
        params: &["a"],
        variants: &[("None", &[]), ("Some", &["a"])],
    },
    Adt {
        name: "Result",
        params: &["a", "e"],
        variants: &[("Ok", &["a"]), ("Err", &["e"])],
    },
    Adt {
        name: "Ordering",
        params: &[],
        variants: &[("Less", &[]), ("Equal", &[]), ("Greater", &[])],
    },
    // The six `decimal_div` and `decimal_round` take. Naming the mode is the
    // whole point of refusing `/`: a rounding the caller did not write down is
    // the defect `Decimal` exists to prevent.
    Adt {
        name: "Rounding",
        params: &[],
        variants: &[
            ("HalfEven", &[]),
            ("HalfUp", &[]),
            ("Down", &[]),
            ("Up", &[]),
            ("Ceiling", &[]),
            ("Floor", &[]),
        ],
    },
];

/// The prelude's constructors, keyed by program-wide name exactly as a module's
/// are.
///
/// The quantified variables are fixed numbers rather than drawn from a run's
/// counter, which is sound because every use goes through `instantiate` — the
/// same reasoning [`effects`] is built on, and the reason both can be rebuilt by
/// a caller that has no checker.
pub fn ctors() -> IndexMap<Symbol, CtorInfo> {
    let mut out = IndexMap::new();
    for adt in ADTS {
        let vars: Vec<TyVar> = (0..adt.params.len()).map(|i| TyVar(i as u32)).collect();
        let result = Type::Con(
            Symbol::new(adt.name),
            vars.iter().map(|v| Type::Var(*v)).collect(),
        );
        for (index, (ctor, params)) in adt.variants.iter().enumerate() {
            let fields: Vec<Type> = params
                .iter()
                .map(|p| {
                    let slot = adt
                        .params
                        .iter()
                        .position(|q| q == p)
                        .expect("a variant's field names one of its type's parameters");
                    Type::Var(vars[slot])
                })
                .collect();
            let ty = if fields.is_empty() {
                result.clone()
            } else {
                Type::Fn {
                    params: fields.clone(),
                    ret: Box::new(result.clone()),
                    effects: Row::empty(),
                }
            };
            let name = Symbol::new(*ctor);
            out.insert(
                name.clone(),
                CtorInfo {
                    name: name.clone(),
                    // No module declares these, and the anonymous name qualifies
                    // to itself, so the program-wide name is the written one.
                    module: ModuleName::anonymous(),
                    simple_name: name,
                    type_name: Symbol::new(adt.name),
                    index,
                    arity: fields.len(),
                    fields,
                    scheme: Scheme {
                        ty_vars: vars.clone(),
                        row_vars: vec![],
                        ty,
                    },
                    span: Span::DUMMY,
                },
            );
        }
    }
    out
}

/// Every prelude constructor and its arity, in declaration order. What the
/// evaluator needs to build a `Value::Ctor` without a `type` item to read.
pub fn ctor_arities() -> Vec<(Symbol, usize)> {
    ADTS.iter()
        .flat_map(|adt| adt.variants)
        .map(|(name, fields)| (Symbol::new(*name), fields.len()))
        .collect()
}

/// `sim.read`: the seed dependency, in the type. `sim` is deliberately not
/// `nondet` — a seed is an input rather than a nondeterminism — which is the
/// whole type-level content of a simulated test being `det` and cacheable.
pub fn seed_atom() -> EffectAtom {
    EffectAtom::new(SIM, Resource::Singleton, Mode::Read)
}

/// The atoms a `simulate` region removes from its body's row, derived from the
/// declarations above so the two cannot disagree.
pub fn simulated_atoms() -> BTreeSet<EffectAtom> {
    let mut out = BTreeSet::new();
    for effect in effects().values() {
        if !SIMULATED.contains(&effect.name.as_str()) {
            continue;
        }
        for op in effect.ops.values() {
            out.insert(EffectAtom::new(
                effect.name.clone(),
                Resource::Singleton,
                op.mode,
            ));
        }
    }
    out
}

pub fn is_simulated(atom: &EffectAtom) -> bool {
    SIMULATED.contains(&atom.effect.as_str())
}

pub fn task_type(elem: Type) -> Type {
    Type::Con(Symbol::new(TASK_TYPE), vec![elem])
}

/// Whether a type mentions a `Task` anywhere, which is what the region's
/// result-type check asks.
pub fn mentions_task(t: &Type) -> bool {
    match t {
        Type::Con(name, args) => name.as_str() == TASK_TYPE || args.iter().any(mentions_task),
        Type::Fn { params, ret, .. } => params.iter().any(mentions_task) || mentions_task(ret),
        Type::Record(fields) => fields.values().any(mentions_task),
        Type::Var(_) => false,
    }
}

/// Keyed by program-wide name, exactly as a declared effect is. The quantified
/// variables are fixed numbers rather than drawn from a run's counter, which is
/// sound because every use goes through `instantiate`.
pub fn effects() -> IndexMap<Symbol, EffectInfo> {
    let a = TyVar(0);
    let e = RowVar(0);
    let ta = Type::Var(a);
    // `spawn`'s row carries `e`. Without it a test that spawns a task writing
    // `db.write[orders]` would report an empty footprint, and the cross-test
    // conflict graph would run it beside a test reading `orders`.
    let body = Type::Fn {
        params: vec![],
        ret: Box::new(ta.clone()),
        effects: Row::open(e),
    };

    let mut out = IndexMap::new();
    for effect in [
        declare(
            TASK,
            true,
            vec![
                op(
                    "spawn",
                    Mode::Write,
                    vec![a],
                    vec![e],
                    vec![body],
                    task_type(ta.clone()),
                    Row::open(e),
                ),
                op(
                    "join",
                    Mode::Write,
                    vec![a],
                    vec![],
                    vec![task_type(ta.clone())],
                    ta,
                    Row::empty(),
                ),
                op(
                    "yield",
                    Mode::Write,
                    vec![],
                    vec![],
                    vec![],
                    Type::unit(),
                    Row::empty(),
                ),
            ],
        ),
        declare(
            CLOCK,
            true,
            vec![
                // `now` observes virtual time; it does not move it. `sleep`
                // changes when this task is next runnable, which changes what
                // `now` answers elsewhere.
                op(
                    "now",
                    Mode::Read,
                    vec![],
                    vec![],
                    vec![],
                    Type::int(),
                    Row::empty(),
                ),
                op(
                    "sleep",
                    Mode::Write,
                    vec![],
                    vec![],
                    vec![Type::int()],
                    Type::unit(),
                    Row::empty(),
                ),
            ],
        ),
        declare(
            RANDOM,
            true,
            vec![
                // Both are writes: drawing advances the stream, so two tasks
                // drawing in the other order get the other values. Declaring a
                // draw a read would hide a whole class of order dependence.
                op(
                    "next",
                    Mode::Write,
                    vec![],
                    vec![],
                    vec![],
                    Type::int(),
                    Row::empty(),
                ),
                op(
                    "below",
                    Mode::Write,
                    vec![],
                    vec![],
                    vec![Type::int()],
                    Type::int(),
                    Row::empty(),
                ),
            ],
        ),
        declare(
            SIM,
            false,
            vec![op(
                "seed",
                Mode::Read,
                vec![],
                vec![],
                vec![],
                Type::int(),
                Row::empty(),
            )],
        ),
    ] {
        out.insert(effect.name.clone(), effect);
    }
    out
}

fn declare(name: &str, nondet: bool, ops: Vec<OpInfo>) -> EffectInfo {
    let name = Symbol::new(name);
    EffectInfo {
        name: name.clone(),
        // No module declares these, and the anonymous name qualifies to
        // itself, so the program-wide name and the written one coincide.
        module: ModuleName::anonymous(),
        simple_name: name,
        nondet,
        ops: ops.into_iter().map(|o| (o.name.clone(), o)).collect(),
        span: Span::DUMMY,
    }
}

fn op(
    name: &str,
    mode: Mode,
    ty_vars: Vec<TyVar>,
    row_vars: Vec<RowVar>,
    params: Vec<Type>,
    ret: Type,
    row: Row,
) -> OpInfo {
    OpInfo {
        name: Symbol::new(name),
        mode,
        resource_param: false,
        params: params.clone(),
        ret: ret.clone(),
        span: Span::DUMMY,
        scheme: Some(Scheme {
            ty_vars,
            row_vars,
            ty: Type::Fn {
                params,
                ret: Box::new(ret),
                effects: row,
            },
        }),
    }
}
