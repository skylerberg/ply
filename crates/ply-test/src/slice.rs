//! The dynamic half of a failure: which definitions actually ran.
//!
//! A test's closure is what it *could* reach. The causal slice is what it *did*
//! reach on the way to the assertion that failed. Forty definitions in the
//! closure and three on the stack is the difference between a list to read and
//! an answer to act on, and the two are not derivable from each other.

use ply_core::{EffectAtom, Footprint};
use ply_hash::DefHash;
use ply_span::{Span, Symbol};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entered {
    /// The program-wide name.
    pub name: Symbol,
    pub hash: Option<DefHash>,
    /// How many times it was entered. A count in the thousands next to an
    /// assertion about a list length is itself a finding.
    pub calls: u32,
}

/// One frame of the call stack as it stood when the test failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub name: Symbol,
    pub hash: Option<DefHash>,
    /// Where the *caller* made this call, so the frames read as a path through
    /// the source rather than a list of names.
    pub call_site: Span,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CausalSlice {
    /// False when nothing was traced, in which case every other field is empty
    /// rather than meaningfully absent.
    pub traced: bool,
    /// Whether the traced execution failed the same way the untraced one did.
    /// A `det` test always reproduces; `test/nondet` may not, and a slice from a
    /// run that went green is evidence about a different execution.
    pub reproduced: bool,
    /// First-entry order.
    pub entered: Vec<Entered>,
    /// Outermost first, innermost last. The last frame is where the failure
    /// happened.
    pub stack: Vec<Frame>,
    /// The atoms actually performed.
    ///
    /// > **Corrected in place (2026-08-24): it is not a subset of the declared
    /// > footprint.** This read *"The atoms actually performed, which is a
    /// > subset of the declared footprint. A declared atom that never fired
    /// > means a branch was not taken."* The second sentence stands. The first
    /// > does not, and no change was needed to refute it: both engines record
    /// > **every** `perform` (`ply_eval::machine`'s `State::Perform`,
    /// > `ply_eval::interp`'s `perform`), including one a `handle` inside the
    /// > call discharges — and discharging is exactly what keeps an atom out of
    /// > a row. Measured on a definition that performs `state.get` and handles
    /// > it itself: its published `footprint` and its inferred `performed` are
    /// > both empty and `ply_eval::Trace` holds `state.read`. So `observed` and
    /// > the declared footprint are two sets that overlap, and a reader may
    /// > conclude "a branch was not taken" from a declared atom that is absent
    /// > here, but may **not** conclude that an atom present here was declared.
    /// >
    /// > Nothing outside `ply-test/tests/bisect_audit.rs` builds one of these
    /// > yet — see `CONTRIBUTING.md` §"Things known to be broken" item 15 —
    /// > so the correction is to the contract rather than to an output anyone
    /// > has seen.
    pub observed: Footprint,
    /// A trace that hit its size cap. The stack is still exact; `entered` is not.
    pub truncated: bool,
}

impl CausalSlice {
    /// A slice from a run where tracing was never switched on. Distinct from an
    /// empty slice, which would claim nothing ran.
    pub fn untraced() -> CausalSlice {
        CausalSlice::default()
    }

    pub fn ran(&self, name: &Symbol) -> bool {
        self.entered.iter().any(|e| &e.name == name)
    }

    /// `ran`, but honest about a roster that hit its cap. ADR 0004 defines
    /// `ran: false` as "it cannot have caused this, whatever its hash did", which
    /// a truncated roster is in no position to claim: past the cap a definition
    /// that ran is simply not recorded. `None` — "was not traced" — is the only
    /// answer available then, and a consumer that cannot tell the two apart acts
    /// on the wrong one.
    pub fn did_run(&self, name: &Symbol) -> Option<bool> {
        if self.ran(name) || self.stack.iter().any(|f| &f.name == name) {
            return Some(true);
        }
        (!self.truncated).then_some(false)
    }

    /// How far above the failure a definition sits, counting the innermost frame
    /// as zero. `None` when it is not on the failing stack at all — it ran, but
    /// it had returned by the time the assertion blew up.
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

// -------------------------------------------------------------- recording

/// `--trace`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Tracing {
    /// Trace a failing test's re-run. The green path stays untraced, which is
    /// the whole reason tracing happens on a replay: a push and a pop per call
    /// sit on the interpreter's hottest path.
    #[default]
    Auto,
    /// Trace the first execution too — what a `test/nondet` that will not
    /// reproduce needs, since for it there is no replay worth having.
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
    /// A named definition was entered. `call_site` is where the *caller* made
    /// the call, so the frames read as a path through the source.
    Enter {
        name: Symbol,
        hash: Option<DefHash>,
        call_site: Span,
    },
    Return,
    /// An atom was actually performed, as opposed to merely declared.
    Perform(EffectAtom),
}

/// The stack is captured at [`SliceBuilder::failed`] rather than at the end,
/// because by the end every frame has unwound and the path to the assertion is
/// gone. Everything on that stack ran; not everything that ran is on it.
pub struct SliceBuilder {
    /// First-entry order, which is what makes the artifact readable top-down.
    entered: Vec<Entered>,
    at: BTreeMap<Symbol, usize>,
    live: Vec<Frame>,
    failed: Option<Vec<Frame>>,
    observed: BTreeSet<EffectAtom>,
    /// Distinct definitions, not calls: a call count is a counter, but a program
    /// that generates names without bound would grow `entered` without bound,
    /// and the test being explained is often the one that ran away.
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

    /// Every enter and return moves the live stack, whatever the cap says: a
    /// dropped frame would leave the stack claiming a call that had already
    /// returned, and the stack is the part of the artifact that has to stay
    /// exact.
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

    /// Freezes the stack. Called where the assertion blew up; calling it twice
    /// keeps the first, because the first failure is the one being explained.
    pub fn failed(&mut self) {
        if self.failed.is_none() {
            self.failed = Some(self.live.clone());
        }
    }

    /// `reproduced` is whether the traced run failed the same way the untraced
    /// one did. A slice from a run that went green is evidence about a different
    /// execution and is reported rather than mixed in.
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

/// What the assertion was checking, in a form a consumer does not have to parse
/// out of a rendered message.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AssertionKind {
    /// `assert_eq`.
    Eq,
    /// `assert`.
    Bool,
    /// `panic`.
    Panic,
    /// Something the evaluator refused to do — a bad cast, a runaway `range`, a
    /// borrowed cell.
    Runtime,
    /// A `perform` reached no handler. Inference should have ruled this out.
    UnhandledEffect,
    RecursionLimit,
    /// A simulated region stopped making progress: nothing was enabled and no
    /// timer could fire, or the per-interleaving step budget was spent. One kind
    /// for both, because from the program's side they are the same finding and
    /// the fix is in the same place.
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
    /// A path into the compared values, `.entries[2].amount`. Empty for scalars.
    pub path: String,
    pub expected: String,
    pub actual: String,
}

/// Rendered strings rather than a value tree: an agent acts on `expected` versus
/// `actual` and on where they first differ, and a faithful serialization of a
/// `Value` would commit this schema to the evaluator's representation.
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

#[cfg(test)]
mod tests {
    use super::*;
    use ply_span::SourceId;

    fn frame(name: &str) -> Frame {
        Frame {
            name: Symbol::new(name),
            hash: None,
            call_site: Span::new(SourceId(0), 0, 1),
        }
    }

    fn slice() -> CausalSlice {
        CausalSlice {
            traced: true,
            reproduced: true,
            entered: ["outer", "middle", "inner", "returned_already"]
                .iter()
                .map(|n| Entered {
                    name: Symbol::new(n),
                    hash: None,
                    calls: 1,
                })
                .collect(),
            stack: vec![frame("outer"), frame("middle"), frame("inner")],
            observed: Footprint::empty(),
            truncated: false,
        }
    }

    #[test]
    fn depth_counts_up_from_the_failure() {
        let s = slice();
        assert_eq!(s.depth_of(&Symbol::new("inner")), Some(0));
        assert_eq!(s.depth_of(&Symbol::new("outer")), Some(2));
    }

    /// Everything on the stack ran, but not everything that ran is on the stack.
    /// Confusing the two would rank a definition that had already returned as if
    /// it were where the failure happened.
    #[test]
    fn a_definition_that_returned_before_the_failure_ran_but_has_no_depth() {
        let s = slice();
        assert!(s.ran(&Symbol::new("returned_already")));
        assert_eq!(s.depth_of(&Symbol::new("returned_already")), None);
    }

    #[test]
    fn an_untraced_slice_claims_nothing_rather_than_claiming_emptiness() {
        let s = CausalSlice::untraced();
        assert!(!s.traced);
        assert!(!s.ran(&Symbol::new("anything")));
        assert!(s.path().is_empty());
    }

    #[test]
    fn recursion_is_visible_as_a_call_count() {
        let mut s = slice();
        s.entered[2].calls = 4096;
        assert_eq!(s.entered[2].calls, 4096);
        assert_eq!(s.depth_of(&Symbol::new("inner")), Some(0));
    }

    fn enter(name: &str) -> Event {
        Event::Enter {
            name: Symbol::new(name),
            hash: None,
            call_site: Span::new(SourceId(0), 0, 1),
        }
    }

    fn built(events: &[Event]) -> SliceBuilder {
        let mut b = SliceBuilder::new();
        for e in events {
            b.record(e.clone());
        }
        b
    }

    /// The two halves of the slice answer different questions, and confusing
    /// them ranks a definition that had already returned as if it were where the
    /// failure happened.
    #[test]
    fn the_stack_is_the_path_and_entered_is_everything_that_ran() {
        let mut b = built(&[
            enter("post"),
            enter("format"),
            Event::Return,
            enter("apply_debit"),
        ]);
        b.failed();
        b.record(Event::Return);
        b.record(Event::Return);
        let slice = b.finish(true);

        assert_eq!(
            slice.path(),
            vec![&Symbol::new("post"), &Symbol::new("apply_debit")]
        );
        assert!(slice.ran(&Symbol::new("format")));
        assert_eq!(slice.depth_of(&Symbol::new("format")), None);
        assert_eq!(slice.depth_of(&Symbol::new("apply_debit")), Some(0));
        assert!(!slice.truncated);
    }

    #[test]
    fn a_definition_entered_twice_is_one_row_with_a_count() {
        let mut b = built(&[
            enter("loop_body"),
            Event::Return,
            enter("loop_body"),
            Event::Return,
            enter("loop_body"),
        ]);
        b.failed();
        let slice = b.finish(true);
        assert_eq!(slice.entered.len(), 1);
        assert_eq!(slice.entered[0].calls, 3);
    }

    /// The cap bounds how many *distinct* definitions are remembered. Letting it
    /// bound the stack too would leave frames on it that had already returned.
    #[test]
    fn hitting_the_cap_truncates_the_roster_and_not_the_stack() {
        let mut b = SliceBuilder::with_cap(2);
        for name in ["a", "b", "c", "d"] {
            b.record(enter(name));
        }
        b.failed();
        let slice = b.finish(true);

        assert!(slice.truncated);
        assert_eq!(slice.entered.len(), 2);
        assert_eq!(slice.stack.len(), 4);
        assert_eq!(slice.depth_of(&Symbol::new("d")), Some(0));
    }

    #[test]
    fn only_performed_atoms_are_observed() {
        let atom = ply_core::EffectAtom::new(
            "db",
            ply_core::Resource::Named(Symbol::new("users")),
            ply_syntax::ast::Mode::Read,
        );
        let mut b = built(&[enter("f"), Event::Perform(atom.clone())]);
        b.failed();
        let slice = b.finish(true);
        assert_eq!(slice.observed.atoms().collect::<Vec<_>>(), vec![&atom]);
    }

    /// A failure that is never reported leaves no path, which is different from
    /// a path of length zero and has to stay so.
    #[test]
    fn a_builder_that_was_never_told_of_a_failure_reports_no_stack() {
        let slice = built(&[enter("f")]).finish(true);
        assert!(slice.traced);
        assert!(slice.stack.is_empty());
        assert!(slice.ran(&Symbol::new("f")));
    }

    #[test]
    fn the_first_failure_is_the_one_explained() {
        let mut b = built(&[enter("outer"), enter("inner")]);
        b.failed();
        b.record(Event::Return);
        b.failed();
        assert_eq!(
            b.finish(true).path(),
            vec![&Symbol::new("outer"), &Symbol::new("inner")]
        );
    }
}
