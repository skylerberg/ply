pub mod env;
pub mod infer;
pub mod print;
mod scc;
pub mod ty;
pub mod unify;

#[cfg(test)]
mod tests;

use indexmap::IndexMap;
use ply_span::{Diagnostic, Span, Symbol};
use ply_syntax::ast::{Mode, Module};

pub use print::{print_row, print_scheme, print_type};
pub use ty::{EffectAtom, Footprint, Resource, Row, RowVar, Scheme, TyVar, Type};

#[derive(Clone, Debug)]
pub struct OpInfo {
    pub name: Symbol,
    pub mode: Mode,
    pub resource_param: bool,
    pub params: Vec<Type>,
    pub ret: Type,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct EffectInfo {
    pub name: Symbol,
    pub nondet: bool,
    pub ops: IndexMap<Symbol, OpInfo>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct CtorInfo {
    pub name: Symbol,
    pub type_name: Symbol,
    /// Position among the owning type's variants, in declaration order.
    pub index: usize,
    pub arity: usize,
    pub fields: Vec<Type>,
    /// Nullary variants have the sum type itself; the rest have a function type.
    pub scheme: Scheme,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct DefInfo {
    pub name: Symbol,
    pub scheme: Scheme,
    /// Closed after solving. Empty for a pure function.
    pub footprint: Footprint,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct TestInfo {
    pub name: String,
    pub index: usize,
    pub nondet: bool,
    pub footprint: Footprint,
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct CheckOutput {
    pub defs: IndexMap<Symbol, DefInfo>,
    pub tests: Vec<TestInfo>,
    pub effects: IndexMap<Symbol, EffectInfo>,
    pub ctors: IndexMap<Symbol, CtorInfo>,
}

impl CheckOutput {
    pub fn effect_of(&self, atom: &EffectAtom) -> Option<&EffectInfo> {
        self.effects.get(&atom.effect)
    }

    pub fn is_nondet(&self, atom: &EffectAtom) -> bool {
        self.effects.get(&atom.effect).is_some_and(|e| e.nondet)
    }
}

pub fn check_module(module: &Module) -> Result<CheckOutput, Vec<Diagnostic>> {
    infer::check_module(module)
}
