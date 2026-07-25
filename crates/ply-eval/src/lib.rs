//! The v0 evaluator: tree-walking, environment-passing, single-threaded.
//!
//! `Value` holds `Rc`, so an `Interp` and every value it produces belong to one
//! thread. The scheduler hands each worker its own.

// `Value` pins `Arc` for its shared payloads and `Rc` for cells, so none of
// those `Arc`s can ever be `Send` — which is the intended design, not an
// oversight the lint should keep reporting.
#![allow(clippy::arc_with_non_send_sync)]

mod builtins;
mod env;
mod interp;
mod value;

pub use builtins::Builtin;
pub use env::Env;
pub use interp::{DEFAULT_MAX_DEPTH, Interp};
pub use value::{Closure, ClosureKind, Value, Vector, first_difference, values_equal};

#[cfg(test)]
mod build;
#[cfg(test)]
mod tests;
