//! The dynamic half of a failure: which definitions actually ran.

use ply_core::{EffectAtom, Footprint};
use ply_hash::DefHash;
use ply_span::{Span, Symbol};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entered {
    /// The program-wide name.
    pub name: Symbol,
    pub hash: Option<DefHash>,
    /// How many times it was entered.
    pub calls: u32,
}

/// One frame of the call stack as it stood when the test failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub name: Symbol,
    pub hash: Option<DefHash>,
    /// Where the *caller* made this call, so the frames read as a path through the source rather
    /// than a list of names.
    pub call_site: Span,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CausalSlice {
    /// False when nothing was traced, in which case every other field is empty rather than
    /// meaningfully absent.
    pub traced: bool,
    /// Whether the traced execution failed the same way the untraced one did.
    pub reproduced: bool,
    /// First-entry order.
    pub entered: Vec<Entered>,
    /// Outermost first, innermost last.
    pub stack: Vec<Frame>,
    /// The atoms actually performed.
    pub observed: Footprint,
    /// A trace that hit its size cap.
    pub truncated: bool,
}

impl CausalSlice {
    /// A slice from a run where tracing was never switched on.
    pub fn untraced() -> CausalSlice {
        CausalSlice::default()
    }

    pub fn ran(&self, name: &Symbol) -> bool {
        self.entered.iter().any(|e| &e.name == name)
    }

    /// `ran`, but honest about a roster that hit its cap.
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
    /// Trace the first execution too — what a `test/nondet` that will not reproduce needs, since
    /// for it there is no replay worth having.
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
    /// A named definition was entered.
    Enter {
        name: Symbol,
        hash: Option<DefHash>,
        call_site: Span,
    },
    Return,
    /// An atom was actually performed, as opposed to merely declared.
    Perform(EffectAtom),
}

/// The stack is captured at [`SliceBuilder::failed`] rather than at the end, because by the end
/// every frame has unwound and the path to the assertion is gone.
pub struct SliceBuilder {
    /// First-entry order, which is what makes the artifact readable top-down.
    entered: Vec<Entered>,
    at: BTreeMap<Symbol, usize>,
    live: Vec<Frame>,
    failed: Option<Vec<Frame>>,
    observed: BTreeSet<EffectAtom>,
    /// Distinct definitions, not calls: a call count is a counter, but a program that generates
    /// names without bound would grow `entered` without bound, and the test being explained is
    /// often the one that ran away.
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

    /// Every enter and return moves the live stack, whatever the cap says: a dropped frame would
    /// leave the stack claiming a call that had already returned, and the stack is the part of the
    /// artifact that has to stay exact.
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

    /// Freezes the stack.
    pub fn failed(&mut self) {
        if self.failed.is_none() {
            self.failed = Some(self.live.clone());
        }
    }

    /// `reproduced` is whether the traced run failed the same way the untraced one did.
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

/// What the assertion was checking, in a form a consumer does not have to parse out of a rendered
/// message.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AssertionKind {
    /// `assert_eq`.
    Eq,
    /// `assert`.
    Bool,
    /// `panic`.
    Panic,
    /// Something the evaluator refused to do — a bad cast, a runaway `range`, a borrowed cell.
    Runtime,
    /// A `perform` reached no handler.
    UnhandledEffect,
    RecursionLimit,
    /// A simulated region stopped making progress: nothing was enabled and no timer could fire, or
    /// the per-interleaving step budget was spent.
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

/// Rendered strings rather than a value tree: an agent acts on `expected` versus `actual` and on
/// where they first differ, and a faithful serialization of a `Value` would commit this schema to
/// the evaluator's representation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assertion {
    pub kind: AssertionKind,
    pub expected: Option<String>,
    pub actual: Option<String>,
    pub first_difference: Option<Difference>,
    /// The message passed to `assert` or `panic`, when there was one.
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
