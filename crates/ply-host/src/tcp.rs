//! The `net` effect and the two handlers that serve it.

mod pool;
mod sim;
mod socket;

pub use pool::MAX_BLOCKING_OPERATIONS;
pub use sim::SimNet;
pub use socket::TcpHost;

use ply_core::ty::Resource;
use ply_eval::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRegistry, HostRequest, HostResource,
    HostRuntime, Linearity,
};
use ply_span::{Diagnostic, Span, Symbol, codes};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// The Ply declaration the registrations below are checked against: the source of the module
/// `std.net`, which ships with the compiler.
pub const DECLARATION: &str = ply_std::NET;

/// The module the declaration ships as, which is what qualifies [`EFFECT`].
pub const MODULE: &str = "std.net";

/// The program-wide effect name.
pub const EFFECT: &str = "std.net.net";

/// The most bytes one `recv` allocates for, whatever `max` asks.
pub const MAX_RECV: usize = 1 << 20;

/// The six operations.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    Listen,
    ListenTls,
    Accept,
    Recv,
    Send,
    Close,
}

impl Op {
    pub const ALL: [Op; 6] = [
        Op::Listen,
        Op::ListenTls,
        Op::Accept,
        Op::Recv,
        Op::Send,
        Op::Close,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Op::Listen => "listen",
            Op::ListenTls => "listen_tls",
            Op::Accept => "accept",
            Op::Recv => "recv",
            Op::Send => "send",
            Op::Close => "close",
        }
    }

    /// How a diagnostic names it.
    pub fn what(self) -> &'static str {
        match self {
            Op::Listen => "`net.listen`",
            Op::ListenTls => "`net.listen_tls`",
            Op::Accept => "`net.accept`",
            Op::Recv => "`net.recv`",
            Op::Send => "`net.send`",
            Op::Close => "`net.close`",
        }
    }

    fn arity(self) -> usize {
        match self {
            Op::Listen | Op::Accept | Op::Close => 1,
            Op::ListenTls => 2,
            // The deadline is the third argument.
            Op::Recv | Op::Send => 3,
        }
    }

    /// Whether the operation has to wait on a peer.
    fn waits(self) -> bool {
        matches!(self, Op::Accept | Op::Recv | Op::Send)
    }

    /// The registration.
    fn declaration(self, net: &dyn Net) -> HostOp {
        HostOp {
            effect: Symbol::new(EFFECT),
            op: Symbol::new(self.name()),
            // Whichever labels the program uses.
            resource: HostResource::Any,
            determinism: Determinism::Nondeterministic,
            linearity: Linearity::AtMostOnce,
            blocking: self.waits() && net.waits(),
            // A socket write takes `Bytes`, and `bytes_of_string` takes a `String`: no expression
            // turns a `Secret` into either.
            secrets: false,
            path: net.path(self),
        }
    }
}

/// What a `net` implementation has to answer.
pub trait Net: Send + Sync {
    /// Whether this implementation's waiting operations leave the machine's thread.
    fn waits(&self) -> bool;

    /// The Rust path `ply hosts` prints.
    fn path(&self, op: Op) -> &'static str;

    fn listen(&self, at: &Resource, port: u16, span: Span) -> Result<HostAnswer, Diagnostic>;
    /// The same listener, terminating TLS.
    fn listen_tls(
        &self,
        at: &Resource,
        port: u16,
        credential: &str,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic>;
    fn accept(&self, at: &Resource, listener: i64, span: Span) -> Result<HostAnswer, Diagnostic>;
    /// `None` is the deadline expiring; `Some(b"")` is the peer having stopped sending.
    fn recv(
        &self,
        at: &Resource,
        conn: i64,
        max: usize,
        timeout: Duration,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic>;
    /// `None` is the deadline expiring; `Some(0)` is the peer being gone.
    fn send(
        &self,
        at: &Resource,
        conn: i64,
        payload: &[u8],
        timeout: Duration,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic>;
    fn close(&self, at: &Resource, socket: i64, span: Span) -> Result<HostAnswer, Diagnostic>;
}

/// Which resource label each open socket is being operated under, and the one mechanical defence
/// this handler has against misreporting its own footprint.
pub struct Handles {
    open: Mutex<BTreeMap<i64, Option<Resource>>>,
    next: AtomicI64,
}

impl Default for Handles {
    fn default() -> Handles {
        Handles::new()
    }
}

impl Handles {
    pub fn new() -> Handles {
        Handles {
            // Handles ascend from 1 and are never reused, so 0 is never a live socket and a handle
            // held past its `close` names nothing rather than naming whatever opened next.
            open: Mutex::new(BTreeMap::new()),
            next: AtomicI64::new(1),
        }
    }

    pub fn open(&self, at: Option<&Resource>) -> i64 {
        let handle = self.next.fetch_add(1, Ordering::Relaxed);
        lock(&self.open).insert(handle, at.cloned());
        handle
    }

    /// Check the label, binding it if this is the socket's first use.
    pub fn check(&self, handle: i64, at: &Resource, span: Span) -> Result<(), Diagnostic> {
        let mut open = lock(&self.open);
        let Some(label) = open.get_mut(&handle) else {
            return Err(unknown_handle(handle, span));
        };
        match label {
            Some(existing) if existing == at => Ok(()),
            Some(existing) => Err(wrong_label(handle, existing, at, span)),
            None => {
                *label = Some(at.clone());
                Ok(())
            }
        }
    }

    pub fn close(&self, handle: i64) {
        lock(&self.open).remove(&handle);
    }
}

#[cold]
fn wrong_label(handle: i64, existing: &Resource, at: &Resource, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("socket {handle} is being used as `net{at}` and `net{existing}`"),
    )
    .primary(span, format!("this operation names `net{at}`"))
    .note(format!(
        "it was first used as `net{existing}`, and the resource label is what decides whether two computations conflict"
    ))
    .note("one socket under two labels is two resources the scheduler will not serialise, over one socket it must; give each socket a label and keep it")
}

/// See `pool::lock`: a map with no invariant a panicking caller can break.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Register every operation of `net` against `net`'s implementation.
pub fn register(registry: &mut HostRegistry, net: Arc<dyn Net>) {
    for op in Op::ALL {
        registry.register(
            op.declaration(net.as_ref()),
            Arc::new(Operation {
                op,
                net: Arc::clone(&net),
            }),
        );
    }
}

/// A registry serving `net` and nothing else.
pub fn registry(net: Arc<dyn Net>) -> HostRegistry {
    let mut registry = HostRegistry::new();
    register(&mut registry, net);
    registry
}

struct Operation {
    op: Op,
    net: Arc<dyn Net>,
}

impl HostHandler for Operation {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        if req.args.len() != self.op.arity() {
            return Err(arity(self.op, req.args.len(), span));
        }
        // The resolved atom's resource, never one the handler re-derives: the registry already
        // decided which label this perform named, and a handler that disagreed with it about that
        // would be the one disagreement nothing downstream can detect.
        let at = &req.atom.resource;
        match self.op {
            Op::Listen => {
                let port = port(self.op, req.args[0].as_int(span, "a port")?, span)?;
                self.net.listen(at, port, span)
            }
            Op::ListenTls => {
                let port = port(self.op, req.args[0].as_int(span, "a port")?, span)?;
                let credential = req.args[1].as_str(span, "a credential name")?;
                self.net.listen_tls(at, port, credential, span)
            }
            Op::Accept => {
                let listener = req.args[0].as_int(span, "a socket handle")?;
                self.net.accept(at, listener, span)
            }
            Op::Recv => {
                let conn = req.args[0].as_int(span, "a socket handle")?;
                let max = bound(req.args[1].as_int(span, "a byte count")?, span)?;
                let timeout = deadline(self.op, req.args[2].as_int(span, "a timeout")?, span)?;
                self.net.recv(at, conn, max, timeout, span)
            }
            Op::Send => {
                let conn = req.args[0].as_int(span, "a socket handle")?;
                let payload = Arc::clone(req.args[1].as_bytes(span, "a payload")?);
                let timeout = deadline(self.op, req.args[2].as_int(span, "a timeout")?, span)?;
                if payload.is_empty() {
                    return Err(empty_payload(span));
                }
                self.net.send(at, conn, &payload, timeout, span)
            }
            Op::Close => {
                let socket = req.args[0].as_int(span, "a socket handle")?;
                self.net.close(at, socket, span)
            }
        }
    }
}

/// A caller that wants no deadline passes a large one, and being made to write the number down is
/// the point: an operation with no bound is a connection a peer can hold for the life of the run.
fn deadline(op: Op, ms: i64, span: Span) -> Result<Duration, Diagnostic> {
    if ms <= 0 {
        return Err(Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("{} was given a timeout of {ms} milliseconds", op.what()),
        )
        .primary(span, "a deadline must be positive")
        .note("pass a large number for an operation that should not time out; there is no value that means `never`"));
    }
    Ok(Duration::from_millis(ms as u64))
}

/// What keeps `Some(0)` unambiguous: with an empty payload permitted, a caller could not tell "the
/// peer is gone" from "there was nothing to write".
#[cold]
fn empty_payload(span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        "`net.send` was given an empty payload",
    )
    .primary(span, "there is nothing to write")
    .note("`send` answers `Some(0)` when the peer is gone, so an empty payload would be indistinguishable from one")
}

fn port(op: Op, port: i64, span: Span) -> Result<u16, Diagnostic> {
    u16::try_from(port).map_err(|_| {
        Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!(
                "{} was given port {port}, which is not a TCP port",
                op.what()
            ),
        )
        .primary(span, "a port is 1 to 65535, or 0 to be assigned one")
    })
}

/// The loopback bind both listeners share, so that the two cannot come to differ about which
/// interface a Ply program listens on.
pub(crate) fn bind(what: &str, port: u16, span: Span) -> Result<std::net::TcpListener, Diagnostic> {
    std::net::TcpListener::bind(("127.0.0.1", port)).map_err(|e| {
        Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("{what} could not bind 127.0.0.1:{port}: {e}"),
        )
        .primary(span, "this listen reached the host and the host refused")
    })
}

/// Never a clamp on the low side: asking for nothing and being told nothing is how a program
/// mistakes an empty answer for a peer's close.
fn bound(max: i64, span: Span) -> Result<usize, Diagnostic> {
    if max <= 0 {
        return Err(Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("`net.recv` was asked for {max} bytes"),
        )
        .primary(span, "a read wants at least one byte")
        .note("an empty answer already means the peer has stopped sending, so a zero-length read would be indistinguishable from end of stream"));
    }
    // Capped before the cast, not after: `as usize` on a target narrower than `Int` would wrap, and
    // a wrap to zero is a read that answers empty, which is exactly the value that means the peer
    // went away.
    Ok(max.min(MAX_RECV as i64) as usize)
}

#[cold]
fn arity(op: Op, got: usize, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!(
            "{} was performed with {got} arguments and takes {}",
            op.what(),
            op.arity()
        ),
    )
    .primary(span, "this perform reached the host handler")
    .note("inference checks a perform's arity, so reaching this means the evaluator was handed a module that was never checked")
}

/// A handle the table does not hold.
#[cold]
fn unknown_handle(handle: i64, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("there is no open socket with handle {handle}"),
    )
    .primary(span, "this handle was closed, or never opened")
    .note("handles ascend and are never reused, so a handle past its `net.close` names nothing rather than naming whatever opened next")
}

#[cold]
fn not_a_listener(handle: i64, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("socket {handle} is a connection, and `net.accept` wants a listener"),
    )
    .primary(span, "this handle came from `net.accept`, not `net.listen`")
}

#[cold]
fn not_a_stream(handle: i64, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("socket {handle} is a listener, and this operation wants a connection"),
    )
    .primary(span, "this handle came from `net.listen`, not `net.accept`")
}

/// Only the twin can raise this: a real `accept` waits for a peer, and a script has nothing left to
/// wait for.
#[cold]
fn no_connection_scripted(span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        "the simulated network has no further connection to accept",
    )
    .primary(span, "this accept would wait forever")
    .note("script another connection, or stop accepting after the ones there are")
}

#[cfg(test)]
mod tests;
