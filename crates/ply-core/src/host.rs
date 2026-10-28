//! The host effect boundary: the types the machine speaks, the registry of host handlers, and the
//! binding a run's grants make of one.

use crate::value::Value;
use crate::{Diagnostic, EffectAtom, Footprint, Resource, Span, Symbol, codes};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum HostResource {
    Only(Resource),
    /// Every label the program uses with this operation.
    Any,
}

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

    pub fn as_str(self) -> &'static str {
        match self {
            Determinism::Deterministic => "yes",
            Determinism::Nondeterministic => "no",
        }
    }
}

/// Whether replaying this operation changes anything outside the program.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Linearity {
    AtMostOnce,
    Repeatable,
}

impl Linearity {
    /// Whether performing this counts against a later resumption.
    pub fn is_linear(self) -> bool {
        self == Linearity::AtMostOnce
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Linearity::AtMostOnce => "at-most-once",
            Linearity::Repeatable => "repeatable",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HostOp {
    /// The effect's name as its declaration writes it: `net`, not `hello.net`.
    pub effect: Symbol,
    pub op: Symbol,
    pub resource: HostResource,
    pub determinism: Determinism,
    pub linearity: Linearity,
    /// Dispatches off the scheduler's thread and answers [`HostAnswer::Pending`] immediately.
    pub blocking: bool,
    /// Whether this operation may be handed a value containing a [`Value::Secret`].
    pub secrets: bool,
    /// The Rust path `ply hosts` prints: the handler's reviewable identity.
    pub path: &'static str,
}

impl HostOp {
    fn serves_label(&self, resource: &Resource) -> bool {
        match &self.resource {
            HostResource::Only(r) => r == resource,
            HostResource::Any => true,
        }
    }
}

impl fmt::Display for HostOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.resource {
            HostResource::Only(r) => write!(f, "{}.{}{r}", self.effect, self.op),
            HostResource::Any => write!(f, "{}.{}[..]", self.effect, self.op),
        }
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

pub struct HostRequest<'a> {
    pub atom: EffectAtom,
    pub op: &'a HostOp,
    pub args: &'a [Value],
    pub span: Span,
    /// With `task`, the whole identity a handler keys scoped state on.
    pub machine: MachineId,
    pub task: Option<crate::sim::TaskId>,
    pub declared: Option<&'a Footprint>,
}

pub enum HostAnswer {
    Value(Value),
    Pending(Pending),
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

pub trait HostHandler: Send + Sync {
    fn call(&self, rt: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic>;

    /// `machine` has ended and asks nothing more: what the handler kept for it alone goes.
    fn machine_ended(&self, machine: MachineId) {
        let _ = machine;
    }
}

/// Makes a reactor for the thread that calls it: one belongs to its thread, and a machine, a test
/// worker or a `parallel` branch each drive their own.
pub type RuntimeFactory = std::sync::Arc<dyn Fn() -> std::rc::Rc<dyn HostRuntime> + Send + Sync>;

pub trait HostRuntime {
    /// Waits until at least one ticket the run watches is answered.
    fn park(&self) -> Result<(), Diagnostic>;
    fn block_on(&self, pending: Pending) -> Result<Value, Diagnostic>;

    /// What the natives of the real handlers reach the machine through: the run's sockets, its
    /// configuration and the rest the host made, which only the crate that made them names. `None`
    /// for a runtime that holds none.
    fn facilities(&self) -> Option<&dyn std::any::Any> {
        None
    }

    /// Nanoseconds the host's monotonic clock reads, which the run's region's sleeps are deadlines
    /// on; only the difference of two readings means anything.
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

#[derive(Default)]
pub struct HostRegistry {
    entries: Vec<(HostOp, Arc<dyn HostHandler>)>,
    /// Indices of `entries` this run declines to bind, each with where a run does bind it.
    withheld: BTreeMap<usize, &'static str>,
}

impl HostRegistry {
    pub fn new() -> HostRegistry {
        HostRegistry::default()
    }

    pub fn register(&mut self, op: HostOp, handler: Arc<dyn HostHandler>) {
        self.entries.push((op, handler));
    }

    /// `served` finishes the sentence "`<path>` serves this ..." that a refusal says.
    pub fn register_withheld(
        &mut self,
        op: HostOp,
        handler: Arc<dyn HostHandler>,
        served: &'static str,
    ) {
        self.withheld.insert(self.entries.len(), served);
        self.entries.push((op, handler));
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn ops(&self) -> impl Iterator<Item = &HostOp> {
        self.entries.iter().map(|(op, _)| op)
    }

    /// A binding of every registration, which answers a perform its effect's name, operation and
    /// label find a registration for: what a run is lent, `grants`, decides the registrations, and
    /// no row of the program's is read. Two registrations a binding serves of one effect, operation
    /// and label are refused (`E0422`): neither would silently win.
    pub fn granted(self, grants: Grants) -> Result<HostBinding, Vec<Diagnostic>> {
        let refused = self.conflicts();
        if !refused.is_empty() {
            return Err(refused);
        }
        Ok(HostBinding {
            entries: self.entries,
            withheld: self.withheld,
            granted: true,
            grants,
        })
    }

    /// Each registration served beside an earlier one of the same effect, operation and label.
    fn conflicts(&self) -> Vec<Diagnostic> {
        let mut first: BTreeMap<(&str, &str, String), &'static str> = BTreeMap::new();
        let mut refused = Vec::new();
        for (at, (op, _)) in self.entries.iter().enumerate() {
            if self.withheld.contains_key(&at) {
                continue;
            }
            let label = match &op.resource {
                HostResource::Only(r) => format!("{r:?}"),
                HostResource::Any => "*".to_string(),
            };
            match first.get(&(op.effect.as_str(), op.op.as_str(), label.clone())) {
                Some(earlier) => refused.push(
                    Diagnostic::error(
                        codes::HOST_HANDLER_CONFLICT,
                        format!(
                            "`{}.{}` is registered twice: by `{earlier}` and by `{}`",
                            op.effect, op.op, op.path
                        ),
                    )
                    .note("a binding serves each operation of each label by one handler"),
                ),
                None => {
                    first.insert((op.effect.as_str(), op.op.as_str(), label), op.path);
                }
            }
        }
        refused
    }
}

/// The standard library's reserved root itself, or a module under it.
pub fn is_std(name: &str) -> bool {
    name == "std" || name.starts_with("std.")
}

/// The effects the language declares itself. A module may declare one under the same name, which
/// hides the language's there, and a registration by the name is for the language's alone.
pub const LANGUAGE_EFFECTS: &[&str] = &["task", "clock", "random", "sim", "abort", "diverges"];

/// Whether a registration for `registered` is for the effect a program knows as `program_wide`
/// and a module declared as `declared`: reserved std effects and the language's own match by
/// program-wide name, others by their declared name.
pub fn registration_names(registered: &Symbol, program_wide: &Symbol, declared: &Symbol) -> bool {
    if is_std(registered.as_str()) || LANGUAGE_EFFECTS.contains(&registered.as_str()) {
        registered == program_wide
    } else {
        registered == declared
    }
}

pub struct Bound<'a> {
    pub op: &'a HostOp,
    pub handler: &'a Arc<dyn HostHandler>,
}

pub struct HostBinding {
    entries: Vec<(HostOp, Arc<dyn HostHandler>)>,
    withheld: BTreeMap<usize, &'static str>,
    /// Whether a run's grants made it; an unbound one serves nothing.
    granted: bool,
    grants: Grants,
}

/// What a run is lent, as named entries, each a list of fields: the runtime's own handlers read the
/// ones they serve (`fs NAME PATH`, `exec NAME PATH`, `allow FAMILY`, `argv ARG..`, ...), and every
/// entry is kept for the real handlers a program declares.
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

impl Default for HostBinding {
    fn default() -> HostBinding {
        HostBinding::unbound()
    }
}

/// Hand-written: `dyn HostHandler` has no `Debug`.
impl fmt::Debug for HostBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostBinding")
            .field("granted", &self.granted)
            .field("registrations", &self.entries.len())
            .finish()
    }
}

impl HostBinding {
    /// What a context entered with no binding has: it serves nothing past the program's own
    /// handlers.
    pub fn unbound() -> HostBinding {
        HostBinding {
            entries: Vec::new(),
            withheld: BTreeMap::new(),
            granted: false,
            grants: Grants::default(),
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

    /// Tells every handler that `machine` has ended.
    pub fn machine_ended(&self, machine: MachineId) {
        for (_, handler) in &self.entries {
            handler.machine_ended(machine);
        }
    }

    /// The registration that serves `effect.op[resource]`, the first not withheld of those for it,
    /// or why there is none.
    pub fn resolve(
        &self,
        effect: &Symbol,
        op: &Symbol,
        resource: Option<&Symbol>,
    ) -> Result<Bound<'_>, Unserved> {
        let mut matching = self.matching(effect, op, resource);
        let Some((first, (registered, _))) = matching.next() else {
            return Err(Unserved::Unregistered);
        };
        let withheld = |index: &usize| self.withheld.get(index).copied();
        match withheld(&first) {
            None => Ok(Bound {
                op: registered,
                handler: &self.entries[first].1,
            }),
            Some(served) => matching
                .find(|(index, _)| withheld(index).is_none())
                .map(|(_, (op, handler))| Bound { op, handler })
                .ok_or(Unserved::Withheld {
                    path: registered.path,
                    served,
                }),
        }
    }

    /// Each registration for `effect.op[resource]`, by its place among the registrations.
    fn matching<'s, 'n>(
        &'s self,
        effect: &'n Symbol,
        op: &'n Symbol,
        resource: Option<&Symbol>,
    ) -> impl Iterator<Item = (usize, &'s (HostOp, Arc<dyn HostHandler>))> + use<'s, 'n> {
        let wanted = resource_of(resource);
        let declared = Symbol::new(simple_name(effect.as_str()));
        self.entries
            .iter()
            .enumerate()
            .filter(move |(_, (candidate, _))| {
                registration_names(&candidate.effect, effect, &declared)
                    && candidate.op == *op
                    && candidate.serves_label(&wanted)
            })
    }
}

/// Why a binding serves no registration of an operation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unserved {
    /// No registration is for it.
    Unregistered,
    /// Each registration for it is withheld from this run; `served` says where a run does bind
    /// the first, `path`.
    Withheld {
        path: &'static str,
        served: &'static str,
    },
}

/// A `parallel` block's branches share one binding, so it must stay `Send + Sync`.
const _: fn() = || {
    fn shareable<T: Send + Sync>() {}
    shareable::<HostRegistry>();
    shareable::<HostBinding>();
};

pub fn resource_of(resource: Option<&Symbol>) -> Resource {
    match resource {
        Some(r) => Resource::Named(r.clone()),
        None => Resource::Singleton,
    }
}

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

/// Codes a host handler may not raise.
pub const RESERVED_CODES: [&str; 17] = [
    codes::INTERNAL_ERROR,
    codes::SIMULATION_DIVERGENCE,
    codes::DEADLOCK,
    codes::NESTED_SIMULATION,
    codes::TASK_ESCAPES_SCOPE,
    codes::HOST_OPERATION_UNKNOWN,
    codes::HOST_HANDLER_CONFLICT,
    codes::HOST_DETERMINISM_MISMATCH,
    codes::HOST_UNBOUND,
    codes::HOST_IN_SIMULATION,
    codes::HOST_CONTINUATION_RESUMED,
    codes::HOST_FOOTPRINT_ESCAPE,
    codes::HOST_BLOCKING_ANSWER,
    codes::SECRET_TO_HOST,
    codes::REGION_ESCAPE_AT_BOUNDARY,
    // Raised by the artifact loader before any binding exists.
    codes::ARTIFACT_INVALID,
    codes::ARTIFACT_VERSION,
];

pub fn is_reserved_code(code: &str) -> bool {
    RESERVED_CODES.contains(&code)
}

/// Stamps a handler's refusal with where it came from.
pub fn attribute(
    mut diagnostic: Diagnostic,
    path: &'static str,
    operation: &str,
    span: Span,
) -> Diagnostic {
    diagnostic.severity = crate::Severity::Error;
    if is_reserved_code(diagnostic.code) {
        let claimed = diagnostic.code;
        diagnostic.code = codes::RUNTIME_ERROR;
        diagnostic.notes.push(format!(
            "`{path}` raised `{claimed}`, which only the run itself may raise; it was reported as `{}` instead",
            codes::RUNTIME_ERROR
        ));
        diagnostic.notes.push(
            "a code that says the evaluator broke its own invariants would send a reader to file a bug against Ply for a failure the handler produced"
                .to_string(),
        );
    }
    diagnostic
        .notes
        .push(format!("raised by `{path}` while answering `{operation}`"));
    if !span.is_dummy() && !diagnostic.labels.iter().any(|l| l.span == span) {
        diagnostic = diagnostic.secondary(span, "this perform reached the host boundary");
    }
    diagnostic
}

/// Whether a value handed across the boundary holds a credential anywhere.
pub fn check_host_answer(
    operation: &str,
    path: &'static str,
    value: &Value,
    span: Span,
) -> Result<(), Diagnostic> {
    crate::escape::check(
        &crate::escape::Boundary::HostAnswer { operation, path },
        value,
        span,
    )
}

pub fn carries_secret(v: &Value) -> bool {
    match v {
        Value::Secret(_) => true,
        Value::List(xs) => crate::limit::grow(|| xs.iter().any(carries_secret)),
        Value::Array(xs) => crate::limit::grow(|| xs.iter().any(carries_secret)),
        Value::Map(m) => crate::limit::grow(|| {
            m.iter()
                .any(|(k, v)| carries_secret(k) || carries_secret(v))
        }),
        Value::Record(fields) => crate::limit::grow(|| fields.values().any(carries_secret)),
        Value::Ctor { args, .. } => crate::limit::grow(|| args.iter().any(carries_secret)),
        _ => false,
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
pub fn err_blocking_answered_inline(span: Span, operation: &str, path: &'static str) -> Diagnostic {
    Diagnostic::error(
        codes::HOST_BLOCKING_ANSWER,
        format!("`{path}` is registered `blocking` and answered `{operation}` with a value"),
    )
    .primary(span, "performed here")
    .note("`blocking: true` means the work leaves the machine's thread: the handler dispatches it and answers `HostAnswer::Pending` immediately")
    .note("a value returned from `call` is this thread having done the work, so every task sharing it was stalled for the duration")
    .note("dispatch the work to the host's pool and answer `Pending`, or register the operation `blocking: false`")
}

#[cold]
#[inline(never)]
pub fn err_footprint_escape(
    span: Span,
    operation: &str,
    atom: &EffectAtom,
    declared: &Footprint,
    path: &'static str,
) -> Diagnostic {
    Diagnostic::error(
        codes::HOST_FOOTPRINT_ESCAPE,
        format!("`{path}` answered `{atom}`, which is outside this entry point's declared footprint"),
    )
    .primary(span, format!("`{operation}` performed here"))
    .note(format!("the entry point's declared footprint is {declared}"))
    .note("scheduling and world isolation are decided from that footprint, so an operation outside it may have run beside work it conflicts with")
    .note("this is Ply's fault: the run knows two of its own answers disagree and nothing in the definition graph decides which was meant")
}

#[cold]
#[inline(never)]
pub fn err_secret_to_host(
    span: Span,
    operation: &str,
    position: usize,
    path: &'static str,
) -> Diagnostic {
    Diagnostic::error(
        codes::SECRET_TO_HOST,
        format!("`{operation}` was handed a `Secret` in argument {}", position + 1),
    )
    .primary(span, "performed here")
    .note(format!("`{path}` is registered `secrets: no`, so nothing above the boundary knows a credential can reach it"))
    .note("below the boundary nothing is checkable: what a handler does with a credential is invisible to every guarantee this language makes")
    .note("`ply hosts` prints the column; a handler that must receive one declares `secrets: true` there and becomes a reviewed member of the trusted computing base")
    .note("this is Ply's fault: the registration and what crossed it disagree, and no definition in the program decides which was meant")
}

#[cold]
#[inline(never)]
fn err_no_clock(what: &str) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("a task slept, and this run's host runtime cannot {what}"),
    )
    .note("a production region's sleeps are deadlines on the clock its `HostRuntime` reads, so a runtime that keeps none runs no sleeping task")
}

#[cold]
#[inline(never)]
pub fn err_unbound(span: Span, operation: &str) -> Diagnostic {
    Diagnostic::error(
        codes::HOST_UNBOUND,
        format!("`{operation}` reached the host boundary in a run bound to no host"),
    )
    .primary(span, "no handler here, and no host is bound")
    .note("an entry made with no binding, as `hermetic_machine` makes one, serves only what the program handles")
    .note(format!("handle `{operation}` in the program"))
}

/// An operation of an effect that no handler of the program, no real handler and no host family
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

fn simple_name(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}
