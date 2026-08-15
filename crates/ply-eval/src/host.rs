//! The host effect boundary: the types the machine speaks, and the registry
//! that turns a Rust function into the handler for a Ply-declared operation.
//!
//! A Ply program never calls the host. It performs an ordinary effect
//! operation, and the runtime's handler stack may resolve it to a handler
//! implemented in Rust — consulted **only when [`Stack::find_handler`] returns
//! `None`**, so the host is the handler of last resort and any `handle` or
//! `simulate` in scope shadows it by the ordinary innermost-first rule. The host
//! binding is not a [`Delimiter`], and no [`Continuation`] ever contains it,
//! which is what keeps capture, splice and `Next::Leave` untouched.
//!
//! This module carries no runtime and depends on none. `ply-host` implements
//! [`HostHandler`] and [`HostRuntime`]; here they are only named, so `ply-eval`
//! can dispatch across the boundary without learning what a socket is.
//!
//! Three rules carry the design, and each decides a signature below:
//!
//! - **A binding may not change what the front end computes.** So a handler's
//!   determinism is checked *against* the source declaration by [`bind`] rather
//!   than injected into inference. E0412 is untouched.
//! - **A binding may not change what a green result means.** So a run that
//!   reached the host is never written to the result cache.
//! - **When the static picture and the dynamic one disagree the dynamic one
//!   wins and says so.** That is `E0427`, checked per host answer against the
//!   entry point's declared footprint. It catches a *program* footprint that
//!   under-reports; it cannot catch a handler that does more than its
//!   registration declared, and ADR 0008 §2 states that residual rather than
//!   implying a backstop that is not there.
//! - **A handler does not decide how its own failure is classified.** Every
//!   refusal is passed through [`attribute`], which names the handler and
//!   rewrites a code in [`RESERVED_CODES`].
//!
//! [`Stack::find_handler`]: crate::cont::Stack::find_handler
//! [`Delimiter`]: crate::cont::Delimiter
//! [`Continuation`]: crate::cont::Continuation
//! [`bind`]: HostRegistry::bind

use crate::value::Value;
use ply_core::ty::{EffectAtom, Footprint, Resource};
use ply_core::{CheckOutput, EffectInfo};
use ply_span::{Diagnostic, Span, Symbol, codes};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

/// Domain tag for [`HostListing::digest`]. A listing's digest is compared across
/// releases in CI, so it may never collide with any other hash this project
/// writes.
const DIGEST_DOMAIN: &[u8] = b"ply.hosts.1";

/// Which resource labels a registration serves.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum HostResource {
    /// Exactly this label. [`Resource::Singleton`] for an operation declared
    /// without `[r]`.
    Only(Resource),
    /// Every label the *program* uses with this operation. A postgres driver
    /// serves `db.get[users]` and `db.get[orders]` alike, and requiring one
    /// registration per table makes the trusted computing base unbounded.
    ///
    /// Resolved against [`CheckOutput`] at bind time and never printed as `*`:
    /// the difference between "this handler claims everything" and "this handler
    /// claims these four tables" is the whole value of the listing, and it costs
    /// nothing to have the honest one.
    Any,
}

/// Whether a handler may serve an effect the program did not declare `nondet`.
///
/// Consulted at bind time and by [`HostListing`], and **nowhere in inference, in
/// a cache key, or in the evaluator**. A verdict that depended on whether
/// `--host` was passed would split every cache in the system on a flag.
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
    /// A send, an insert, a charge. Bumps the machine's host-operation counter
    /// and therefore closes multi-shot resumption over it.
    AtMostOnce,
    /// A clock read, a read of an immutable resource. Replay is harmless, so it
    /// costs the linearity rule nothing — which is what keeps that rule's
    /// over-approximation tight.
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

    /// The `--json` spelling, which is snake_case where the human one is
    /// hyphenated.
    pub fn as_json(self) -> &'static str {
        match self {
            Linearity::AtMostOnce => "at_most_once",
            Linearity::Repeatable => "repeatable",
        }
    }
}

/// One registration: the `(effect, operation, resource)` triple it serves, a
/// determinism flag, and its linearity obligation.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HostOp {
    /// The effect's name **as its `effect` declaration writes it** — `net`, not
    /// `hello.net`.
    ///
    /// The trusted computing base is a fixed list in one Rust file, compiled
    /// once and read by a reviewer before it meets any program. A registration
    /// keyed on the program-wide name could not be written down: that name
    /// carries the *consumer's* module path, so shipping `ply_host::tcp` would
    /// mean regenerating the list per program and there would be nothing to
    /// diff. [`bind`] therefore resolves this against the program's declarations
    /// and the resolved row carries the program-wide name, which is what the
    /// atoms, `ply hosts` and the machine's lookup all speak in.
    ///
    /// Effects stay nominal, and the cost of that lands here: a program with two
    /// modules each declaring `net` gives this registration two declarations to
    /// serve, and binding refuses it rather than picking one.
    ///
    /// [`bind`]: HostRegistry::bind
    pub effect: Symbol,
    pub op: Symbol,
    pub resource: HostResource,
    pub determinism: Determinism,
    pub linearity: Linearity,
    /// The handler's *work* may not run on the scheduler's thread: it dispatches
    /// to its own pool and answers [`HostAnswer::Pending`] immediately, so a
    /// handler that calls a blocking library cannot stall the tasks sharing its
    /// thread. [`HostHandler::call`] itself is always entered on the machine's
    /// thread — a [`Value`] is not `Send`, so nothing here could hand the work
    /// anywhere else.
    ///
    /// Half of that is checked and half is not. A `true` handler that answers
    /// [`HostAnswer::Value`] did the work on this thread, and that is
    /// [`codes::HOST_BLOCKING_ANSWER`]. A `false` handler that blocks anyway is
    /// undetectable — no budget on `call`, no watchdog, no cancellation in W1 —
    /// which is why the column is printed for a reviewer.
    pub blocking: bool,
    /// Whether this operation may be handed a value containing a
    /// [`Value::Secret`].
    ///
    /// Printed by `ply hosts` in its own column and covered by the listing's
    /// digest, because a handler that receives a credential is the one place
    /// above the boundary where ADR 0015 §2.1's claim stops being enforceable
    /// and starts being review.
    ///
    /// Checked before the handler is called: a `perform` carrying a `Secret`
    /// into a registration that declares `false` is [`codes::SECRET_TO_HOST`].
    /// **In W5 no registration declares `true`**, so the check lands with a user
    /// count of zero — which is the right order, because the alternative is
    /// adding it after the first operation that needed it already shipped.
    pub secrets: bool,
    /// The Rust path, as `ply hosts` prints it: the reviewable identity of a
    /// member of the trusted computing base.
    pub path: &'static str,
}

impl HostOp {
    /// The atom a perform of this operation against `resource` contributes.
    ///
    /// `effect` is the program-wide name the registration resolved to, never
    /// [`HostOp::effect`]: an atom is what scheduling and isolation are decided
    /// from, so it has to be the name the program's own footprints carry.
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
///
/// One per [`Machine`], minted at construction and never reused, because a
/// handler holding scoped state has to be able to tell two entry points apart
/// and [`HostRequest::task`] cannot: every entry point outside a scheduler
/// region reports `None`, and `ply test` runs the members of a non-conflicting
/// concurrency group on rayon threads, each driving a machine of its own. Keyed
/// on the task alone, two of those are one owner — one reads the other's
/// uncommitted rows, and either one's teardown ends the other's transaction.
///
/// Process-wide rather than per-run, so that two runs in one process (the test
/// suite's own shape) cannot collide either.
///
/// [`Machine`]: crate::machine::Machine
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
///
/// `atom` is the **resolved** atom — the concrete resource, never `Any` — so a
/// handler never re-derives its own footprint and cannot disagree with the
/// registry about what it just did. `span` points at the `perform`, so a
/// diagnostic a handler raises points at Ply source rather than at Rust.
pub struct HostRequest<'a> {
    pub atom: EffectAtom,
    pub op: &'a HostOp,
    pub args: &'a [Value],
    pub span: Span,
    /// The machine that performed this operation, which with [`task`] is the
    /// whole identity a handler keys scoped state on. A handler that used only
    /// the task would file every concurrently running entry point under one key.
    ///
    /// [`task`]: HostRequest::task
    pub machine: MachineId,
    /// The task that performed this operation. `None` outside a scheduler
    /// region, which is **one identity rather than an absence of one**: an
    /// entry point that never spawned is a single thread of control, and a
    /// handler keying scoped state on the performer has to be able to name it.
    ///
    /// A handler that holds something scoped — an open transaction, a lock —
    /// compares this against the identity that opened the scope, and a mismatch
    /// is the case where a statement would silently run outside the scope its
    /// author believed it was in.
    pub task: Option<crate::sim::TaskId>,
    /// The declared footprint of the entry point that reached this operation,
    /// when the caller stated one.
    ///
    /// A handler whose true footprint is a function of a runtime value — a SQL
    /// statement's table set is the only one in the system — can compute it and
    /// **refuse instead of acting**. Every other handler's footprint is a
    /// property of its registration, which the registry checked against the row
    /// before the operation was dispatched, and those ignore this.
    ///
    /// `None` when the caller declared nothing. Then only the checks the handler
    /// can make on its own apply, which is weaker and is stated rather than
    /// hidden.
    pub declared: Option<&'a Footprint>,
}

/// What a host handler answers a perform with.
pub enum HostAnswer {
    /// Completed. Returned into the perform site along the ordinary
    /// tail-resumptive path, so a value-shaped host operation costs what a Ply
    /// handler clause costs.
    Value(Value),
    /// Did not complete. The performing task leaves the enabled set until the
    /// token resolves, exactly as [`Answer::Sleeping`] parks one on a deadline —
    /// which is why the production scheduler is not a second scheduler.
    ///
    /// [`Answer::Sleeping`]: crate::sim::Answer::Sleeping
    Pending(Pending),
}

/// Opaque to `ply-eval`: minted by a [`HostRuntime`], polled by it, and never
/// interpreted here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Pending {
    pub token: u64,
    /// What the run is waiting on, as a diagnostic renders it: `"accept"`,
    /// `"read"`. A blocked run has to be able to say what it is blocked on.
    pub label: &'static str,
}

impl fmt::Display for Pending {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (#{})", self.label, self.token)
    }
}

/// A Rust implementation of one Ply operation.
///
/// `Send + Sync` because the registry is shared across the test runner's
/// workers. The `Value`s it receives and produces are not `Send`, and never
/// cross a thread: a handler that needs a real thread hands the *work* to
/// `ply-host`'s pool and answers [`HostAnswer::Pending`].
pub trait HostHandler: Send + Sync {
    fn call(&self, rt: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic>;
}

/// The one thing `ply-eval` needs from `ply-host`.
pub trait HostRuntime {
    /// `Ok(None)` when the token has not resolved. The scheduler polls; it never
    /// spins, because [`park`](HostRuntime::park) is what it calls when nothing
    /// is ready.
    fn poll(&self, pending: &Pending) -> Result<Option<Value>, Diagnostic>;
    /// Wait until at least one outstanding token resolves. Called only with no
    /// task enabled.
    fn park(&self) -> Result<(), Diagnostic>;
    /// Drive until this token resolves. The only place a Ply computation blocks
    /// a real thread, and reached only outside a scheduler region, where there
    /// is nothing to park on.
    fn block_on(&self, pending: Pending) -> Result<Value, Diagnostic>;

    /// Called on **every** exit path from an entry point — a value, a
    /// diagnostic, or a spent budget — before the machine resets.
    ///
    /// It exists for the one thing a host handler can leave behind that the
    /// machine cannot see: a scoped resource whose closing operation the program
    /// never reached. A `transaction` body that raises propagates past the
    /// `handle` that would have committed or aborted it, so the `BEGIN` is still
    /// open on a connection that is about to go back to a pool — and the next
    /// request reads uncommitted rows of a request that already failed,
    /// invisibly from either. The driver rolls back every scope still open here
    /// and releases or discards the connections holding them.
    ///
    /// The default is `Ok(())`, which is the true answer for a runtime that owns
    /// no scoped state: W1's sockets are closed by the program or by the process
    /// and there is nothing to unwind. A runtime that acquires something with a
    /// close operation implements this, and a failure here does not change the
    /// entry point's verdict — it is the run's own fault rather than the
    /// program's, and reporting it as a failure would attribute a discarded
    /// connection to whatever test happened to be running.
    ///
    /// `machine` is whose entry point ended. It is a parameter rather than
    /// something a runtime knows about itself because the facilities behind a
    /// runtime are shared across every worker in the run: a teardown that closed
    /// *everything* would roll back a transaction another entry point is still
    /// writing into, and neither of them would see it happen.
    fn end_entry_point(&self, machine: MachineId) -> Result<(), Diagnostic> {
        let _ = machine;
        Ok(())
    }

    /// Whether a stop has been requested.
    ///
    /// [`park`](HostRuntime::park) returns when this becomes true even with no
    /// token outstanding, and the deadlock check consults it so that a park
    /// which woke on a stop is not counted as fruitless. Without both, an idle
    /// service — parked on an `accept` nobody is connecting to — never observes
    /// a signal, and `ctrl-C` does nothing until the next request arrives.
    ///
    /// `ply-eval` reads it in exactly two places, the park loop and the deadlock
    /// check, and nowhere else. It is not consulted by inference, by a cache key
    /// or by isolation: a verdict that depended on when a signal arrived would
    /// be a verdict that depends on the terminal.
    fn stopping(&self) -> bool {
        false
    }

    /// The refusal that ends the region when the drain deadline has passed, and
    /// `None` while there is still time or no stop was requested.
    ///
    /// Checked once per scheduling decision, which is the only place the machine
    /// gives control back while a request is still running. **W5 has no
    /// cancellation**: the task is not unwound and is not handed a `503`, so the
    /// honest thing the run can do is stop scheduling, tear down in the pinned
    /// order — which rolls the task's open transaction back — and exit. The
    /// client sees a connection closed with no response, which is the outcome a
    /// retry can fix; a committed half-transaction is the one it cannot, and
    /// that is what the ordering makes unreachable.
    fn drain_expired(&self) -> Option<Diagnostic> {
        None
    }

    /// Called once, after the last entry point, before the process exits.
    ///
    /// Distinct from [`end_entry_point`](HostRuntime::end_entry_point), which
    /// runs per entry point and knows whose. This runs the process-level pinned
    /// order — roll every open transaction back, close every open span, flush
    /// the sink, close the pool — and answers what it managed.
    ///
    /// **Never called from a signal handler.** The handler sets a flag; this
    /// runs on the machine's thread, after it has stopped running Ply code, so
    /// nothing here races a statement the program is still issuing.
    ///
    /// `drain_ms` is a **bound and not a hint**. A teardown step that waits on a
    /// peer — a `ROLLBACK` behind a statement the server has not finished —
    /// waits for at most this long and then discards the connection, because
    /// closing the socket is what aborts the statement and a stop that outlasts
    /// the deadline the operator set is a stop that is not bounded at all.
    fn shutdown(&self, drain_ms: u64) -> ShutdownReport {
        let _ = drain_ms;
        ShutdownReport::default()
    }
}

/// What [`HostRuntime::shutdown`] managed, and what the run reports about it.
///
/// Every field is a fact the teardown already held. Nothing here is computed for
/// the banner.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ShutdownReport {
    /// Transaction scopes that were still open and were rolled back. **Never
    /// committed**: a commit at a deadline commits a half-finished body, and the
    /// only thing that knows whether a body finished is the body.
    pub transactions_rolled_back: usize,
    /// Connections closed rather than returned to the pool, and why.
    pub connections_closed: Vec<String>,
    /// Spans still open at teardown, closed with the `Abandoned` outcome.
    pub spans_abandoned: usize,
    /// Records the sink held when it was flushed, and `None` for a run with no
    /// sink bound.
    pub records_flushed: Option<usize>,
    /// What the teardown could not hand back, as `W0606` renders it. Empty is a
    /// clean teardown, which a run reports nothing about.
    pub problems: Vec<String>,
}

impl ShutdownReport {
    pub fn is_clean(&self) -> bool {
        self.problems.is_empty()
    }
}

/// Whether an entry point ended because the drain deadline expired rather than
/// because the program failed.
///
/// The run's configuration is at fault — `--drain-ms` was below the program's
/// own `body_timeout_ms + write_timeout_ms` — so it is not the program's
/// verdict, is not attributed to a definition and is not bisected. What carries
/// it is the exit code.
pub fn is_drain_incomplete(d: &Diagnostic) -> bool {
    d.code == codes::DRAIN_INCOMPLETE
}

/// The trusted computing base, before it meets a program.
///
/// Built by one function in `ply-host` so the whole of it is a list read top to
/// bottom in one file: no attribute macro, no link-time registry, no global
/// constructor. What a reviewer must be able to do with the TCB is read it.
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
    ///
    /// It is in the trusted computing base — [`HostBinding::would_serve`] names
    /// its path — and it is in no listing, no footprint and no index, so a
    /// perform that reaches it is `E0424` naming the handler that would have
    /// served it under a run that bound it. That is a different sentence from
    /// "nothing registers this", which is `E0303` and sends the reader looking
    /// for a bug in inference.
    ///
    /// `signal` is the case it exists for. A stop flag set once ends every test
    /// after it, so `ply test` binds none of it with or without `--host`, and
    /// the diagnostic has to say that rather than pretend the operation is
    /// unknown.
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
    ///
    /// Fails loudly, and every failure is the host author's rather than the
    /// program's: the common case is a rename on the Ply side that the Rust side
    /// did not follow, which is why `E0421` names the nearest declared
    /// operation. An `Any` registration resolving to no atom is **not** an
    /// error — a driver linked into a program that never queries is idle, not
    /// wrong — and an empty registry is a legal hermetic binding.
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

    /// What *would* bind, without binding. `ply hosts` prints this in a hermetic
    /// run, because "hermetic" has to be distinguishable from "the registry
    /// failed to load".
    pub fn preview(&self, check: &CheckOutput) -> Result<HostListing, Vec<Diagnostic>> {
        Ok(HostListing {
            handlers: self.entries.len(),
            rows: resolve(&self.entries, &self.withheld, check)?,
        })
    }
}

/// Every registration, resolved against the program's atoms, ascending by
/// `(effect, op, resource)` and with every registration-time check applied.
///
/// The key is the **triple**, not the atom. An [`EffectAtom`] carries no
/// operation, so `db.get[users]` and `db.peek[users]` are one atom and two
/// operations; keying rows by the atom would report a conflict between two
/// handlers that serve different things.
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
        // A withheld registration is in no row, so it is in no listing, no
        // footprint and no index: what a run declined to bind must not appear in
        // the trusted computing base it prints, or the listing is the one thing
        // in the system that lies about the boundary.
        if withheld.contains(&index) {
            continue;
        }
        // A member of the trusted computing base that `ply hosts` cannot name is
        // a member no reviewer can find, which defeats the whole of the listing.
        if op.path.trim().is_empty() {
            diagnostics.push(err_anonymous(op));
            continue;
        }
        // By the declared name, resolved to the program's own, and where a
        // program that declares the name twice is refused rather than served by
        // a coin flip.
        //
        let declarations: Vec<&EffectInfo> = check
            .effects
            .values()
            .filter(|e| registration_names(&op.effect, &e.name, &e.simple_name))
            .collect();
        let effect = match declarations.as_slice() {
            // Nothing declares it. `Only` named a specific resource, so it is
            // asserting the program has one and is wrong; `Any` is a driver
            // linked into a program that never uses it, which is idle rather
            // than wrong. The same line §1 draws for an unperformed resource,
            // one level up, and it is what lets one registry be compiled into
            // every program: `ply hosts` on a program with no `net` prints an
            // empty trusted computing base, which is the true answer.
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

        // The atoms this registration resolves to: for `Only`, the one it names,
        // provided the program can perform it; for `Any`, every label the
        // program actually uses with this operation.
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
            // `Only` names a resource the program never performs: the claim is
            // about nothing, which is a rename that was not followed through.
            // `Any` resolving to nothing is silence, not an error.
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
///
/// A registration may spell a **program-wide** name only under the reserved
/// stdlib root, and then it matches that declaration and nothing else. That is
/// the one case where the registration side knows the module: `std.net` ships
/// with the compiler, so `ply_host::tcp` can name `std.net.net` exactly, and a
/// program's own `effect net` then cannot silently acquire a real socket.
/// Anywhere else the module belongs to the consumer, a registration guessing at
/// it is `E0421`, and the two meet on the declared name.
///
/// One function, because `resolve` and [`HostBinding::would_serve`] must agree:
/// the second is a prediction of what the first would do, and a second matcher
/// could drift from it.
fn registration_names(registered: &Symbol, program_wide: &Symbol, declared: &Symbol) -> bool {
    if ply_std::is_reserved(registered.as_str()) {
        registered == program_wide
    } else {
        registered == declared
    }
}

/// Every atom the program can perform, from the declared footprints of its
/// definitions and tests.
///
/// This is what makes [`HostResource::Any`] expandable: a resource label is a
/// ground identifier in the source, so the set of labels a program uses with an
/// operation is finite and known before anything runs. A footprint is an upper
/// bound on what is performed, which is the direction that makes an expansion
/// safe — the listing may name an atom nothing reaches, and never miss one that
/// is reached.
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

/// One line of `ply hosts`. One per resolved **triple**, never one per
/// registration: an `Any` handler must not hide a resource behind a `*`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HostRow {
    pub effect: Symbol,
    pub op: Symbol,
    pub resource: Resource,
    /// The atom this triple contributes to a footprint. Not a key: an
    /// [`EffectAtom`] carries no operation, so two operations of one effect at
    /// one mode and resource share it.
    pub atom: EffectAtom,
    /// Which registration produced this row.
    pub row: usize,
    pub path: &'static str,
    pub deterministic: bool,
    pub linearity: Linearity,
    pub blocking: bool,
    /// [`HostOp::secrets`], carried through so the listing can print it.
    pub secrets: bool,
    /// Whether the *declaration* carries `nondet`. Printed so a reviewer sees
    /// the pair `E0423` checks rather than half of it.
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
    /// Ascending by `(effect, op, resource)`, which is [`EffectAtom`]'s own
    /// order. A listing whose order depended on registration order would produce
    /// a diff for a reordering that changed nothing.
    pub rows: Vec<HostRow>,
    /// Registrations, which is at most `rows.len()`.
    pub handlers: usize,
}

impl HostListing {
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// BLAKE3 over the canonical rows, domain-tagged and length-prefixed.
    ///
    /// Every column is covered, `linearity` and `blocking` included: a handler
    /// that quietly became repeatable is exactly the change worth a reviewer's
    /// attention, and a digest that missed it would be worse than none.
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
///
/// A hermetic binding still carries the registry, so `E0424` can name the
/// handler that *would* have served the operation. That name is the whole
/// difference between a diagnostic a reader can act on and one they cannot.
pub struct HostBinding {
    entries: Vec<(HostOp, Arc<dyn HostHandler>)>,
    /// Indices of `entries` this run declined to bind. In `entries` so that
    /// [`HostBinding::would_serve`] can still name the handler, and in nothing
    /// else.
    withheld: BTreeSet<usize>,
    listing: HostListing,
    footprint: Footprint,
    /// What [`HostBinding::serves`] answers: the atoms, for intersecting a
    /// test's footprint. Coarser than `index` on purpose — an atom is the unit
    /// selection and isolation speak in.
    atoms: BTreeSet<EffectAtom>,
    /// Triple -> index into `listing.rows`. Built once, because the machine
    /// looks up once per perform that reached the boundary.
    index: BTreeMap<RowKey, usize>,
    bound: bool,
}

impl Default for HostBinding {
    fn default() -> HostBinding {
        HostBinding::hermetic()
    }
}

/// Hand-written, because a `dyn HostHandler` has no `Debug` and should not be
/// made to have one: what identifies a handler is its declared path, which the
/// listing already carries.
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
    /// The default everywhere. Nothing bound, and nothing to name in a
    /// diagnostic either — for that, use [`hermetic_with`].
    ///
    /// [`hermetic_with`]: HostBinding::hermetic_with
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

    /// Every atom this binding serves. The set selection intersects a test's
    /// footprint against, and it is exact rather than an upper bound: an atom
    /// outside it can never reach a host handler.
    pub fn footprint(&self) -> &Footprint {
        &self.footprint
    }

    pub fn serves(&self, atom: &EffectAtom) -> bool {
        self.atoms.contains(atom)
    }

    /// Whether any atom of `footprint` reaches this binding. What `Reason::Host`
    /// and [`Isolation::Host`] are decided by.
    ///
    /// [`Isolation::Host`]: ply_test::Isolation
    pub fn reaches(&self, footprint: &Footprint) -> bool {
        footprint.atoms().any(|a| self.serves(a))
    }

    pub fn listing(&self) -> &HostListing {
        &self.listing
    }

    /// The resolution the machine performs per perform that reached the
    /// boundary. `None` in a hermetic run and for a triple nothing registered.
    ///
    /// By the triple rather than by the atom, because that is what a `perform`
    /// carries and because an atom names no operation.
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

    /// The path a hermetic `E0424` names. Answers from the registry rather than
    /// from the binding, so it works precisely when nothing is bound.
    ///
    /// `effect` is program-wide and a registration's is as declared, so the two
    /// meet through [`registration_names`] — the same rule [`bind`] resolves by,
    /// which is what makes this an honest prediction of what `--host` would do
    /// rather than a second matcher that can drift from it.
    ///
    /// [`bind`]: HostRegistry::bind
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
    ///
    /// The difference between "nothing registers this" and "this run binds no
    /// handler for it" is the whole of the diagnostic a reader acts on, and only
    /// the binding can tell them apart.
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

/// One binding serves a whole run, shared across the test runner's workers by
/// `Arc`, so it has to be shareable — and a field that broke that would be added
/// in this file, which is why the requirement is stated in this file.
///
/// It is also why the [`HostRuntime`] is *not* here: a runtime handle belongs to
/// the one thread its machine runs on, and folding it in would make the binding
/// per-thread or the runtime `Sync`, neither of which is true.
const _: fn() = || {
    fn shareable<T: Send + Sync>() {}
    shareable::<HostRegistry>();
    shareable::<HostBinding>();
};

/// The resource a `perform` names. A `perform` without a label is against the
/// operation's singleton resource, which is the same rule inference applies.
pub fn resource_of(resource: Option<&Symbol>) -> Resource {
    match resource {
        Some(r) => Resource::Named(r.clone()),
        None => Resource::Singleton,
    }
}

/// What a run reached across the boundary.
///
/// Reported, and the **authority** on whether a green verdict may be cached: the
/// static prediction drives selection, and this drives the cache write. When the
/// two disagree the run has observed a footprint that under-reports, which is
/// `E0427`.
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
///
/// Two groups, and one rule covering both: a handler may not mint a code that
/// means *the run watched its own machinery break*, and it may not mint a code
/// the boundary itself raises about the handler.
///
/// The first group is what makes this more than tidiness. `ply_test` reads
/// [`codes::INTERNAL_ERROR`] and the two divergence codes as "the evaluator
/// failed rather than the program": the failure becomes `Status::Panicked` and
/// the consumer is told to file a bug against Ply. A handler that returns one
/// has redirected the reader away from itself, and the E-codes are the
/// machine-readable contract an agent consumer acts on. The second group —
/// `E0421`–`E0428`, `E0504` — are verdicts about the machine's own state, which
/// a handler is not in a position to observe.
///
/// A handler's own refusals are [`codes::RUNTIME_ERROR`], which is what
/// [`attribute`] rewrites a reserved code to. The message, the labels and the
/// notes survive; only the classification and the added note change.
/// W4 adds only the three codes raised by `bind`, and deliberately not the five
/// the postgres driver raises from inside `call`. `E0432`, `E0433`, `E0434`,
/// `E0436` and `E0437` are refusals a handler is the only component in a
/// position to compute — a statement's table set, a result description, the task
/// holding a scope, a pool's occupancy — so reserving them would rewrite the
/// driver's own diagnosis to `E0502` and send the reader looking for a defect in
/// Ply. The rule is unchanged; it is the second group that they do not belong
/// to, because they are not verdicts about the machine's state.
pub const RESERVED_CODES: [&str; 21] = [
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
    codes::DB_NOT_CONFIGURED,
    codes::DB_SCHEMA_MISMATCH,
    codes::DB_UNMODELLED_SIDE_EFFECT,
    // Raised by the artifact loader, before any binding exists — so a handler
    // that answered with one would be claiming the program it is running failed
    // to load, which is a verdict about the machine's own state rather than
    // about anything the handler can see.
    codes::ARTIFACT_INVALID,
    codes::ARTIFACT_VERSION,
];

pub fn is_reserved_code(code: &str) -> bool {
    RESERVED_CODES.contains(&code)
}

/// Stamps a handler's refusal with where it came from.
///
/// Applied to every `Err` a [`HostHandler`] returns, so that no failure raised
/// across the boundary can be read as one raised by the evaluator. Two things
/// happen and both are needed:
///
/// - the note naming the handler and the operation, so the reader is pointed at
///   the trusted computing base rather than away from it;
/// - a reserved code is replaced by [`codes::RUNTIME_ERROR`], because the
///   classification a code decides is not a handler's to choose.
///
/// The severity is forced to `Error` for the same reason: an `Err` that rendered
/// as a warning would be a failure a summary counts as advice.
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
///
/// The sibling of a host operation *inside* a `simulate` region, and the same
/// hazard: `ply test` re-runs a searched test **whole** per interleaving, so an
/// operation written in the prefix or the suffix around the region is performed
/// once per schedule explored. A program whose source sends one packet sends
/// one per interleaving, and the run then reports the total as a proof over all
/// of them.
///
/// Refused before the handler is called, on the first interleaving, so the count
/// is zero rather than one.
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
///
/// The declaration says this handler leaves the machine's thread and answers a
/// token immediately; a value returned from `call` is the machine's own thread
/// having done the work. The scheduler's account of which of its threads are
/// free is then wrong, and every task in the region paid for it.
///
/// This catches the declaration's structural half. Nothing catches the other
/// half — a handler declared `blocking: false` that blocks anyway — and ADR 0008
/// §8 says so rather than implying otherwise.
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
///
/// Deliberately not `E0303`. That one means inference should have prevented the
/// perform and did not, and it calls for a bug report; this one means inference
/// was right, the row was legal, and the run was configured hermetically. A
/// consumer that could not tell them apart would do the wrong thing with both.
///
/// Raised by both engines from here rather than by each of them, because a
/// hermetic refusal that differed between the two would be an `E0503` over a
/// disagreement neither engine had about the program.
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

/// `E0424` — an operation this run knows how to serve and deliberately did not
/// bind.
///
/// The same code as [`err_hermetic`] and for the same reason: inference was
/// right, the row was legal, and the run was configured not to serve this. It is
/// raised by the boundary rather than by a handler, which is what keeps it out
/// of [`attribute`]'s rewrite — a handler may not mint a verdict about the
/// machine's own state, and "this run bound nothing here" is one.
///
/// `ply test` withholds `signal`, with or without `--host`. A stop requested
/// once ends every test after it, and a suite whose verdicts depend on the
/// terminal is the shared-state coupling every other mechanism here exists to
/// prevent.
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
///
/// It is a refusal to start rather than a failure, so it carries
/// [`codes::MACHINE_ONLY_CLAUSE`] and `is_machine_only` holds of it: under
/// `--engine both` the machine's answer is then the only one there is, which is
/// the point. Comparing an engine that cannot reach a socket against one that
/// did would report a divergence about the boundary rather than about the
/// program — and running both for real would send every packet twice.
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
///
/// A registration names the effect as declared, so a program with two modules
/// each declaring that name gives it two nominally distinct effects to answer
/// for. Serving both would put one socket table behind two effects that the type
/// system says are different things; serving whichever sorted first is the coin
/// flip over which real resource gets touched that `E0422` exists to refuse.
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
///
/// Two causes account for nearly every `E0421`, and this answers both. The
/// first is a registration written against the *simple* name when a program-wide
/// one was wanted — `net` where the program declares `ply.net.net` — which an
/// exact match on the last dotted component finds. The second is a rename on the
/// Ply side that the Rust side did not follow, which a shared prefix finds:
/// `store.db` against `store.dbs` beats every unrelated name.
///
/// Deliberately not an edit distance. A wrong suggestion costs a reader more
/// than no suggestion, so a prefix match has to cover at least half of the
/// shorter name.
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
