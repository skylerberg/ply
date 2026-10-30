//! The host effect boundary: what a run is lent, how much of the host it asked, the runtime a
//! machine waits on the host through, and the refusals of an operation that reaches it.

use crate::{Diagnostic, Span, Symbol, codes};
use std::fmt;

/// Whether a handler may serve an effect the program did not declare `nondet`.
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

/// Opaque to `ply-core`: minted and polled by a [`HostRuntime`].
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

/// Makes a reactor for the thread that calls it: one belongs to its thread, and a machine, a test
/// worker or a `parallel` branch each drive their own.
pub type RuntimeFactory = std::sync::Arc<dyn Fn() -> std::rc::Rc<dyn HostRuntime> + Send + Sync>;

pub trait HostRuntime {
    /// Waits until at least one ticket the run watches is answered.
    fn park(&self) -> Result<(), Diagnostic>;

    /// What the natives of the host handlers reach the machine through: the run's sockets, its
    /// configuration and the rest the host made, which only the crate that made them names. `None`
    /// for a runtime that holds none.
    fn facilities(&self) -> Option<&dyn std::any::Any> {
        None
    }

    /// Nanoseconds the host's monotonic clock reads, which `clock.now` answers and the run's
    /// region's sleeps are deadlines on; only the difference of two readings means anything.
    fn now(&self) -> Result<i64, Diagnostic> {
        Err(err_no_clock("read the clock"))
    }

    /// Waits until a watched ticket is answered or the clock reads `deadline`, whichever is first.
    fn park_until(&self, deadline: i64) -> Result<(), Diagnostic> {
        let _ = deadline;
        Err(err_no_clock("wait on the clock"))
    }

    /// Called on every exit path from an entry point, before the machine resets; answers what its
    /// ending warns of.
    fn end_entry_point(&self, machine: MachineId) -> Vec<Diagnostic> {
        let _ = machine;
        Vec::new()
    }

    /// Called when the run's region lets go of `task`, whose id nothing will name again.
    fn end_task(&self, machine: MachineId, task: crate::sim::TaskId) {
        let _ = (machine, task);
    }

    fn stopping(&self) -> bool {
        false
    }

    fn drain_expired(&self) -> Option<Diagnostic> {
        None
    }

    /// Called once, after the last entry point, before the process exits.
    fn shutdown(&self) -> ShutdownReport {
        ShutdownReport::default()
    }

    /// Called with a raise no clause answers, before the entry fails with it: a host whose own
    /// the raise is may end the run its way here and never return.
    fn unanswered(&self, effect: &Symbol, op: &Symbol) {
        let _ = (effect, op);
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ShutdownReport {
    /// Spans left open when their task or entry point ended: the sum of what each `W0609` counted.
    pub spans_left_open: usize,
}

/// A run's grants; the default, [`HostBinding::unbound`], is a run lent nothing.
#[derive(Debug, Default)]
pub struct HostBinding {
    /// Whether a run's grants made it.
    granted: bool,
    grants: Grants,
}

/// What a run is lent, as named entries, each a list of fields: the natives of the host handlers
/// read the ones they serve (`fs NAME PATH`, `exec NAME PATH`, `allow FAMILY`, `argv ARG..`, ...).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grants {
    pub entries: Vec<(String, Vec<String>)>,
}

impl Grants {
    /// The fields of each entry named `name`, in order.
    pub fn named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a [String]> + 'a {
        (self.entries.iter())
            .filter(move |(n, _)| n == name)
            .map(|(_, fields)| fields.as_slice())
    }

    /// Whether an entry is named `name`.
    pub fn has(&self, name: &str) -> bool {
        self.named(name).next().is_some()
    }

    /// The grants with one entry more.
    pub fn with(mut self, name: &str, fields: &[&str]) -> Grants {
        self.entries.push((
            name.to_string(),
            fields.iter().map(|f| f.to_string()).collect(),
        ));
        self
    }

    /// Whether `allow` names the family `family`.
    pub fn allows(&self, family: &str) -> bool {
        self.named("allow")
            .any(|fields| fields.first().is_some_and(|f| f == family))
    }
}

impl HostBinding {
    /// What a context entered with no binding has: a run lent nothing.
    pub fn unbound() -> HostBinding {
        HostBinding::default()
    }

    /// The binding of a run lent `grants`, which the natives read.
    pub fn granted(grants: Grants) -> HostBinding {
        HostBinding {
            granted: true,
            grants,
        }
    }

    /// Whether a run's grants made this binding.
    pub fn is_granted(&self) -> bool {
        self.granted
    }

    /// What the run this binding serves is lent.
    pub fn grants(&self) -> &Grants {
        &self.grants
    }
}

/// A `parallel` block's branches share one binding, so it must stay `Send + Sync`.
const _: fn() = || {
    fn shareable<T: Send + Sync>() {}
    shareable::<HostBinding>();
};

/// How much of the host a run asked: every operation and native answered, and how many of those
/// a run asking again may be answered otherwise.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct HostUse {
    /// Every host operation answered, `Repeatable` ones included.
    pub operations: u64,
    pub nondeterministic: u64,
}

impl HostUse {
    pub fn is_empty(&self) -> bool {
        self.operations == 0
    }

    pub fn acted(&self) -> bool {
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
pub fn err_host_in_simulation(span: Span, operation: &str, region: Span) -> Diagnostic {
    Diagnostic::error(
        codes::HOST_IN_SIMULATION,
        format!("`{operation}` reached the host boundary from inside a `simulate` region"),
    )
    .primary(span, "performed here, against a real resource")
    .secondary(region, "this region re-runs its body once per interleaving")
    .note("the search runs the region whole for every schedule it explores, so this operation would be performed once per interleaving")
    .note("and the result would then be reported as a proof over every interleaving")
    .note("handle the operation with a test double inside the region, or hoist it out of the region entirely")
}

#[cold]
#[inline(never)]
fn err_no_clock(what: &str) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("this run's host runtime cannot {what}"),
    )
    .note("`clock.now` reads the clock a run's `HostRuntime` keeps, and a production region's sleeps are deadlines on it, so a runtime that keeps none answers neither")
}

/// An operation of an effect that no handler of the program, no host handler and no host family
/// answers, refused at the entry that performed it.
#[cold]
#[inline(never)]
pub fn err_unserved(span: Span, operation: &str) -> Diagnostic {
    Diagnostic::error(
        codes::HOST_UNBOUND,
        format!("`{operation}` reached the host boundary, and nothing serves it"),
    )
    .primary(span, "no handler here, and no host serves this effect")
    .note(format!("handle `{operation}` in the program"))
}

#[cold]
#[inline(never)]
pub fn err_withheld(
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
        "handle `{operation}` over `{module}`'s twin, which is what makes a test that reads it `det`, cached and hermetic"
    ))
}
