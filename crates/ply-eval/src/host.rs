//! The host effect boundary: the types the machine speaks, and the registry that turns a Rust
//! function into the handler for a Ply-declared operation.

use crate::value::Value;
use ply_core::ty::{EffectAtom, Footprint, Resource};
use ply_core::{CheckOutput, EffectInfo};
use ply_span::{Diagnostic, Span, Symbol, codes};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

/// Domain tag for [`HostListing::digest`].
const DIGEST_DOMAIN: &[u8] = b"ply.hosts.1";

/// Which resource labels a registration serves.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum HostResource {
    /// Exactly this label.
    Only(Resource),
    /// Every label the *program* uses with this operation.
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
    /// A send, an insert, a charge.
    AtMostOnce,
    /// A clock read, a read of an immutable resource.
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

    /// The `--json` spelling, which is snake_case where the human one is hyphenated.
    pub fn as_json(self) -> &'static str {
        match self {
            Linearity::AtMostOnce => "at_most_once",
            Linearity::Repeatable => "repeatable",
        }
    }
}

/// One registration: the `(effect, operation, resource)` triple it serves, a determinism flag, and
/// its linearity obligation.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HostOp {
    /// The effect's name **as its `effect` declaration writes it** — `net`, not `hello.net`.
    pub effect: Symbol,
    pub op: Symbol,
    pub resource: HostResource,
    pub determinism: Determinism,
    pub linearity: Linearity,
    /// The handler's *work* may not run on the scheduler's thread: it dispatches to its own pool
    /// and answers [`HostAnswer::Pending`] immediately, so a handler that calls a blocking library
    /// cannot stall the tasks sharing its thread.
    pub blocking: bool,
    /// Whether this operation may be handed a value containing a [`Value::Secret`].
    pub secrets: bool,
    /// The Rust path, as `ply hosts` prints it: the reviewable identity of a member of the trusted
    /// computing base.
    pub path: &'static str,
}

impl HostOp {
    /// The atom a perform of this operation against `resource` contributes.
    fn atom(&self, effect: &Symbol, resource: Resource, mode: ply_syntax::ast::Mode) -> EffectAtom {
        EffectAtom::new(effect.clone(), resource, mode)
    }

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
            HostResource::Only(Resource::Singleton) => write!(f, "{}.{}", self.effect, self.op),
            HostResource::Only(Resource::Named(r)) => {
                write!(f, "{}.{}[{r}]", self.effect, self.op)
            }
            HostResource::Any => write!(f, "{}.{}[..]", self.effect, self.op),
        }
    }
}

/// Which machine performed an operation.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct MachineId(pub u64);

impl MachineId {
    /// The next unused identity.
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

/// What a handler is called with.
pub struct HostRequest<'a> {
    pub atom: EffectAtom,
    pub op: &'a HostOp,
    pub args: &'a [Value],
    pub span: Span,
    /// The machine that performed this operation, which with [`task`] is the whole identity a
    /// handler keys scoped state on.
    pub machine: MachineId,
    /// The task that performed this operation.
    pub task: Option<crate::sim::TaskId>,
    /// The declared footprint of the entry point that reached this operation, when the caller
    /// stated one.
    pub declared: Option<&'a Footprint>,
}

/// What a host handler answers a perform with.
pub enum HostAnswer {
    /// Completed.
    Value(Value),
    /// Did not complete.
    Pending(Pending),
}

/// Opaque to `ply-eval`: minted by a [`HostRuntime`], polled by it, and never interpreted here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Pending {
    pub token: u64,
    /// What the run is waiting on, as a diagnostic renders it: `"accept"`, `"read"`.
    pub label: &'static str,
}

impl fmt::Display for Pending {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (#{})", self.label, self.token)
    }
}

/// A Rust implementation of one Ply operation.
pub trait HostHandler: Send + Sync {
    fn call(&self, rt: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic>;
}

/// The one thing `ply-eval` needs from `ply-host`.
pub trait HostRuntime {
    /// `Ok(None)` when the token has not resolved.
    fn poll(&self, pending: &Pending) -> Result<Option<Value>, Diagnostic>;
    /// Wait until at least one outstanding token resolves.
    fn park(&self) -> Result<(), Diagnostic>;
    /// Drive until this token resolves.
    fn block_on(&self, pending: Pending) -> Result<Value, Diagnostic>;

    /// Called on **every** exit path from an entry point — a value, a diagnostic, or a spent budget
    /// — before the machine resets.
    fn end_entry_point(&self, machine: MachineId) -> Result<(), Diagnostic> {
        let _ = machine;
        Ok(())
    }

    /// Whether a stop has been requested.
    fn stopping(&self) -> bool {
        false
    }

    /// The refusal that ends the region when the drain deadline has passed, and `None` while there
    /// is still time or no stop was requested.
    fn drain_expired(&self) -> Option<Diagnostic> {
        None
    }

    /// Called once, after the last entry point, before the process exits.
    fn shutdown(&self, drain_ms: u64) -> ShutdownReport {
        let _ = drain_ms;
        ShutdownReport::default()
    }
}

/// What [`HostRuntime::shutdown`] managed, and what the run reports about it.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ShutdownReport {
    /// Transaction scopes that were still open and were rolled back.
    pub transactions_rolled_back: usize,
    /// Connections closed rather than returned to the pool, and why.
    pub connections_closed: Vec<String>,
    /// Spans still open at teardown, closed with the `Abandoned` outcome.
    pub spans_abandoned: usize,
    /// Records the sink held when it was flushed, and `None` for a run with no sink bound.
    pub records_flushed: Option<usize>,
    /// What the teardown could not hand back, as `W0606` renders it.
    pub problems: Vec<String>,
}

impl ShutdownReport {
    pub fn is_clean(&self) -> bool {
        self.problems.is_empty()
    }
}

/// Whether an entry point ended because the drain deadline expired rather than because the program
/// failed.
pub fn is_drain_incomplete(d: &Diagnostic) -> bool {
    d.code == codes::DRAIN_INCOMPLETE
}

/// The trusted computing base, before it meets a program.
#[derive(Default)]
pub struct HostRegistry {
    entries: Vec<(HostOp, Arc<dyn HostHandler>)>,
    /// Indices of [`HostRegistry::entries`] this run declines to bind.
    withheld: BTreeSet<usize>,
}

impl HostRegistry {
    pub fn new() -> HostRegistry {
        HostRegistry::default()
    }

    pub fn register(&mut self, op: HostOp, handler: Arc<dyn HostHandler>) {
        self.entries.push((op, handler));
    }

    /// Register an operation the run knows how to serve and has decided not to.
    pub fn register_withheld(&mut self, op: HostOp, handler: Arc<dyn HostHandler>) {
        self.withheld.insert(self.entries.len());
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

    /// Resolve every registration against the program.
    pub fn bind(self, check: &CheckOutput) -> Result<HostBinding, Vec<Diagnostic>> {
        let rows = resolve(&self.entries, &self.withheld, check)?;
        let footprint = Footprint::from_atoms(rows.iter().map(|r| r.atom.clone()));
        let atoms = rows.iter().map(|r| r.atom.clone()).collect();
        let index = rows
            .iter()
            .enumerate()
            .map(|(row, r)| (r.key(), row))
            .collect();
        let listing = HostListing {
            handlers: self.entries.len(),
            rows,
        };
        Ok(HostBinding {
            entries: self.entries,
            withheld: self.withheld,
            listing,
            footprint,
            atoms,
            index,
            bound: true,
        })
    }

    /// What *would* bind, without binding.
    pub fn preview(&self, check: &CheckOutput) -> Result<HostListing, Vec<Diagnostic>> {
        Ok(HostListing {
            handlers: self.entries.len(),
            rows: resolve(&self.entries, &self.withheld, check)?,
        })
    }
}

/// Every registration, resolved against the program's atoms, ascending by `(effect, op, resource)`
/// and with every registration-time check applied.
fn resolve(
    entries: &[(HostOp, Arc<dyn HostHandler>)],
    withheld: &BTreeSet<usize>,
    check: &CheckOutput,
) -> Result<Vec<HostRow>, Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();
    let mut rows: BTreeMap<RowKey, HostRow> = BTreeMap::new();
    let mut claimed: BTreeMap<RowKey, &'static str> = BTreeMap::new();
    let performed = performed_atoms(check);

    for (index, (op, _)) in entries.iter().enumerate() {
        // A withheld registration is in no row, so it is in no listing, no footprint and no index:
        // what a run declined to bind must not appear in the trusted computing base it prints, or
        // the listing is the one thing in the system that lies about the boundary.
        if withheld.contains(&index) {
            continue;
        }
        // A member of the trusted computing base that `ply hosts` cannot name is a member no
        // reviewer can find, which defeats the whole of the listing.
        if op.path.trim().is_empty() {
            diagnostics.push(err_anonymous(op));
            continue;
        }
        // By the declared name, resolved to the program's own, and where a program that declares
        // the name twice is refused rather than served by a coin flip.
        let declarations: Vec<&EffectInfo> = check
            .effects
            .values()
            .filter(|e| registration_names(&op.effect, &e.name, &e.simple_name))
            .collect();
        let effect = match declarations.as_slice() {
            // Nothing declares it.
            [] => {
                if matches!(op.resource, HostResource::Only(_)) {
                    diagnostics.push(err_unknown_effect(op, check));
                }
                continue;
            }
            [only] => *only,
            several => {
                diagnostics.push(err_ambiguous_effect(op, several));
                continue;
            }
        };
        let name = &effect.name;
        let Some(decl) = effect.ops.get(&op.op) else {
            diagnostics.push(err_unknown_op(op, effect));
            continue;
        };
        if op.determinism == Determinism::Nondeterministic && !effect.nondet {
            diagnostics.push(err_determinism(op, effect));
            continue;
        }

        // The atoms this registration resolves to: for `Only`, the one it names, provided the
        // program can perform it; for `Any`, every label the program actually uses with this
        // operation.
        let candidates: Vec<Resource> = match &op.resource {
            HostResource::Only(r) => vec![r.clone()],
            HostResource::Any => performed
                .iter()
                .filter(|a| a.effect == *name && a.mode == decl.mode)
                .map(|a| a.resource.clone())
                .collect(),
        };
        if !decl.resource_param && candidates.iter().any(|r| *r != Resource::Singleton) {
            diagnostics.push(err_resource_unexpected(op, effect));
            continue;
        }

        for resource in candidates {
            if !op.serves_label(&resource) {
                continue;
            }
            let atom = op.atom(name, resource.clone(), decl.mode);
            // `Only` names a resource the program never performs: the claim is about nothing, which
            // is a rename that was not followed through.
            if matches!(op.resource, HostResource::Only(_)) && !performed.contains(&atom) {
                diagnostics.push(err_unused_resource(op, &atom, effect));
                continue;
            }
            let key = (name.clone(), op.op.clone(), resource.clone());
            if let Some(other) = claimed.get(&key) {
                diagnostics.push(err_conflict(op, other, op.path));
                continue;
            }
            claimed.insert(key.clone(), op.path);
            rows.insert(
                key,
                HostRow {
                    effect: name.clone(),
                    op: op.op.clone(),
                    resource,
                    atom,
                    row: index,
                    path: op.path,
                    deterministic: op.determinism.is_deterministic(),
                    linearity: op.linearity,
                    blocking: op.blocking,
                    secrets: op.secrets,
                    declared_nondet: effect.nondet,
                },
            );
        }
    }

    if diagnostics.is_empty() {
        Ok(rows.into_values().collect())
    } else {
        Err(diagnostics)
    }
}

/// Whether a registration's `effect` names a given declaration.
fn registration_names(registered: &Symbol, program_wide: &Symbol, declared: &Symbol) -> bool {
    if ply_std::is_reserved(registered.as_str()) {
        registered == program_wide
    } else {
        registered == declared
    }
}

/// Every atom the program can perform, from the declared footprints of its definitions and tests.
fn performed_atoms(check: &CheckOutput) -> BTreeSet<EffectAtom> {
    let mut out = BTreeSet::new();
    for def in check.defs.values() {
        out.extend(def.footprint.atoms().cloned());
    }
    for test in &check.tests {
        out.extend(test.footprint.atoms().cloned());
    }
    for law in &check.laws {
        out.extend(law.footprint.atoms().cloned());
    }
    out
}

/// What identifies a row and what a perform resolves against: the triple.
pub type RowKey = (Symbol, Symbol, Resource);

/// One line of `ply hosts`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HostRow {
    pub effect: Symbol,
    pub op: Symbol,
    pub resource: Resource,
    /// The atom this triple contributes to a footprint.
    pub atom: EffectAtom,
    /// Which registration produced this row.
    pub row: usize,
    pub path: &'static str,
    pub deterministic: bool,
    pub linearity: Linearity,
    pub blocking: bool,
    /// [`HostOp::secrets`], carried through so the listing can print it.
    pub secrets: bool,
    /// Whether the *declaration* carries `nondet`.
    pub declared_nondet: bool,
}

impl HostRow {
    pub fn key(&self) -> RowKey {
        (self.effect.clone(), self.op.clone(), self.resource.clone())
    }
}

impl fmt::Display for HostRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.resource {
            Resource::Singleton => write!(f, "{}.{}", self.effect, self.op),
            Resource::Named(r) => write!(f, "{}.{}[{r}]", self.effect, self.op),
        }
    }
}

/// The trusted computing base of a Ply program, as one command prints it.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct HostListing {
    /// Ascending by `(effect, op, resource)`, which is [`EffectAtom`]'s own order.
    pub rows: Vec<HostRow>,
    /// Registrations, which is at most `rows.len()`.
    pub handlers: usize,
}

impl HostListing {
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// BLAKE3 over the canonical rows, domain-tagged and length-prefixed.
    pub fn digest(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(DIGEST_DOMAIN);
        hasher.update(&(self.rows.len() as u64).to_le_bytes());
        for row in &self.rows {
            for text in [row.to_string(), row.atom.to_string()] {
                hasher.update(&(text.len() as u64).to_le_bytes());
                hasher.update(text.as_bytes());
            }
            hasher.update(&(row.path.len() as u64).to_le_bytes());
            hasher.update(row.path.as_bytes());
            hasher.update(&[
                u8::from(row.deterministic),
                u8::from(row.linearity.is_linear()),
                u8::from(row.blocking),
                u8::from(row.secrets),
                u8::from(row.declared_nondet),
            ]);
        }
        *hasher.finalize().as_bytes()
    }

    /// `b3:` and the first twelve hex characters — the one line a CI check pins.
    pub fn digest_short(&self) -> String {
        let digest = self.digest();
        let mut out = String::with_capacity(15);
        out.push_str("b3:");
        for byte in &digest[..6] {
            out.push_str(&format!("{byte:02x}"));
        }
        out
    }
}

/// One resolved registration, ready to be called.
pub struct Bound<'a> {
    pub atom: EffectAtom,
    pub op: &'a HostOp,
    pub handler: &'a Arc<dyn HostHandler>,
}

/// What a run actually has bound.
pub struct HostBinding {
    entries: Vec<(HostOp, Arc<dyn HostHandler>)>,
    /// Indices of `entries` this run declined to bind.
    withheld: BTreeSet<usize>,
    listing: HostListing,
    footprint: Footprint,
    /// What [`HostBinding::serves`] answers: the atoms, for intersecting a test's footprint.
    atoms: BTreeSet<EffectAtom>,
    /// Triple -> index into `listing.rows`.
    index: BTreeMap<RowKey, usize>,
    bound: bool,
}

impl Default for HostBinding {
    fn default() -> HostBinding {
        HostBinding::hermetic()
    }
}

/// Hand-written, because a `dyn HostHandler` has no `Debug` and should not be made to have one:
/// what identifies a handler is its declared path, which the listing already carries.
impl fmt::Debug for HostBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostBinding")
            .field("bound", &self.bound)
            .field("registrations", &self.entries.len())
            .field("listing", &self.listing)
            .finish()
    }
}

impl HostBinding {
    /// The default everywhere.
    pub fn hermetic() -> HostBinding {
        HostBinding::hermetic_with(HostRegistry::new())
    }

    pub fn hermetic_with(registry: HostRegistry) -> HostBinding {
        HostBinding {
            entries: registry.entries,
            withheld: registry.withheld,
            listing: HostListing::default(),
            footprint: Footprint::empty(),
            atoms: BTreeSet::new(),
            index: BTreeMap::new(),
            bound: false,
        }
    }

    pub fn is_hermetic(&self) -> bool {
        !self.bound
    }

    /// Every atom this binding serves.
    pub fn footprint(&self) -> &Footprint {
        &self.footprint
    }

    pub fn serves(&self, atom: &EffectAtom) -> bool {
        self.atoms.contains(atom)
    }

    /// Whether any atom of `footprint` reaches this binding.
    pub fn reaches(&self, footprint: &Footprint) -> bool {
        footprint.atoms().any(|a| self.serves(a))
    }

    pub fn listing(&self) -> &HostListing {
        &self.listing
    }

    /// The resolution the machine performs per perform that reached the boundary.
    pub fn resolve(
        &self,
        effect: &Symbol,
        op: &Symbol,
        resource: Option<&Symbol>,
    ) -> Option<Bound<'_>> {
        let key = (effect.clone(), op.clone(), resource_of(resource));
        let row = &self.listing.rows[*self.index.get(&key)?];
        let (registered, handler) = &self.entries[row.row];
        Some(Bound {
            atom: row.atom.clone(),
            op: registered,
            handler,
        })
    }

    /// The path a hermetic `E0424` names.
    pub fn would_serve(
        &self,
        effect: &Symbol,
        op: &Symbol,
        resource: Option<&Symbol>,
    ) -> Option<&'static str> {
        self.matching(effect, op, resource)
            .map(|(_, candidate)| candidate.path)
    }

    /// Whether this run knows how to serve the operation and declined to.
    pub fn withholds(
        &self,
        effect: &Symbol,
        op: &Symbol,
        resource: Option<&Symbol>,
    ) -> Option<&'static str> {
        self.matching(effect, op, resource)
            .filter(|(index, _)| self.withheld.contains(index))
            .map(|(_, candidate)| candidate.path)
    }

    fn matching(
        &self,
        effect: &Symbol,
        op: &Symbol,
        resource: Option<&Symbol>,
    ) -> Option<(usize, &HostOp)> {
        let wanted = resource_of(resource);
        let declared = Symbol::new(simple_name(effect.as_str()));
        self.entries
            .iter()
            .enumerate()
            .find(|(_, (candidate, _))| {
                registration_names(&candidate.effect, effect, &declared)
                    && candidate.op == *op
                    && candidate.serves_label(&wanted)
            })
            .map(|(index, (candidate, _))| (index, candidate))
    }
}

/// One binding serves a whole run, shared across the test runner's workers by `Arc`, so it has to
/// be shareable — and a field that broke that would be added in this file, which is why the
/// requirement is stated in this file.
const _: fn() = || {
    fn shareable<T: Send + Sync>() {}
    shareable::<HostRegistry>();
    shareable::<HostBinding>();
};

/// The resource a `perform` names.
pub fn resource_of(resource: Option<&Symbol>) -> Resource {
    match resource {
        Some(r) => Resource::Named(r.clone()),
        None => Resource::Singleton,
    }
}

/// What a run reached across the boundary.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct HostUse {
    pub atoms: Footprint,
    /// Every host operation answered, `Repeatable` ones included.
    pub operations: u64,
}

impl HostUse {
    pub fn is_empty(&self) -> bool {
        self.operations == 0
    }

    pub fn record(&mut self, atom: &EffectAtom) {
        self.atoms = self.atoms.union(&Footprint::from_atoms([atom.clone()]));
        self.operations += 1;
    }
}

/// How the source spells the operation a `perform` named.
pub fn operation_label(effect: &Symbol, op: &Symbol, resource: Option<&Symbol>) -> String {
    match resource {
        Some(r) => format!("{effect}.{op}[{r}]"),
        None => format!("{effect}.{op}"),
    }
}

/// Codes a host handler may not raise.
pub const RESERVED_CODES: [&str; 22] = [
    codes::INTERNAL_ERROR,
    codes::ENGINE_DIVERGENCE,
    codes::SIMULATION_DIVERGENCE,
    codes::DEADLOCK,
    codes::NESTED_SIMULATION,
    codes::TASK_ESCAPES_SCOPE,
    codes::MACHINE_ONLY_CLAUSE,
    codes::HOST_OPERATION_UNKNOWN,
    codes::HOST_HANDLER_CONFLICT,
    codes::HOST_DETERMINISM_MISMATCH,
    codes::HERMETIC_BOUNDARY,
    codes::HOST_IN_SIMULATION,
    codes::HOST_CONTINUATION_RESUMED,
    codes::HOST_FOOTPRINT_ESCAPE,
    codes::HOST_BLOCKING_ANSWER,
    codes::SECRET_TO_HOST,
    // Raised by the machine about what crossed the boundary, in both directions.
    codes::REGION_ESCAPE_AT_BOUNDARY,
    codes::DB_NOT_CONFIGURED,
    codes::DB_SCHEMA_MISMATCH,
    codes::DB_UNMODELLED_SIDE_EFFECT,
    // Raised by the artifact loader, before any binding exists — so a handler that answered with
    // one would be claiming the program it is running failed to load, which is a verdict about the
    // machine's own state rather than about anything the handler can see.
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
    diagnostic.severity = ply_span::Severity::Error;
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

/// `E0425` — a host operation in a test the search re-runs.
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

/// `E0428` — a `blocking: true` handler that answered inline.
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

/// `E0424` — the boundary, reached in a hermetic run.
#[cold]
#[inline(never)]
pub fn err_hermetic(span: Span, operation: &str, path: &'static str) -> Diagnostic {
    Diagnostic::error(
        codes::HERMETIC_BOUNDARY,
        format!("`{operation}` reached the host boundary in a hermetic run"),
    )
    .primary(span, "no handler here, and no host handler is bound")
    .note("`ply test` is hermetic: it binds simulated handlers and refuses real ones")
    .note(format!(
        "handle `{operation}` in the test, or run `ply test --host`"
    ))
    .note(format!("`{path}` would serve this under `--host`"))
}

/// `E0424` — an operation this run knows how to serve and deliberately did not bind.
#[cold]
#[inline(never)]
pub fn err_withheld(
    span: Span,
    operation: &str,
    effect: &Symbol,
    path: &'static str,
) -> Diagnostic {
    let module = effect
        .as_str()
        .rsplit_once('.')
        .map(|(module, _)| module.to_string())
        .unwrap_or_else(|| effect.to_string());
    Diagnostic::error(
        codes::HERMETIC_BOUNDARY,
        format!("`{operation}` reached the host boundary in a run that binds no handler for it"),
    )
    .primary(span, "no handler here, and this run bound none")
    .note(format!(
        "`{path}` serves this under `ply run --host`, and `ply test` withholds it whether or not `--host` was passed"
    ))
    .note(format!(
        "handle `{operation}` over `{module}`'s twin, which is what makes a test that reads it `det`, cached and hermetic"
    ))
}

/// The tree-walker's refusal of a bound host operation.
#[cold]
#[inline(never)]
pub fn err_machine_only_host(span: Span, operation: &str, path: &'static str) -> Diagnostic {
    Diagnostic::error(
        codes::MACHINE_ONLY_CLAUSE,
        format!("`{operation}` resolves to a host handler, which the tree-walker cannot drive"),
    )
    .primary(span, "this operation needs the machine's control stack")
    .note(format!(
        "`{path}` is bound here, and a host answer may be pending on a reactor the tree-walker has no way to poll"
    ))
    .note("run this with `--engine machine`, which is the default")
}

#[cold]
#[inline(never)]
fn err_anonymous(op: &HostOp) -> Diagnostic {
    Diagnostic::error(
        codes::HOST_OPERATION_UNKNOWN,
        format!("a host handler for `{op}` declares no Rust path"),
    )
    .note("`ply hosts` prints the path, and it is the reviewable identity of a member of the trusted computing base")
    .note("give the registration a `path` naming the function that serves it, such as `ply_host::tcp::send`")
}

#[cold]
#[inline(never)]
fn err_unknown_effect(op: &HostOp, check: &CheckOutput) -> Diagnostic {
    let mut diagnostic = Diagnostic::error(
        codes::HOST_OPERATION_UNKNOWN,
        format!(
            "`{}` registers for `{op}`, which this program does not declare",
            op.path
        ),
    )
    .note(format!(
        "no effect named `{}` is declared anywhere in the program",
        op.effect
    ));
    if let Some(near) = nearest_effect(&op.effect, check) {
        diagnostic = diagnostic
            .note(format!("the closest declared effect is `{near}`"))
            .note(format!(
                "declare `effect {}` with the operations this handler serves, or register against `{}`",
                op.effect,
                simple_name(near.as_str()),
            ));
    }
    diagnostic.note("a host handler's triple is its footprint claim; a claim about an effect nothing declares is a claim about nothing")
}

/// One registration, two declarations it could serve.
#[cold]
#[inline(never)]
fn err_ambiguous_effect(op: &HostOp, declarations: &[&EffectInfo]) -> Diagnostic {
    let names: Vec<String> = declarations
        .iter()
        .map(|e| format!("`{}`", e.name))
        .collect();
    let mut diagnostic = Diagnostic::error(
        codes::HOST_HANDLER_CONFLICT,
        format!(
            "`{}` registers for `{op}`, and this program declares `{}` {} times",
            op.path,
            op.effect,
            declarations.len()
        ),
    )
    .note(format!("declared as {}", names.join(" and ")));
    for declaration in declarations {
        if !declaration.span.is_dummy() {
            diagnostic = diagnostic.secondary(declaration.span, "declared here");
        }
    }
    diagnostic
        .note("effects are nominal, so these are different effects that share a spelling, and one host handler cannot be both")
        .note("rename one declaration, or keep a single one and import it where it is used")
}

#[cold]
#[inline(never)]
fn err_unknown_op(op: &HostOp, effect: &EffectInfo) -> Diagnostic {
    let mut diagnostic = Diagnostic::error(
        codes::HOST_OPERATION_UNKNOWN,
        format!(
            "`{}` registers for `{op}`, but effect `{}` has no operation `{}`",
            op.path, op.effect, op.op
        ),
    );
    if !effect.span.is_dummy() {
        diagnostic = diagnostic.secondary(effect.span, "declared here");
    }
    let declared: Vec<String> = effect.ops.keys().map(|k| format!("`{k}`")).collect();
    if declared.is_empty() {
        diagnostic.note("this effect declares no operations at all")
    } else {
        diagnostic.note(format!("it declares {}", declared.join(", ")))
    }
}

#[cold]
#[inline(never)]
fn err_resource_unexpected(op: &HostOp, effect: &EffectInfo) -> Diagnostic {
    let mut diagnostic = Diagnostic::error(
        codes::HOST_OPERATION_UNKNOWN,
        format!(
            "`{}` registers for `{op}`, but `{}.{}` is not resource-parameterized",
            op.path, op.effect, op.op
        ),
    );
    if let Some(decl) = effect.ops.get(&op.op)
        && !decl.span.is_dummy()
    {
        diagnostic = diagnostic.secondary(decl.span, "declared without `[r]`");
    }
    diagnostic
        .note("an operation declared without `[r]` has one singleton resource")
        .note("register `HostResource::Only(Resource::Singleton)`, or add `[r]` to the declaration")
}

#[cold]
#[inline(never)]
fn err_unused_resource(op: &HostOp, atom: &EffectAtom, effect: &EffectInfo) -> Diagnostic {
    let mut diagnostic = Diagnostic::error(
        codes::HOST_OPERATION_UNKNOWN,
        format!(
            "`{}` registers for `{atom}`, which this program never performs",
            op.path
        ),
    );
    if !effect.span.is_dummy() {
        diagnostic = diagnostic.secondary(effect.span, "the effect is declared here");
    }
    diagnostic
        .note("no definition, test or law in the program has that atom in its footprint")
        .note("check the resource label, or register `HostResource::Any` to serve whichever labels the program uses")
}

#[cold]
#[inline(never)]
fn err_determinism(op: &HostOp, effect: &EffectInfo) -> Diagnostic {
    let mut diagnostic = Diagnostic::error(
        codes::HOST_DETERMINISM_MISMATCH,
        format!(
            "`{}` is nondeterministic, but `effect {}` is not declared `nondet`",
            op.path, effect.simple_name
        ),
    );
    if !effect.span.is_dummy() {
        diagnostic = diagnostic.secondary(effect.span, "declared here, without `nondet`");
    }
    diagnostic
        .note(format!("write `nondet effect {}` so that a `det` test reaching it is E0412", effect.simple_name))
        .note("or declare the handler `Determinism::Deterministic` if its answers really are a function of the program state")
        .note("the declaration is the authority: a binding may not change what inference computed, or `ply check` would answer differently under `--host`")
}

#[cold]
#[inline(never)]
fn err_conflict(op: &HostOp, first: &str, second: &str) -> Diagnostic {
    Diagnostic::error(
        codes::HOST_HANDLER_CONFLICT,
        format!("two host handlers claim `{op}`"),
    )
    .note(format!("`{first}` and `{second}` both serve it"))
    .note("which one answers would decide which real resource is touched; narrow one registration's resource, or remove it")
}

/// The declared effect a registration most likely meant.
fn nearest_effect(wanted: &Symbol, check: &CheckOutput) -> Option<Symbol> {
    let simple = simple_name(wanted.as_str());
    if let Some(name) = check
        .effects
        .values()
        .find(|effect| effect.simple_name.as_str() == simple)
    {
        return Some(name.name.clone());
    }
    check
        .effects
        .keys()
        .map(|name| (shared_prefix(name.as_str(), wanted.as_str()), name))
        .filter(|(shared, name)| {
            let shortest = name.as_str().len().min(wanted.as_str().len());
            *shared >= 2 && *shared * 2 >= shortest
        })
        .max_by_key(|(shared, name)| (*shared, std::cmp::Reverse((*name).clone())))
        .map(|(_, name)| name.clone())
}

fn simple_name(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

fn shared_prefix(a: &str, b: &str) -> usize {
    a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count()
}

#[cfg(test)]
mod tests;
