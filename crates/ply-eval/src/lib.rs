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

pub mod builtins;
pub mod code;
pub mod cont;
pub mod differential;
mod env;
mod frame;
pub mod handler;
mod interp;
pub mod limit;
pub mod machine;
pub mod trace;
mod value;
pub mod world;

pub use builtins::{Builtin, Step, assert_failure, assertion_failure};
pub use code::{Code, Node, NodeKind, lower};
pub use cont::{Continuation, Frame, Handled, Next, Prompt, Segment, Stack};
pub use differential::{
    Compared, Detail, Divergence, Evaluator, Report, compare_answers, compare_outcomes,
    is_machine_only, machine_only_clause, machine_only_clauses,
};
pub use env::Env;
pub use interp::Interp;
pub use limit::{DEFAULT_MAX_CALLS, MAX_VALUE_DEPTH};
pub use machine::{DEFAULT_MAX_FRAMES, Machine, Progress};
pub use trace::Trace;
pub use value::{Closure, ClosureKind, Value, Vector, first_difference, values_equal};
pub use world::{CellId, Fixture, World};

/// The default is the engine whose results are authoritative. Flipping it is a
/// `RUNTIME_VERSION` bump, because a cached `Pass` is a claim about what the
/// authoritative engine did.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Engine {
    Treewalk,
    /// Explicit control stack, forkable world, multi-shot continuations.
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
mod tests;
