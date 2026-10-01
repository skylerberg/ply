//! The evaluator, and the runtime's vocabulary: spans, diagnostics and their codes, and the
//! program record the compiler answers with.

// `Value` mixes `Arc` payloads with `Rc`-backed persistent maps, so it is never `Send` by design.
#![allow(clippy::arc_with_non_send_sync)]

pub mod arena;
pub mod argv;
pub mod backend;
pub mod builtins;
pub mod codec;
pub mod codes;
pub mod compiled;
pub mod cont;
pub mod decode;
pub mod escape;
pub mod explore;
pub mod expr;
mod footprint;
pub mod handler;
mod hash;
pub mod host;
pub mod intty;
pub mod limit;
pub mod list;
pub use expr::{BinOp, Lit, UnOp, render_float};
/// The built-in type names the runtime recognizes: a credential, and the handle `task.spawn`
/// answers with. The checker declares them; only their names reach here.
pub const SECRET: &str = "Secret";
pub const TASK_TYPE: &str = "Task";

pub use intty::{INT_TYPES, IntTy};
pub use list::List;
pub mod evaluator;
pub mod map;
pub mod memo;
mod plain;
mod pool;
mod program;
pub mod rc;
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
pub use builtins::{Builtin, Step, assert_failure, assertion_failure};
pub use compiled::{Compiled, Entered};
pub use cont::{Frame, Next, Prompt, Segment, SimId, Stack};
pub use escape::{Boundary, Escapee, Handle};
pub use host::{
    Bound, Determinism, HostAnswer, HostBinding, HostHandler, HostListing, HostOp, HostRegistry,
    HostRequest, HostResource, HostRow, HostRuntime, HostUse, Linearity, Pending, ShutdownReport,
};
pub use task_regions::{Fixture, TaskRegions};
// `explore::Step` is not re-exported: `Step` at the root is the builtin's.
pub use evaluator::{
    Ended, Machine, Unbound, carries_secret, check_host_answer, err_footprint_escape,
    err_host_in_simulation, err_nested_simulation, err_no_runtime, err_not_compiled,
    err_secret_to_host, err_unenumerated_atom,
};
pub use explore::{
    Dependence, Explored, Interleaving, Simulation, Verdict, explore, explore_under,
    measure_reduction,
};
pub use footprint::{EffectAtom, Footprint, Mode, Resource, atom_texts, label_var_name};
pub use hash::{DefHash, HashOutput};
pub use limit::{DEFAULT_MAX_CALLS, DEFAULT_STEP_BUDGET, MAX_VALUE_DEPTH};
pub use plain::{Fun, Plain, SHOWN_DEPTH, SHOWN_ITEMS};
pub use program::{
    CheckOutput, DefInfo, DefWritten, EffectInfo, EffectSet, EmitterRoot, Front, Hashed, LawInfo,
    Literal, ModuleInfo, ModuleName, OpInfo, Ordinal, Pinned, SpecInfo, SpecKind, TestInfo,
    TypeDecl, Visibility, WrittenParam, is_ident, is_ident_continue, is_ident_start,
};
pub use rc::Stats as RcStats;
pub use sched::TaskHandle;
pub use semantics::strict_binary;
pub use sim::{
    Access, Answer, Clock, Cost, Domain, Exploration, Handlers, OpSignature, Plan, Race, RaceSite,
    Rand, SEEDED_EFFECTS, SEEDED_OPS, Seed, SimMode, SimTy, Sleep, StepFootprint, Stream, TaskId,
    Wake,
};
pub use span::{
    Diagnostic, Edit, Fix, Label, Severity, SourceFile, SourceId, SourceMap, Span, Sparse, Symbol,
    intern_code, slot,
};
pub use trace::Trace;
pub use value::{
    Closure, ClosureKind, Decimal, Difference, Fields, Fixed, FixedOp, Map, Step as PathStep,
    Synth, Value, constant_time_eq, first_difference, values_equal,
};
