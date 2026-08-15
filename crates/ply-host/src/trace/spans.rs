//! Which spans each performer has open, and what closes one.
//!
//! **The program does not maintain the stack — the driver does, per task** — and
//! that is not a convenience, it is the only correct answer, because three of the
//! four ways a Ply computation leaves a span never run another line of it:
//!
//! | how the body leaves | what runs after `enter` |
//! | --- | --- |
//! | returns normally | the program's `exit` |
//! | `db.rollback` | **nothing** — the clause discards the continuation |
//! | a raise, or a spent budget | **nothing** — the raise propagates past |
//! | the entry point ends with the span open | **nothing** |
//!
//! So `exit` closes the span it names *and every span the same task opened above
//! it*, each of the latter `Abandoned`; and whatever is still open when an entry
//! point ends is closed by [`Spans::end_entry_point`], which is what `W0609`
//! reports.
//!
//! The key is [`Owner`] — the machine **and** the task — and it is the same key
//! `ply_host::db`'s scope table uses, for the same reason. Keyed on the task
//! alone, every entry point outside a scheduler region reports `None` and `ply
//! test`'s rayon workers all become one owner: one request's `exit` would close
//! another request's span, and one request's timing would land under another
//! request's span. Keyed on the machine alone, two tasks of one entry point
//! interleaving `enter`/`exit` would nest into each other. Both halves are
//! load-bearing and this module is where that is checked rather than hoped.

use super::Outcome;
use ply_core::ty::Resource;
use ply_eval::TaskId;
use ply_eval::host::MachineId;
use ply_span::{Diagnostic, Span, codes};
use std::collections::BTreeMap;

/// Who a span belongs to: the machine that opened it and the task inside it, if
/// any. `None` for the task is one identity rather than an absence of one — an
/// entry point that never spawned is a single thread of control.
pub type Owner = (MachineId, Option<TaskId>);

/// One span a performer has open.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Open {
    pub id: i64,
    /// The channel the `enter` named, kept so that a `close` can write the
    /// record on the channel the span was opened on rather than on whichever
    /// channel happened to close it.
    pub channel: Resource,
    /// The `Arc<str>` the argument already held, so keeping it allocates
    /// nothing. `W0609` names the innermost one, which is why a discarding sink
    /// keeps it too.
    pub name: std::sync::Arc<str>,
    pub parent: i64,
    /// When the sink stamped this span's `enter`. `None` when the sink did not
    /// want the record — under `--trace off`, or below `--trace-level` — in
    /// which case the `exit` it will not write carries no duration either.
    pub started: Option<i64>,
}

/// A span the driver has just popped, and what the sink should be told about it.
pub struct Closing {
    pub open: Open,
    pub outcome: Outcome,
}

/// Why an `exit` names a span that is not open on the performing task's stack.
///
/// Three cases and they are told apart exactly, because ids ascend from 1 within
/// an entry point and are minted only by `enter`: an id at or above *this entry
/// point's* next one was never opened, an id below it that is on none of this
/// entry point's stacks has been closed, and an id on another of this entry
/// point's stacks names both tasks.
///
/// Every one of those questions is asked of the performing machine's own table
/// and of nothing else. `E0445` is attributed and bisected like any other
/// program failure, so its text is what a `--json` failure object, a cached
/// failure report and a bisection carry — and a classification that consulted
/// the run would make one program's failure report a function of what a
/// footprint-disjoint entry point happened to be doing beside it.
pub enum Unbalanced {
    NeverOpened,
    AlreadyClosed,
    /// Open, but on another performer's stack. The dangerous one: accepting it
    /// is how one request's timing lands under another request's span.
    OtherOwner(Owner),
    /// Open on this stack under a different channel, which only a forged `Span`
    /// record can produce.
    OtherChannel(Resource),
}

/// Every span every entry point has open, and one id counter **per entry
/// point**.
///
/// One table per driver, because `ply test` runs a machine per worker over one
/// driver and a teardown has to reach exactly one of them. One *counter* per
/// machine, because an id is a value that crosses back into the program: a
/// `Span` is an ordinary record precisely so that a program can put its id in a
/// field, and a run-global counter therefore made a program's own answer a
/// function of how much tracing a footprint-disjoint entry point had done beside
/// it. That is W4's pooled connection with a different noun — a host state
/// shared across two computations the conflict graph believes are disjoint —
/// and the fix is the same shape: give the state the identity that should have
/// scoped it, rather than serialising the tests that expose it.
///
/// What that costs, stated: two entry points of one run may both hold span `1`,
/// so an id is unique **within an entry point** and a reader correlating two
/// lines across entry points needs the record's `seq`, which is the sink's and
/// stays run-global. For a service that is no change at all — `desk.ply` serves
/// every request from one entry point — and for `ply test` it is the difference
/// between a verdict that reproduces and one that does not.
pub struct Spans {
    open: BTreeMap<Owner, Vec<Open>>,
    /// The next id each entry point will mint, absent until its first `enter`
    /// and dropped by [`Spans::end_entry_point`] so a suite of ten thousand
    /// tests does not carry ten thousand counters.
    next: BTreeMap<MachineId, i64>,
    /// Every span this driver has opened, and how many of them were closed
    /// `Abandoned`. Read by the shutdown banner, which is a fact the run already
    /// holds rather than something computed for it. Run-level on purpose: a
    /// banner counts the run, and §7's rule is that nothing per-test may be
    /// built on top of these.
    opened: u64,
    abandoned: u64,
}

impl Default for Spans {
    fn default() -> Spans {
        Spans::new()
    }
}

impl Spans {
    pub fn new() -> Spans {
        Spans {
            open: BTreeMap::new(),
            next: BTreeMap::new(),
            opened: 0,
            abandoned: 0,
        }
    }

    pub fn opened(&self) -> u64 {
        self.opened
    }

    pub fn abandoned(&self) -> u64 {
        self.abandoned
    }

    /// How many spans `owner` has open. For a test, and for the "0 transactions
    /// open"-shaped line the shutdown banner prints.
    pub fn depth(&self, owner: Owner) -> usize {
        self.open.get(&owner).map_or(0, Vec::len)
    }

    pub fn total_open(&self) -> usize {
        self.open.values().map(Vec::len).sum()
    }

    /// The span an event or a metric performed by `owner` belongs to, and that
    /// span's parent. `(0, 0)` outside any span, which is unambiguous because
    /// ids ascend from 1.
    pub fn innermost(&self, owner: Owner) -> (i64, i64) {
        self.open
            .get(&owner)
            .and_then(|stack| stack.last())
            .map_or((0, 0), |open| (open.id, open.parent))
    }

    /// `trace.enter[c]`. Answers the span it opened, whose `parent` is whatever
    /// this owner had innermost.
    pub fn enter(
        &mut self,
        owner: Owner,
        channel: Resource,
        name: std::sync::Arc<str>,
        started: Option<i64>,
    ) -> Open {
        // From 1 per entry point, so `0` in a record's `span` or `parent` is
        // unambiguously "no span" rather than a span that might exist.
        let next = self.next.entry(owner.0).or_insert(1);
        let id = *next;
        *next += 1;
        let stack = self.open.entry(owner).or_default();
        let parent = stack.last().map_or(0, |open| open.id);
        let open = Open {
            id,
            channel,
            name,
            parent,
            started,
        };
        self.opened += 1;
        stack.push(open.clone());
        open
    }

    /// `trace.exit[c]`, which closes `id` **and every span this owner opened
    /// above it**, innermost first.
    ///
    /// The abandoned ones are not a warning: a discarded continuation is what a
    /// rollback *is*, so the spans between the rollback and the scope that
    /// answered it never ran their own `exit`, and there is nothing else they
    /// could be.
    pub fn exit(
        &mut self,
        owner: Owner,
        id: i64,
        channel: &Resource,
        outcome: Outcome,
    ) -> Result<Vec<Closing>, Unbalanced> {
        let stack: &[Open] = self.open.get(&owner).map_or(&[], Vec::as_slice);
        let Some(at) = stack.iter().position(|open| open.id == id) else {
            return Err(self.why(owner, id));
        };
        if stack[at].channel != *channel {
            return Err(Unbalanced::OtherChannel(stack[at].channel.clone()));
        }
        let stack = self
            .open
            .get_mut(&owner)
            .expect("the position came from it");
        // Drained rather than removed. A request that opens and closes one span
        // per iteration would otherwise pay a map node and a fresh `Vec` every
        // time, and the entry is bounded by the entry point's life:
        // `end_entry_point` takes every one of this machine's, empty or not.
        let popped: Vec<Open> = stack.drain(at..).collect();
        // Outermost first in `popped`, and the outermost is the span the program
        // named — everything after it was opened above it and never ran its own
        // `exit`. Reversed on the way out, because a reader of the records wants
        // the innermost first: that is the one the computation was actually
        // inside when it stopped.
        let closings: Vec<Closing> = popped
            .into_iter()
            .enumerate()
            .rev()
            .map(|(i, open)| Closing {
                open,
                outcome: if i == 0 {
                    outcome.clone()
                } else {
                    Outcome::Abandoned
                },
            })
            .collect();
        self.abandoned += (closings.len() - 1) as u64;
        Ok(closings)
    }

    /// Every span this machine still has open, innermost first per task, and the
    /// table emptied of them.
    ///
    /// Called on **every** exit path from an entry point — a value, a
    /// diagnostic, or a spent budget — which is the whole of its value, because
    /// the exits it exists for are the three where the program stopped without
    /// reaching its own `exit`.
    ///
    /// This machine's spans and no others: `ply test` runs a machine per worker
    /// over one driver, and a teardown that emptied the table would close a span
    /// another entry point is still writing into.
    pub fn end_entry_point(&mut self, machine: MachineId) -> Vec<Closing> {
        // With the entry point goes its counter. Nothing can mint an id under it
        // again, and a suite that kept one per test would carry a counter per
        // test for the life of the run.
        self.next.remove(&machine);
        let mine: Vec<Owner> = self
            .open
            .keys()
            .filter(|(owner, _)| *owner == machine)
            .copied()
            .collect();
        let mut closings = Vec::new();
        for owner in mine {
            let Some(stack) = self.open.remove(&owner) else {
                continue;
            };
            for open in stack.into_iter().rev() {
                self.abandoned += 1;
                closings.push(Closing {
                    open,
                    outcome: Outcome::Abandoned,
                });
            }
        }
        closings
    }

    /// Which of the three ways an `exit` named a span that is not on this
    /// owner's stack, decided from the performing entry point's own table.
    ///
    /// Both questions are scoped to `owner.0`: the counter is this machine's, so
    /// "never opened" means *this* program never opened it; and the search for
    /// another holder is over this machine's other tasks, so `OtherOwner` names
    /// a task of the same program rather than a `MachineId` belonging to a test
    /// running beside this one.
    fn why(&self, owner: Owner, id: i64) -> Unbalanced {
        let next = self.next.get(&owner.0).copied().unwrap_or(1);
        if id < 1 || id >= next {
            return Unbalanced::NeverOpened;
        }
        match self.open.iter().find(|(other, stack)| {
            other.0 == owner.0 && **other != owner && stack.iter().any(|open| open.id == id)
        }) {
            Some((other, _)) => Unbalanced::OtherOwner(*other),
            None => Unbalanced::AlreadyClosed,
        }
    }
}

/// `E0445` — a `trace.exit` naming a span that is not open on the performing
/// task's stack.
///
/// The program's fault, and **not** reserved: it is a refusal the trace driver
/// is the only component in a position to compute, because which task holds
/// which span is state that exists nowhere else. Reserving it would have
/// `attribute` rewrite the driver's own diagnosis to `E0502` and send a reader
/// looking for a defect in Ply.
#[cold]
#[inline(never)]
pub fn err_unbalanced(span: Span, operation: &str, id: i64, why: &Unbalanced) -> Diagnostic {
    let diagnostic = Diagnostic::error(
        codes::SPAN_UNBALANCED,
        format!("{operation} names span {id}, which is not open on this task"),
    )
    .primary(span, "this span is not open here");
    let diagnostic = match why {
        Unbalanced::NeverOpened => diagnostic
            .note("no `trace.enter` in this entry point has answered that id")
            .note("a `Span` is an ordinary record, so one can be built rather than entered; pass the value `trace.enter` answered"),
        Unbalanced::AlreadyClosed => diagnostic
            .note("it was closed by an earlier `trace.exit`, or by the `trace.exit` of a span below it")
            .note("closing a span closes every span opened above it, so an inner `exit` after an outer one has nothing left to close"),
        Unbalanced::OtherOwner(other) => diagnostic
            .note(format!("it is open on {}", describe(*other)))
            .note("accepting it would put this task's timing under the other task's span, which is a wrong answer about what happened rather than a missing one")
            .note("open and close a span in one task, and pass what it measured across the boundary as a field"),
        Unbalanced::OtherChannel(channel) => diagnostic
            .note(format!("it is open on channel `{}`", label(channel)))
            .note("a channel is part of a span's identity because it is part of the atom the row carries"),
    };
    diagnostic.note("silently accepting an unbalanced exit is how one request's timing lands under another request's span")
}

/// `W0609` — spans were still open when an entry point ended.
///
/// A warning rather than an error: the records are written either way, and the
/// `Abandoned` outcome on them is the signal. What the count is for is that a
/// program leaking a span per request leaks one per request forever, and the
/// only place that is visible is here.
#[cold]
#[inline(never)]
pub fn warn_abandoned(closings: &[Closing]) -> Diagnostic {
    let innermost = closings
        .first()
        .map(|c| c.open.name.to_string())
        .unwrap_or_default();
    Diagnostic::warning(
        codes::SPAN_ABANDONED,
        format!(
            "{} {} still open when the entry point ended; the innermost was `{innermost}`",
            closings.len(),
            if closings.len() == 1 { "span was" } else { "spans were" }
        ),
    )
    .note("each was closed `Abandoned` and written, so the last records a dying computation produced say what it was doing")
    .note("a body that raises, or that rolls back, never reaches its own `trace.exit`; close the span where the value comes out, or read the `Abandoned` records as the report")
}

fn describe(owner: Owner) -> String {
    match owner.1 {
        Some(task) => format!("task {task} of {}", owner.0),
        None => format!("the entry point of {}", owner.0),
    }
}

pub fn label(resource: &Resource) -> &str {
    match resource {
        Resource::Named(name) => name.as_str(),
        Resource::Singleton => "",
    }
}

#[cfg(test)]
mod tests;
