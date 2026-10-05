//! The evaluator, and the runtime's vocabulary: spans, diagnostics and their codes, and the
//! program record the compiler answers with.

// `Value` mixes `Arc` payloads with a task handle's `Rc`, so it is never `Send` by design.
#![allow(clippy::arc_with_non_send_sync)]

pub mod arena;
pub mod argv;
pub mod backend;
pub mod builtins;
mod carry;
pub mod codec;
pub mod codes;
pub mod compiled;
pub mod decode;
pub mod digest;
pub mod escape;
pub mod expr;
pub mod files;
mod footprint;
pub mod handler;
mod hash;
pub mod host;
pub mod instances;
pub mod intty;
pub mod limit;
pub mod list;
pub use expr::{BinOp, render_float};

pub use intty::{INT_TYPES, IntTy};
pub use list::List;
pub mod evaluator;
pub mod map;
pub mod memo;
mod plain;
mod program;
pub mod rc;
pub mod reflect;
pub mod region;
pub mod sched;
pub mod semantics;
pub mod sim;
mod span;
pub mod task_regions;
pub mod trace;
mod value;

// `Slot` and `RegionId` stay behind `arena::`: each name means something else here.
pub use arena::{Arena, RegionKind};
pub use argv::CLASSES as ARGUMENT_VECTOR_CLASSES;
pub use backend::{Compilation, Counters, Offers, Provider};
pub use builtins::{Builtin, assert_failure, assertion_failure};
pub use carry::{Carry, CtorCarries};
pub use compiled::{Compiled, Entered};
pub use escape::{Boundary, Escapee, Handle};
pub use evaluator::{
    Case, Ended, Machine, Unbound, carries_secret, check_host_answer, err_footprint_escape,
    err_host_in_simulation, err_nested_simulation, err_no_runtime, err_not_compiled,
    err_secret_to_host, err_unenumerated_atom,
};
pub use footprint::{EffectAtom, Footprint, Mode, Resource, atom_texts, label_var_name};
pub use hash::{DefHash, HashOutput};
pub use host::{
    Bound, Determinism, HostAnswer, HostBinding, HostHandler, HostListing, HostOp, HostRegistry,
    HostRequest, HostResource, HostRow, HostRuntime, HostUse, Linearity, Pending, RuntimeFactory,
    ShutdownReport,
};
pub use instances::Instances;
pub use limit::{DEFAULT_MAX_CALLS, DEFAULT_STEP_BUDGET, MAX_VALUE_DEPTH};
pub use plain::{Fun, Plain, SHOWN_DEPTH, SHOWN_ITEMS};
pub use program::{
    Analysis, CheckOutput, DefInfo, EffectInfo, EmitterRoot, LawInfo, ModuleInfo, ModuleName,
    OpInfo, Ordinal, SpecKind, TestInfo, TypeDecl, Visibility, is_ident, is_ident_continue,
    is_ident_start,
};
pub use rc::RcStats;
pub use region::{Interleaving, SimId, Verdict};
pub use sched::TaskHandle;
pub use semantics::strict_binary;
pub use sim::{
    Access, Answer, Clock, Domain, Handlers, OpSignature, Rand, SEEDED_EFFECTS, SEEDED_OPS, Seed,
    SimType, Sleep, StepFootprint, Stream, TaskId, Wakeup,
};
pub use span::{
    Diagnostic, Edit, Fix, Label, Severity, SourceFile, SourceId, SourceMap, Span, Sparse, Symbol,
    intern_code, slot,
};
pub use task_regions::{Fixture, TaskRegions};
pub use trace::Trace;
pub use value::{
    Closure, ClosureKind, Decimal, Difference, Fields, Fixed, FixedOp, Map, PathStep, Synth, Value,
    constant_time_eq, first_difference, values_equal,
};
