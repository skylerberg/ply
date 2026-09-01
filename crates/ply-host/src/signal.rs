//! The `signal` effect, and the coordinator that turns a stop into a shutdown.

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

/// The program-wide effect name.
pub const EFFECT: &str = "std.signal.signal";

/// `--drain-ms`: how long in-flight requests have to finish once accept stops.
pub const DEFAULT_DRAIN_MS: u64 = 30_000;

/// `--drain-lead-ms`: how long accept keeps running after the signal.
pub const DEFAULT_LEAD_MS: u64 = 0;

/// How long [`Shutdown::park`] sleeps before giving the scheduler its turn back.
pub const DRAIN_POLL: Duration = Duration::from_millis(20);

/// How long a wake connection waits for this process's own listener.
const WAKE_TIMEOUT: Duration = Duration::from_millis(250);

/// How long phase 2 spends getting parked `accept`s to return before it gives up and leaves them to
/// the drain deadline.
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

    /// What an immediate second signal exits with: the shell's own convention, `128 + n`, so a
    /// supervisor reads the same number it would have read from a process that did not catch the
    /// signal at all.
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
pub trait Accepting: Send + Sync {
    /// Answer `0` to every further `net.accept`, close the listening sockets, and return any
    /// `accept` already parked on a pool thread.
    fn stop_accepting(&self) -> usize;

    /// The addresses a parked `accept` is waiting on.
    fn listening_at(&self) -> Vec<SocketAddr>;

    /// Accepted connections the program has not closed.
    fn connections_in_flight(&self) -> usize;

    /// `accept` operations parked on a pool thread right now.
    fn accepts_in_flight(&self) -> usize;
}

/// The transactional half, as the coordinator needs it for one line of output.
pub trait Transactions: Send + Sync {
    /// Transaction scopes open right now.
    fn open_scopes(&self) -> usize;
}

#[derive(Default)]
struct State {
    signal: Option<Signal>,
    /// When the stop was requested.
    at: Option<Instant>,
    /// When the drain expires.
    deadline: Option<Instant>,
    listeners_closed: usize,
    /// Connections open when phase 2 finished, which is the number the shutdown banner reports as
    /// in flight.
    in_flight_at_stop: usize,
    scopes_at_stop: usize,
}

/// The stop flag, the phases, and the answers the two Ply operations read.
pub struct Shutdown {
    bounds: Bounds,
    /// The whole of what a signal handler touches.
    requested: AtomicBool,
    /// Set once phase 2 has run, so a `net.accept` after it answers `0` even if the socket table
    /// was rebuilt.
    stopped_accepting: AtomicBool,
    second: AtomicBool,
    state: Mutex<State>,
    /// Signalled when the stop is requested and at the end of each phase, so a park with nothing
    /// outstanding wakes rather than sleeping out its bound.
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

    /// Which signals this run is listening for.
    pub fn signals(&self) -> &[Signal] {
        &self.signals
    }

    /// Hand the coordinator the socket table, **catching up** with a phase machine that has already
    /// run.
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
        // The listener is closed, but an `accept` posted before this may be parked inside it; the
        // same dial phase 2 would have done gets it back.
        wake_parked_accepts(net.as_ref());
        self.woke.notify_all();
    }

    pub fn attach_db(&self, db: Arc<dyn Transactions>) {
        *lock(&self.db) = Some(db);
    }

    /// Whether a stop has been requested.
    pub fn stopping(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }

    /// Milliseconds left before the run stops scheduling, and `-1` when no stop has been requested.
    pub fn deadline_ms(&self) -> i64 {
        if !self.stopping() {
            return -1;
        }
        let state = lock(&self.state);
        let left = match (state.deadline, state.at) {
            (Some(deadline), _) => deadline.saturating_duration_since(Instant::now()),
            // Still in the lead: the drain has not started, so what is left is whatever the lead
            // has plus the whole of it.
            (None, Some(at)) => {
                let lead_left = self.bounds.lead.saturating_sub(at.elapsed());
                lead_left + self.bounds.drain
            }
            (None, None) => self.bounds.drain,
        };
        left.as_millis().min(i64::MAX as u128) as i64
    }

    /// Whether the drain deadline has passed.
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
    pub fn park(&self, bound: Duration) {
        let state = lock(&self.state);
        let _ = self.woke.wait_timeout(state, bound);
    }

    /// A stop, from the signal reactor or from a test.
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
        // The phase machine on a thread of its own, so the reactor that delivered this is free to
        // notice a second signal while the first is still leading or draining.
        let coordinator = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("ply-host-drain".to_string())
            .spawn(move || coordinator.run_phases());
        if spawned.is_err() {
            // No thread to run the phases on, so run them here.
            self.run_phases();
        }
        true
    }

    /// Whether a second signal has arrived.
    pub fn second_requested(&self) -> bool {
        self.second.load(Ordering::Acquire)
    }

    /// Phases 1 and 2.
    fn run_phases(&self) {
        if !self.bounds.lead.is_zero() {
            let state = lock(&self.state);
            let _ = self.woke.wait_timeout(state, self.bounds.lead);
        }
        let net = {
            // `net` then `state`, which is the order `attach_net` and `exit_now` take them in too.
            let slot = lock(&self.net);
            let net = slot.clone();
            let mut state = lock(&self.state);
            let closed = net.as_ref().map_or(0, |n| n.stop_accepting());
            self.stopped_accepting.store(true, Ordering::Release);
            state.listeners_closed = closed;
            state.in_flight_at_stop = net.as_ref().map_or(0, |n| n.connections_in_flight());
            state.scopes_at_stop = lock(&self.db).as_ref().map_or(0, |db| db.open_scopes());
            // The drain starts when accept stops, not when the signal arrived: a lead is time the
            // operator asked for and charging it to the drain would silently shorten the drain by
            // the lead.
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

/// The second signal.
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

/// Register the two operations, bound when this run has a coordinator and withheld when it does
/// not.
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
        // A withheld registration is never resolved, so reaching here without a coordinator means
        // the boundary dispatched one it had declined to bind.
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

/// See `tcp::lock`: the state behind these has no invariant a panicking caller can break, so
/// recovering is correct and propagating would take out the machine's thread as well.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests;
