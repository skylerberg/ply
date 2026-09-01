//! Which spans each performer has open, and what closes one.

use super::Outcome;
use ply_core::ty::Resource;
use ply_eval::TaskId;
use ply_eval::host::MachineId;
use ply_span::{Diagnostic, Span, codes};
use std::collections::BTreeMap;

/// Who a span belongs to: the machine that opened it and the task inside it, if any.
pub type Owner = (MachineId, Option<TaskId>);

/// One span a performer has open.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Open {
    pub id: i64,
    /// The channel the `enter` named, kept so that a `close` can write the record on the channel
    /// the span was opened on rather than on whichever channel happened to close it.
    pub channel: Resource,
    /// The `Arc<str>` the argument already held, so keeping it allocates nothing.
    pub name: std::sync::Arc<str>,
    pub parent: i64,
    /// When the sink stamped this span's `enter`.
    pub started: Option<i64>,
}

/// A span the driver has just popped, and what the sink should be told about it.
pub struct Closing {
    pub open: Open,
    pub outcome: Outcome,
}

/// Why an `exit` names a span that is not open on the performing task's stack.
pub enum Unbalanced {
    NeverOpened,
    AlreadyClosed,
    /// Open, but on another performer's stack.
    OtherOwner(Owner),
    /// Open on this stack under a different channel, which only a forged `Span` record can produce.
    OtherChannel(Resource),
}

/// Every span every entry point has open, and one id counter **per entry point**.
pub struct Spans {
    open: BTreeMap<Owner, Vec<Open>>,
    /// The next id each entry point will mint, absent until its first `enter` and dropped by
    /// [`Spans::end_entry_point`] so a suite of ten thousand tests does not carry ten thousand
    /// counters.
    next: BTreeMap<MachineId, i64>,
    /// Every span this driver has opened, and how many of them were closed `Abandoned`.
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

    /// How many spans `owner` has open.
    pub fn depth(&self, owner: Owner) -> usize {
        self.open.get(&owner).map_or(0, Vec::len)
    }

    pub fn total_open(&self) -> usize {
        self.open.values().map(Vec::len).sum()
    }

    /// The span an event or a metric performed by `owner` belongs to, and that span's parent.
    pub fn innermost(&self, owner: Owner) -> (i64, i64) {
        self.open
            .get(&owner)
            .and_then(|stack| stack.last())
            .map_or((0, 0), |open| (open.id, open.parent))
    }

    /// `trace.enter[c]`.
    pub fn enter(
        &mut self,
        owner: Owner,
        channel: Resource,
        name: std::sync::Arc<str>,
        started: Option<i64>,
    ) -> Open {
        // From 1 per entry point, so `0` in a record's `span` or `parent` is unambiguously "no
        // span" rather than a span that might exist.
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

    /// `trace.exit[c]`, which closes `id` **and every span this owner opened above it**, innermost
    /// first.
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
        // Drained rather than removed.
        let popped: Vec<Open> = stack.drain(at..).collect();
        // Outermost first in `popped`, and the outermost is the span the program named — everything
        // after it was opened above it and never ran its own `exit`.
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

    /// Every span this machine still has open, innermost first per task, and the table emptied of
    /// them.
    pub fn end_entry_point(&mut self, machine: MachineId) -> Vec<Closing> {
        // With the entry point goes its counter.
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

    /// Which of the three ways an `exit` named a span that is not on this owner's stack, decided
    /// from the performing entry point's own table.
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

/// `E0445` — a `trace.exit` naming a span that is not open on the performing task's stack.
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
