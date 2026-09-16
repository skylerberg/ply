//! Where a natively compiled body may be entered in place of evaluating one.

use crate::host::{HostBinding, HostRuntime, HostUse};
use crate::region::Record;
use crate::sim::Seed;
use crate::value::Value;
use ply_span::{Diagnostic, Symbol};
use ply_syntax::ast::Program;
use ply_ty::CheckOutput;
use ply_ty::Footprint;
use ply_ty::{EffectAtom, IntTy, SECRET, TyVar, Type};
use rustc_hash::FxHashMap;
use std::rc::Rc;
use std::sync::Arc;

/// A source of natively compiled bodies for a program's definitions.
pub trait Compiled {
    /// Whether these bodies were compiled from `program`.
    fn describes(&self, program: &Program) -> bool;

    /// Runs `name`'s body over `args`, or declines for any reason at all.
    fn enter(&self, name: &Symbol, args: &[Value], budget: usize) -> Option<Value>;

    /// Runs a test's body whole, through the nullary root the backend synthesized for it, and
    /// says whether the body ran and raised — which `enter` folds into a decline — because a
    /// test the backend fails and the machine passes is a disagreement, not a decline.
    fn enter_test(&self, _name: &Symbol, _budget: usize) -> Entered {
        Entered::Declined
    }

    /// A definition entered whole with its answer, refusal or failure told apart: what an engine
    /// with no machine behind it asks, where [`Compiled::enter`] folds a failure into a decline
    /// for the machine to run again.
    fn enter_whole(&self, _name: &Symbol, _args: &[Value], _budget: usize) -> Entered {
        Entered::Declined
    }

    /// The atoms compiled code performed since the last entry, for the machine's trace. A
    /// handled perform is still a perform, and the observed row is a claim the tests make.
    fn take_performed(&self) -> Vec<EffectAtom> {
        Vec::new()
    }

    /// The seed and step budget the next entry's `simulate` regions run under.
    fn set_seed(&self, _seed: Seed, _steps: u32) {}

    /// What the last entry's regions did, for the search, if it opened any.
    fn simulated(&self) -> Option<Record> {
        None
    }

    /// The host binding a `perform` nothing on the stack answers reaches, and the reactor a
    /// pending answer is waited on.
    fn set_host(&self, _binding: Arc<HostBinding>, _runtime: Option<Rc<dyn HostRuntime>>) {}

    fn set_declared(&self, _declared: Option<Footprint>) {}

    fn set_re_executed(&self, _re_executed: bool) {}

    /// What the entries since the last take asked of the host, and how many linear operations.
    fn take_host_use(&self) -> (HostUse, u64) {
        (HostUse::default(), 0)
    }

    /// What the host runtime said when the entries ended.
    fn take_teardown(&self) -> Vec<Diagnostic> {
        Vec::new()
    }

    /// Whether the backend is to be the only engine: a test or an entry it does not hold fails
    /// rather than falling to the machine.
    fn tier_only(&self) -> bool {
        false
    }
}

/// How a test root's entry ended.
#[derive(Debug)]
pub enum Entered {
    /// The body ran to its answer.
    Answered(Value),
    /// The body ran and raised this.
    Raised(Diagnostic),
    /// The backend did not run the body.
    Declined,
}

/// What may cross this boundary, in either direction: the two unboxed scalars, the two byte
/// carriers and unit — every leaf kind that holds no handle.
pub fn crossable(value: &Value) -> bool {
    matches!(
        value,
        Value::Int(_) | Value::Bool(_) | Value::Bytes(_) | Value::Str(_) | Value::Unit
    )
}

/// Whether `ty` mentions a fixed-width integer anywhere, at any depth.
pub fn mentions_a_width(ty: &Type) -> bool {
    match ty {
        Type::Var(_) => false,
        Type::Con(name, args) => {
            IntTy::from_name(name.as_str()).is_some() || args.iter().any(mentions_a_width)
        }
        Type::Fn { params, ret, .. } => {
            params.iter().any(mentions_a_width) || mentions_a_width(ret)
        }
        Type::Record(fields) => fields.values().any(mentions_a_width),
    }
}

/// Which definitions' **declared parameter types** cannot reach a world handle, decided once per
/// program rather than once per call.
pub struct CarriedTypes {
    /// A declared sum type's own parameters and the field types of every one of its constructors,
    /// by program-wide type name.
    decls: FxHashMap<Symbol, Decl>,
    /// The fixpoint over [`CarriedTypes::decls`]: whether a value of that type can reach a world
    /// handle, its type arguments left to each occurrence.
    safe: FxHashMap<Symbol, bool>,
    /// Per definition, its declared signature read as [`Denotes`].
    sigs: FxHashMap<Symbol, Sig>,
}

/// One definition's declared signature, with every position answered once.
struct Sig {
    /// One entry per declared parameter: the `Value` kind that parameter's type denotes when it is
    /// carried, and `None` when it is not.
    params: Vec<Option<Denotes>>,
    /// The same for the declared return type.
    ret: Option<Denotes>,
}

struct Decl {
    vars: Vec<TyVar>,
    fields: Vec<Type>,
}

/// The one `Value` kind a carried type denotes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Denotes {
    Int,
    Bool,
    Bytes,
    Str,
    Unit,
    List,
    Map,
    Record,
    Ctor,
}

impl Denotes {
    fn matches(self, value: &Value) -> bool {
        match self {
            Denotes::Int => matches!(value, Value::Int(_)),
            Denotes::Bool => matches!(value, Value::Bool(_)),
            Denotes::Bytes => matches!(value, Value::Bytes(_)),
            Denotes::Str => matches!(value, Value::Str(_)),
            Denotes::Unit => matches!(value, Value::Unit),
            Denotes::List => matches!(value, Value::List(_)),
            Denotes::Map => matches!(value, Value::Map(_)),
            Denotes::Record => matches!(value, Value::Record(_)),
            Denotes::Ctor => matches!(value, Value::Ctor { .. }),
        }
    }
}

impl CarriedTypes {
    /// The table for `check`, or an empty one — which admits nothing — for a machine built without
    /// a `CheckOutput`, for the reason [`Gate::PublishedRow`] refuses one: a machine that cannot
    /// read the fact has not been told it holds.
    pub fn over(check: Option<&CheckOutput>) -> CarriedTypes {
        let mut table = CarriedTypes {
            decls: FxHashMap::default(),
            safe: FxHashMap::default(),
            sigs: FxHashMap::default(),
        };
        let Some(check) = check else { return table };
        for ctor in check.ctors.values() {
            let decl = table
                .decls
                .entry(ctor.type_name.clone())
                .or_insert_with(|| Decl {
                    vars: ctor.scheme.ty_vars.clone(),
                    fields: Vec::new(),
                });
            decl.fields.extend(ctor.fields.iter().cloned());
        }
        table.safe = table.decls.keys().map(|n| (n.clone(), true)).collect();
        // Lowering only ever removes, so this settles; the bound is one round per declaration and
        // the loop asserts nothing about how many it took.
        loop {
            let lowered: Vec<Symbol> = table
                .decls
                .iter()
                .filter(|(name, decl)| {
                    table.safe[*name]
                        && !decl
                            .fields
                            .iter()
                            .all(|f| table.carries(f, Some(&decl.vars)))
                })
                .map(|(name, _)| name.clone())
                .collect();
            if lowered.is_empty() {
                break;
            }
            for name in lowered {
                table.safe.insert(name, false);
            }
        }
        let flags: Vec<(Symbol, Sig)> = check
            .defs
            .iter()
            .filter_map(|(name, def)| match &def.scheme.ty {
                Type::Fn { params, ret, .. } => Some((
                    name.clone(),
                    Sig {
                        params: params.iter().map(|t| table.denotes(t)).collect(),
                        ret: table.denotes(ret),
                    },
                )),
                _ => None,
            })
            .collect();
        table.sigs.extend(flags);
        table
    }

    /// The `Value` kind `ty` denotes, when `ty` is carried.
    fn denotes(&self, ty: &Type) -> Option<Denotes> {
        if !self.carries(ty, None) {
            return None;
        }
        match ty {
            Type::Record(_) => Some(Denotes::Record),
            Type::Con(name, _) => Some(match name.as_str() {
                "Int" => Denotes::Int,
                "Bool" => Denotes::Bool,
                "Bytes" => Denotes::Bytes,
                "String" => Denotes::Str,
                "Unit" => Denotes::Unit,
                "List" => Denotes::List,
                "Map" => Denotes::Map,
                // `carries` cleared it and it is none of the builtin heads, so it is a declared sum
                // type and its values are constructors.
                _ => Denotes::Ctor,
            }),
            // `carries` refuses both of these, so this is unreachable rather than conservative — it
            // is spelled out so that a future kind added to `carries` without an entry here is
            // refused rather than silently denoting whatever the arm above it did.
            Type::Var(_) | Type::Fn { .. } => None,
        }
    }

    /// Whether `ty` is carried.
    pub fn carries(&self, ty: &Type, decl_vars: Option<&[TyVar]>) -> bool {
        match ty {
            Type::Var(v) => decl_vars.is_some_and(|vars| vars.contains(v)),
            Type::Fn { .. } => false,
            Type::Record(fields) => fields.values().all(|t| self.carries(t, decl_vars)),
            Type::Con(name, args) => match name.as_str() {
                // The leaf set is `crossable`'s exactly, so it is the same list in both directions.
                "Int" | "Bool" | "Bytes" | "String" | "Unit" => args.is_empty(),
                "List" | "Map" => args.iter().all(|t| self.carries(t, decl_vars)),
                // The fragment has no path for either literal, so a body over them is refused
                // before this table is asked; keeping them out here keeps the leaf set honest.
                "Float" | "Decimal" => false,
                // A world handle and a credential are `Type::Con`s like any other.
                "Cell" | ply_core::prelude::TASK_TYPE | SECRET => false,
                // The fixed-width integer types, explicitly rather than by falling through to the
                // undeclared arm below. Compiled code holds one as a tagged immediate, which is
                // what an `Int` is held as, so a value crossing back would arrive as an `Int` and
                // be a *wrong* answer rather than a slow one. The bodies still compile and still
                // call each other directly (ADR 0039); it is the crossing that is refused, and
                // this arm is what makes `std.hash`'s `compress` unreachable from the differential
                // while `blake3` itself is entered whole.
                n if IntTy::from_name(n).is_some() => false,
                _ => match self.decls.get(name) {
                    Some(decl) => {
                        decl.vars.len() == args.len()
                            && self.safe.get(name).copied().unwrap_or(false)
                            && args.iter().all(|t| self.carries(t, decl_vars))
                    }
                    None => false,
                },
            },
        }
    }

    /// Whether `value` may cross back as `name`'s answer.
    pub fn answer_crosses(&self, name: &Symbol, value: &Value) -> bool {
        self.sigs
            .get(name)
            .and_then(|sig| sig.ret)
            .is_some_and(|d| d.matches(value))
            || crossable(value)
    }

    /// Whether every position of `name`'s declared signature is carried — the registry question,
    /// asked of a definition rather than of a call.
    pub fn signature_carried(&self, name: &Symbol) -> bool {
        self.sigs
            .get(name)
            .is_some_and(|sig| sig.ret.is_some() && sig.params.iter().all(Option::is_some))
    }
}
