use crate::{Diagnostic, Span, codes};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Handle {
    Continuation,
    Task,
    Channel,
}

impl Handle {
    fn noun(self) -> &'static str {
        match self {
            Handle::Continuation => "a continuation",
            Handle::Task => "a `Task`",
            Handle::Channel => "a `Chan`",
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Handle::Continuation => {
                "a continuation resumes a body that lives only as long as the entry that captured \
                 it"
            }
            Handle::Task | Handle::Channel => {
                "a task or a channel is a key into the scheduler that made it, which ends before the \
                 entry does"
            }
        }
    }

    fn unseen(self) -> &'static str {
        match self {
            Handle::Continuation => {
                "no type records that a function is a continuation, so it is rejected at the \
                 boundary instead of wherever it would be resumed"
            }
            Handle::Task | Handle::Channel => {
                "a value may carry it out of its scheduler, which no check stops, so it is rejected \
                 at the boundary instead of wherever it would be joined"
            }
        }
    }

    fn instead(self) -> &'static str {
        match self {
            Handle::Continuation => {
                "resume the continuation before the entry returns, and return what it \
                 returns"
            }
            Handle::Task | Handle::Channel => {
                "join the task, or drain the channel, inside the scheduler that made it, and \
                 return what that gives"
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EscapedHandle {
    pub handle: Handle,
    pub route: Vec<String>,
}

impl EscapedHandle {
    pub fn reached(&self) -> String {
        if self.route.is_empty() {
            return String::new();
        }
        format!(", reached through {}", self.route.join(" → "))
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Boundary<'a> {
    ReturnedFromEntry { name: &'a str },
}

impl Boundary<'_> {
    fn headline(&self, handle: Handle, reached: &str) -> String {
        let what = handle.noun();
        match self {
            Boundary::ReturnedFromEntry { name } => format!("`{name}` returned {what}{reached}"),
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Boundary::ReturnedFromEntry { .. } => "returned here",
        }
    }

    fn outlives(&self) -> &'static str {
        match self {
            Boundary::ReturnedFromEntry { .. } => {
                "an entry point's result goes to its caller, which keeps it after the entry has \
                 ended and closed every scheduler it opened"
            }
        }
    }

    fn remedy(&self, handle: Handle) -> &'static str {
        match self {
            Boundary::ReturnedFromEntry { .. } => handle.instead(),
        }
    }
}

#[cold]
#[inline(never)]
pub fn rejected(boundary: &Boundary<'_>, escapee: &EscapedHandle, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::SCHEDULER_ESCAPE_AT_BOUNDARY,
        boundary.headline(escapee.handle, &escapee.reached()),
    )
    .primary(span, boundary.label())
    .note(escapee.handle.reason())
    .note(boundary.outlives())
    .note(escapee.handle.unseen())
    .note(boundary.remedy(escapee.handle))
}
