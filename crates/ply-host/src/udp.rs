//! `std.udp`'s datagram sockets, in the socket host's table, so a datagram socket is labelled,
//! closed and tuned as a stream socket is. A datagram received is parts: the address it came from
//! as `wire` writes one, its payload, and byte 1 where it was cut short.

use crate::jobs::Job;
use crate::native::{int, parts};
use crate::tcp::TcpHost;
use crate::tcp::wire::{self, Endpoint, Refusal, SocketOption};
use ply_core::{Diagnostic, Span, codes};
use socket2::{Domain, MaybeUninitSlice, Protocol, SockAddr, SockRef, Socket, Type};
use std::mem::MaybeUninit;
use std::net::UdpSocket;
use std::time::Duration;

/// The most bytes one `recv_from` allocates for: no datagram is longer.
pub const MAX_DATAGRAM: usize = 65535;

/// A socket bound at `to`, with `options` set before the bind: its handle, or why not. Which
/// addresses a run may bind is the real handler's to hold it to.
pub fn bind(net: &TcpHost, label: &str, to: &Endpoint, options: &[SocketOption]) -> Vec<u8> {
    wire::answer(bound(to, options).map(|socket| int(net.open_datagram(label, socket)).to_vec()))
}

/// Fixes the socket's far end: nothing, or why not.
pub fn connect(
    net: &TcpHost,
    label: &str,
    handle: i64,
    to: &Endpoint,
    span: Span,
) -> Result<Vec<u8>, Diagnostic> {
    let socket = net.datagram(handle, label, span)?;
    let connected = to
        .socket_addr()
        .and_then(|to| socket.connect(to).map_err(|e| Refusal::of(&e)));
    Ok(wire::answer(connected.map(|()| Vec::new())))
}

/// Sends `payload` to `to` as one datagram: its length, or why not.
pub fn send_to(
    net: &TcpHost,
    label: &str,
    handle: i64,
    to: Endpoint,
    payload: Vec<u8>,
    timeout: Duration,
    span: Span,
) -> Result<Job, Diagnostic> {
    let socket = net.datagram(handle, label, span)?;
    Ok(Box::new(move || {
        let _ = socket.set_write_timeout(Some(timeout));
        let sent = to
            .socket_addr()
            .and_then(|to| socket.send_to(&payload, to).map_err(|e| Refusal::of(&e)));
        Ok(wire::answer(sent.map(|n| int(n as i64).to_vec())))
    }))
}

/// `send_to` the far end `connect` fixed; a socket with none is a runtime error.
pub fn send(
    net: &TcpHost,
    label: &str,
    handle: i64,
    payload: Vec<u8>,
    timeout: Duration,
    span: Span,
) -> Result<Job, Diagnostic> {
    let socket = net.datagram(handle, label, span)?;
    if socket.peer_addr().is_err() {
        return Err(unconnected(span));
    }
    Ok(Box::new(move || {
        let _ = socket.set_write_timeout(Some(timeout));
        let sent = socket.send(&payload).map_err(|e| Refusal::of(&e));
        Ok(wire::answer(sent.map(|n| int(n as i64).to_vec())))
    }))
}

/// The next datagram, at most `max` bytes of it, which is capped at [`MAX_DATAGRAM`]: the
/// datagram, or why not.
pub fn recv_from(
    net: &TcpHost,
    label: &str,
    handle: i64,
    max: usize,
    timeout: Duration,
    span: Span,
) -> Result<Job, Diagnostic> {
    let socket = net.datagram(handle, label, span)?;
    let max = max.clamp(1, MAX_DATAGRAM);
    Ok(Box::new(move || {
        let _ = socket.set_read_timeout(Some(timeout));
        Ok(wire::answer(received(&socket, max)))
    }))
}

/// One datagram, at most `max` bytes of it; what the kernel cut off a longer one is gone, and the
/// answer says it was.
fn received(socket: &UdpSocket, max: usize) -> Result<Vec<u8>, Refusal> {
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
    Ok(parts(&[
        &Endpoint::of(from).written(),
        &buffer,
        &[u8::from(flags.is_truncated())],
    ]))
}

/// A socket bound at `to` with `options` set before the bind.
fn bound(to: &Endpoint, options: &[SocketOption]) -> Result<UdpSocket, Refusal> {
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

#[cold]
fn unconnected(span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        "`udp.send` was given a socket with no far end",
    )
    .primary(span, "this socket was never connected")
    .note("`udp.connect` fixes where a socket sends, or `udp.send_to` names it each time")
}
