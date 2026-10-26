//! The runtime's vocabulary: values, spans, diagnostics and their codes, effects and their
//! footprints, the host boundary, and the deterministic simulation compiled code runs under.

// `Value` mixes `Arc` payloads with a task handle's `Rc`, so it is never `Send` by design.
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
mod plain;
pub mod rc;
pub mod reflect;
pub mod region;
pub mod sched;
pub mod semantics;
pub mod sim;
mod span;
pub mod sqlite;
mod value;

pub use argv::CLASSES as ARGUMENT_VECTOR_CLASSES;
pub use builtins::{Builtin, assert_failure, assertion_failure};
pub use escape::{Boundary, Escapee, Handle};
pub use footprint::{EffectAtom, Footprint, Mode, Resource, atom_texts, label_var_name};
pub use host::{
    Bound, Determinism, HostAnswer, HostBinding, HostHandler, HostOp, HostRegistry, HostRequest,
    HostResource, HostRuntime, HostUse, Linearity, Pending, RuntimeFactory, ShutdownReport,
};
pub use instances::Instances;
pub use limit::{DEFAULT_MAX_CALLS, DEFAULT_STEP_BUDGET, MAX_VALUE_DEPTH};
pub use plain::{Fun, Plain, SHOWN_DEPTH, SHOWN_ITEMS};
pub use rc::RcStats;
pub use region::{Interleaving, SimId, Verdict};
pub use sched::TaskHandle;
pub use semantics::{strict_binary, unstated};
pub use sim::{
    Access, Answer, Clock, Domain, Handlers, OpSignature, Rand, SEEDED_EFFECTS, SEEDED_OPS, Seed,
    SimType, Sleep, StepFootprint, Stream, TaskId, Wakeup,
};
pub use span::{
    Diagnostic, Edit, Fix, Label, Severity, SourceFile, SourceId, SourceMap, Span, Sparse, Symbol,
    intern_code, slot,
};
pub use value::{
    Closure, ClosureKind, Decimal, Difference, Fields, Fixed, FixedOp, Map, PathStep, Sealed,
    Synth, Value, constant_time_eq, first_difference, type_error, values_equal,
};
