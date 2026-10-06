//! The `signal` effect, and the coordinator that turns a stop into a shutdown.

use crate::pool::Bell;
use crate::process::Children;
use ply_eval::host::HostRegistry;
use ply_eval::{
    Determinism, Diagnostic, HostAnswer, HostHandler, HostOp, HostRequest, HostResource,
    HostRuntime, Linearity, Span, Symbol, Value, codes,
};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

pub const MODULE: &str = "std.signal";

pub const EFFECT: &str = "std.signal.signal";

/// `--drain-ms`: how long in-flight requests have to finish once accept stops.
pub const DEFAULT_DRAIN_MS: u64 = 30_000;

/// `--drain-lead-ms`: how long accept keeps running after the signal.
pub const DEFAULT_LEAD_MS: u64 = 0;

/// How long [`Shutdown::park`] sleeps before giving the scheduler its turn back.
pub const DRAIN_POLL: Duration = Duration::from_millis(20);

/// How long a wake connection waits for this process's own listener.
const WAKE_TIMEOUT: Duration = Duration::from_millis(250);

/// How long the stop spends waking parked `accept`s before leaving them to the drain deadline.
const WAKE_BUDGET: Duration = Duration::from_millis(1_000);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShutdownSignal {
    Interrupt,
    Terminate,
}

impl ShutdownSignal {
    pub fn name(self) -> &'static str {
        match self {
            ShutdownSignal::Interrupt => "INT",
            ShutdownSignal::Terminate => "TERM",
        }
    }

    /// What a second signal exits with: `128 + n`, as if the signal had not been caught.
    pub fn exit_code(self) -> i32 {
        match self {
            ShutdownSignal::Interrupt => 130,
            ShutdownSignal::Terminate => 143,
        }
    }
}

/// A signal a program asks about. Asking is what installs its handler: one nobody asked about
/// does what the platform does with it, which for all but `WindowChange` is to end the process.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Asked {
    Hangup,
    User1,
    User2,
    WindowChange,
}

impl Asked {
    pub fn name(self) -> &'static str {
        match self {
            Asked::Hangup => "HUP",
            Asked::User1 => "USR1",
            Asked::User2 => "USR2",
            Asked::WindowChange => "WINCH",
        }
    }
}

/// One signal a program may ask about: whether its handler is installed, and whether it has
/// arrived since it was last asked about.
#[derive(Default)]
struct Watch {
    installed: Mutex<bool>,
    arrived: Arc<AtomicBool>,
}

/// `--drain-lead-ms` and `--drain-ms`.
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

pub trait Accepting: Send + Sync {
    /// Answer `0` to every further `net.accept`, close the listening sockets, and return any
    /// `accept` already parked on a pool thread.
    fn stop_accepting(&self) -> usize;

    fn listening_at(&self) -> Vec<SocketAddr>;

    /// Accepted connections the program has not closed.
    fn connections_in_flight(&self) -> usize;

    fn accepts_in_flight(&self) -> usize;
}

#[derive(Default)]
struct State {
    signal: Option<ShutdownSignal>,
    at: Option<Instant>,
    deadline: Option<Instant>,
    listeners_closed: usize,
    /// Connections open at the stop; the banner reports these as in flight.
    in_flight_at_stop: usize,
}

pub struct Shutdown {
    bounds: Bounds,
    /// The whole of what a signal handler touches.
    requested: AtomicBool,
    /// Set at the stop, so a later `net.accept` answers `0` even if the table is rebuilt.
    stopped_accepting: AtomicBool,
    second: AtomicBool,
    state: Mutex<State>,
    /// Signalled on the request and each phase end, so an idle park wakes before its bound.
    woke: Condvar,
    signals: Vec<ShutdownSignal>,
    net: Mutex<Option<Arc<dyn Accepting>>>,
    /// Weak, so the children still go when their host does.
    children: Mutex<Option<Weak<Children>>>,
    /// Rung with `woke`, so a reactor parked on a sleeper's deadline wakes for the stop too.
    bell: Mutex<Option<Arc<Bell>>>,
    /// Indexed by [`Asked`].
    watches: [Watch; 4],
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
            children: Mutex::new(None),
            bell: Mutex::new(None),
            watches: Default::default(),
        })
    }

    pub fn attach_bell(&self, bell: &Arc<Bell>) {
        *lock(&self.bell) = Some(Arc::clone(bell));
    }

    /// Wakes whatever waits on the stop: the request and each phase end.
    fn wake(&self) {
        self.woke.notify_all();
        if let Some(bell) = lock(&self.bell).as_ref() {
            bell.ring();
        }
    }

    /// Whether `which` has arrived since it was last asked about, however many times it did:
    /// arrivals are not counted. The first ask installs the handler and answers `false`.
    pub fn arrived(&self, which: Asked) -> std::io::Result<bool> {
        let watch = &self.watches[which as usize];
        let mut installed = lock(&watch.installed);
        if !*installed {
            watch_for(which, Arc::clone(&watch.arrived))?;
            *installed = true;
        }
        Ok(watch.arrived.swap(false, Ordering::AcqRel))
    }

    pub fn bounds(&self) -> Bounds {
        self.bounds
    }

    pub fn signals(&self) -> &[ShutdownSignal] {
        &self.signals
    }

    /// Hand over the socket table, catching up with a phase machine that has already run.
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
        // An `accept` posted before the close may still be parked inside it.
        wake_parked_accepts(net.as_ref());
        self.wake();
    }

    /// The children a second signal ends before it exits, since that exit skips the teardown.
    pub fn attach_children(&self, children: &Arc<Children>) {
        *lock(&self.children) = Some(Arc::downgrade(children));
    }

    fn end_children(&self) {
        let children = lock(&self.children).as_ref().and_then(Weak::upgrade);
        if let Some(children) = children {
            children.end_all();
        }
    }

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
            // Still in the lead: the rest of the lead plus the whole drain.
            (None, Some(at)) => {
                let lead_left = self.bounds.lead.saturating_sub(at.elapsed());
                lead_left + self.bounds.drain
            }
            (None, None) => self.bounds.drain,
        };
        left.as_millis().min(i64::MAX as u128) as i64
    }

    pub fn drain_expired(&self) -> bool {
        match lock(&self.state).deadline {
            Some(deadline) => Instant::now() >= deadline,
            None => false,
        }
    }

    pub fn stopped_accepting(&self) -> bool {
        self.stopped_accepting.load(Ordering::Acquire)
    }

    pub fn elapsed(&self) -> Option<Duration> {
        lock(&self.state).at.map(|at| at.elapsed())
    }

    pub fn signal(&self) -> Option<ShutdownSignal> {
        lock(&self.state).signal
    }

    /// What the stop found: listeners closed, and connections open.
    pub fn at_stop(&self) -> (usize, usize) {
        let state = lock(&self.state);
        (state.listeners_closed, state.in_flight_at_stop)
    }

    /// Sleep for at most `bound`, or until the stop moves to its next phase.
    pub fn park(&self, bound: Duration) {
        let state = lock(&self.state);
        let _ = self.woke.wait_timeout(state, bound);
    }

    /// Request a stop; `false` when one was already requested.
    pub fn request(self: &Arc<Shutdown>, signal: ShutdownSignal) -> bool {
        if self.requested.swap(true, Ordering::AcqRel) {
            self.second.store(true, Ordering::Release);
            self.wake();
            return false;
        }
        {
            let mut state = lock(&self.state);
            state.signal = Some(signal);
            state.at = Some(Instant::now());
        }
        self.wake();
        // Phases run on their own thread so the reactor can still notice a second signal.
        let coordinator = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("ply-host-drain".to_string())
            .spawn(move || coordinator.lead_then_stop());
        if spawned.is_err() {
            self.lead_then_stop();
        }
        true
    }

    pub fn second_requested(&self) -> bool {
        self.second.load(Ordering::Acquire)
    }

    /// Waits out the lead, then stops accepting and starts the drain.
    fn lead_then_stop(&self) {
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
            // The drain starts when accept stops, so a lead never shortens the drain.
            state.deadline = Some(Instant::now() + self.bounds.drain);
            net
        };
        self.wake();
        if let Some(net) = &net {
            wake_parked_accepts(net.as_ref());
        }
        self.wake();
    }
}

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
fn signals_of_this_platform() -> Vec<ShutdownSignal> {
    vec![ShutdownSignal::Interrupt, ShutdownSignal::Terminate]
}

#[cfg(not(unix))]
fn signals_of_this_platform() -> Vec<ShutdownSignal> {
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
fn deliver(shutdown: Arc<Shutdown>, which: ShutdownSignal) {
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
                ShutdownSignal::Interrupt => SignalKind::interrupt(),
                ShutdownSignal::Terminate => SignalKind::terminate(),
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

/// Installs the handler for `which` before it returns, on a thread of its own that marks each
/// arrival.
#[cfg(unix)]
fn watch_for(which: Asked, arrived: Arc<AtomicBool>) -> std::io::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};
    let (installed, told) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name(format!("ply-host-signal-{}", which.name().to_lowercase()))
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(e) => {
                    let _ = installed.send(Err(e));
                    return;
                }
            };
            runtime.block_on(async move {
                let kind = match which {
                    Asked::Hangup => SignalKind::hangup(),
                    Asked::User1 => SignalKind::user_defined1(),
                    Asked::User2 => SignalKind::user_defined2(),
                    Asked::WindowChange => SignalKind::window_change(),
                };
                let mut stream = match signal(kind) {
                    Ok(stream) => stream,
                    Err(e) => {
                        let _ = installed.send(Err(e));
                        return;
                    }
                };
                let _ = installed.send(Ok(()));
                while stream.recv().await.is_some() {
                    arrived.store(true, Ordering::Release);
                }
            });
        })?;
    told.recv().unwrap_or_else(|_| {
        Err(std::io::Error::other(
            "the thread that listens for it ended before it listened",
        ))
    })
}

#[cfg(not(unix))]
fn watch_for(which: Asked, _: Arc<AtomicBool>) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        format!("this platform has no SIG{}", which.name()),
    ))
}

fn exit_now(shutdown: &Arc<Shutdown>, which: ShutdownSignal) -> ! {
    let connections = lock(&shutdown.net)
        .as_ref()
        .map_or(0, |net| net.connections_in_flight());
    eprintln!(
        "   abandoned   {connections} connection{} in flight",
        if connections == 1 { "" } else { "s" },
    );
    shutdown.end_children();
    crate::stdio::settle();
    crate::observe::exiting();
    std::process::exit(which.exit_code());
}

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

pub fn register(registry: &mut HostRegistry, shutdown: Option<&Arc<Shutdown>>) {
    for (op, handler) in registrations(shutdown) {
        match shutdown {
            Some(_) => registry.register(op, handler),
            None => registry.register_withheld(op, handler, crate::process::ONLY_A_RUN),
        }
    }
}

operations! {
    what "signal";
    path "signal";
    Stopping = "stopping",
    DeadlineMs = "deadline_ms",
    Hangup = "hangup",
    User1 = "user1",
    User2 = "user2",
    Resized = "resized",
}

impl Op {
    /// The signal an operation asks about, which the stop's two readings do not.
    pub fn asks(self) -> Option<Asked> {
        match self {
            Op::Stopping | Op::DeadlineMs => None,
            Op::Hangup => Some(Asked::Hangup),
            Op::User1 => Some(Asked::User1),
            Op::User2 => Some(Asked::User2),
            Op::Resized => Some(Asked::WindowChange),
        }
    }

    pub fn declaration(self) -> HostOp {
        HostOp {
            effect: Symbol::new(EFFECT),
            op: Symbol::new(self.name()),
            resource: HostResource::Any,
            determinism: Determinism::Nondeterministic,
            // An ask takes the arrival it answers, so a second one answers otherwise.
            linearity: match self.asks() {
                None => Linearity::Repeatable,
                Some(_) => Linearity::AtMostOnce,
            },
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
        // A withheld registration is never resolved, so this is a dispatch bug.
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
        Ok(HostAnswer::Value(match (self.op, self.op.asks()) {
            (_, Some(which)) => Value::Bool(
                shutdown
                    .arrived(which)
                    .map_err(|e| unheard(self.op, which, &e, req.span))?,
            ),
            (Op::DeadlineMs, None) => Value::Int(shutdown.deadline_ms()),
            (_, None) => Value::Bool(shutdown.stopping()),
        }))
    }
}

#[cold]
fn unheard(op: Op, which: Asked, e: &std::io::Error, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("{} could not listen for SIG{}: {e}", op.what(), which.name()),
    )
    .primary(span, "asking about a signal installs its handler")
    .note("without the handler the signal would do what the platform does with it, so the ask is refused rather than answered `false`")
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

/// The guarded state has no invariant a panicking caller can break, so recovering is correct.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
