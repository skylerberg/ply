//! Where a natively compiled body may be entered in place of evaluating one.

use crate::host::{HostBinding, HostRuntime, HostUse};
use crate::region::Record;
use crate::sim::Seed;
use crate::value::Value;
use ply_span::{Diagnostic, Symbol};
use ply_ty::CheckOutput;
use ply_ty::Footprint;
use ply_ty::{DefHash, EffectAtom, IntTy, SECRET, TyVar, Type};
use rustc_hash::FxHashMap;
use std::rc::Rc;
use std::sync::Arc;

pub trait Compiled {
    /// Whether this was built over the program [`ply_ty::HashOutput::digest`] names.
    fn describes(&self, program: DefHash) -> bool;

    /// Runs `name`'s body over `args`, or declines for any reason at all.
    fn enter(&self, name: &Symbol, args: &[Value], budget: usize) -> Option<Value>;

    /// Runs a test whole, telling a raise from a decline, which [`Compiled::enter`] conflates.
    fn enter_test(&self, _name: &Symbol, _budget: usize) -> Entered {
        Entered::Declined
    }

    /// A definition entered whole, for an engine with no machine behind it to fall back to.
    fn enter_whole(&self, _name: &Symbol, _args: &[Value], _budget: usize) -> Entered {
        Entered::Declined
    }

    /// Atoms performed since the last entry, handled ones included.
    fn take_performed(&self) -> Vec<EffectAtom> {
        Vec::new()
    }

    fn set_seed(&self, _seed: Seed, _steps: u32) {}

    fn simulated(&self) -> Option<Record> {
        None
    }

    fn set_host(&self, _binding: Arc<HostBinding>, _runtime: Option<Rc<dyn HostRuntime>>) {}

    fn set_declared(&self, _declared: Option<Footprint>) {}

    fn set_re_executed(&self, _re_executed: bool) {}

    /// What the entries since the last take asked of the host, and how many linear operations.
    fn take_host_use(&self) -> (HostUse, u64) {
        (HostUse::default(), 0)
    }

    fn take_teardown(&self) -> Vec<Diagnostic> {
        Vec::new()
    }

    /// A test or entry this backend does not hold fails rather than falling to the machine.
    fn tier_only(&self) -> bool {
        false
    }
}

#[derive(Debug)]
pub enum Entered {
    Answered(Value),
    Raised(Diagnostic),
    Declined,
}

/// The leaf kinds that hold no handle, which may cross in either direction.
pub fn crossable(value: &Value) -> bool {
    matches!(
        value,
        Value::Int(_) | Value::Bool(_) | Value::Bytes(_) | Value::Str(_) | Value::Unit
    )
}

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

/// Which definitions' declared types cannot reach a world handle, decided once per program.
pub struct CarriedTypes {
    decls: FxHashMap<Symbol, Decl>,
    /// Fixpoint over `decls`: `true` when the type, arguments aside, cannot reach a world handle.
    safe: FxHashMap<Symbol, bool>,
    sigs: FxHashMap<Symbol, Sig>,
}

struct Sig {
    /// Per declared parameter: the `Value` kind it denotes, or `None` when not carried.
    params: Vec<Option<Denotes>>,
    /// The same for the declared return type.
    ret: Option<Denotes>,
}

struct Decl {
    vars: Vec<TyVar>,
    fields: Vec<Type>,
}

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
    /// Without a `CheckOutput` the table is empty and admits nothing.
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
        // Lowering only removes, so this settles.
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
                // Carried and not builtin, so a declared sum type.
                _ => Denotes::Ctor,
            }),
            // Unreachable: `carries` refuses both. Spelled out so a new kind fails closed.
            Type::Var(_) | Type::Fn { .. } => None,
        }
    }

    pub fn carries(&self, ty: &Type, decl_vars: Option<&[TyVar]>) -> bool {
        match ty {
            Type::Var(v) => decl_vars.is_some_and(|vars| vars.contains(v)),
            Type::Fn { .. } => false,
            Type::Record(fields) => fields.values().all(|t| self.carries(t, decl_vars)),
            Type::Con(name, args) => match name.as_str() {
                // Must match `crossable`'s leaf set.
                "Int" | "Bool" | "Bytes" | "String" | "Unit" => args.is_empty(),
                "List" | "Map" => args.iter().all(|t| self.carries(t, decl_vars)),
                // Refused before this table is asked; excluded to keep the leaf set honest.
                "Float" | "Decimal" => false,
                "Cell" | ply_ty::prelude::TASK_TYPE | SECRET => false,
                // Compiled code holds these as `Int` immediates, so one crossing back is wrong.
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

    pub fn answer_crosses(&self, name: &Symbol, value: &Value) -> bool {
        self.sigs
            .get(name)
            .and_then(|sig| sig.ret)
            .is_some_and(|d| d.matches(value))
            || crossable(value)
    }

    pub fn signature_carried(&self, name: &Symbol) -> bool {
        self.sigs
            .get(name)
            .is_some_and(|sig| sig.ret.is_some() && sig.params.iter().all(Option::is_some))
    }
}
