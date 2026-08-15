//! The `signal` effect, and the coordinator that turns a stop into a shutdown.
//!
//! [`DECLARATION`] is the Ply source this registers against — the module
//! `std.signal`, which ships with the compiler — and `HostRegistry::bind` is
//! what checks the two still agree.
//!
//! A way to stop is ambient in every other language: a global handler installed
//! by whoever got there first, observable from nowhere in a signature. Here it
//! is an effect, so a readiness route that sheds load while the instance is
//! draining says so in its row, and `ply check --types` answers "what does
//! readiness actually verify" out of the type.
//!
//! ## What a signal does, in order
//!
//! | phase | what happens | bound by |
//! | --- | --- | --- |
//! | 0 | the flag is set; `signal.stopping()` answers `true` from the next perform | — |
//! | 1 | **lead**: accept keeps running, so a readiness route can answer `503` and a load balancer can take the instance out | `--drain-lead-ms` |
//! | 2 | **stop accepting**: every `net.accept[s]` answers `0` and the listening sockets are closed | immediate |
//! | 3 | **drain**: in-flight requests finish | `--drain-ms` |
//! | 4 | **teardown**: the pinned order in [`crate::registry`] | — |
//! | 5 | **exit**: `0` if the drain completed, `3` if the deadline expired | — |
//!
//! Two rules decide the shape of everything below.
//!
//! **A signal handler sets a flag and does nothing else.** `tokio::signal`
//! delivers on a reactor of its own; [`Shutdown::request`] stores the flag and
//! hands the phase machine to a second thread, so the reactor is free to notice
//! a *second* signal while the first is still draining. A process that ignores
//! the second signal is a process people learn to `kill -9`, which abandons the
//! transaction rollback the teardown exists for and is strictly worse than
//! stopping now.
//!
//! **Nothing here ever touches a `Value`.** A `Value` holds `Rc` and belongs to
//! the machine's thread. The coordinator moves an `AtomicBool`, an `Instant` and
//! a socket, and the two Ply operations are read off the flag by
//! [`Operation::call`] on the machine's own thread.
//!
//! ## Why `signal` is withheld under `ply test`
//!
//! A stop requested once ends every test after it, and a suite whose verdicts
//! depend on the terminal is exactly the shared-state coupling the footprint
//! graph cannot see — W4's pooled connection in a new costume. So `ply test`
//! registers these operations [withheld]: they are in the trusted computing base
//! and in no binding, and reaching one is `E0424` naming `std.signal`'s twin
//! rather than `E0303`, which would send the reader looking for a bug in
//! inference.
//!
//! This is a deliberate asymmetry with `config`, whose frozen snapshot is
//! read-only and cannot couple two tests.
//!
//! [withheld]: ply_eval::HostRegistry::register_withheld

use ply_eval::host::HostRegistry;
use ply_eval::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRequest, HostResource, HostRuntime,
    Linearity, Value,
};
use ply_span::{Diagnostic, Span, Symbol, codes};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// The Ply declaration the registrations below are checked against.
pub const DECLARATION: &str = ply_std::SIGNAL;

/// The module the declaration ships as, which is what qualifies [`EFFECT`].
pub const MODULE: &str = "std.signal";

/// The program-wide effect name. Effect names are qualified (ADR 0001).
pub const EFFECT: &str = "std.signal.signal";

/// `--drain-ms`: how long in-flight requests have to finish once accept stops.
///
/// It should exceed the program's own `body_timeout_ms + write_timeout_ms`,
/// which for `http::default_limits()` is 60 seconds. The run cannot check that —
/// `Limits` is a Ply value the run never sees — so both numbers are printed at
/// start-up where they can be compared by eye.
pub const DEFAULT_DRAIN_MS: u64 = 30_000;

/// `--drain-lead-ms`: how long accept keeps running after the signal.
///
/// Zero, because the useful value is a deployment's and a default that waited
/// would make every `ctrl-C` in development take as long as a rolling restart.
pub const DEFAULT_LEAD_MS: u64 = 0;

/// How long [`Shutdown::park`] sleeps before giving the scheduler its turn back.
///
/// The drain deadline is observed between scheduling decisions, so a park that
/// blocked until a token resolved would let one task waiting on a host operation
/// that never finishes outlast the whole drain. Bounded, and only while
/// stopping: an ordinary park still blocks until there is something to do.
pub const DRAIN_POLL: Duration = Duration::from_millis(20);

/// How long a wake connection waits for this process's own listener.
const WAKE_TIMEOUT: Duration = Duration::from_millis(250);

/// How long phase 2 spends getting parked `accept`s to return before it gives up
/// and leaves them to the drain deadline.
const WAKE_BUDGET: Duration = Duration::from_millis(1_000);

/// Which signal arrived, and what a second one exits with.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Signal {
    Interrupt,
    Terminate,
}

impl Signal {
    pub fn name(self) -> &'static str {
        match self {
            Signal::Interrupt => "INT",
            Signal::Terminate => "TERM",
        }
    }

    /// What an immediate second signal exits with: the shell's own convention,
    /// `128 + n`, so a supervisor reads the same number it would have read from
    /// a process that did not catch the signal at all.
    pub fn exit_code(self) -> i32 {
        match self {
            Signal::Interrupt => 130,
            Signal::Terminate => 143,
        }
    }
}

/// The two knobs, as the command line supplies them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Bounds {
    pub lead: Duration,
    pub drain: Duration,
}

impl Default for Bounds {
    fn default() -> Bounds {
        Bounds {
            lead: Duration::from_millis(DEFAULT_LEAD_MS),
            drain: Duration::from_millis(DEFAULT_DRAIN_MS),
        }
    }
}

/// The listening half of the server, as the coordinator needs it.
///
/// A trait rather than an `Arc<TcpHost>` so that this module holds no socket:
/// what phase 2 needs is "stop answering accepts" and "how many connections are
/// open", and both are questions the thing that owns the sockets answers.
pub trait Accepting: Send + Sync {
    /// Answer `0` to every further `net.accept`, close the listening sockets,
    /// and return any `accept` already parked on a pool thread. Answers how many
    /// listeners were closed. Idempotent.
    fn stop_accepting(&self) -> usize;

    /// The addresses a parked `accept` is waiting on. Phase 2 dials each of them
    /// from this process, because a blocking `accept` returns for a connection
    /// and for nothing else — closing the descriptor underneath it is a race on
    /// every platform and wakes it on none of them portably.
    fn listening_at(&self) -> Vec<SocketAddr>;

    /// Accepted connections the program has not closed. What "in flight" means
    /// to a run that has no idea what a request is.
    fn connections_in_flight(&self) -> usize;

    /// `accept` operations parked on a pool thread right now.
    fn accepts_in_flight(&self) -> usize;
}

/// The transactional half, as the coordinator needs it for one line of output.
pub trait Transactions: Send + Sync {
    /// Transaction scopes open right now. Every one of them is rolled back at
    /// teardown and none of them is committed.
    fn open_scopes(&self) -> usize;
}

#[derive(Default)]
struct State {
    signal: Option<Signal>,
    /// When the stop was requested. The elapsed time the shutdown banner prints
    /// is measured from here and from nothing else.
    at: Option<Instant>,
    /// When the drain expires. Set at the end of the lead phase, so a run with a
    /// lead is not draining while it is still accepting.
    deadline: Option<Instant>,
    listeners_closed: usize,
    /// Connections open when phase 2 finished, which is the number the shutdown
    /// banner reports as in flight.
    in_flight_at_stop: usize,
    scopes_at_stop: usize,
}

/// The stop flag, the phases, and the answers the two Ply operations read.
///
/// One per run, shared by `Arc`: the signal reactor, the coordinator thread and
/// the machine's thread all hold it, and a run with two would have two answers
/// to whether it is stopping.
pub struct Shutdown {
    bounds: Bounds,
    /// The whole of what a signal handler touches. Separate from `state` because
    /// a mutex is not what a delivery path may take.
    requested: AtomicBool,
    /// Set once phase 2 has run, so a `net.accept` after it answers `0` even if
    /// the socket table was rebuilt.
    stopped_accepting: AtomicBool,
    second: AtomicBool,
    state: Mutex<State>,
    /// Signalled when the stop is requested and at the end of each phase, so a
    /// park with nothing outstanding wakes rather than sleeping out its bound.
    woke: Condvar,
    signals: Vec<Signal>,
    net: Mutex<Option<Arc<dyn Accepting>>>,
    db: Mutex<Option<Arc<dyn Transactions>>>,
}

impl Shutdown {
    pub fn new(bounds: Bounds) -> Arc<Shutdown> {
        Arc::new(Shutdown {
            bounds,
            requested: AtomicBool::new(false),
            stopped_accepting: AtomicBool::new(false),
            second: AtomicBool::new(false),
            state: Mutex::new(State::default()),
            woke: Condvar::new(),
            signals: signals_of_this_platform(),
            net: Mutex::new(None),
            db: Mutex::new(None),
        })
    }

    pub fn bounds(&self) -> Bounds {
        self.bounds
    }

    /// Which signals this run is listening for. `SIGTERM` does not exist on
    /// Windows, so the difference is a fact `ply hosts` prints rather than a
    /// surprise a deployment discovers.
    pub fn signals(&self) -> &[Signal] {
        &self.signals
    }

    /// Hand the coordinator the socket table, **catching up** with a phase
    /// machine that has already run.
    ///
    /// `ply run --host` calls `signal::listen` before it loads the TLS material,
    /// opens the pool — including a real connect probe bounded by
    /// `--db-connect-ms` — binds the registry and verifies the schema, and only
    /// then attaches this. A `SIGTERM` in that window is the ordinary shape of a
    /// rolling restart against an instance that is still coming up, and without
    /// the catch-up below it produced the worst available answer: `stopping()`
    /// true, so a readiness route sheds and the load balancer takes the instance
    /// out, while the listener stays open and keeps serving until the drain
    /// deadline turns a clean stop into `W0608` and exit `3`.
    ///
    /// The lock discipline is what makes it a decision rather than a race. The
    /// `net` slot is held across the read *and* the flag test, and
    /// [`Shutdown::run_phases`] sets `stopped_accepting` while holding the same
    /// slot — so either phase 2 saw this table, or this sees phase 2. There is
    /// no interleaving in which neither happens.
    pub fn attach_net(&self, net: Arc<dyn Accepting>) {
        let mut slot = lock(&self.net);
        *slot = Some(Arc::clone(&net));
        if !self.stopped_accepting.load(Ordering::Acquire) {
            return;
        }
        let closed = net.stop_accepting();
        let mut state = lock(&self.state);
        state.listeners_closed += closed;
        state.in_flight_at_stop = net.connections_in_flight();
        drop(state);
        drop(slot);
        // The listener is closed, but an `accept` posted before this may be
        // parked inside it; the same dial phase 2 would have done gets it back.
        wake_parked_accepts(net.as_ref());
        self.woke.notify_all();
    }

    pub fn attach_db(&self, db: Arc<dyn Transactions>) {
        *lock(&self.db) = Some(db);
    }

    /// Whether a stop has been requested. What `signal.stopping()` answers and
    /// what `HostRuntime::stopping` reports to the scheduler.
    pub fn stopping(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }

    /// Milliseconds left before the run stops scheduling, and `-1` when no stop
    /// has been requested.
    ///
    /// During the lead phase it is the lead plus the whole drain, because that
    /// is the question the operation exists to answer: a handler asks whether
    /// there is time to finish work it is about to start, and the lead is time.
    pub fn deadline_ms(&self) -> i64 {
        if !self.stopping() {
            return -1;
        }
        let state = lock(&self.state);
        let left = match (state.deadline, state.at) {
            (Some(deadline), _) => deadline.saturating_duration_since(Instant::now()),
            // Still in the lead: the drain has not started, so what is left is
            // whatever the lead has plus the whole of it.
            (None, Some(at)) => {
                let lead_left = self.bounds.lead.saturating_sub(at.elapsed());
                lead_left + self.bounds.drain
            }
            (None, None) => self.bounds.drain,
        };
        left.as_millis().min(i64::MAX as u128) as i64
    }

    /// Whether the drain deadline has passed. `false` during the lead, because
    /// the drain has not begun.
    pub fn drain_expired(&self) -> bool {
        match lock(&self.state).deadline {
            Some(deadline) => Instant::now() >= deadline,
            None => false,
        }
    }

    /// Whether accept has been stopped, which is phase 2 having run.
    pub fn stopped_accepting(&self) -> bool {
        self.stopped_accepting.load(Ordering::Acquire)
    }

    /// How long since the stop was requested, for the shutdown banner.
    pub fn elapsed(&self) -> Option<Duration> {
        lock(&self.state).at.map(|at| at.elapsed())
    }

    pub fn signal(&self) -> Option<Signal> {
        lock(&self.state).signal
    }

    /// What phase 2 found: listeners closed, connections open, scopes open.
    pub fn at_stop(&self) -> (usize, usize, usize) {
        let state = lock(&self.state);
        (
            state.listeners_closed,
            state.in_flight_at_stop,
            state.scopes_at_stop,
        )
    }

    /// Sleep for at most `bound`, or until the stop moves to its next phase.
    ///
    /// What `HostRuntime::park` calls when a stop is in progress and there is
    /// nothing outstanding to wait on. It never blocks indefinitely: the drain
    /// deadline is checked by the scheduler between turns, so a park that waited
    /// for a token would let one request that never finishes outlast the drain
    /// it was supposed to be bounded by.
    pub fn park(&self, bound: Duration) {
        let state = lock(&self.state);
        let _ = self.woke.wait_timeout(state, bound);
    }

    /// A stop, from the signal reactor or from a test.
    ///
    /// Returns `false` when a stop was already in progress, which is the second
    /// signal: a person has decided to stop waiting, and the caller exits.
    pub fn request(self: &Arc<Shutdown>, signal: Signal) -> bool {
        if self.requested.swap(true, Ordering::AcqRel) {
            self.second.store(true, Ordering::Release);
            self.woke.notify_all();
            return false;
        }
        {
            let mut state = lock(&self.state);
            state.signal = Some(signal);
            state.at = Some(Instant::now());
        }
        self.woke.notify_all();
        // The phase machine on a thread of its own, so the reactor that
        // delivered this is free to notice a second signal while the first is
        // still leading or draining.
        let coordinator = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("ply-host-drain".to_string())
            .spawn(move || coordinator.run_phases());
        if spawned.is_err() {
            // No thread to run the phases on, so run them here. Blocking the
            // delivery thread is worse than a stop that never stops accepting.
            self.run_phases();
        }
        true
    }

    /// Whether a second signal has arrived. The caller — never a signal handler
    /// — is what exits on it.
    pub fn second_requested(&self) -> bool {
        self.second.load(Ordering::Acquire)
    }

    /// Phases 1 and 2. Phase 3 is the scheduler continuing to run, phase 4 is
    /// the teardown the machine's thread performs, and phase 5 is the exit code.
    /// Phases 1 and 2. Phase 3 is the scheduler continuing to run, phase 4 is
    /// the teardown the machine's thread performs, and phase 5 is the exit code.
    ///
    /// **What the banner reads is written before the run can observe the stop.**
    /// `stop_accepting` is the instant `net.accept` starts answering `0`, which
    /// is what ends a sequential accept loop — so a machine thread can be
    /// through the drain, the teardown and the banner in a couple of
    /// milliseconds. Taking the state lock *around* that call is what makes
    /// `at_stop()` a fact the run already holds rather than one it is about to:
    /// a reader of the banner blocks on this section instead of reading three
    /// zeroes for a run that had a listener, a connection and a transaction.
    ///
    /// `wake_parked_accepts` is deliberately outside it. It sleeps five
    /// milliseconds a round against a whole second of budget, it is waiting on
    /// nothing the banner reports, and holding either lock across it is what put
    /// the write after the stop in the first place.
    fn run_phases(&self) {
        if !self.bounds.lead.is_zero() {
            let state = lock(&self.state);
            let _ = self.woke.wait_timeout(state, self.bounds.lead);
        }
        let net = {
            // `net` then `state`, which is the order `attach_net` and `exit_now`
            // take them in too.
            let slot = lock(&self.net);
            let net = slot.clone();
            let mut state = lock(&self.state);
            let closed = net.as_ref().map_or(0, |n| n.stop_accepting());
            self.stopped_accepting.store(true, Ordering::Release);
            state.listeners_closed = closed;
            state.in_flight_at_stop = net.as_ref().map_or(0, |n| n.connections_in_flight());
            state.scopes_at_stop = lock(&self.db).as_ref().map_or(0, |db| db.open_scopes());
            // The drain starts when accept stops, not when the signal arrived:
            // a lead is time the operator asked for and charging it to the drain
            // would silently shorten the drain by the lead.
            state.deadline = Some(Instant::now() + self.bounds.drain);
            net
        };
        self.woke.notify_all();
        if let Some(net) = &net {
            wake_parked_accepts(net.as_ref());
        }
        self.woke.notify_all();
    }
}

/// Get every parked `accept` to return.
///
/// A blocking `accept` returns for a connection and for nothing else. Closing
/// the descriptor underneath one is a race with the thread inside it and does
/// not wake it portably — `shutdown(2)` on a listening socket is `ENOTCONN` on
/// the BSDs — so the portable move is to *be* the connection: dial the listener
/// from this process, and the job that was waiting takes it, sees the stop flag
/// and answers `0` after closing what it accepted.
///
/// Bounded on both sides. One dial per listener per round, because a service
/// with several acceptor tasks has several parked accepts and one connection
/// wakes exactly one of them; and a total budget, because an accept still parked
/// after it belongs to the drain deadline rather than to this loop.
fn wake_parked_accepts(net: &dyn Accepting) {
    let until = Instant::now() + WAKE_BUDGET;
    while net.accepts_in_flight() > 0 && Instant::now() < until {
        let addresses = net.listening_at();
        if addresses.is_empty() {
            return;
        }
        for address in addresses {
            if let Ok(stream) = TcpStream::connect_timeout(&address, WAKE_TIMEOUT) {
                drop(stream);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(unix)]
fn signals_of_this_platform() -> Vec<Signal> {
    vec![Signal::Interrupt, Signal::Terminate]
}

#[cfg(not(unix))]
fn signals_of_this_platform() -> Vec<Signal> {
    vec![Signal::Interrupt]
}

/// Register with the operating system, on a thread of this coordinator's own.
///
/// ADR 0015 §4.2 puts this on the reactor thread the db driver already owns.
/// It is a thread of its own instead, for one reason stated rather than hidden:
/// a run with no `--db` has no reactor, and a service that listened for
/// `SIGTERM` only when it had a database would be a shutdown story that depended
/// on an unrelated flag. It is one `tokio` current-thread runtime that owns no
/// sockets and holds no `Value`.
pub fn listen(shutdown: &Arc<Shutdown>) -> Result<(), Diagnostic> {
    for which in shutdown.signals().to_vec() {
        let coordinator = Arc::clone(shutdown);
        std::thread::Builder::new()
            .name(format!("ply-host-signal-{}", which.name().to_lowercase()))
            .spawn(move || deliver(coordinator, which))
            .map_err(|e| {
                Diagnostic::error(
                    codes::RUNTIME_ERROR,
                    format!("the run could not start a thread to listen for signals: {e}"),
                )
                .note("without one a `SIGTERM` would end the process where a drain should have started")
            })?;
    }
    Ok(())
}

/// One thread and one current-thread `tokio` runtime per signal.
///
/// Two threads rather than one runtime awaiting both, because awaiting two
/// streams together is `tokio::select!` and that is the `macros` feature — one
/// more thing in the dependency tree for a pair of threads that are asleep for
/// the life of the run.
fn deliver(shutdown: Arc<Shutdown>, which: Signal) {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return;
    };
    runtime.block_on(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            let kind = match which {
                Signal::Interrupt => SignalKind::interrupt(),
                Signal::Terminate => SignalKind::terminate(),
            };
            let Ok(mut stream) = signal(kind) else {
                return;
            };
            loop {
                if stream.recv().await.is_none() {
                    return;
                }
                if !shutdown.request(which) {
                    exit_now(&shutdown, which);
                }
            }
        }
        #[cfg(not(unix))]
        {
            loop {
                if tokio::signal::ctrl_c().await.is_err() {
                    return;
                }
                if !shutdown.request(which) {
                    exit_now(&shutdown, which);
                }
            }
        }
    });
}

/// The second signal. Print what is being abandoned, then go.
///
/// One line, on stderr, before the exit: a person who pressed ctrl-C twice has
/// decided to stop waiting, and what they need is to know what it cost rather
/// than to be made to wait longer. Written from the delivery thread because
/// there is nothing left to hand it to — and a second signal means a person has
/// decided that whatever the drain is still doing is not worth the wait, which
/// a process that ignores it turns into a `kill -9` that abandons the rollback
/// as well.
fn exit_now(shutdown: &Arc<Shutdown>, which: Signal) -> ! {
    let connections = lock(&shutdown.net)
        .as_ref()
        .map_or(0, |net| net.connections_in_flight());
    let scopes = lock(&shutdown.db).as_ref().map_or(0, |db| db.open_scopes());
    eprintln!(
        "   abandoned   {connections} connection{} in flight · {scopes} transaction{} open · nothing was committed",
        if connections == 1 { "" } else { "s" },
        if scopes == 1 { "" } else { "s" },
    );
    std::process::exit(which.exit_code());
}

/// The two operations, both `Repeatable` and neither blocking.
///
/// `Repeatable` because reading a flag twice is the definition of harmless: a
/// multi-shot continuation captured across `signal.stopping()` is unaffected,
/// which keeps ADR 0011 §3's over-approximation tight.
///
/// `HostResource::Any` and not `Only(Singleton)` for the reason `task.*` gives:
/// an `Only` registration whose atom the program never performs is `E0421`, and
/// this registry is compiled into every program, most of which never stop.
pub fn registrations(shutdown: Option<&Arc<Shutdown>>) -> Vec<(HostOp, Arc<dyn HostHandler>)> {
    Op::ALL
        .iter()
        .map(|op| {
            let handler: Arc<dyn HostHandler> = Arc::new(Operation {
                op: *op,
                shutdown: shutdown.cloned(),
            });
            (op.declaration(), handler)
        })
        .collect()
}

/// Register the two operations, bound when this run has a coordinator and
/// withheld when it does not.
pub fn register(registry: &mut HostRegistry, shutdown: Option<&Arc<Shutdown>>) {
    for (op, handler) in registrations(shutdown) {
        match shutdown {
            Some(_) => registry.register(op, handler),
            None => registry.register_withheld(op, handler),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    Stopping,
    DeadlineMs,
}

impl Op {
    pub const ALL: [Op; 2] = [Op::Stopping, Op::DeadlineMs];

    pub fn name(self) -> &'static str {
        match self {
            Op::Stopping => "stopping",
            Op::DeadlineMs => "deadline_ms",
        }
    }

    pub fn what(self) -> &'static str {
        match self {
            Op::Stopping => "`signal.stopping`",
            Op::DeadlineMs => "`signal.deadline_ms`",
        }
    }

    pub fn path(self) -> &'static str {
        match self {
            Op::Stopping => "ply_host::signal::stopping",
            Op::DeadlineMs => "ply_host::signal::deadline_ms",
        }
    }

    fn declaration(self) -> HostOp {
        HostOp {
            effect: Symbol::new(EFFECT),
            op: Symbol::new(self.name()),
            resource: HostResource::Any,
            determinism: Determinism::Nondeterministic,
            linearity: Linearity::Repeatable,
            blocking: false,
            secrets: false,
            path: self.path(),
        }
    }
}

struct Operation {
    op: Op,
    shutdown: Option<Arc<Shutdown>>,
}

impl HostHandler for Operation {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        if !req.args.is_empty() {
            return Err(arity(self.op, req.args.len(), req.span));
        }
        // A withheld registration is never resolved, so reaching here without a
        // coordinator means the boundary dispatched one it had declined to bind.
        let Some(shutdown) = &self.shutdown else {
            return Err(Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!(
                    "{} was dispatched to a handler this run withheld",
                    self.op.what()
                ),
            )
            .primary(req.span, "performed here")
            .note("`ply test` registers `signal` withheld, and a withheld registration is in no binding index")
            .note("this is a defect in Ply's host dispatch rather than in the program"));
        };
        Ok(HostAnswer::Value(match self.op {
            Op::Stopping => Value::Bool(shutdown.stopping()),
            Op::DeadlineMs => Value::Int(shutdown.deadline_ms()),
        }))
    }
}

#[cold]
fn arity(op: Op, got: usize, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("{} was performed with {got} arguments and takes none", op.what()),
    )
    .primary(span, "this perform reached the host handler")
    .note("inference checks a perform's arity, so reaching this means the evaluator was handed a module that was never checked")
}

/// See `tcp::lock`: the state behind these has no invariant a panicking caller
/// can break, so recovering is correct and propagating would take out the
/// machine's thread as well.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests;
