//! The `net` effect and the two handlers that serve it.
//!
//! [`DECLARATION`] is the Ply source this registers against, and
//! [`HostRegistry::bind`] is what checks the two still agree: an operation
//! renamed on either side is `E0421` before anything runs, naming the nearest
//! declared operation.
//!
//! Three properties of the registration are the whole of what this milestone
//! claims, and each is one line of [`Op::declaration`]:
//!
//! - **Every operation is resource-parameterized.** `net.write[a]` and
//!   `net.write[b]` do not conflict, so two connections a program labels apart
//!   may run concurrently. The label is static and the `Int` handle is dynamic;
//!   two sockets accepted at one source site share a label and do contend, which
//!   is the honest limit of ground resource labels.
//! - **Every operation is nondeterministic.** The network is. So `net` is
//!   declared `nondet`, a `det` test that reaches it is `E0412` at compile time
//!   whether or not `--host` was passed, and a run that reaches it is never
//!   cached.
//! - **Every operation is [`Linearity::AtMostOnce`].** Resuming a continuation
//!   across a `recv` is not a replay of that `recv`; it is a second one, and the
//!   bytes the first took are gone. Nothing here is `Repeatable` and nothing
//!   here can be.
//!
//! [`TcpHost`] serves it over loopback sockets and [`SimNet`] over a script.
//! Both go through [`register`], so the triples, the determinism flag, the
//! linearity flag, the argument decoding and the domain checks are one
//! implementation rather than two that agree today.
//!
//! `net.listen_tls` is here rather than behind a `tls` effect of its own,
//! because a row claims which resources a computation touches and whether two
//! computations contend, and encryption decides neither. What it does change is
//! the trusted computing base, so it is registered with its own handler path —
//! [`ply_host::tls::listen`] — and `ply hosts` prints it on its own line.
//!
//! [`ply_host::tls::listen`]: crate::tls::listen

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

/// The Ply declaration the registrations below are checked against: the source
/// of the module `std.net`, which ships with the compiler.
///
/// A program that wants these handlers writes `import std.net (net)` rather than
/// copying the declaration, so the signature the host binds to and the signature
/// the program performs are one text that cannot drift.
pub const DECLARATION: &str = ply_std::NET;

/// The module the declaration ships as, which is what qualifies [`EFFECT`].
pub const MODULE: &str = "std.net";

/// The program-wide effect name. Effect names are qualified (ADR 0001), so the
/// `net` declared by `std.net` is `std.net.net`, and a program that declares its
/// own `net` instead is `E0421` naming the operation it found.
pub const EFFECT: &str = "std.net.net";

/// The most bytes one `recv` allocates for, whatever `max` asks.
///
/// A short answer is always legal — TCP preserves the stream and not the
/// boundaries a peer wrote it in — so capping changes nothing a correct program
/// relies on, and it stops `net.recv(c, 1 << 40)` from being an allocation the
/// host makes on a peer's word.
pub const MAX_RECV: usize = 1 << 20;

/// The six operations. Listen, listen over TLS, accept, read, write, close, and
/// nothing else: no pooling and no keep-alive.
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
            // The deadline is the third argument. ADR 0013 §7.2: a deadline on
            // the operation needs one `setsockopt` inside a job that already
            // owns the socket, where a cancellation would need a token registry
            // and a race between the cancel and the completion.
            Op::Recv | Op::Send => 3,
        }
    }

    /// Whether the operation has to wait on a peer. `listen` and `close` are
    /// syscalls that return; the other three are the ones that would stall a
    /// scheduler thread if they ran on it.
    fn waits(self) -> bool {
        matches!(self, Op::Accept | Op::Recv | Op::Send)
    }

    /// The registration. Everything a reviewer reads in `ply hosts` is decided
    /// here, and the only column an implementation gets a say in is `blocking`,
    /// because whether an operation leaves the machine's thread is a property of
    /// the thing serving it rather than of the signature.
    fn declaration(self, net: &dyn Net) -> HostOp {
        HostOp {
            effect: Symbol::new(EFFECT),
            op: Symbol::new(self.name()),
            // Whichever labels the program uses. `bind` expands this against the
            // program's own atoms and `ply hosts` prints one row per expansion,
            // so a handler that serves every socket still has to list the
            // sockets it got.
            resource: HostResource::Any,
            determinism: Determinism::Nondeterministic,
            linearity: Linearity::AtMostOnce,
            blocking: self.waits() && net.waits(),
            path: net.path(self),
        }
    }
}

/// What a `net` implementation has to answer.
///
/// Arguments arrive decoded and inside the declared domain: the adapter that
/// implements [`HostHandler`] checks arity, types, the port range and the read
/// bound once, for both implementations, so the socket and the script cannot
/// come to differ about what a legal call is.
///
/// `at` is the resource the `perform` named, and it is not decoration. The
/// handle is the socket's dynamic identity and the label is its static one, and
/// the runtime schedules on the label; a program that touched one socket under
/// two labels would have two atoms that do not conflict over a resource that
/// does. [`Handles`] is what refuses that.
pub trait Net: Send + Sync {
    /// Whether this implementation's waiting operations leave the machine's
    /// thread. A socket's do; a script has nothing to wait for.
    fn waits(&self) -> bool;

    /// The Rust path `ply hosts` prints. It must identify the implementation,
    /// not the effect: a listing that named a socket handler for a run served by
    /// a script would be the trusted computing base lying about itself.
    fn path(&self, op: Op) -> &'static str;

    fn listen(&self, at: &Resource, port: u16, span: Span) -> Result<HostAnswer, Diagnostic>;
    /// The same listener, terminating TLS. `credential` names material the run
    /// was configured with rather than carrying any of it, so a program that
    /// serves TLS holds no key and its hashes carry none. One credential per
    /// listener: SNI-based selection and mTLS are not in W3.
    fn listen_tls(
        &self,
        at: &Resource,
        port: u16,
        credential: &str,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic>;
    fn accept(&self, at: &Resource, listener: i64, span: Span) -> Result<HostAnswer, Diagnostic>;
    /// `None` is the deadline expiring; `Some(b"")` is the peer having stopped
    /// sending. Both are ordinary outcomes rather than diagnostics — a server
    /// that died because a client reset a connection would not be a server.
    fn recv(
        &self,
        at: &Resource,
        conn: i64,
        max: usize,
        timeout: Duration,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic>;
    /// `None` is the deadline expiring; `Some(0)` is the peer being gone.
    /// `Some(n)` may be short of the payload, which is what `std.net.send_all`
    /// loops over.
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

/// Which resource label each open socket is being operated under, and the one
/// mechanical defence this handler has against misreporting its own footprint.
///
/// The runtime records the atom a `perform` named and schedules on it. The
/// handler acts on the `Int` handle. Nothing connects the two but this: a socket
/// takes the label of the first operation performed on it and keeps it, and an
/// operation naming a different one is refused. Without it, `net.recv[a](5, 8)`
/// beside `net.send[b](5, ..)` is two atoms that do not conflict over one
/// socket that does, and the scheduler would run them together and be right by
/// its own lights.
///
/// A listener takes its label at `listen`, where the program named one. An
/// accepted connection takes none until it is first used, because `accept`
/// names the *listener's* label and the connection's is whatever the program
/// reads and writes it under.
///
/// Shared by both implementations so the two allocate handles identically and
/// enforce the same rule, rather than agreeing today.
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
            // Handles ascend from 1 and are never reused, so 0 is never a live
            // socket and a handle held past its `close` names nothing rather
            // than naming whatever opened next.
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
///
/// One function for both implementations on purpose. ADR 0008 §5 wants a
/// simulated twin that satisfies the same declared signature; the cheapest way
/// to keep that true is for there to be one place the signature is written.
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
        // The resolved atom's resource, never one the handler re-derives: the
        // registry already decided which label this perform named, and a handler
        // that disagreed with it about that would be the one disagreement
        // nothing downstream can detect.
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

/// A caller that wants no deadline passes a large one, and being made to write
/// the number down is the point: an operation with no bound is a connection a
/// peer can hold for the life of the run.
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

/// What keeps `Some(0)` unambiguous: with an empty payload permitted, a caller
/// could not tell "the peer is gone" from "there was nothing to write".
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

/// The loopback bind both listeners share, so that the two cannot come to
/// differ about which interface a Ply program listens on. Loopback because
/// neither operation takes an address, and a handler that bound `0.0.0.0` by
/// default would put a program's responses on the network of whoever ran it.
pub(crate) fn bind(what: &str, port: u16, span: Span) -> Result<std::net::TcpListener, Diagnostic> {
    std::net::TcpListener::bind(("127.0.0.1", port)).map_err(|e| {
        Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("{what} could not bind 127.0.0.1:{port}: {e}"),
        )
        .primary(span, "this listen reached the host and the host refused")
    })
}

/// Never a clamp on the low side: asking for nothing and being told nothing is
/// how a program mistakes an empty answer for a peer's close.
fn bound(max: i64, span: Span) -> Result<usize, Diagnostic> {
    if max <= 0 {
        return Err(Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("`net.recv` was asked for {max} bytes"),
        )
        .primary(span, "a read wants at least one byte")
        .note("an empty answer already means the peer has stopped sending, so a zero-length read would be indistinguishable from end of stream"));
    }
    // Capped before the cast, not after: `as usize` on a target narrower than
    // `Int` would wrap, and a wrap to zero is a read that answers empty, which
    // is exactly the value that means the peer went away.
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

/// A handle the table does not hold. Ascending handles are never reused, so this
/// is a closed socket rather than someone else's.
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

/// Only the twin can raise this: a real `accept` waits for a peer, and a script
/// has nothing left to wait for.
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
