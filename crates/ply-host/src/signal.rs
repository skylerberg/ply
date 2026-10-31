use crate::pool::WakeCounter;
use crate::process::Children;
use ply_core::{Diagnostic, Span, codes};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

pub const DEFAULT_DRAIN_MS: u64 = 30_000;

pub const DEFAULT_STOP_DELAY_MS: u64 = 0;

pub const DRAIN_POLL: Duration = Duration::from_millis(20);

const WAKE_TIMEOUT: Duration = Duration::from_millis(250);

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

    pub fn exit_code(self) -> i32 {
        match self {
            ShutdownSignal::Interrupt => 130,
            ShutdownSignal::Terminate => 143,
        }
    }
}

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

#[derive(Default)]
struct Watch {
    installed: Mutex<bool>,
    arrived: Arc<AtomicBool>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Bounds {
    pub lead: Duration,
    pub drain: Duration,
}

impl Default for Bounds {
    fn default() -> Bounds {
        Bounds {
            lead: Duration::from_millis(DEFAULT_STOP_DELAY_MS),
            drain: Duration::from_millis(DEFAULT_DRAIN_MS),
        }
    }
}

pub trait Accepting: Send + Sync {
    fn stop_accepting(&self) -> usize;

    fn listening_at(&self) -> Vec<SocketAddr>;

    fn connections_in_flight(&self) -> usize;

    fn accepts_in_flight(&self) -> usize;
}

#[derive(Default)]
struct State {
    signal: Option<ShutdownSignal>,
    at: Option<Instant>,
    deadline: Option<Instant>,
    listeners_closed: usize,
    in_flight_at_stop: usize,
}

pub struct Shutdown {
    bounds: Bounds,
    drains: bool,
    requested: AtomicBool,
    stopped_accepting: AtomicBool,
    second: AtomicBool,
    state: Mutex<State>,
    woke: Condvar,
    signals: Vec<ShutdownSignal>,
    net: Mutex<Option<Arc<dyn Accepting>>>,
    children: Mutex<Option<Weak<Children>>>,
    bell: Mutex<Option<Arc<WakeCounter>>>,
}

static WATCHES: std::sync::LazyLock<[Watch; 4]> = std::sync::LazyLock::new(Default::default);

impl Shutdown {
    pub fn arrived(&self, which: Asked) -> std::io::Result<bool> {
        let watch = &WATCHES[which as usize];
        let mut installed = lock(&watch.installed);
        if !*installed {
            watch_for(which, Arc::clone(&watch.arrived))?;
            *installed = true;
        }
        Ok(watch.arrived.swap(false, Ordering::AcqRel))
    }

    pub fn new(bounds: Bounds) -> Arc<Shutdown> {
        Shutdown::made(bounds, true)
    }

    pub fn without_drain() -> Arc<Shutdown> {
        Shutdown::made(Bounds::default(), false)
    }

    fn made(bounds: Bounds, drains: bool) -> Arc<Shutdown> {
        Arc::new(Shutdown {
            bounds,
            drains,
            requested: AtomicBool::new(false),
            stopped_accepting: AtomicBool::new(false),
            second: AtomicBool::new(false),
            state: Mutex::new(State::default()),
            woke: Condvar::new(),
            signals: signals_of_this_platform(),
            net: Mutex::new(None),
            children: Mutex::new(None),
            bell: Mutex::new(None),
        })
    }

    pub fn attach_notifier(&self, bell: &Arc<WakeCounter>) {
        *lock(&self.bell) = Some(Arc::clone(bell));
    }

    fn wake(&self) {
        self.woke.notify_all();
        if let Some(bell) = lock(&self.bell).as_ref() {
            bell.ring();
        }
    }

    pub fn bounds(&self) -> Bounds {
        self.bounds
    }

    pub fn signals(&self) -> &[ShutdownSignal] {
        &self.signals
    }

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
        wake_parked_accepts(net.as_ref());
        self.wake();
    }

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

    pub fn deadline_ms(&self) -> i64 {
        if !self.stopping() {
            return -1;
        }
        let state = lock(&self.state);
        let left = match (state.deadline, state.at) {
            (Some(deadline), _) => deadline.saturating_duration_since(Instant::now()),
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

    pub fn stop_counts(&self) -> (usize, usize) {
        let state = lock(&self.state);
        (state.listeners_closed, state.in_flight_at_stop)
    }

    pub fn park(&self, bound: Duration) {
        let state = lock(&self.state);
        let _ = self.woke.wait_timeout(state, bound);
    }

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
        let coordinator = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("ply-host-drain".to_string())
            .spawn(move || coordinator.delay_then_stop_accepting());
        if spawned.is_err() {
            self.delay_then_stop_accepting();
        }
        true
    }

    pub fn second_requested(&self) -> bool {
        self.second.load(Ordering::Acquire)
    }

    fn delay_then_stop_accepting(&self) {
        if !self.bounds.lead.is_zero() {
            let state = lock(&self.state);
            let _ = self.woke.wait_timeout(state, self.bounds.lead);
        }
        let net = {
            let slot = lock(&self.net);
            let net = slot.clone();
            let mut state = lock(&self.state);
            let closed = net.as_ref().map_or(0, |n| n.stop_accepting());
            self.stopped_accepting.store(true, Ordering::Release);
            state.listeners_closed = closed;
            state.in_flight_at_stop = net.as_ref().map_or(0, |n| n.connections_in_flight());
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
                if !shutdown.drains || !shutdown.request(which) {
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
                if !shutdown.drains || !shutdown.request(which) {
                    exit_now(&shutdown, which);
                }
            }
        }
    });
}

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
    if shutdown.drains {
        let connections = lock(&shutdown.net)
            .as_ref()
            .map_or(0, |net| net.connections_in_flight());
        eprintln!(
            "   abandoned   {connections} connection{} in flight",
            if connections == 1 { "" } else { "s" },
        );
    }
    shutdown.end_children();
    crate::stdio::settle();
    crate::observe::report_at_exit();
    std::process::exit(which.exit_code());
}

#[cold]
pub fn err_signal_watch_failed(
    what: &str,
    which: Asked,
    e: &std::io::Error,
    span: Span,
) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("{what} could not listen for SIG{}: {e}", which.name()),
    )
    .primary(span, "asking about a signal installs its handler")
    .note("without the handler the signal would do what the platform does with it, so the ask fails rather than returning `false`")
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
