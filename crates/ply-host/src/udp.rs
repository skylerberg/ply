use crate::jobs::Job;
use crate::native::{int, parts};
use crate::tcp::TcpHost;
use crate::tcp::wire::{self, Endpoint, NetError, SocketOption};
use ply_core::{Diagnostic, Span, codes};
use socket2::{Domain, MaybeUninitSlice, Protocol, SockAddr, SockRef, Socket, Type};
use std::mem::MaybeUninit;
use std::net::UdpSocket;
use std::time::Duration;

pub const MAX_DATAGRAM: usize = 65535;

pub fn bind(net: &TcpHost, label: &str, to: &Endpoint, options: &[SocketOption]) -> Vec<u8> {
    wire::result(bound(to, options).map(|socket| int(net.open_datagram(label, socket)).to_vec()))
}

pub fn connect(
    net: &TcpHost,
    label: &str,
    handle: i64,
    to: &Endpoint,
    span: Span,
) -> Result<Vec<u8>, Diagnostic> {
    let socket = net.datagram(handle, label, span)?;
    let connected = to
        .socket_address()
        .and_then(|to| socket.connect(to).map_err(|e| NetError::of(&e)));
    Ok(wire::result(connected.map(|()| Vec::new())))
}

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
            .socket_address()
            .and_then(|to| socket.send_to(&payload, to).map_err(|e| NetError::of(&e)));
        Ok(wire::result(sent.map(|n| int(n as i64).to_vec())))
    }))
}

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
        let sent = socket.send(&payload).map_err(|e| NetError::of(&e));
        Ok(wire::result(sent.map(|n| int(n as i64).to_vec())))
    }))
}

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
        Ok(wire::result(received(&socket, max)))
    }))
}

fn received(socket: &UdpSocket, max: usize) -> Result<Vec<u8>, NetError> {
    let mut buffer = vec![0u8; max];
    let spare =
        unsafe { &mut *(std::ptr::from_mut::<[u8]>(&mut buffer) as *mut [MaybeUninit<u8>]) };
    let (read, flags, from) = SockRef::from(socket)
        .recv_from_vectored(&mut [MaybeUninitSlice::new(spare)])
        .map_err(|e| NetError::of(&e))?;
    buffer.truncate(read);
    let Some(from) = from.as_socket() else {
        return Err(NetError::Other(
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

fn bound(to: &Endpoint, options: &[SocketOption]) -> Result<UdpSocket, NetError> {
    let address = to.socket_address()?;
    let rejection = |e: std::io::Error| NetError::of(&e);
    let socket = Socket::new(
        Domain::for_address(address),
        Type::DGRAM,
        Some(Protocol::UDP),
    )
    .map_err(rejection)?;
    for option in options {
        crate::tcp::apply(&socket, option).map_err(rejection)?;
    }
    socket.bind(&SockAddr::from(address)).map_err(rejection)?;
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
