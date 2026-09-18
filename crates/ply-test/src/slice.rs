//! The dynamic half of a failure: which definitions actually ran.

use ply_hash::DefHash;
use ply_span::{Span, Symbol};
use ply_ty::{EffectAtom, Footprint};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entered {
    pub name: Symbol,
    pub hash: Option<DefHash>,
    pub calls: u32,
}

/// A call-stack frame as it stood when the test failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub name: Symbol,
    pub hash: Option<DefHash>,
    /// Where the *caller* made this call.
    pub call_site: Span,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CausalSlice {
    /// False when nothing was traced; every other field is then empty, not meaningfully absent.
    pub traced: bool,
    /// Whether the traced execution failed the same way the untraced one did.
    pub reproduced: bool,
    /// First-entry order.
    pub entered: Vec<Entered>,
    /// Outermost first, innermost last.
    pub stack: Vec<Frame>,
    pub observed: Footprint,
    pub truncated: bool,
}

impl CausalSlice {
    pub fn untraced() -> CausalSlice {
        CausalSlice::default()
    }

    pub fn ran(&self, name: &Symbol) -> bool {
        self.entered.iter().any(|e| &e.name == name)
    }

    /// `ran`, but `None` when a truncated roster cannot say.
    pub fn did_run(&self, name: &Symbol) -> Option<bool> {
        if self.ran(name) || self.stack.iter().any(|f| &f.name == name) {
            return Some(true);
        }
        (!self.truncated).then_some(false)
    }

    /// How far above the failure a definition sits, counting the innermost frame as zero.
    pub fn depth_of(&self, name: &Symbol) -> Option<usize> {
        self.stack
            .iter()
            .rposition(|f| &f.name == name)
            .map(|at| self.stack.len() - 1 - at)
    }

    pub fn path(&self) -> Vec<&Symbol> {
        self.stack.iter().map(|f| &f.name).collect()
    }
}

/// `--trace`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Tracing {
    /// Trace a failing test's re-run.
    #[default]
    Auto,
    /// Trace the first execution too, for a `test/nondet` that will not reproduce.
    Always,
    Never,
}

impl Tracing {
    pub fn as_str(self) -> &'static str {
        match self {
            Tracing::Auto => "auto",
            Tracing::Always => "always",
            Tracing::Never => "never",
        }
    }

    pub fn parse(s: &str) -> Option<Tracing> {
        match s {
            "auto" => Some(Tracing::Auto),
            "always" => Some(Tracing::Always),
            "never" => Some(Tracing::Never),
            _ => None,
        }
    }

    pub fn traces_first_run(self) -> bool {
        self == Tracing::Always
    }

    pub fn traces_replay(self) -> bool {
        self != Tracing::Never
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Enter {
        name: Symbol,
        hash: Option<DefHash>,
        call_site: Span,
    },
    Return,
    /// An atom was actually performed, as opposed to merely declared.
    Perform(EffectAtom),
}

/// The stack is captured at [`SliceBuilder::failed`]: by the end every frame has unwound.
pub struct SliceBuilder {
    entered: Vec<Entered>,
    at: BTreeMap<Symbol, usize>,
    live: Vec<Frame>,
    failed: Option<Vec<Frame>>,
    observed: BTreeSet<EffectAtom>,
    /// Distinct definitions, not calls: a runaway test can generate names without bound.
    cap: usize,
    truncated: bool,
}

impl Default for SliceBuilder {
    fn default() -> SliceBuilder {
        SliceBuilder::with_cap(SliceBuilder::DEFAULT_CAP)
    }
}

impl SliceBuilder {
    pub const DEFAULT_CAP: usize = 1 << 16;

    pub fn new() -> SliceBuilder {
        SliceBuilder::default()
    }

    pub fn with_cap(cap: usize) -> SliceBuilder {
        SliceBuilder {
            entered: Vec::new(),
            at: BTreeMap::new(),
            live: Vec::new(),
            failed: None,
            observed: BTreeSet::new(),
            cap,
            truncated: false,
        }
    }

    /// Moves the live stack on every enter and return regardless of the cap, so it stays exact.
    pub fn record(&mut self, event: Event) {
        match event {
            Event::Return => {
                self.live.pop();
            }
            Event::Enter {
                name,
                hash,
                call_site,
            } => {
                self.live.push(Frame {
                    name: name.clone(),
                    hash,
                    call_site,
                });
                match self.at.get(&name) {
                    Some(&at) => self.entered[at].calls = self.entered[at].calls.saturating_add(1),
                    None if self.entered.len() < self.cap => {
                        self.at.insert(name.clone(), self.entered.len());
                        self.entered.push(Entered {
                            name,
                            hash,
                            calls: 1,
                        });
                    }
                    None => self.truncated = true,
                }
            }
            Event::Perform(atom) => {
                self.observed.insert(atom);
            }
        }
    }

    pub fn failed(&mut self) {
        if self.failed.is_none() {
            self.failed = Some(self.live.clone());
        }
    }

    pub fn finish(self, reproduced: bool) -> CausalSlice {
        CausalSlice {
            traced: true,
            reproduced,
            entered: self.entered,
            stack: self.failed.unwrap_or_default(),
            observed: Footprint::from_atoms(self.observed),
            truncated: self.truncated,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AssertionKind {
    /// `assert_eq`.
    Eq,
    /// `assert`.
    Bool,
    Panic,
    /// The evaluator refused: a bad cast, a runaway `range`, a borrowed cell.
    Runtime,
    UnhandledEffect,
    RecursionLimit,
    /// A simulated region made no progress, or spent its per-interleaving step budget.
    Deadlock,
}

impl AssertionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AssertionKind::Eq => "eq",
            AssertionKind::Bool => "bool",
            AssertionKind::Panic => "panic",
            AssertionKind::Runtime => "runtime",
            AssertionKind::UnhandledEffect => "unhandled_effect",
            AssertionKind::RecursionLimit => "recursion_limit",
            AssertionKind::Deadlock => "deadlock",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Difference {
    /// A path into the compared values, `.entries[2].amount`.
    pub path: String,
    pub expected: String,
    pub actual: String,
}

/// Rendered strings rather than a value tree, so the schema is not tied to the evaluator's `Value`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assertion {
    pub kind: AssertionKind,
    pub expected: Option<String>,
    pub actual: Option<String>,
    pub first_difference: Option<Difference>,
    pub message: Option<String>,
}

impl Assertion {
    pub fn new(kind: AssertionKind) -> Assertion {
        Assertion {
            kind,
            expected: None,
            actual: None,
            first_difference: None,
            message: None,
        }
    }

    pub fn eq(expected: impl Into<String>, actual: impl Into<String>) -> Assertion {
        Assertion {
            expected: Some(expected.into()),
            actual: Some(actual.into()),
            ..Assertion::new(AssertionKind::Eq)
        }
    }

    pub fn with_difference(mut self, difference: Difference) -> Assertion {
        self.first_difference = Some(difference);
        self
    }

    pub fn with_message(mut self, message: impl Into<String>) -> Assertion {
        self.message = Some(message.into());
        self
    }
}
