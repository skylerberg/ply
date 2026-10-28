//! The runtime's vocabulary: values, spans, diagnostics and their codes, effects and their atoms,
//! and the host boundary.

// `Value` holds `Arc` payloads and is not `Send`.
#![allow(clippy::arc_with_non_send_sync)]

pub mod argv;
pub mod builtins;
pub mod codec;
pub mod codes;
pub mod crypto;
pub mod digest;
pub mod escape;
pub mod expr;
pub mod files;
pub mod float;
mod footprint;
pub mod handler;
pub mod host;
pub mod instances;
pub mod intty;
pub mod limit;
pub mod list;
pub use expr::{BinOp, render_float};

pub use intty::{INT_TYPES, IntTy};
pub use list::List;
pub mod memo;
pub mod rc;
pub mod reflect;
pub mod semantics;
pub mod sim;
mod span;
pub mod sqlite;
mod value;

pub use argv::CLASSES as ARGUMENT_VECTOR_CLASSES;
pub use builtins::{Builtin, assert_failure};
pub use escape::{Boundary, Escapee, Handle};
pub use footprint::{EffectAtom, Mode, Resource, label_var_name};
pub use host::{
    Determinism, HostBinding, HostRuntime, HostUse, Linearity, Pending, RuntimeFactory,
    ShutdownReport,
};
pub use instances::Instances;
pub use limit::{DEFAULT_MAX_CALLS, DEFAULT_STEP_BUDGET, MAX_VALUE_DEPTH};
pub use rc::RcStats;
pub use semantics::{strict_binary, unstated};
pub use sim::{Seed, SimId, TaskId};
pub use span::{
    Diagnostic, Edit, Fix, Label, Severity, SourceFile, SourceId, SourceMap, Span, Sparse, Symbol,
    intern_code, slot,
};
pub use value::{
    Closure, ClosureKind, Decimal, Fields, Fixed, FixedOp, Map, Sealed, Synth, Value,
    constant_time_eq, type_error, values_equal,
};
