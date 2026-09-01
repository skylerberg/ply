//! The evaluator.

// `Value` pins `Arc` for its shared payloads and `Rc` for shared code and continuations, so none of
// those `Arc`s can ever be `Send` — which is the intended design, not an oversight the lint should
// keep reporting.
#![allow(clippy::arc_with_non_send_sync)]

pub mod arena;
mod argv;
pub mod backend;
pub mod builtins;
pub mod census;
pub mod code;
mod compiled;
pub mod cont;
pub mod costs;
pub mod differential;
mod env;
pub mod escape;
pub mod explore;
mod frame;
pub mod handler;
pub mod host;
pub mod limit;
pub mod machine;
pub mod map;
mod memo;
mod pool;
pub mod rc;
pub mod region;
pub mod region_kind;
pub mod sched;
mod semantics;
pub mod sim;
pub mod slots;
pub mod task_regions;
pub mod trace;
mod value;

// `Slot`, `RegionId` and `Snapshot` stay behind `arena::`: they are the allocator's own vocabulary
// and each of those names means something else somewhere in this crate.
pub use arena::{Arena, RegionKind};
// The one thing outside this crate needs from `argv`: the attribution harness splits a request's
// surviving argument vectors at the free list's widest class and must split at the same number this
// crate serves.
pub use argv::CLASSES as ARGUMENT_VECTOR_CLASSES;
pub use backend::{
    Compilation, Counters, Fragment, Kind as BackendKind, Mutant, Mutation, Offers, Policed,
    Provider, Reference, Spec as BackendSpec,
};
pub use builtins::{Builtin, Step, assert_failure, assertion_failure};
pub use code::{Code, Lowering, Node, NodeKind, lower};
pub use compiled::Compiled;
pub use cont::{
    Continuation, Delimiter, Frame, Handled, Next, Prompt, Segment, SimId, Stack, Target,
};
pub use costs::{Cause as CostCause, Costs, DefKind as CostDefKind, Verdict as CostVerdict};
pub use differential::{
    Compared, Detail, Divergence, Evaluator, Report, compare_answers, compare_outcomes,
};
pub use env::{Env, Slot as ScopeSlot};
pub use escape::{Boundary, Escapee, Handle};
pub use host::{
    Bound, Determinism, HostAnswer, HostBinding, HostHandler, HostListing, HostOp, HostRegistry,
    HostRequest, HostResource, HostRow, HostRuntime, HostUse, Linearity, Pending, ShutdownReport,
    is_drain_incomplete,
};
pub use task_regions::{Fixture, TaskRegions};
// `explore::Step` is deliberately not re-exported: `Step` at the root is the builtin's, and one
// name for two things is worse than a qualified path.
pub use explore::{
    Dependence, Explored, Interleaving, Simulation, Verdict, explore, explore_under,
    measure_reduction,
};
pub use limit::{DEFAULT_MAX_CALLS, MAX_VALUE_DEPTH};
pub use machine::{Machine, Progress};
pub use rc::{Own, Stats as RcStats};
pub use region_kind::Regions;
pub use sim::{
    Access, Answer, Clock, Domain, Exploration, Handlers, Naive, OpSignature, Plan, Race, RaceSite,
    Rand, SEEDED_EFFECTS, SEEDED_OPS, Seed, SimMode, SimTy, Sleep, StepFootprint, Stream, TaskId,
    Wake,
};
pub use trace::Trace;
pub use value::{
    Closure, ClosureKind, Decimal, Fields, Map, SECRET_REDACTED, Value, Vector, constant_time_eq,
    first_difference, values_equal,
};

#[cfg(test)]
mod build;
#[cfg(test)]
mod numerics;
#[cfg(test)]
mod tests;
