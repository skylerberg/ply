use super::{
    Nondeterministic, done, given, if_host_allowed, required_argument, waited, with_host_facilities,
};
use crate::c::{MachineNative, runtime_native};
use crate::emitted::SecretHandle;
use crate::native::{Address, I64, TakenBytes};
use crate::rt::{Context, borrowed};
use crate::secret;
use ply_core::Diagnostic;
use ply_host::native::parts_of;
use ply_host::tcp::{Endpoint, MAX_RECV, SocketOption, TcpHost, Upgrade};
use std::sync::Arc;
use std::time::Duration;

pub(super) fn natives() -> Vec<MachineNative> {
    vec![
        runtime_native(
            "net_listen",
            rt_net_listen as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        runtime_native(
            "net_listen_tls",
            rt_net_listen_tls as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "net_listen_on",
            rt_net_listen_on as unsafe extern "C" fn(_, _, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "net_listen_unix",
            rt_net_listen_unix as unsafe extern "C" fn(_, _, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "net_connect",
            rt_net_connect as unsafe extern "C" fn(_, _, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "net_connect_tls",
            rt_net_connect_tls as unsafe extern "C" fn(_, _, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "net_connect_to",
            rt_net_connect_to as unsafe extern "C" fn(_, _, _, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "net_connect_unix",
            rt_net_connect_unix as unsafe extern "C" fn(_, _, _, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "net_handshake",
            rt_net_handshake as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        runtime_native(
            "net_start_tls",
            rt_net_start_tls as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "net_serve_tls",
            rt_net_serve_tls as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "net_accept",
            rt_net_accept as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        runtime_native(
            "net_recv",
            rt_net_recv as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "net_send",
            rt_net_send as unsafe extern "C" fn(_, _, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "net_send_secret",
            rt_net_send_secret as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "net_close_write",
            rt_net_close_write as unsafe extern "C" fn(_, _, _, _),
        ),
        runtime_native(
            "net_close",
            rt_net_close as unsafe extern "C" fn(_, _, _, _),
        ),
        runtime_native(
            "net_set_option",
            rt_net_set_option as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "net_local_port",
            rt_net_local_port as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        runtime_native(
            "net_local_address",
            rt_net_local_address as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        runtime_native(
            "net_peer_address",
            rt_net_peer_address as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        runtime_native(
            "net_peer_credentials",
            rt_net_peer_credentials as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        runtime_native(
            "net_peer_certificate",
            rt_net_peer_certificate as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        runtime_native(
            "net_protocol",
            rt_net_protocol as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        runtime_native(
            "net_options",
            rt_net_options as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        runtime_native(
            "udp_bind",
            rt_udp_bind as unsafe extern "C" fn(_, _, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "udp_connect",
            rt_udp_connect as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "udp_send_to",
            rt_udp_send_to as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "udp_send",
            rt_udp_send as unsafe extern "C" fn(_, _, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "udp_recv_from",
            rt_udp_recv_from as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "udp_set_option",
            rt_udp_set_option as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "udp_close",
            rt_udp_close as unsafe extern "C" fn(_, _, _, _),
        ),
        runtime_native(
            "udp_local_address",
            rt_udp_local_address as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        runtime_native(
            "udp_options",
            rt_udp_options as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
    ]
}

fn labelled<'a>(c: &mut Context, label: &'a [u8]) -> Option<(Arc<TcpHost>, &'a str)> {
    let label = c.text(label)?;
    let net = with_host_facilities(c, |facilities| Arc::clone(facilities.sockets()))?;
    Some((net, label))
}

fn port(port: I64) -> Option<u16> {
    u16::try_from(port.0).ok()
}

fn deadline(ms: I64) -> Option<Duration> {
    u64::try_from(ms.0)
        .ok()
        .filter(|ms| *ms > 0)
        .map(Duration::from_millis)
}

fn count(n: I64) -> Option<usize> {
    usize::try_from(n.0).ok().filter(|n| *n > 0)
}

fn texts(flat: &[u8]) -> Option<Vec<String>> {
    parts_of(flat)?
        .into_iter()
        .map(|part| std::str::from_utf8(part).ok().map(str::to_string))
        .collect()
}

pub unsafe extern "C" fn rt_net_listen(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    at: I64,
) -> I64 {
    if_host_allowed(
        context,
        "net_listen",
        Nondeterministic,
        I64(0),
        |c: &mut Context| {
            let Some((net, label)) = labelled(c, unsafe { borrowed(label_at, label_n) }) else {
                return I64(0);
            };
            let Some(at) = required_argument(c, "net_listen", "a port", port(at)) else {
                return I64(0);
            };
            match net.listen(label, at, c.site()) {
                Ok(handle) => I64(handle),
                Err(d) => {
                    c.fail(d);
                    I64(0)
                }
            }
        },
    )
}

pub unsafe extern "C" fn rt_net_listen_tls(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    at: I64,
    credential_at: Address,
    credential_n: I64,
) -> I64 {
    if_host_allowed(
        context,
        "net_listen_tls",
        Nondeterministic,
        I64(0),
        |c: &mut Context| {
            let Some((net, label)) = labelled(c, unsafe { borrowed(label_at, label_n) }) else {
                return I64(0);
            };
            let Some(at) = required_argument(c, "net_listen_tls", "a port", port(at)) else {
                return I64(0);
            };
            let Some(credential) = c.text(unsafe { borrowed(credential_at, credential_n) }) else {
                return I64(0);
            };
            match net.listen_tls(label, at, credential, c.site()) {
                Ok(handle) => I64(handle),
                Err(d) => {
                    c.fail(d);
                    I64(0)
                }
            }
        },
    )
}

pub unsafe extern "C" fn rt_net_listen_on(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    endpoint_at: Address,
    endpoint_n: I64,
    options_at: Address,
    options_n: I64,
) -> TakenBytes {
    if_host_allowed(
        context,
        "net_listen_on",
        Nondeterministic,
        TakenBytes(0),
        |c: &mut Context| {
            let Some((net, label)) = labelled(c, unsafe { borrowed(label_at, label_n) }) else {
                return TakenBytes(0);
            };
            let to = Endpoint::read(unsafe { borrowed(endpoint_at, endpoint_n) });
            let options = SocketOption::list(unsafe { borrowed(options_at, options_n) });
            let (Some(to), Some(options)) = (
                required_argument(c, "net_listen_on", "an address", to),
                required_argument(c, "net_listen_on", "socket options", options),
            ) else {
                return TakenBytes(0);
            };
            c.given(net.listen_on(label, &to, &options))
        },
    )
}

pub unsafe extern "C" fn rt_net_listen_unix(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    root_at: Address,
    root_n: I64,
    path_at: Address,
    path_n: I64,
) -> TakenBytes {
    if_host_allowed(
        context,
        "net_listen_unix",
        Nondeterministic,
        TakenBytes(0),
        |c: &mut Context| {
            let Some((net, label)) = labelled(c, unsafe { borrowed(label_at, label_n) }) else {
                return TakenBytes(0);
            };
            let (Some(root), Some(path)) = (
                c.text(unsafe { borrowed(root_at, root_n) }),
                c.text(unsafe { borrowed(path_at, path_n) }),
            ) else {
                return TakenBytes(0);
            };
            let result = net.listen_unix(label, root, path, c.site());
            given(c, result)
        },
    )
}

pub unsafe extern "C" fn rt_net_connect(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    host_at: Address,
    host_n: I64,
    at: I64,
    timeout_ms: I64,
) -> I64 {
    if_host_allowed(
        context,
        "net_connect",
        Nondeterministic,
        I64(0),
        |c: &mut Context| {
            let Some((net, label)) = labelled(c, unsafe { borrowed(label_at, label_n) }) else {
                return I64(0);
            };
            let Some(host) = c.text(unsafe { borrowed(host_at, host_n) }) else {
                return I64(0);
            };
            let (Some(at), Some(timeout)) = (
                required_argument(c, "net_connect", "a port", port(at)),
                required_argument(c, "net_connect", "a deadline", deadline(timeout_ms)),
            ) else {
                return I64(0);
            };
            let job = net.connect(label, host.to_string(), at, timeout);
            waited(c, "net_connect", "`net.connect`", Ok(job))
        },
    )
}

pub unsafe extern "C" fn rt_net_connect_tls(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    host_at: Address,
    host_n: I64,
    at: I64,
    timeout_ms: I64,
) -> I64 {
    if_host_allowed(
        context,
        "net_connect_tls",
        Nondeterministic,
        I64(0),
        |c: &mut Context| {
            let Some((net, label)) = labelled(c, unsafe { borrowed(label_at, label_n) }) else {
                return I64(0);
            };
            let Some(host) = c.text(unsafe { borrowed(host_at, host_n) }) else {
                return I64(0);
            };
            let (Some(at), Some(timeout)) = (
                required_argument(c, "net_connect_tls", "a port", port(at)),
                required_argument(c, "net_connect_tls", "a deadline", deadline(timeout_ms)),
            ) else {
                return I64(0);
            };
            let job = net.connect_tls(label, host.to_string(), at, timeout);
            waited(c, "net_connect_tls", "`net.connect_tls`", Ok(job))
        },
    )
}

#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_net_connect_to(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    endpoint_at: Address,
    endpoint_n: I64,
    options_at: Address,
    options_n: I64,
    timeout_ms: I64,
) -> I64 {
    if_host_allowed(
        context,
        "net_connect_to",
        Nondeterministic,
        I64(0),
        |c: &mut Context| {
            let Some((net, label)) = labelled(c, unsafe { borrowed(label_at, label_n) }) else {
                return I64(0);
            };
            let to = Endpoint::read(unsafe { borrowed(endpoint_at, endpoint_n) });
            let options = SocketOption::list(unsafe { borrowed(options_at, options_n) });
            let (Some(to), Some(options), Some(timeout)) = (
                required_argument(c, "net_connect_to", "an address", to),
                required_argument(c, "net_connect_to", "socket options", options),
                required_argument(c, "net_connect_to", "a deadline", deadline(timeout_ms)),
            ) else {
                return I64(0);
            };
            let job = net.connect_to(label, to, options, timeout);
            waited(c, "net_connect_to", "`net.connect_to`", Ok(job))
        },
    )
}

#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_net_connect_unix(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    root_at: Address,
    root_n: I64,
    path_at: Address,
    path_n: I64,
    timeout_ms: I64,
) -> I64 {
    if_host_allowed(
        context,
        "net_connect_unix",
        Nondeterministic,
        I64(0),
        |c: &mut Context| {
            let Some((net, label)) = labelled(c, unsafe { borrowed(label_at, label_n) }) else {
                return I64(0);
            };
            let (Some(root), Some(path)) = (
                c.text(unsafe { borrowed(root_at, root_n) }),
                c.text(unsafe { borrowed(path_at, path_n) }),
            ) else {
                return I64(0);
            };
            let Some(timeout) =
                required_argument(c, "net_connect_unix", "a deadline", deadline(timeout_ms))
            else {
                return I64(0);
            };
            let job = net.connect_unix(label, root, path, timeout, c.site());
            waited(c, "net_connect_unix", "`net.connect_unix`", job)
        },
    )
}

pub unsafe extern "C" fn rt_net_handshake(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    conn: I64,
) -> I64 {
    if_host_allowed(
        context,
        "net_handshake",
        Nondeterministic,
        I64(0),
        |c: &mut Context| {
            let Some((net, label)) = labelled(c, unsafe { borrowed(label_at, label_n) }) else {
                return I64(0);
            };
            let job = net.handshake(label, conn.0, c.site());
            waited(c, "net_handshake", "`net.handshake`", job)
        },
    )
}

#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_net_start_tls(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    conn: I64,
    name_at: Address,
    name_n: I64,
    alpn_at: Address,
    alpn_n: I64,
    unread: I64,
    timeout_ms: I64,
) -> I64 {
    if_host_allowed(
        context,
        "net_start_tls",
        Nondeterministic,
        I64(0),
        |c: &mut Context| {
            let Some((net, label)) = labelled(c, unsafe { borrowed(label_at, label_n) }) else {
                return I64(0);
            };
            let Some(name) = c.text(unsafe { borrowed(name_at, name_n) }) else {
                return I64(0);
            };
            let Some(upgrade) = upgrade(c, "net_start_tls", alpn_at, alpn_n, unread, timeout_ms)
            else {
                return I64(0);
            };
            let job = net.start_tls(label, conn.0, name, upgrade, c.site());
            waited(c, "net_start_tls", "`net.start_tls`", job)
        },
    )
}

#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_net_serve_tls(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    conn: I64,
    credential_at: Address,
    credential_n: I64,
    alpn_at: Address,
    alpn_n: I64,
    unread: I64,
    timeout_ms: I64,
) -> I64 {
    if_host_allowed(
        context,
        "net_serve_tls",
        Nondeterministic,
        I64(0),
        |c: &mut Context| {
            let Some((net, label)) = labelled(c, unsafe { borrowed(label_at, label_n) }) else {
                return I64(0);
            };
            let Some(credential) = c.text(unsafe { borrowed(credential_at, credential_n) }) else {
                return I64(0);
            };
            let Some(upgrade) = upgrade(c, "net_serve_tls", alpn_at, alpn_n, unread, timeout_ms)
            else {
                return I64(0);
            };
            let job = net.serve_tls(label, conn.0, credential, upgrade, c.site());
            waited(c, "net_serve_tls", "`net.serve_tls`", job)
        },
    )
}

fn upgrade(
    c: &mut Context,
    native: &str,
    alpn_at: Address,
    alpn_n: I64,
    unread: I64,
    timeout_ms: I64,
) -> Option<Upgrade> {
    let alpn = required_argument(
        c,
        native,
        "application protocols",
        texts(unsafe { borrowed(alpn_at, alpn_n) }),
    )?;
    let unread = required_argument(
        c,
        native,
        "a count of unread bytes",
        usize::try_from(unread.0).ok(),
    )?;
    let timeout = required_argument(c, native, "a deadline", deadline(timeout_ms))?;
    Some(Upgrade {
        alpn,
        unread,
        timeout,
    })
}

pub unsafe extern "C" fn rt_net_accept(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    listener: I64,
) -> I64 {
    if_host_allowed(
        context,
        "net_accept",
        Nondeterministic,
        I64(0),
        |c: &mut Context| {
            let Some((net, label)) = labelled(c, unsafe { borrowed(label_at, label_n) }) else {
                return I64(0);
            };
            let job = net.accept(label, listener.0, c.site());
            waited(c, "net_accept", "`net.accept`", job)
        },
    )
}

pub unsafe extern "C" fn rt_net_recv(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    conn: I64,
    max: I64,
    timeout_ms: I64,
) -> I64 {
    if_host_allowed(
        context,
        "net_recv",
        Nondeterministic,
        I64(0),
        |c: &mut Context| {
            let Some((net, label)) = labelled(c, unsafe { borrowed(label_at, label_n) }) else {
                return I64(0);
            };
            let (Some(max), Some(timeout)) = (
                required_argument(c, "net_recv", "a byte count", count(max)),
                required_argument(c, "net_recv", "a deadline", deadline(timeout_ms)),
            ) else {
                return I64(0);
            };
            let job = net.recv(label, conn.0, max.min(MAX_RECV), timeout, c.site());
            waited(c, "net_recv", "`net.recv`", job)
        },
    )
}

pub unsafe extern "C" fn rt_net_send(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    conn: I64,
    payload_at: Address,
    payload_len: I64,
    timeout_ms: I64,
) -> I64 {
    if_host_allowed(
        context,
        "net_send",
        Nondeterministic,
        I64(0),
        |c: &mut Context| {
            let Some((net, label)) = labelled(c, unsafe { borrowed(label_at, label_n) }) else {
                return I64(0);
            };
            let Some(timeout) =
                required_argument(c, "net_send", "a deadline", deadline(timeout_ms))
            else {
                return I64(0);
            };
            let payload = unsafe { borrowed(payload_at, payload_len) }.to_vec();
            let job = net.send(label, conn.0, payload, timeout, c.site());
            waited(c, "net_send", "`net.send`", job)
        },
    )
}

pub unsafe extern "C" fn rt_net_send_secret(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    conn: I64,
    framed: SecretHandle,
    timeout_ms: I64,
) -> I64 {
    if_host_allowed(
        context,
        "net_send_secret",
        Nondeterministic,
        I64(0),
        |c: &mut Context| {
            let Some((net, label)) = labelled(c, unsafe { borrowed(label_at, label_n) }) else {
                return I64(0);
            };
            let Some(framed) = secret::sealed_value(c, &framed) else {
                return I64(0);
            };
            let Some(timeout) =
                required_argument(c, "net_send_secret", "a deadline", deadline(timeout_ms))
            else {
                return I64(0);
            };
            let job = net.send_secret(label, conn.0, framed.bytes(), timeout, c.site());
            waited(c, "net_send_secret", "`net.send_secret`", job)
        },
    )
}

pub unsafe extern "C" fn rt_net_close_write(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    conn: I64,
) {
    if_host_allowed(
        context,
        "net_close_write",
        Nondeterministic,
        (),
        |c: &mut Context| {
            let Some((net, label)) = labelled(c, unsafe { borrowed(label_at, label_n) }) else {
                return;
            };
            let closed = net.close_write(label, conn.0, c.site());
            done(c, closed);
        },
    );
}

pub unsafe extern "C" fn rt_net_close(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    socket: I64,
) {
    if_host_allowed(
        context,
        "net_close",
        Nondeterministic,
        (),
        |c: &mut Context| {
            let Some((net, label)) = labelled(c, unsafe { borrowed(label_at, label_n) }) else {
                return;
            };
            let closed = net.close(label, socket.0, c.site());
            done(c, closed);
        },
    );
}

pub unsafe extern "C" fn rt_net_set_option(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    socket: I64,
    option_address: Address,
    option_len: I64,
) -> TakenBytes {
    if_host_allowed(
        context,
        "net_set_option",
        Nondeterministic,
        TakenBytes(0),
        |c: &mut Context| {
            set_option(
                c,
                "net_set_option",
                label_at,
                label_n,
                socket,
                option_address,
                option_len,
                false,
            )
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn set_option(
    c: &mut Context,
    native: &str,
    label_at: Address,
    label_n: I64,
    socket: I64,
    option_address: Address,
    option_len: I64,
    datagram: bool,
) -> TakenBytes {
    let Some((net, label)) = labelled(c, unsafe { borrowed(label_at, label_n) }) else {
        return TakenBytes(0);
    };
    let option = SocketOption::read(unsafe { borrowed(option_address, option_len) });
    let Some(option) = required_argument(c, native, "a socket option", option) else {
        return TakenBytes(0);
    };
    let span = c.site();
    let result = register_datagram_when_requested(&net, label, socket, datagram, span)
        .and_then(|()| net.set_option(label, socket.0, option, span));
    given(c, result)
}

fn register_datagram_when_requested(
    net: &TcpHost,
    label: &str,
    socket: I64,
    datagram: bool,
    span: ply_core::Span,
) -> Result<(), Diagnostic> {
    if datagram {
        net.datagram(socket.0, label, span)?;
    }
    Ok(())
}

pub unsafe extern "C" fn rt_net_local_port(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    socket: I64,
) -> I64 {
    if_host_allowed(
        context,
        "net_local_port",
        Nondeterministic,
        I64(-1),
        |c: &mut Context| {
            let Some((net, label)) = labelled(c, unsafe { borrowed(label_at, label_n) }) else {
                return I64(-1);
            };
            match net.local_port(label, socket.0, c.site()) {
                Ok(at) => I64(at.map_or(-1, i64::from)),
                Err(d) => {
                    c.fail(d);
                    I64(-1)
                }
            }
        },
    )
}

type SocketInfoReader = fn(&TcpHost, &str, i64, ply_core::Span) -> Result<Vec<u8>, Diagnostic>;

fn described(
    context: *mut Context,
    native: &'static str,
    label_at: Address,
    label_n: I64,
    socket: I64,
    datagram: bool,
    read: SocketInfoReader,
) -> TakenBytes {
    if_host_allowed(
        context,
        native,
        Nondeterministic,
        TakenBytes(0),
        |c: &mut Context| {
            let Some((net, label)) = labelled(c, unsafe { borrowed(label_at, label_n) }) else {
                return TakenBytes(0);
            };
            let span = c.site();
            let result = register_datagram_when_requested(&net, label, socket, datagram, span)
                .and_then(|()| read(&net, label, socket.0, span));
            given(c, result)
        },
    )
}

pub unsafe extern "C" fn rt_net_local_address(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    socket: I64,
) -> TakenBytes {
    described(
        context,
        "net_local_address",
        label_at,
        label_n,
        socket,
        false,
        TcpHost::local_address,
    )
}

pub unsafe extern "C" fn rt_net_peer_address(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    conn: I64,
) -> TakenBytes {
    described(
        context,
        "net_peer_address",
        label_at,
        label_n,
        conn,
        false,
        TcpHost::peer_address,
    )
}

pub unsafe extern "C" fn rt_net_peer_credentials(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    conn: I64,
) -> TakenBytes {
    described(
        context,
        "net_peer_credentials",
        label_at,
        label_n,
        conn,
        false,
        TcpHost::peer_credentials,
    )
}

pub unsafe extern "C" fn rt_net_peer_certificate(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    conn: I64,
) -> TakenBytes {
    described(
        context,
        "net_peer_certificate",
        label_at,
        label_n,
        conn,
        false,
        TcpHost::peer_certificate,
    )
}

pub unsafe extern "C" fn rt_net_protocol(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    conn: I64,
) -> TakenBytes {
    described(
        context,
        "net_protocol",
        label_at,
        label_n,
        conn,
        false,
        TcpHost::protocol,
    )
}

pub unsafe extern "C" fn rt_net_options(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    socket: I64,
) -> TakenBytes {
    described(
        context,
        "net_options",
        label_at,
        label_n,
        socket,
        false,
        TcpHost::options,
    )
}

pub unsafe extern "C" fn rt_udp_bind(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    endpoint_at: Address,
    endpoint_n: I64,
    options_at: Address,
    options_n: I64,
) -> TakenBytes {
    if_host_allowed(
        context,
        "udp_bind",
        Nondeterministic,
        TakenBytes(0),
        |c: &mut Context| {
            let Some((net, label)) = labelled(c, unsafe { borrowed(label_at, label_n) }) else {
                return TakenBytes(0);
            };
            let to = Endpoint::read(unsafe { borrowed(endpoint_at, endpoint_n) });
            let options = SocketOption::list(unsafe { borrowed(options_at, options_n) });
            let (Some(to), Some(options)) = (
                required_argument(c, "udp_bind", "an address", to),
                required_argument(c, "udp_bind", "socket options", options),
            ) else {
                return TakenBytes(0);
            };
            c.given(ply_host::udp::bind(&net, label, &to, &options))
        },
    )
}

pub unsafe extern "C" fn rt_udp_connect(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    socket: I64,
    endpoint_at: Address,
    endpoint_n: I64,
) -> TakenBytes {
    if_host_allowed(
        context,
        "udp_connect",
        Nondeterministic,
        TakenBytes(0),
        |c: &mut Context| {
            let Some((net, label)) = labelled(c, unsafe { borrowed(label_at, label_n) }) else {
                return TakenBytes(0);
            };
            let to = Endpoint::read(unsafe { borrowed(endpoint_at, endpoint_n) });
            let Some(to) = required_argument(c, "udp_connect", "an address", to) else {
                return TakenBytes(0);
            };
            let result = ply_host::udp::connect(&net, label, socket.0, &to, c.site());
            given(c, result)
        },
    )
}

#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_udp_send_to(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    socket: I64,
    endpoint_at: Address,
    endpoint_n: I64,
    payload_at: Address,
    payload_len: I64,
    timeout_ms: I64,
) -> I64 {
    if_host_allowed(
        context,
        "udp_send_to",
        Nondeterministic,
        I64(0),
        |c: &mut Context| {
            let Some((net, label)) = labelled(c, unsafe { borrowed(label_at, label_n) }) else {
                return I64(0);
            };
            let to = Endpoint::read(unsafe { borrowed(endpoint_at, endpoint_n) });
            let (Some(to), Some(timeout)) = (
                required_argument(c, "udp_send_to", "an address", to),
                required_argument(c, "udp_send_to", "a deadline", deadline(timeout_ms)),
            ) else {
                return I64(0);
            };
            let payload = unsafe { borrowed(payload_at, payload_len) }.to_vec();
            let job = ply_host::udp::send_to(&net, label, socket.0, to, payload, timeout, c.site());
            waited(c, "udp_send_to", "`udp.send_to`", job)
        },
    )
}

pub unsafe extern "C" fn rt_udp_send(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    socket: I64,
    payload_at: Address,
    payload_len: I64,
    timeout_ms: I64,
) -> I64 {
    if_host_allowed(
        context,
        "udp_send",
        Nondeterministic,
        I64(0),
        |c: &mut Context| {
            let Some((net, label)) = labelled(c, unsafe { borrowed(label_at, label_n) }) else {
                return I64(0);
            };
            let Some(timeout) =
                required_argument(c, "udp_send", "a deadline", deadline(timeout_ms))
            else {
                return I64(0);
            };
            let payload = unsafe { borrowed(payload_at, payload_len) }.to_vec();
            let job = ply_host::udp::send(&net, label, socket.0, payload, timeout, c.site());
            waited(c, "udp_send", "`udp.send`", job)
        },
    )
}

pub unsafe extern "C" fn rt_udp_recv_from(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    socket: I64,
    max: I64,
    timeout_ms: I64,
) -> I64 {
    if_host_allowed(
        context,
        "udp_recv_from",
        Nondeterministic,
        I64(0),
        |c: &mut Context| {
            let Some((net, label)) = labelled(c, unsafe { borrowed(label_at, label_n) }) else {
                return I64(0);
            };
            let (Some(max), Some(timeout)) = (
                required_argument(c, "udp_recv_from", "a byte count", count(max)),
                required_argument(c, "udp_recv_from", "a deadline", deadline(timeout_ms)),
            ) else {
                return I64(0);
            };
            let job = ply_host::udp::recv_from(&net, label, socket.0, max, timeout, c.site());
            waited(c, "udp_recv_from", "`udp.recv_from`", job)
        },
    )
}

pub unsafe extern "C" fn rt_udp_set_option(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    socket: I64,
    option_address: Address,
    option_len: I64,
) -> TakenBytes {
    if_host_allowed(
        context,
        "udp_set_option",
        Nondeterministic,
        TakenBytes(0),
        |c: &mut Context| {
            set_option(
                c,
                "udp_set_option",
                label_at,
                label_n,
                socket,
                option_address,
                option_len,
                true,
            )
        },
    )
}

pub unsafe extern "C" fn rt_udp_close(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    socket: I64,
) {
    if_host_allowed(
        context,
        "udp_close",
        Nondeterministic,
        (),
        |c: &mut Context| {
            let Some((net, label)) = labelled(c, unsafe { borrowed(label_at, label_n) }) else {
                return;
            };
            let span = c.site();
            let closed = register_datagram_when_requested(&net, label, socket, true, span)
                .and_then(|()| net.close(label, socket.0, span));
            done(c, closed);
        },
    );
}

pub unsafe extern "C" fn rt_udp_local_address(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    socket: I64,
) -> TakenBytes {
    described(
        context,
        "udp_local_address",
        label_at,
        label_n,
        socket,
        true,
        TcpHost::local_address,
    )
}

pub unsafe extern "C" fn rt_udp_options(
    context: *mut Context,
    label_at: Address,
    label_n: I64,
    socket: I64,
) -> TakenBytes {
    described(
        context,
        "udp_options",
        label_at,
        label_n,
        socket,
        true,
        TcpHost::options,
    )
}
