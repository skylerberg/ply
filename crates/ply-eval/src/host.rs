//! The host effect boundary: the types the machine speaks and the registry of host handlers.

use crate::value::Value;
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_ty::{CheckOutput, EffectInfo};
use ply_ty::{EffectAtom, Footprint, Resource};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

const DIGEST_DOMAIN: &[u8] = b"ply.hosts.1";

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

    pub fn as_json(self) -> &'static str {
        match self {
            Linearity::AtMostOnce => "at_most_once",
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

/// Opaque to `ply-eval`: minted and polled by a [`HostRuntime`].
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
}

pub trait HostRuntime {
    /// `Ok(None)` when the token has not resolved.
    fn poll(&self, pending: &Pending) -> Result<Option<Value>, Diagnostic>;
    /// Waits until at least one outstanding token resolves.
    fn park(&self) -> Result<(), Diagnostic>;
    fn block_on(&self, pending: Pending) -> Result<Value, Diagnostic>;

    /// Called on every exit path from an entry point, before the machine resets.
    fn end_entry_point(&self, machine: MachineId) -> Result<(), Diagnostic> {
        let _ = machine;
        Ok(())
    }

    fn stopping(&self) -> bool {
        false
    }

    fn drain_expired(&self) -> Option<Diagnostic> {
        None
    }

    /// Called once, after the last entry point, before the process exits.
    fn shutdown(&self, drain_ms: u64) -> ShutdownReport {
        let _ = drain_ms;
        ShutdownReport::default()
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ShutdownReport {
    pub transactions_rolled_back: usize,
    pub connections_closed: Vec<String>,
    /// Spans still open at teardown, closed as `Abandoned`.
    pub spans_abandoned: usize,
    /// `None` when no sink is bound.
    pub records_flushed: Option<usize>,
    pub problems: Vec<String>,
}

impl ShutdownReport {
    pub fn is_clean(&self) -> bool {
        self.problems.is_empty()
    }
}

pub fn is_drain_incomplete(d: &Diagnostic) -> bool {
    d.code == codes::DRAIN_INCOMPLETE
}

#[derive(Default)]
pub struct HostRegistry {
    entries: Vec<(HostOp, Arc<dyn HostHandler>)>,
    /// Indices of `entries` this run declines to bind.
    withheld: BTreeSet<usize>,
}

impl HostRegistry {
    pub fn new() -> HostRegistry {
        HostRegistry::default()
    }

    pub fn register(&mut self, op: HostOp, handler: Arc<dyn HostHandler>) {
        self.entries.push((op, handler));
    }

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

    /// What would bind, without binding.
    pub fn preview(&self, check: &CheckOutput) -> Result<HostListing, Vec<Diagnostic>> {
        Ok(HostListing {
            handlers: self.entries.len(),
            rows: resolve(&self.entries, &self.withheld, check)?,
        })
    }
}

/// Rows ascending by `(effect, op, resource)`, with every registration-time check applied.
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
        // A withheld registration must not appear in the listing, footprint or index.
        if withheld.contains(&index) {
            continue;
        }
        // A handler `ply hosts` cannot name is one no reviewer can find.
        if op.path.trim().is_empty() {
            diagnostics.push(err_anonymous(op));
            continue;
        }
        // A name declared twice is refused rather than served by a coin flip.
        let declarations: Vec<&EffectInfo> = check
            .effects
            .values()
            .filter(|e| registration_names(&op.effect, &e.name, &e.simple_name))
            .collect();
        let effect = match declarations.as_slice() {
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
            // `Only` naming a resource the program never performs is usually an unfollowed rename.
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

/// Reserved std effects match by program-wide name; others by their declared name.
fn registration_names(registered: &Symbol, program_wide: &Symbol, declared: &Symbol) -> bool {
    if ply_std::is_reserved(registered.as_str()) {
        registered == program_wide
    } else {
        registered == declared
    }
}

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

/// `(effect, op, resource)`.
pub type RowKey = (Symbol, Symbol, Resource);

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HostRow {
    pub effect: Symbol,
    pub op: Symbol,
    pub resource: Resource,
    pub atom: EffectAtom,
    /// Index of the registration that produced this row.
    pub row: usize,
    pub path: &'static str,
    pub deterministic: bool,
    pub linearity: Linearity,
    pub blocking: bool,
    pub secrets: bool,
    /// Whether the declaration, not the handler, carries `nondet`.
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

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct HostListing {
    /// Ascending by `(effect, op, resource)`, [`EffectAtom`]'s own order.
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

    /// `b3:` and the first twelve hex characters.
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

pub struct Bound<'a> {
    pub atom: EffectAtom,
    pub op: &'a HostOp,
    pub handler: &'a Arc<dyn HostHandler>,
}

pub struct HostBinding {
    entries: Vec<(HostOp, Arc<dyn HostHandler>)>,
    withheld: BTreeSet<usize>,
    listing: HostListing,
    footprint: Footprint,
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

/// Hand-written: `dyn HostHandler` has no `Debug`, and the listing already carries each path.
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

    pub fn footprint(&self) -> &Footprint {
        &self.footprint
    }

    pub fn serves(&self, atom: &EffectAtom) -> bool {
        self.atoms.contains(atom)
    }

    pub fn reaches(&self, footprint: &Footprint) -> bool {
        footprint.atoms().any(|a| self.serves(a))
    }

    pub fn listing(&self) -> &HostListing {
        &self.listing
    }

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

    pub fn would_serve(
        &self,
        effect: &Symbol,
        op: &Symbol,
        resource: Option<&Symbol>,
    ) -> Option<&'static str> {
        self.matching(effect, op, resource)
            .map(|(_, candidate)| candidate.path)
    }

    /// The path of a handler this run could serve the operation with but declined to bind.
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

/// The test runner shares one binding across workers, so it must stay `Send + Sync`.
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

pub fn operation_label(effect: &Symbol, op: &Symbol, resource: Option<&Symbol>) -> String {
    match resource {
        Some(r) => format!("{effect}.{op}[{r}]"),
        None => format!("{effect}.{op}"),
    }
}

/// Codes a host handler may not raise.
pub const RESERVED_CODES: [&str; 20] = [
    codes::INTERNAL_ERROR,
    codes::SIMULATION_DIVERGENCE,
    codes::DEADLOCK,
    codes::NESTED_SIMULATION,
    codes::TASK_ESCAPES_SCOPE,
    codes::HOST_OPERATION_UNKNOWN,
    codes::HOST_HANDLER_CONFLICT,
    codes::HOST_DETERMINISM_MISMATCH,
    codes::HERMETIC_BOUNDARY,
    codes::HOST_IN_SIMULATION,
    codes::HOST_CONTINUATION_RESUMED,
    codes::HOST_FOOTPRINT_ESCAPE,
    codes::HOST_BLOCKING_ANSWER,
    codes::SECRET_TO_HOST,
    codes::REGION_ESCAPE_AT_BOUNDARY,
    codes::DB_NOT_CONFIGURED,
    codes::DB_SCHEMA_MISMATCH,
    codes::DB_UNMODELLED_SIDE_EFFECT,
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
