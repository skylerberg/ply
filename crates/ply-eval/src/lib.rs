//! The evaluator.
//!
//! Two engines during M6: the tree-walking `Interp` that has run every test to
//! date, and the explicit-control machine that replaces it. `Engine` selects
//! between them so both can run every test and be compared; the tree-walker is
//! deleted at the end of the milestone.
//!
//! `Value` holds `Rc`, so an `Interp` and every value it produces belong to one
//! thread. The scheduler hands each worker its own.

// `Value` pins `Arc` for its shared payloads and `Rc` for shared code and
// continuations, so none of those `Arc`s can ever be `Send` — which is the
// intended design, not an oversight the lint should keep reporting.
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
mod interp;
pub mod limit;
pub mod machine;
pub mod map;
mod memo;
mod pool;
pub mod rc;
pub mod region;
pub mod region_kind;
pub mod sched;
pub mod sim;
pub mod task_regions;
pub mod trace;
mod value;

// `Slot`, `RegionId` and `Snapshot` stay behind `arena::`: they are the
// allocator's own vocabulary and each of those names means something else
// somewhere in this crate.
pub use arena::{Arena, RegionKind};
// The one thing outside this crate needs from `argv`: the attribution harness
// splits a request's surviving argument vectors at the free list's widest class
// and must split at the same number this crate serves.
pub use argv::CLASSES as ARGUMENT_VECTOR_CLASSES;
pub use backend::{Fragment, Mutant, Mutation, Offers, Reference, Spec as BackendSpec};
pub use builtins::{Builtin, Step, assert_failure, assertion_failure};
pub use code::{Code, Lowering, Node, NodeKind, lower};
pub use compiled::Compiled;
pub use cont::{
    Continuation, Delimiter, Frame, Handled, Next, Prompt, Segment, SimId, Stack, Target,
};
pub use costs::{Cause as CostCause, Costs, DefKind as CostDefKind, Verdict as CostVerdict};
pub use differential::{
    Compared, Detail, Divergence, Evaluator, Report, compare_answers, compare_outcomes,
    is_machine_only, machine_only_clause, machine_only_clauses,
};
pub use env::{Env, Slot as ScopeSlot};
pub use escape::{Boundary, Escapee, Handle};
pub use host::{
    Bound, Determinism, HostAnswer, HostBinding, HostHandler, HostListing, HostOp, HostRegistry,
    HostRequest, HostResource, HostRow, HostRuntime, HostUse, Linearity, Pending, ShutdownReport,
    is_drain_incomplete,
};
pub use task_regions::{Fixture, TaskRegions};
// `explore::Step` is deliberately not re-exported: `Step` at the root is the
// builtin's, and one name for two things is worse than a qualified path.
pub use explore::{
    Dependence, Explored, Interleaving, Simulation, Verdict, explore, explore_under,
    measure_reduction,
};
pub use interp::Interp;
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
    Closure, ClosureKind, Decimal, Map, SECRET_REDACTED, Value, Vector, constant_time_eq,
    first_difference, values_equal,
};

/// The default is the engine whose results are authoritative. Flipping it is a
/// `RUNTIME_VERSION` bump, because a cached `Pass` is a claim about what the
/// authoritative engine did.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Engine {
    Treewalk,
    /// Explicit control stack, region-allocated state, multi-shot continuations.
    #[default]
    Machine,
}

impl Engine {
    pub fn as_str(self) -> &'static str {
        match self {
            Engine::Treewalk => "treewalk",
            Engine::Machine => "machine",
        }
    }

    pub fn parse(s: &str) -> Option<Engine> {
        match s {
            "treewalk" => Some(Engine::Treewalk),
            "machine" => Some(Engine::Machine),
            _ => None,
        }
    }
}

/// What a `--engine` flag selects. `Both` is not an engine — it is a request to
/// run two and fail on any disagreement — so it lives beside [`Engine`] rather
/// than inside it, and nothing that must name a single evaluator can hold one.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum EngineChoice {
    Treewalk,
    #[default]
    Machine,
    Both,
}

impl EngineChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            EngineChoice::Treewalk => "treewalk",
            EngineChoice::Machine => "machine",
            EngineChoice::Both => "both",
        }
    }

    pub fn parse(s: &str) -> Option<EngineChoice> {
        match s {
            "treewalk" => Some(EngineChoice::Treewalk),
            "machine" => Some(EngineChoice::Machine),
            "both" => Some(EngineChoice::Both),
            _ => None,
        }
    }

    /// The engine whose verdict is reported. Under `Both` the authoritative
    /// engine answers and the other one audits it, so which engine a run
    /// *reports* never depends on whether auditing was switched on.
    pub fn primary(self) -> Engine {
        match self {
            EngineChoice::Treewalk => Engine::Treewalk,
            EngineChoice::Machine => Engine::Machine,
            EngineChoice::Both => Engine::default(),
        }
    }

    pub fn auditor(self) -> Option<Engine> {
        match self {
            EngineChoice::Both => Some(match Engine::default() {
                Engine::Treewalk => Engine::Machine,
                Engine::Machine => Engine::Treewalk,
            }),
            _ => None,
        }
    }

    /// A cached `Pass` is a claim about what the authoritative engine did, so a
    /// run that is not purely that engine may neither read one nor write one.
    pub fn bypasses_cache(self) -> bool {
        self.primary() != Engine::default() || self.auditor().is_some()
    }
}

#[cfg(test)]
mod build;
#[cfg(test)]
mod numerics;
#[cfg(test)]
mod tests;
