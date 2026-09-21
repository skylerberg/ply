//! The evaluator.

// `Value` mixes `Arc` payloads with `Rc`-backed persistent maps, so it is never `Send` by design.
#![allow(clippy::arc_with_non_send_sync)]

pub mod arena;
pub mod argv;
pub mod backend;
pub mod builtins;
pub mod census;
pub mod compiled;
pub mod cont;
pub mod escape;
pub mod explore;
pub mod handler;
pub mod host;
pub mod limit;
pub mod list;
pub use list::List;
pub mod evaluator;
pub mod map;
pub mod memo;
mod pool;
pub mod rc;
pub mod region;
pub mod sched;
pub mod semantics;
pub mod sim;
pub mod task_regions;
pub mod trace;
mod value;

// `Slot` and `RegionId` stay behind `arena::`: each name means something else here.
pub use arena::{Arena, RegionKind};
pub use argv::CLASSES as ARGUMENT_VECTOR_CLASSES;
pub use backend::{
    Compilation, Counters, Kind as BackendKind, Offers, Provider, Spec as BackendSpec,
};
pub use builtins::{Builtin, Step, assert_failure, assertion_failure};
pub use compiled::{Compiled, Entered, mentions_a_width};
pub use cont::{Frame, Next, Prompt, Segment, SimId, Stack};
pub use escape::{Boundary, Escapee, Handle};
pub use host::{
    Bound, Determinism, HostAnswer, HostBinding, HostHandler, HostListing, HostOp, HostRegistry,
    HostRequest, HostResource, HostRow, HostRuntime, HostUse, Linearity, Pending, ShutdownReport,
    is_drain_incomplete,
};
pub use task_regions::{Fixture, TaskRegions};
// `explore::Step` is not re-exported: `Step` at the root is the builtin's.
pub use evaluator::{
    Machine, Unbound, carries_secret, check_host_answer, err_footprint_escape,
    err_host_in_simulation, err_nested_simulation, err_no_runtime, err_not_compiled,
    err_secret_to_host, err_unenumerated_atom,
};
pub use explore::{
    Dependence, Explored, Interleaving, Simulation, Verdict, explore, explore_under,
    measure_reduction,
};
pub use limit::{DEFAULT_MAX_CALLS, DEFAULT_STEP_BUDGET, MAX_VALUE_DEPTH};
pub use rc::Stats as RcStats;
pub use semantics::strict_binary;
pub use sim::{
    Access, Answer, Clock, Domain, Exploration, Handlers, Naive, OpSignature, Plan, Race, RaceSite,
    Rand, SEEDED_EFFECTS, SEEDED_OPS, Seed, SimMode, SimTy, Sleep, StepFootprint, Stream, TaskId,
    Wake,
};
pub use trace::Trace;
pub use value::{
    Closure, ClosureKind, Decimal, Fields, Fixed, IntTy, Map, SECRET_REDACTED, Synth, Value,
    constant_time_eq, first_difference, values_equal,
};
