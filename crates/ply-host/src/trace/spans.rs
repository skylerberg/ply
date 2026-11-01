use super::Outcome;
use ply_core::host::MachineId;
use ply_core::{Diagnostic, Resource, Span, TaskId, codes};
use std::collections::BTreeMap;

pub type Owner = (MachineId, Option<TaskId>);

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct OpenSpan {
    pub id: i64,
    pub channel: Resource,
    pub name: std::sync::Arc<str>,
    pub parent: i64,
    pub started: Option<i64>,
}

pub struct Closing {
    pub open: OpenSpan,
    pub outcome: Outcome,
}

#[derive(Debug)]
pub enum Unbalanced {
    NeverOpened,
    AlreadyClosed,
    OtherOwner(Owner),
    OtherChannel(Resource),
}

pub const NAMED: usize = 3;

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Abandoned {
    pub count: u64,
    pub first: Vec<(std::sync::Arc<str>, Resource)>,
}

impl Abandoned {
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    fn note(&mut self, open: &OpenSpan) {
        self.count += 1;
        if self.first.len() < NAMED {
            self.first
                .push((std::sync::Arc::clone(&open.name), open.channel.clone()));
        }
    }

    fn then(mut self, later: Abandoned) -> Abandoned {
        self.count += later.count;
        let room = NAMED.saturating_sub(self.first.len());
        self.first.extend(later.first.into_iter().take(room));
        self
    }
}

pub struct Spans {
    open: BTreeMap<Owner, Vec<OpenSpan>>,
    next: BTreeMap<MachineId, i64>,
    retired: BTreeMap<MachineId, Abandoned>,
    opened: u64,
    abandoned: u64,
    left_open: u64,
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
            retired: BTreeMap::new(),
            opened: 0,
            abandoned: 0,
            left_open: 0,
        }
    }

    pub fn opened(&self) -> u64 {
        self.opened
    }

    pub fn abandoned(&self) -> u64 {
        self.abandoned
    }

    pub fn left_open(&self) -> u64 {
        self.left_open
    }

    pub fn depth(&self, owner: Owner) -> usize {
        self.open.get(&owner).map_or(0, Vec::len)
    }

    pub fn total_open(&self) -> usize {
        self.open.values().map(Vec::len).sum()
    }

    pub fn innermost(&self, owner: Owner) -> (i64, i64) {
        self.open
            .get(&owner)
            .and_then(|stack| stack.last())
            .map_or((0, 0), |open| (open.id, open.parent))
    }

    pub fn enter(
        &mut self,
        owner: Owner,
        channel: Resource,
        name: std::sync::Arc<str>,
        started: Option<i64>,
    ) -> OpenSpan {
        let next = self.next.entry(owner.0).or_insert(1);
        let id = *next;
        *next += 1;
        let stack = self.open.entry(owner).or_default();
        let parent = stack.last().map_or(0, |open| open.id);
        let open = OpenSpan {
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

    pub fn exit(
        &mut self,
        owner: Owner,
        id: i64,
        channel: &Resource,
        outcome: Outcome,
    ) -> Result<Vec<Closing>, Unbalanced> {
        let stack: &[OpenSpan] = self.open.get(&owner).map_or(&[], Vec::as_slice);
        let Some(at) = stack.iter().position(|open| open.id == id) else {
            return Err(self.reason(owner, id));
        };
        if stack[at].channel != *channel {
            return Err(Unbalanced::OtherChannel(stack[at].channel.clone()));
        }
        let stack = self
            .open
            .get_mut(&owner)
            .expect("the position came from it");
        let popped: Vec<OpenSpan> = stack.drain(at..).collect();
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

    pub fn end_entry_point(&mut self, machine: MachineId) -> (Vec<Closing>, Abandoned) {
        self.next.remove(&machine);
        let mine: Vec<Owner> = self
            .open
            .keys()
            .filter(|(owner, _)| *owner == machine)
            .copied()
            .collect();
        let mut closings = Vec::new();
        for owner in mine {
            closings.extend(self.end_owner(owner));
        }
        let mut abandoned = Abandoned::default();
        for closing in &closings {
            abandoned.note(&closing.open);
        }
        let retired = self.retired.remove(&machine).unwrap_or_default();
        let abandoned = abandoned.then(retired);
        self.left_open += abandoned.count;
        (closings, abandoned)
    }

    pub fn end_task(&mut self, machine: MachineId, task: TaskId) -> Vec<Closing> {
        let closings = self.end_owner((machine, Some(task)));
        if !closings.is_empty() {
            let noted = self.retired.entry(machine).or_default();
            for closing in &closings {
                noted.note(&closing.open);
            }
        }
        closings
    }

    pub fn retired(&self, machine: MachineId) -> Option<&Abandoned> {
        self.retired.get(&machine)
    }

    fn end_owner(&mut self, owner: Owner) -> Vec<Closing> {
        let Some(stack) = self.open.remove(&owner) else {
            return Vec::new();
        };
        self.abandoned += stack.len() as u64;
        stack
            .into_iter()
            .rev()
            .map(|open| Closing {
                open,
                outcome: Outcome::Abandoned,
            })
            .collect()
    }

    pub fn owners(&self) -> usize {
        self.open.len()
    }

    fn reason(&self, owner: Owner, id: i64) -> Unbalanced {
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

#[cold]
#[inline(never)]
pub fn err_unbalanced(span: Span, operation: &str, id: i64, reason: &Unbalanced) -> Diagnostic {
    let diagnostic = Diagnostic::error(
        codes::SPAN_UNBALANCED,
        format!("{operation} names span {id}, which is not open on this task"),
    )
    .primary(span, "this span is not open here");
    let diagnostic = match reason {
        Unbalanced::NeverOpened => diagnostic
            .note("no `trace.enter` in this entry point has returned that id")
            .note("a `Span` is an ordinary record, so one can be built rather than entered; pass the value `trace.enter` returned"),
        Unbalanced::AlreadyClosed => diagnostic
            .note("it was closed by an earlier `trace.exit`, or by the `trace.exit` of a span below it")
            .note("closing a span closes every span opened above it, so an inner `exit` after an outer one has nothing left to close"),
        Unbalanced::OtherOwner(other) => diagnostic
            .note(format!("it is open on {}", describe(*other)))
            .note("accepting it would put this task's timing under the other task's span, which is a wrong answer about what happened rather than a missing one")
            .note("open and close a span in one task, and pass what it measured across the boundary as a field"),
        Unbalanced::OtherChannel(channel) => diagnostic
            .note(format!("it is open on channel `{}`", label(channel)))
            .note("a channel is part of a span's identity, because the trace records it as part of the span's effect"),
    };
    diagnostic.note("silently accepting an unbalanced exit is how one request's timing lands under another request's span")
}

#[cold]
#[inline(never)]
pub fn warn_abandoned(abandoned: &Abandoned) -> Diagnostic {
    let mut named: Vec<String> = abandoned
        .first
        .iter()
        .map(|(name, channel)| format!("`{name}` on `{}`", label(channel)))
        .collect();
    let unnamed = abandoned.count.saturating_sub(abandoned.first.len() as u64);
    if unnamed > 0 {
        named.push(format!("{unnamed} more"));
    }
    let (spans, whose) = if abandoned.count == 1 {
        ("span was", "its task")
    } else {
        ("spans were", "their task")
    };
    Diagnostic::warning(
        codes::SPAN_ABANDONED,
        format!(
            "{} {spans} still open when {whose} or the entry point ended: {}",
            abandoned.count,
            named.join(", ")
        ),
    )
    .note("each was closed `Abandoned` and written, so the last records a dying computation produced say what it was doing")
    .note("a body that raises, or that rolls back, never reaches its own `trace.exit`; close the span where the value comes out, or read the `Abandoned` records as the report")
    .note("a task's spans close when it retires: once it has finished and no handle to it is left")
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
        Resource::Var(_) | Resource::Singleton => "",
        Resource::Every => "*",
    }
}
