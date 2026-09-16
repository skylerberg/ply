pub mod derivable;
pub mod env;
pub mod infer;
pub mod prelude;
pub mod scc;
pub mod unify;

use ply_span::Diagnostic;
use ply_syntax::ast::{Module, Program};
use ply_syntax::resolve::Resolved;

pub use derivable::{Adt, Blocked, Context as Derivability, Why, derivable, ordered};
pub use ply_ty::*;

/// Modules are inferred in [`Resolved::order`], so every name a module reaches already has a scheme
/// by the time its bodies are walked.
pub fn prelude_arity(name: &str) -> Option<usize> {
    infer::prelude_arity(name)
}

pub fn check_program(
    program: &Program,
    resolved: &Resolved,
) -> Result<CheckOutput, Vec<Diagnostic>> {
    infer::check_program_with(program, resolved, &Known::default())
}

/// [`check_program`] with interfaces supplied for definitions the caller has already proved
/// unchanged.
pub fn check_program_with(
    program: &Program,
    resolved: &Resolved,
    known: &Known,
) -> Result<CheckOutput, Vec<Diagnostic>> {
    infer::check_program_with(program, resolved, known)
}

/// Checks one module with nothing imported.
pub fn check_module(module: &Module) -> Result<CheckOutput, Vec<Diagnostic>> {
    infer::check_module(module)
}
