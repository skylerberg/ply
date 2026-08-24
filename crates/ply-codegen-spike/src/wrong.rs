//! A deliberately wrong backend, so that "0 disagreements" can be read as
//! evidence.
//!
//! R5's agreement result is 2,396 generated cases over 29 functions and 24
//! whole-kernel searches with no disagreement. A corpus that tests nothing
//! reports exactly that, and this project has shipped a green result over
//! unexplored space more than once. The only way to tell the two apart is to
//! break the thing on purpose and watch a named case go red.
//!
//! [`Mutant`] wraps a real [`SpikeBodies`] and corrupts what it answers. It is
//! **inert unless a caller asks for a mutation**: nothing constructs one on the
//! default path, `mcts` builds one only for `--mutate`, and the tests in
//! `tests/mutations.rs` build one per case and assert that the same comparison
//! the corpus uses reports it.
//!
//! What each mutation is for, and what it is meant to prove:
//!
//! | mutation | the claim it attacks |
//! | --- | --- |
//! | [`Mutation::OffByOne`] | a compiled arithmetic result is checked, not assumed |
//! | [`Mutation::Inverted`] | so is a compiled comparison |
//! | [`Mutation::Stale`] | an answer is tied to *this* call's arguments |
//! | [`Mutation::WrongType`] | the seam marshals a kind as well as a value |
//! | [`Mutation::Unoffered`] | a backend may not answer for a body it does not have |
//! | [`Mutation::ExceedsBudget`] | `budget` is the machine's bound and not a hint |
//! | [`Mutation::Answers`] | a call the machine must never offer at all |
//!
//! The last one is not a backend defect and cannot be demonstrated by a backend
//! alone: `Machine::compiled_answer` refuses any definition whose published
//! effect row is non-empty, so a backend is never asked. What a `Mutant` can do
//! is stand ready to answer one and count how often it was asked — zero, while
//! that gate holds.
//!
//! What each of them found — which case caught it, which corpus did not, and the
//! two blind spots the exercise turned up — is tabulated in
//! `tests/mutations.rs`, whose tests are the standing form of the same
//! experiment.

use crate::entry::SpikeBodies;
use anyhow::{Result, bail};
use ply_eval::{Compiled, Value};
use ply_span::Symbol;
use ply_syntax::ast::Program;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// One way of being wrong.
#[derive(Clone)]
pub enum Mutation {
    /// The honest answer, so a harness can check that the wrapper itself changes
    /// nothing. Every mutation result is read against this.
    None,
    /// `Int(n)` becomes `Int(n + 1)`: the arithmetic is off by one.
    OffByOne,
    /// `Bool(b)` becomes `Bool(!b)`: the comparison is inverted.
    Inverted,
    /// This call answers what the *previous* entered call answered.
    Stale,
    /// The same information in the wrong kind — `Bool` where the definition
    /// returns `Int`, and `Int` where it returns `Bool`. Both are crossable, so
    /// the seam carries them and whatever notices has to be downstream of it.
    WrongType,
    /// Answers for a name this backend has no compiled body for, instead of
    /// declining. The invented answer is the first `Int` argument, which is the
    /// shape a plausible bug takes — a registry that answered for the wrong
    /// entry point would look like this rather than like `Int(0)`.
    Unoffered,
    /// Runs the body with more fuel than the machine allowed instead of
    /// declining. `None` is unlimited; `Some(k)` is `k` times the budget, which
    /// is the bounded form — a native runaway with unlimited fuel has no bound
    /// at all and takes the process down with it.
    ExceedsBudget(Option<u32>),
    /// Answers this value for the target name whatever the machine asked, and
    /// whether or not a body exists. The only mutation that can accept a call
    /// the machine must decline — and it only ever gets the chance if the gate
    /// in `Machine::compiled_answer` is removed.
    Answers(Value),
}

impl Mutation {
    pub fn describe(&self) -> String {
        match self {
            Mutation::None => "nothing".to_string(),
            Mutation::OffByOne => "every `Int` answer is one too high".to_string(),
            Mutation::Inverted => "every `Bool` answer is inverted".to_string(),
            Mutation::Stale => "every answer is the previous call's".to_string(),
            Mutation::WrongType => "`Int` answers come back as `Bool` and back".to_string(),
            Mutation::Unoffered => {
                "names with no compiled body are answered rather than declined".to_string()
            }
            Mutation::ExceedsBudget(None) => {
                "the machine's call budget is ignored entirely".to_string()
            }
            Mutation::ExceedsBudget(Some(k)) => {
                format!("bodies run with {k}x the budget the machine allowed")
            }
            Mutation::Answers(v) => {
                format!("the target is answered `{}` unconditionally", v.render())
            }
        }
    }
}

/// Parses `--mutate`'s argument: `<mutation>[@<function>]`, plus `=<int>` for
/// the two that carry a number.
///
/// Named rather than positional so a run's provenance says what was broken:
/// `off-by-one@mcts.ucb` in a log is a reproducible experiment.
pub fn parse(spec: &str) -> Result<(Mutation, Option<String>)> {
    let (head, target) = match spec.split_once('@') {
        Some((head, target)) => (head, Some(target.to_string())),
        None => (spec, None),
    };
    let (kind, argument) = match head.split_once('=') {
        Some((kind, argument)) => (kind, Some(argument)),
        None => (head, None),
    };
    let mutation = match (kind, argument) {
        ("off-by-one", None) => Mutation::OffByOne,
        ("inverted", None) => Mutation::Inverted,
        ("stale", None) => Mutation::Stale,
        ("wrong-type", None) => Mutation::WrongType,
        ("unoffered", None) => Mutation::Unoffered,
        ("exceeds-budget", None) => Mutation::ExceedsBudget(None),
        ("exceeds-budget", Some(k)) => Mutation::ExceedsBudget(Some(k.parse()?)),
        ("answers", Some(v)) => Mutation::Answers(Value::Int(v.parse()?)),
        _ => bail!(
            "unknown mutation `{spec}`; one of off-by-one, inverted, stale, wrong-type, \
             unoffered, exceeds-budget[={{k}}], answers={{int}}, each optionally @function"
        ),
    };
    if matches!(mutation, Mutation::Answers(_)) && target.is_none() {
        bail!("`answers=` needs a target: `answers=302@effects.measured`");
    }
    Ok((mutation, target))
}

/// A backend that is wrong on purpose, wrapped around one that is not.
pub struct Mutant {
    inner: Rc<SpikeBodies>,
    mutation: Mutation,
    /// The one definition to corrupt, or every one of them. A whole-corpus
    /// mutation says whether the corpus notices *at all*; a targeted one says
    /// whether it notices *that function*, which is the coverage question.
    target: Option<Symbol>,
    offered: Cell<u64>,
    /// Offers of the target name specifically — the count that says whether a
    /// gate in `Machine::compiled_answer` held.
    offered_target: Cell<u64>,
    /// Calls whose answer this wrapper actually changed. A mutation that never
    /// fired proves nothing about the corpus that did not catch it, so every
    /// test asserts on this before it asserts on a disagreement.
    fired: Cell<u64>,
    previous: RefCell<Option<Value>>,
}

impl Mutant {
    fn build(inner: Rc<SpikeBodies>, mutation: Mutation, target: Option<Symbol>) -> Rc<Mutant> {
        Rc::new(Mutant {
            inner,
            mutation,
            target,
            offered: Cell::new(0),
            offered_target: Cell::new(0),
            fired: Cell::new(0),
            previous: RefCell::new(None),
        })
    }

    /// Corrupts every call the machine offers.
    pub fn new(inner: Rc<SpikeBodies>, mutation: Mutation) -> Rc<Mutant> {
        Mutant::build(inner, mutation, None)
    }

    /// Corrupts one definition and passes every other call through untouched.
    pub fn over(inner: Rc<SpikeBodies>, mutation: Mutation, target: &str) -> Rc<Mutant> {
        Mutant::build(inner, mutation, Some(Symbol::new(target)))
    }

    pub fn offered(&self) -> u64 {
        self.offered.get()
    }

    pub fn offered_target(&self) -> u64 {
        self.offered_target.get()
    }

    pub fn fired(&self) -> u64 {
        self.fired.get()
    }

    pub fn describe(&self) -> String {
        match &self.target {
            Some(name) => format!("{} (only `{name}`)", self.mutation.describe()),
            None => self.mutation.describe(),
        }
    }

    fn fire(&self, value: Value) -> Option<Value> {
        self.fired.set(self.fired.get() + 1);
        Some(value)
    }
}

impl Compiled for Mutant {
    fn describes(&self, program: &Program) -> bool {
        self.inner.describes(program)
    }

    fn enter(&self, name: &Symbol, args: &[Value], budget: usize) -> Option<Value> {
        self.offered.set(self.offered.get() + 1);
        let targeted = self.target.as_ref().is_none_or(|t| t == name);
        if targeted {
            self.offered_target.set(self.offered_target.get() + 1);
        } else {
            return self.inner.enter(name, args, budget);
        }

        // The two that answer without an honest answer to corrupt.
        match &self.mutation {
            Mutation::Answers(value) => return self.fire(value.clone()),
            Mutation::Unoffered if !self.inner.admits(name) => {
                let invented = args
                    .iter()
                    .find(|v| matches!(v, Value::Int(_)))
                    .cloned()
                    .unwrap_or(Value::Int(0));
                return self.fire(invented);
            }
            _ => {}
        }

        let honest = self.inner.enter(name, args, budget);
        match (&self.mutation, honest) {
            (Mutation::None | Mutation::Unoffered, answer) => answer,
            (Mutation::OffByOne, Some(Value::Int(n))) => self.fire(Value::Int(n.wrapping_add(1))),
            (Mutation::Inverted, Some(Value::Bool(b))) => self.fire(Value::Bool(!b)),
            (Mutation::WrongType, Some(Value::Int(n))) => self.fire(Value::Bool(n != 0)),
            (Mutation::WrongType, Some(Value::Bool(b))) => self.fire(Value::Int(i64::from(b))),
            (Mutation::Stale, Some(value)) => {
                let stale = self.previous.borrow_mut().replace(value.clone());
                match stale {
                    Some(stale) if stale.render() != value.render() => self.fire(stale),
                    // The first entry, or a repeat of the last answer: nothing to
                    // be stale about, so this call is honest and is not counted.
                    _ => Some(value),
                }
            }
            // A body that fits its budget answers the same either way; the
            // mutation is only visible where the honest backend declined.
            (Mutation::ExceedsBudget(times), None) => {
                let fuel = match times {
                    None => i64::MAX,
                    Some(k) => i64::try_from(budget)
                        .unwrap_or(i64::MAX)
                        .saturating_mul(i64::from(*k)),
                };
                match self.inner.call_direct(name.as_str(), args, fuel) {
                    Ok(value @ (Value::Int(_) | Value::Bool(_))) => self.fire(value),
                    _ => None,
                }
            }
            (_, answer) => answer,
        }
    }
}
