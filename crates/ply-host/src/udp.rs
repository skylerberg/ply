//! The `udp` effect: datagram sockets, in the socket host's table and on its pool, so a datagram
//! socket is labelled, closed and tuned as a stream socket is.

use crate::pool::JobOutput;
use crate::tcp::wire::{self, Endpoint, Refusal, SocketOption};
use crate::tcp::{Net, TcpHost};
use ply_eval::host::HostRegistry;
use ply_eval::{
    Determinism, Diagnostic, HostAnswer, HostHandler, HostOp, HostRequest, HostResource,
    HostRuntime, Linearity, Span, Symbol, Value, codes,
};
use socket2::{Domain, MaybeUninitSlice, Protocol, SockAddr, SockRef, Socket, Type};
use std::mem::MaybeUninit;
use std::net::UdpSocket;
use std::sync::Arc;
use std::time::Duration;

pub const MODULE: &str = "std.udp";

pub const EFFECT: &str = "std.udp.udp";

/// The most bytes one `recv_from` allocates for: no datagram is longer.
pub const MAX_DATAGRAM: usize = 65535;

operations! {
    what "udp";
    path "udp";
    Bind = "bind" / 2,
    Connect = "connect" / 2,
    SendTo = "send_to" / 4,
    Send = "send" / 3,
    RecvFrom = "recv_from" / 3,
    SetOption = "set_option" / 2,
    Close = "close" / 1,
    LocalAddress = "local_address" / 1,
    Options = "options" / 1,
}

impl Op {
    pub fn declaration(self) -> HostOp {
        HostOp {
            effect: Symbol::new(EFFECT),
            op: Symbol::new(self.name()),
            resource: HostResource::Any,
            determinism: Determinism::Nondeterministic,
            linearity: match self {
                Op::LocalAddress | Op::Options => Linearity::Repeatable,
                _ => Linearity::AtMostOnce,
            },
            blocking: matches!(self, Op::SendTo | Op::Send | Op::RecvFrom),
            secrets: false,
            path: self.path(),
        }
    }
}

pub fn register(registry: &mut HostRegistry, net: Arc<TcpHost>) {
    for op in Op::ALL {
        registry.register(
            op.declaration(),
            Arc::new(Operation {
                op,
                net: Arc::clone(&net),
            }),
        );
    }
}

struct Operation {
    op: Op,
    net: Arc<TcpHost>,
}

fn answered(value: Value) -> Result<HostAnswer, Diagnostic> {
    Ok(HostAnswer::Value(value))
}

fn settled<T: Send + 'static>(
    outcome: Result<T, Refusal>,
    value: impl FnOnce(T) -> Value + Send + 'static,
) -> JobOutput {
    JobOutput::Made(Box::new(move || match outcome {
        Ok(found) => wire::ok(value(found)),
        Err(refusal) => wire::err(&refusal),
    }))
}

impl HostHandler for Operation {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        if req.args.len() != self.op.arity() {
            return Err(arity(self.op, req.args.len(), span));
        }
        let at = &req.atom.resource;
        let handle = || req.args[0].as_int(span, "a socket handle");
        let timeout =
            |index: usize| deadline(self.op, req.args[index].as_int(span, "a timeout")?, span);
        match self.op {
            Op::Bind => {
                let to = Endpoint::read(&req.args[0], span)?;
                let options = SocketOption::list(&req.args[1], span)?;
                answered(match bound(&to, &options) {
                    Ok(socket) => wire::ok(Value::Int(self.net.open_datagram(at, socket))),
                    Err(refusal) => wire::err(&refusal),
                })
            }
            Op::Connect => {
                let socket = self.net.datagram(handle()?, at, span)?;
                let to = Endpoint::read(&req.args[1], span)?;
                let connected = to
                    .socket_addr()
                    .and_then(|to| socket.connect(to).map_err(|e| Refusal::of(&e)));
                answered(match connected {
                    Ok(()) => wire::ok(Value::Unit),
                    Err(refusal) => wire::err(&refusal),
                })
            }
            Op::SendTo => {
                let socket = self.net.datagram(handle()?, at, span)?;
                let to = Endpoint::read(&req.args[1], span)?;
                let payload = Arc::clone(req.args[2].as_bytes(span, "a payload")?);
                let timeout = timeout(3)?;
                self.net.waiting(span, "send_to", self.op.what(), move || {
                    let _ = socket.set_write_timeout(Some(timeout));
                    let sent = to
                        .socket_addr()
                        .and_then(|to| socket.send_to(&payload, to).map_err(|e| Refusal::of(&e)));
                    settled(sent, |n| Value::Int(n as i64))
                })
            }
            Op::Send => {
                let socket = self.net.datagram(handle()?, at, span)?;
                if socket.peer_addr().is_err() {
                    return Err(unconnected(span));
                }
                let payload = Arc::clone(req.args[1].as_bytes(span, "a payload")?);
                let timeout = timeout(2)?;
                self.net.waiting(span, "send", self.op.what(), move || {
                    let _ = socket.set_write_timeout(Some(timeout));
                    let sent = socket.send(&payload).map_err(|e| Refusal::of(&e));
                    settled(sent, |n| Value::Int(n as i64))
                })
            }
            Op::RecvFrom => {
                let socket = self.net.datagram(handle()?, at, span)?;
                let max = most(req.args[1].as_int(span, "a byte count")?, span)?;
                let timeout = timeout(2)?;
                self.net
                    .waiting(span, "recv_from", self.op.what(), move || {
                        let _ = socket.set_read_timeout(Some(timeout));
                        settled(received(&socket, max), |datagram: Datagram| {
                            datagram.value()
                        })
                    })
            }
            Op::SetOption => {
                let option = SocketOption::read(&req.args[1], span)?;
                // Only to hold the handle to a datagram socket: the setting is the table's.
                let _ = self.net.datagram(handle()?, at, span)?;
                self.net.set_option(at, handle()?, option, span)
            }
            Op::Close => {
                let _ = self.net.datagram(handle()?, at, span)?;
                self.net.close(at, handle()?, span)
            }
            Op::LocalAddress => {
                let _ = self.net.datagram(handle()?, at, span)?;
                self.net.local_address(at, handle()?, span)
            }
            Op::Options => {
                let _ = self.net.datagram(handle()?, at, span)?;
                self.net.options(at, handle()?, span)
            }
        }
    }
}

/// A `std.udp.Datagram`.
struct Datagram {
    from: Endpoint,
    payload: Vec<u8>,
    truncated: bool,
}

impl Datagram {
    fn value(self) -> Value {
        wire::record([
            ("from", self.from.value()),
            ("payload", Value::bytes(self.payload)),
            ("truncated", Value::Bool(self.truncated)),
        ])
    }
}

/// One datagram, at most `max` bytes of it; what the kernel cut off a longer one is gone, and the
/// answer says it was.
fn received(socket: &UdpSocket, max: usize) -> Result<Datagram, Refusal> {
    let mut buffer = vec![0u8; max];
    // SAFETY: `u8` and `MaybeUninit<u8>` share a layout, and a receive writes only initialised
    // bytes into the buffer.
    let spare =
        unsafe { &mut *(std::ptr::from_mut::<[u8]>(&mut buffer) as *mut [MaybeUninit<u8>]) };
    let (read, flags, from) = SockRef::from(socket)
        .recv_from_vectored(&mut [MaybeUninitSlice::new(spare)])
        .map_err(|e| Refusal::of(&e))?;
    buffer.truncate(read);
    let Some(from) = from.as_socket() else {
        return Err(Refusal::Other(
            0,
            "the datagram's sender has no address".to_string(),
        ));
    };
    Ok(Datagram {
        from: Endpoint::of(from),
        payload: buffer,
        truncated: flags.is_truncated(),
    })
}

/// A socket bound at `to` with `options` set before the bind. A run binds loopback, or no address
/// and no port for a socket that asks and hears answers: one that anything on a network could
/// send to at a known port is not a run's to open.
fn bound(to: &Endpoint, options: &[SocketOption]) -> Result<UdpSocket, Refusal> {
    if !(to.ip.is_loopback() || to.ip.is_unspecified() && to.port == 0) {
        return Err(Refusal::PermissionDenied);
    }
    let address = to.socket_addr()?;
    let refusal = |e: std::io::Error| Refusal::of(&e);
    let socket = Socket::new(
        Domain::for_address(address),
        Type::DGRAM,
        Some(Protocol::UDP),
    )
    .map_err(refusal)?;
    for option in options {
        crate::tcp::apply(&socket, option).map_err(refusal)?;
    }
    socket.bind(&SockAddr::from(address)).map_err(refusal)?;
    Ok(socket.into())
}

fn deadline(op: Op, ms: i64, span: Span) -> Result<Duration, Diagnostic> {
    if ms <= 0 {
        return Err(Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("{} was given a timeout of {ms} milliseconds", op.what()),
        )
        .primary(span, "a deadline must be positive"));
    }
    Ok(Duration::from_millis(ms as u64))
}

fn most(max: i64, span: Span) -> Result<usize, Diagnostic> {
    if max <= 0 {
        return Err(Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("`udp.recv_from` was asked for {max} bytes"),
        )
        .primary(span, "a datagram is read into at least one byte")
        .note("a datagram longer than the bytes asked for is cut to them and answered `truncated`, so asking for none would lose every one"));
    }
    Ok(max.min(MAX_DATAGRAM as i64) as usize)
}

#[cold]
fn unconnected(span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        "`udp.send` was given a socket with no far end",
    )
    .primary(span, "this socket was never connected")
    .note("`udp.connect` fixes where a socket sends, or `udp.send_to` names it each time")
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
}
