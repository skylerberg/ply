use crate::{Diagnostic, Span, Symbol, codes};
use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Determinism {
    Deterministic,
    Nondeterministic,
}

impl Determinism {
    pub fn is_deterministic(self) -> bool {
        self == Determinism::Deterministic
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct MachineId(pub u64);

impl MachineId {
    pub fn next() -> MachineId {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        MachineId(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

impl fmt::Display for MachineId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "machine #{}", self.0)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Pending {
    pub token: u64,
    pub label: &'static str,
}

impl fmt::Display for Pending {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (#{})", self.label, self.token)
    }
}

pub type RuntimeFactory = std::sync::Arc<dyn Fn() -> std::rc::Rc<dyn HostRuntime> + Send + Sync>;

pub trait HostRuntime {
    fn park(&self) -> Result<(), Diagnostic>;

    fn facilities(&self) -> Option<&dyn std::any::Any> {
        None
    }

    fn now(&self) -> Result<i64, Diagnostic> {
        Err(err_no_clock("read the clock"))
    }

    fn park_until(&self, deadline: i64) -> Result<(), Diagnostic> {
        let _ = deadline;
        Err(err_no_clock("wait on the clock"))
    }

    fn end_entry_point(&self, machine: MachineId) -> Vec<Diagnostic> {
        let _ = machine;
        Vec::new()
    }

    fn end_task(&self, machine: MachineId, task: crate::sim::TaskId) {
        let _ = (machine, task);
    }

    fn stopping(&self) -> bool {
        false
    }

    fn drain_expired(&self) -> Option<Diagnostic> {
        None
    }

    fn shutdown(&self) -> ShutdownReport {
        ShutdownReport::default()
    }

    fn unhandled(&self, effect: &Symbol, op: &Symbol) {
        let _ = (effect, op);
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ShutdownReport {
    pub spans_left_open: usize,
}

#[derive(Debug, Default)]
pub struct HostBinding {
    granted: bool,
    grants: Grants,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grants {
    pub entries: Vec<(String, Vec<String>)>,
}

impl Grants {
    pub fn named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a [String]> + 'a {
        (self.entries.iter())
            .filter(move |(n, _)| n == name)
            .map(|(_, fields)| fields.as_slice())
    }

    pub fn has(&self, name: &str) -> bool {
        self.named(name).next().is_some()
    }

    pub fn with(mut self, name: &str, fields: &[&str]) -> Grants {
        self.entries.push((
            name.to_string(),
            fields.iter().map(|f| f.to_string()).collect(),
        ));
        self
    }

    pub fn allows(&self, family: &str) -> bool {
        self.named("allow")
            .any(|fields| fields.first().is_some_and(|f| f == family))
    }
}

impl HostBinding {
    pub fn unbound() -> HostBinding {
        HostBinding::default()
    }

    pub fn granted(grants: Grants) -> HostBinding {
        HostBinding {
            granted: true,
            grants,
        }
    }

    pub fn is_granted(&self) -> bool {
        self.granted
    }

    pub fn grants(&self) -> &Grants {
        &self.grants
    }
}

const _: fn() = || {
    fn shareable<T: Send + Sync>() {}
    shareable::<HostBinding>();
};

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct HostUse {
    pub operations: u64,
    pub nondeterministic: u64,
}

impl HostUse {
    pub fn is_empty(&self) -> bool {
        self.operations == 0
    }

    pub fn used_nondeterminism(&self) -> bool {
        self.nondeterministic > 0
    }

    pub fn record(&mut self, determinism: Determinism) {
        self.operations += 1;
        if !determinism.is_deterministic() {
            self.nondeterministic += 1;
        }
    }

    pub fn absorb(&mut self, other: &HostUse) {
        self.operations += other.operations;
        self.nondeterministic += other.nondeterministic;
    }
}

pub fn operation_label(effect: &Symbol, op: &Symbol, resource: Option<&Symbol>) -> String {
    match resource {
        Some(r) => format!("{effect}.{op}[{r}]"),
        None => format!("{effect}.{op}"),
    }
}

#[cold]
#[inline(never)]
pub fn err_host_in_search(span: Span, operation: &str, path: &'static str) -> Diagnostic {
    Diagnostic::error(
        codes::HOST_IN_SIMULATION,
        format!("`{operation}` reached the host boundary in a test the search re-runs"),
    )
    .primary(span, "performed here, against a real resource")
    .note("this test reads a simulation seed, so the search runs it whole once per interleaving it explores")
    .note(format!(
        "`{path}` would therefore be called once per schedule, and the run would report the total as a proof over all of them"
    ))
    .note("handle the operation with a test double, or move it out of a test that simulates")
    .note("`--sim once` runs a single interleaving, which is the one search a host-backed test may have")
}

#[cold]
#[inline(never)]
pub fn err_host_in_simulation(span: Span, operation: &str, scheduler: Span) -> Diagnostic {
    Diagnostic::error(
        codes::HOST_IN_SIMULATION,
        format!("`{operation}` reached the host boundary from inside a `simulate` block"),
    )
    .primary(span, "performed here, against a real resource")
    .secondary(scheduler, "this simulation re-runs its body once per interleaving")
    .note("the search runs the simulation whole for every schedule it explores, so this operation would be performed once per interleaving")
    .note("and the result would then be reported as a proof over every interleaving")
    .note("handle the operation with a test double inside the simulation, or hoist it out of the simulation entirely")
}

#[cold]
#[inline(never)]
fn err_no_clock(what: &str) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("this run's host runtime cannot {what}"),
    )
    .note("`clock.now` reads the clock a run's `HostRuntime` keeps, and a production scheduler's sleeps are deadlines on it, so a runtime that keeps none handles neither")
}

#[cold]
#[inline(never)]
pub fn err_unhandled_operation(span: Span, operation: &str) -> Diagnostic {
    Diagnostic::error(
        codes::HOST_UNBOUND,
        format!("`{operation}` reached the host boundary, and nothing serves it"),
    )
    .primary(span, "no handler here, and no host serves this effect")
    .note(format!("handle `{operation}` in the program"))
}

#[cold]
#[inline(never)]
pub fn err_unbound_operation(
    span: Span,
    operation: &str,
    effect: &Symbol,
    path: &'static str,
    served: &'static str,
) -> Diagnostic {
    let module = effect
        .as_str()
        .rsplit_once('.')
        .map(|(module, _)| module.to_string())
        .unwrap_or_else(|| effect.to_string());
    Diagnostic::error(
        codes::HOST_UNBOUND,
        format!("`{operation}` reached the host boundary in a run that binds no handler for it"),
    )
    .primary(span, "no handler here, and this run bound none")
    .note(format!("`{path}` serves this {served}"))
    .note(format!(
        "handle `{operation}` over `{module}`'s fake, which is what makes a test that reads it `det`, cached and hermetic"
    ))
}
