//! The `net` effect and its handlers.

mod sim;
mod socket;
pub mod wire;

pub use crate::pool::MAX_BLOCKING_OPERATIONS;
pub use sim::SimNet;
pub use socket::TcpHost;
pub(crate) use socket::apply;
pub use wire::{Credentials, Endpoint, Probing, Refusal, SocketOption};

use ply_eval::{
    Determinism, Diagnostic, HostAnswer, HostHandler, HostOp, HostRegistry, HostRequest,
    HostResource, HostRuntime, Linearity, Resource, Span, Symbol, codes,
};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

pub const MODULE: &str = "std.net";

pub const EFFECT: &str = "std.net.net";

/// The most bytes one `recv` allocates for, whatever `max` asks.
pub const MAX_RECV: usize = 1 << 20;

operations! {
    what "net";
    Listen = "listen" / 1,
    ListenTls = "listen_tls" / 2,
    ListenOn = "listen_on" / 2,
    ListenUnix = "listen_unix" / 2,
    Connect = "connect" / 3,
    ConnectTls = "connect_tls" / 3,
    ConnectTo = "connect_to" / 3,
    ConnectUnix = "connect_unix" / 3,
    Handshake = "handshake" / 1,
    StartTls = "start_tls" / 5,
    ServeTls = "serve_tls" / 5,
    Accept = "accept" / 1,
    Recv = "recv" / 3,
    Send = "send" / 3,
    CloseWrite = "close_write" / 1,
    Close = "close" / 1,
    SetOption = "set_option" / 2,
    LocalPort = "local_port" / 1,
    LocalAddress = "local_address" / 1,
    PeerAddress = "peer_address" / 1,
    PeerCredentials = "peer_credentials" / 1,
    Options = "options" / 1,
}

impl Op {
    fn waits(self) -> bool {
        matches!(
            self,
            Op::Connect
                | Op::ConnectTls
                | Op::ConnectTo
                | Op::ConnectUnix
                | Op::Handshake
                | Op::StartTls
                | Op::ServeTls
                | Op::Accept
                | Op::Recv
                | Op::Send
        )
    }

    /// Whether the operation only reads what a socket is: replaying it changes nothing.
    fn reads(self) -> bool {
        matches!(
            self,
            Op::LocalPort | Op::LocalAddress | Op::PeerAddress | Op::PeerCredentials | Op::Options
        )
    }

    pub fn declaration(self, net: &dyn Net) -> HostOp {
        HostOp {
            effect: Symbol::new(EFFECT),
            op: Symbol::new(self.name()),
            resource: HostResource::Any,
            determinism: Determinism::Nondeterministic,
            linearity: if self.reads() {
                Linearity::Repeatable
            } else {
                Linearity::AtMostOnce
            },
            blocking: self.waits() && net.waits(),
            // No expression turns a `Secret` into the `Bytes` a socket write takes.
            secrets: false,
            path: net.path(self),
        }
    }
}

/// What an upgrade to TLS is asked with: the protocols to offer or accept, how many bytes of
/// plaintext the caller read and has not consumed, and the deadline of the whole handshake.
pub struct Upgrade {
    pub alpn: Vec<String>,
    pub unread: usize,
    pub timeout: Duration,
}

pub trait Net: Send + Sync {
    /// Whether this implementation's waiting operations leave the machine's thread.
    fn waits(&self) -> bool;

    fn path(&self, op: Op) -> &'static str;

    fn listen(&self, at: &Resource, port: u16, span: Span) -> Result<HostAnswer, Diagnostic>;
    fn listen_tls(
        &self,
        at: &Resource,
        port: u16,
        credential: &str,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic>;
    /// A listener at exactly `to`, which is a loopback address, with `options` set before the
    /// bind.
    fn listen_on(
        &self,
        at: &Resource,
        to: Endpoint,
        options: Vec<SocketOption>,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic>;
    /// A listener on the Unix socket at `path` under the filesystem root named `root`.
    fn listen_unix(
        &self,
        at: &Resource,
        root: &str,
        path: &str,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic>;
    /// `None` is a host that could not be reached before the deadline, whatever the reason.
    fn connect(
        &self,
        at: &Resource,
        host: &str,
        port: u16,
        timeout: Duration,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic>;
    /// As `connect`, then TLS over it, verifying `host`; a failed handshake ends the connection.
    fn connect_tls(
        &self,
        at: &Resource,
        host: &str,
        port: u16,
        timeout: Duration,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic>;
    /// A connection to exactly `to`, no name looked up, with `options` set before it connects.
    fn connect_to(
        &self,
        at: &Resource,
        to: Endpoint,
        options: Vec<SocketOption>,
        timeout: Duration,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic>;
    fn connect_unix(
        &self,
        at: &Resource,
        root: &str,
        path: &str,
        timeout: Duration,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic>;
    /// The TLS handshake, completed now rather than when a request needs it, answering what it took
    /// in microseconds. `None` for a connection with no handshake to complete.
    fn handshake(&self, at: &Resource, conn: i64, span: Span) -> Result<HostAnswer, Diagnostic>;
    /// An open plaintext connection secured as the client's end, verifying the server as `name`.
    /// The handle is the secured connection afterwards.
    fn start_tls(
        &self,
        at: &Resource,
        conn: i64,
        name: &str,
        upgrade: Upgrade,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic>;
    /// The server's end of the same, with the credential named `credential`.
    fn serve_tls(
        &self,
        at: &Resource,
        conn: i64,
        credential: &str,
        upgrade: Upgrade,
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
    /// Ends this end's sending and leaves its reading open.
    fn close_write(&self, at: &Resource, conn: i64, span: Span) -> Result<HostAnswer, Diagnostic>;
    fn close(&self, at: &Resource, socket: i64, span: Span) -> Result<HostAnswer, Diagnostic>;
    fn set_option(
        &self,
        at: &Resource,
        socket: i64,
        option: SocketOption,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic>;
    /// The port this end of a socket is bound to; `None` for a listener the drain has closed.
    fn local_port(&self, at: &Resource, socket: i64, span: Span) -> Result<HostAnswer, Diagnostic>;
    /// The address this end of a socket is bound to; `None` for one with no IP address.
    fn local_address(
        &self,
        at: &Resource,
        socket: i64,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic>;
    /// The address of a connection's far end; `None` for a listener and for a Unix socket.
    fn peer_address(&self, at: &Resource, conn: i64, span: Span) -> Result<HostAnswer, Diagnostic>;
    /// Who holds the far end of a Unix socket; `None` for any other.
    fn peer_credentials(
        &self,
        at: &Resource,
        conn: i64,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic>;
    /// Each option the socket has, as it stands.
    fn options(&self, at: &Resource, socket: i64, span: Span) -> Result<HostAnswer, Diagnostic>;
}

/// The resource label each open socket is operated under.
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
            // Handles ascend from 1 and are never reused, so a stale handle names nothing.
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

/// Poison is ignored: the map has no invariant a panic can break.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

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
        // The resolved atom's resource, never one the handler re-derives.
        let at = &req.atom.resource;
        let handle = |index: usize| req.args[index].as_int(span, "a socket handle");
        let timeout =
            |index: usize| deadline(self.op, req.args[index].as_int(span, "a timeout")?, span);
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
            Op::ListenOn => {
                let to = Endpoint::read(&req.args[0], span)?;
                let options = SocketOption::list(&req.args[1], span)?;
                self.net.listen_on(at, to, options, span)
            }
            Op::ListenUnix => {
                let root = req.args[0].as_str(span, "a root's name")?;
                let path = req.args[1].as_str(span, "a path")?;
                self.net.listen_unix(at, root, path, span)
            }
            Op::Connect => {
                let host = req.args[0].as_str(span, "a host name")?;
                let port = port(self.op, req.args[1].as_int(span, "a port")?, span)?;
                self.net.connect(at, host, port, timeout(2)?, span)
            }
            Op::ConnectTls => {
                let host = req.args[0].as_str(span, "a host name")?;
                let port = port(self.op, req.args[1].as_int(span, "a port")?, span)?;
                self.net.connect_tls(at, host, port, timeout(2)?, span)
            }
            Op::ConnectTo => {
                let to = Endpoint::read(&req.args[0], span)?;
                let options = SocketOption::list(&req.args[1], span)?;
                self.net.connect_to(at, to, options, timeout(2)?, span)
            }
            Op::ConnectUnix => {
                let root = req.args[0].as_str(span, "a root's name")?;
                let path = req.args[1].as_str(span, "a path")?;
                self.net.connect_unix(at, root, path, timeout(2)?, span)
            }
            Op::Accept => self.net.accept(at, handle(0)?, span),
            Op::Handshake => self.net.handshake(at, handle(0)?, span),
            Op::StartTls => {
                let name = req.args[1].as_str(span, "a server name")?;
                self.net
                    .start_tls(at, handle(0)?, name, upgrade(req, timeout(4)?)?, span)
            }
            Op::ServeTls => {
                let credential = req.args[1].as_str(span, "a credential name")?;
                self.net
                    .serve_tls(at, handle(0)?, credential, upgrade(req, timeout(4)?)?, span)
            }
            Op::Recv => {
                let max = bound(req.args[1].as_int(span, "a byte count")?, span)?;
                self.net.recv(at, handle(0)?, max, timeout(2)?, span)
            }
            Op::Send => {
                let payload = Arc::clone(req.args[1].as_bytes(span, "a payload")?);
                if payload.is_empty() {
                    return Err(empty_payload(span));
                }
                self.net.send(at, handle(0)?, &payload, timeout(2)?, span)
            }
            Op::CloseWrite => self.net.close_write(at, handle(0)?, span),
            Op::Close => self.net.close(at, handle(0)?, span),
            Op::SetOption => {
                let option = SocketOption::read(&req.args[1], span)?;
                self.net.set_option(at, handle(0)?, option, span)
            }
            Op::LocalPort => self.net.local_port(at, handle(0)?, span),
            Op::LocalAddress => self.net.local_address(at, handle(0)?, span),
            Op::PeerAddress => self.net.peer_address(at, handle(0)?, span),
            Op::PeerCredentials => self.net.peer_credentials(at, handle(0)?, span),
            Op::Options => self.net.options(at, handle(0)?, span),
        }
    }
}

/// The last four arguments of either upgrade: the protocols, the unread plaintext, the deadline.
fn upgrade(req: &HostRequest<'_>, timeout: Duration) -> Result<Upgrade, Diagnostic> {
    Ok(Upgrade {
        alpn: wire::strings_of(&req.args[2], req.span, "an application protocol")?,
        unread: req.args[3].as_bytes(req.span, "unread plaintext")?.len(),
        timeout,
    })
}

/// No value means `never`: an unbounded operation lets a peer hold a connection for the whole run.
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

/// Keeps `send`'s `Some(0)` unambiguous as the peer being gone.
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

pub(crate) fn bind(what: &str, port: u16, span: Span) -> Result<std::net::TcpListener, Diagnostic> {
    std::net::TcpListener::bind(("127.0.0.1", port)).map_err(|e| {
        Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("{what} could not bind 127.0.0.1:{port}: {e}"),
        )
        .primary(span, "this listen reached the host and the host refused")
    })
}

/// Refuses rather than clamps a non-positive `max`: an empty answer means the peer closed.
fn bound(max: i64, span: Span) -> Result<usize, Diagnostic> {
    if max <= 0 {
        return Err(Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("`net.recv` was asked for {max} bytes"),
        )
        .primary(span, "a read wants at least one byte")
        .note("an empty answer already means the peer has stopped sending, so a zero-length read would be indistinguishable from end of stream"));
    }
    // Capped before the cast: on a narrow target a wrap to zero would read as the peer's close.
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

/// An upgrade of something that is not a plaintext TCP connection at rest.
#[cold]
fn not_upgradable(op: Op, handle: i64, why: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("{} cannot secure socket {handle}: {why}", op.what()),
    )
    .primary(span, "an upgrade takes a plaintext connection nothing else is waiting on")
    .note("the handle is the secured connection afterwards, so a read or a write still in flight on the plaintext would be one the upgrade could not account for")
}

#[cold]
fn a_datagram_socket(handle: i64, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("socket {handle} is a datagram socket, and this operation wants a `net` one"),
    )
    .primary(span, "this handle came from `udp.bind`")
    .note("a datagram socket is read with `udp.recv_from` and written with `udp.send_to`")
}

#[cold]
fn not_a_datagram_socket(handle: i64, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("socket {handle} is not a datagram socket, and this operation wants one"),
    )
    .primary(span, "this handle did not come from `udp.bind`")
}

/// Only the simulated network raises this; a real `accept` waits for a peer.
#[cold]
fn no_connection_scripted(span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        "the simulated network has no further connection to accept",
    )
    .primary(span, "this accept would wait forever")
    .note("script another connection, or stop accepting after the ones there are")
}
