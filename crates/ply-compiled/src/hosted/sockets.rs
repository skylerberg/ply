//! `std.net`'s and `std.udp`'s natives: the run's socket table, each socket named by its handle and
//! the label the clause that asks binds. What they are handed and answer is flat as
//! `ply_host::tcp::wire` reads and writes it; an operation that waits answers a ticket.

use super::{READS, WRITES, asked, opened, secret_of};
use crate::c::{MachineNative, machine_native};
use crate::emitted::SecretHandle;
use crate::native::{Addr, I64, TakenBytes};
use crate::rt::{Ctx, lent};
use ply_core::{Diagnostic, codes};
use ply_host::jobs::Work;
use ply_host::native::parts_of;
use ply_host::tcp::{Endpoint, Job, MAX_RECV, SocketOption, TcpHost, Upgrade};
use std::sync::Arc;
use std::time::Duration;

pub(super) fn natives() -> Vec<MachineNative> {
    vec![
        machine_native(
            "net_listen",
            rt_net_listen as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        machine_native(
            "net_listen_tls",
            rt_net_listen_tls as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
        machine_native(
            "net_listen_on",
            rt_net_listen_on as unsafe extern "C" fn(_, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "net_listen_unix",
            rt_net_listen_unix as unsafe extern "C" fn(_, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "net_connect",
            rt_net_connect as unsafe extern "C" fn(_, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "net_connect_tls",
            rt_net_connect_tls as unsafe extern "C" fn(_, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "net_connect_to",
            rt_net_connect_to as unsafe extern "C" fn(_, _, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "net_connect_unix",
            rt_net_connect_unix as unsafe extern "C" fn(_, _, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "net_handshake",
            rt_net_handshake as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        machine_native(
            "net_start_tls",
            rt_net_start_tls as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "net_serve_tls",
            rt_net_serve_tls as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "net_accept",
            rt_net_accept as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        machine_native(
            "net_recv",
            rt_net_recv as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
        machine_native(
            "net_send",
            rt_net_send as unsafe extern "C" fn(_, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "net_send_secret",
            rt_net_send_secret as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
        machine_native(
            "net_close_write",
            rt_net_close_write as unsafe extern "C" fn(_, _, _, _),
        ),
        machine_native(
            "net_close",
            rt_net_close as unsafe extern "C" fn(_, _, _, _),
        ),
        machine_native(
            "net_set_option",
            rt_net_set_option as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
        machine_native(
            "net_local_port",
            rt_net_local_port as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        machine_native(
            "net_local_address",
            rt_net_local_address as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        machine_native(
            "net_peer_address",
            rt_net_peer_address as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        machine_native(
            "net_peer_credentials",
            rt_net_peer_credentials as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        machine_native(
            "net_peer_certificate",
            rt_net_peer_certificate as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        machine_native(
            "net_protocol",
            rt_net_protocol as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        machine_native(
            "net_options",
            rt_net_options as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        machine_native(
            "udp_bind",
            rt_udp_bind as unsafe extern "C" fn(_, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "udp_connect",
            rt_udp_connect as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
        machine_native(
            "udp_send_to",
            rt_udp_send_to as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "udp_send",
            rt_udp_send as unsafe extern "C" fn(_, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "udp_recv_from",
            rt_udp_recv_from as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
        machine_native(
            "udp_set_option",
            rt_udp_set_option as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
        machine_native(
            "udp_close",
            rt_udp_close as unsafe extern "C" fn(_, _, _, _),
        ),
        machine_native(
            "udp_local_address",
            rt_udp_local_address as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        machine_native(
            "udp_options",
            rt_udp_options as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
    ]
}

/// The run's socket table, and the label at `label` the clause bound; `None`, with the run failed,
/// for a runtime no host made or a label that is not UTF-8.
fn labelled<'a>(c: &mut Ctx, label: &'a [u8]) -> Option<(Arc<TcpHost>, &'a str)> {
    let label = c.text(label)?;
    let runtime = c.host_runtime();
    let Some(facilities) = ply_host::registry::facilities_of(&*runtime) else {
        c.fail(Diagnostic::error(
            codes::INTERNAL_ERROR,
            "a socket was asked for of a runtime no host made",
        ));
        return None;
    };
    Some((Arc::clone(facilities.sockets()), label))
}

/// What a native was handed, read; `None`, with the run failed, where it is not what the real
/// handler hands on, which checks what it hands.
fn handed<T>(c: &mut Ctx, native: &str, what: &str, read: Option<T>) -> Option<T> {
    if read.is_none() {
        c.fail(Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("the native `{native}` was handed {what} it cannot be"),
        ));
    }
    read
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

/// `answer`'s bytes as taken bytes; none, with the run failed, where it is a diagnostic.
fn given(c: &mut Ctx, answer: Result<Vec<u8>, Diagnostic>) -> TakenBytes {
    match answer {
        Ok(bytes) => c.given(bytes),
        Err(d) => {
            c.fail(d);
            TakenBytes(0)
        }
    }
}

/// The ticket of `job`, which `native` asked for as `what`; 0, with the run failed, where it is a
/// diagnostic.
fn waited(
    c: &mut Ctx,
    native: &'static str,
    what: &'static str,
    job: Result<Job, Diagnostic>,
) -> I64 {
    match job {
        Ok(job) => c.submitted(native, what, Work::Waiting, job),
        Err(d) => {
            c.fail(d);
            I64(0)
        }
    }
}

/// Runs `done`; the run failed where it is a diagnostic.
fn done(c: &mut Ctx, done: Result<(), Diagnostic>) {
    if let Err(d) = done {
        c.fail(d);
    }
}

/// A listener on `port` of the loopback address, as its handle.
pub unsafe extern "C" fn rt_net_listen(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    at: I64,
) -> I64 {
    asked(ctx, "net_listen", WRITES, I64(0), |c: &mut Ctx| {
        let Some((net, label)) = labelled(c, unsafe { lent(label_at, label_n) }) else {
            return I64(0);
        };
        let Some(at) = handed(c, "net_listen", "a port", port(at)) else {
            return I64(0);
        };
        match net.listen(label, at, c.site()) {
            Ok(handle) => I64(handle),
            Err(d) => {
                c.fail(d);
                I64(0)
            }
        }
    })
}

/// A listener on `port` of the loopback address that serves TLS with the credential at
/// `credential_at`, as its handle.
pub unsafe extern "C" fn rt_net_listen_tls(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    at: I64,
    credential_at: Addr,
    credential_n: I64,
) -> I64 {
    asked(ctx, "net_listen_tls", WRITES, I64(0), |c: &mut Ctx| {
        let Some((net, label)) = labelled(c, unsafe { lent(label_at, label_n) }) else {
            return I64(0);
        };
        let Some(at) = handed(c, "net_listen_tls", "a port", port(at)) else {
            return I64(0);
        };
        let Some(credential) = c.text(unsafe { lent(credential_at, credential_n) }) else {
            return I64(0);
        };
        match net.listen_tls(label, at, credential, c.site()) {
            Ok(handle) => I64(handle),
            Err(d) => {
                c.fail(d);
                I64(0)
            }
        }
    })
}

/// A listener at exactly the address at `to_at`, with the options at `options_at` set before the
/// bind: its handle, or why not.
pub unsafe extern "C" fn rt_net_listen_on(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    to_at: Addr,
    to_n: I64,
    options_at: Addr,
    options_n: I64,
) -> TakenBytes {
    asked(
        ctx,
        "net_listen_on",
        WRITES,
        TakenBytes(0),
        |c: &mut Ctx| {
            let Some((net, label)) = labelled(c, unsafe { lent(label_at, label_n) }) else {
                return TakenBytes(0);
            };
            let to = Endpoint::read(unsafe { lent(to_at, to_n) });
            let options = SocketOption::list(unsafe { lent(options_at, options_n) });
            let (Some(to), Some(options)) = (
                handed(c, "net_listen_on", "an address", to),
                handed(c, "net_listen_on", "socket options", options),
            ) else {
                return TakenBytes(0);
            };
            c.given(net.listen_on(label, &to, &options))
        },
    )
}

/// A listener on the Unix socket at the path at `path_at` under the root named at `root_at`: its
/// handle, or why not.
pub unsafe extern "C" fn rt_net_listen_unix(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    root_at: Addr,
    root_n: I64,
    path_at: Addr,
    path_n: I64,
) -> TakenBytes {
    asked(
        ctx,
        "net_listen_unix",
        WRITES,
        TakenBytes(0),
        |c: &mut Ctx| {
            let Some((net, label)) = labelled(c, unsafe { lent(label_at, label_n) }) else {
                return TakenBytes(0);
            };
            let (Some(root), Some(path)) = (
                c.text(unsafe { lent(root_at, root_n) }),
                c.text(unsafe { lent(path_at, path_n) }),
            ) else {
                return TakenBytes(0);
            };
            let answer = net.listen_unix(label, root, path, c.site());
            given(c, answer)
        },
    )
}

/// The ticket of a connection to the host named at `host_at`: its handle found, or none.
pub unsafe extern "C" fn rt_net_connect(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    host_at: Addr,
    host_n: I64,
    at: I64,
    timeout_ms: I64,
) -> I64 {
    asked(ctx, "net_connect", WRITES, I64(0), |c: &mut Ctx| {
        let Some((net, label)) = labelled(c, unsafe { lent(label_at, label_n) }) else {
            return I64(0);
        };
        let Some(host) = c.text(unsafe { lent(host_at, host_n) }) else {
            return I64(0);
        };
        let (Some(at), Some(timeout)) = (
            handed(c, "net_connect", "a port", port(at)),
            handed(c, "net_connect", "a deadline", deadline(timeout_ms)),
        ) else {
            return I64(0);
        };
        let job = net.connect(label, host.to_string(), at, timeout);
        waited(c, "net_connect", "`net.connect`", Ok(job))
    })
}

/// The ticket of a TLS connection to the host named at `host_at`, verified as that name: its
/// handle found, or none.
pub unsafe extern "C" fn rt_net_connect_tls(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    host_at: Addr,
    host_n: I64,
    at: I64,
    timeout_ms: I64,
) -> I64 {
    asked(ctx, "net_connect_tls", WRITES, I64(0), |c: &mut Ctx| {
        let Some((net, label)) = labelled(c, unsafe { lent(label_at, label_n) }) else {
            return I64(0);
        };
        let Some(host) = c.text(unsafe { lent(host_at, host_n) }) else {
            return I64(0);
        };
        let (Some(at), Some(timeout)) = (
            handed(c, "net_connect_tls", "a port", port(at)),
            handed(c, "net_connect_tls", "a deadline", deadline(timeout_ms)),
        ) else {
            return I64(0);
        };
        let job = net.connect_tls(label, host.to_string(), at, timeout);
        waited(c, "net_connect_tls", "`net.connect_tls`", Ok(job))
    })
}

/// The ticket of a connection to exactly the address at `to_at`: its handle, or why not.
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_net_connect_to(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    to_at: Addr,
    to_n: I64,
    options_at: Addr,
    options_n: I64,
    timeout_ms: I64,
) -> I64 {
    asked(ctx, "net_connect_to", WRITES, I64(0), |c: &mut Ctx| {
        let Some((net, label)) = labelled(c, unsafe { lent(label_at, label_n) }) else {
            return I64(0);
        };
        let to = Endpoint::read(unsafe { lent(to_at, to_n) });
        let options = SocketOption::list(unsafe { lent(options_at, options_n) });
        let (Some(to), Some(options), Some(timeout)) = (
            handed(c, "net_connect_to", "an address", to),
            handed(c, "net_connect_to", "socket options", options),
            handed(c, "net_connect_to", "a deadline", deadline(timeout_ms)),
        ) else {
            return I64(0);
        };
        let job = net.connect_to(label, to, options, timeout);
        waited(c, "net_connect_to", "`net.connect_to`", Ok(job))
    })
}

/// The ticket of a connection to the Unix socket at the path at `path_at` under the root named at
/// `root_at`: its handle, or why not.
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_net_connect_unix(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    root_at: Addr,
    root_n: I64,
    path_at: Addr,
    path_n: I64,
    timeout_ms: I64,
) -> I64 {
    asked(ctx, "net_connect_unix", WRITES, I64(0), |c: &mut Ctx| {
        let Some((net, label)) = labelled(c, unsafe { lent(label_at, label_n) }) else {
            return I64(0);
        };
        let (Some(root), Some(path)) = (
            c.text(unsafe { lent(root_at, root_n) }),
            c.text(unsafe { lent(path_at, path_n) }),
        ) else {
            return I64(0);
        };
        let Some(timeout) = handed(c, "net_connect_unix", "a deadline", deadline(timeout_ms))
        else {
            return I64(0);
        };
        let job = net.connect_unix(label, root, path, timeout, c.site());
        waited(c, "net_connect_unix", "`net.connect_unix`", job)
    })
}

/// The ticket of a connection's TLS handshake: the microseconds it took found, or none.
pub unsafe extern "C" fn rt_net_handshake(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    conn: I64,
) -> I64 {
    asked(ctx, "net_handshake", WRITES, I64(0), |c: &mut Ctx| {
        let Some((net, label)) = labelled(c, unsafe { lent(label_at, label_n) }) else {
            return I64(0);
        };
        let job = net.handshake(label, conn.0, c.site());
        waited(c, "net_handshake", "`net.handshake`", job)
    })
}

/// The ticket of a plaintext connection secured as its client, verifying the server as the name
/// at `name_at`: the protocol agreed found, or why not.
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_net_start_tls(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    conn: I64,
    name_at: Addr,
    name_n: I64,
    alpn_at: Addr,
    alpn_n: I64,
    unread: I64,
    timeout_ms: I64,
) -> I64 {
    asked(ctx, "net_start_tls", WRITES, I64(0), |c: &mut Ctx| {
        let Some((net, label)) = labelled(c, unsafe { lent(label_at, label_n) }) else {
            return I64(0);
        };
        let Some(name) = c.text(unsafe { lent(name_at, name_n) }) else {
            return I64(0);
        };
        let Some(upgrade) = upgrade(c, "net_start_tls", alpn_at, alpn_n, unread, timeout_ms) else {
            return I64(0);
        };
        let job = net.start_tls(label, conn.0, name, upgrade, c.site());
        waited(c, "net_start_tls", "`net.start_tls`", job)
    })
}

/// The ticket of a plaintext connection secured as its server, with the credential named at
/// `credential_at`: the protocol agreed found, or why not.
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_net_serve_tls(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    conn: I64,
    credential_at: Addr,
    credential_n: I64,
    alpn_at: Addr,
    alpn_n: I64,
    unread: I64,
    timeout_ms: I64,
) -> I64 {
    asked(ctx, "net_serve_tls", WRITES, I64(0), |c: &mut Ctx| {
        let Some((net, label)) = labelled(c, unsafe { lent(label_at, label_n) }) else {
            return I64(0);
        };
        let Some(credential) = c.text(unsafe { lent(credential_at, credential_n) }) else {
            return I64(0);
        };
        let Some(upgrade) = upgrade(c, "net_serve_tls", alpn_at, alpn_n, unread, timeout_ms) else {
            return I64(0);
        };
        let job = net.serve_tls(label, conn.0, credential, upgrade, c.site());
        waited(c, "net_serve_tls", "`net.serve_tls`", job)
    })
}

/// What an upgrade is asked with: the protocols at `alpn_at` as parts, the count of plaintext
/// unread and the deadline.
fn upgrade(
    c: &mut Ctx,
    native: &str,
    alpn_at: Addr,
    alpn_n: I64,
    unread: I64,
    timeout_ms: I64,
) -> Option<Upgrade> {
    let alpn = handed(
        c,
        native,
        "application protocols",
        texts(unsafe { lent(alpn_at, alpn_n) }),
    )?;
    let unread = handed(
        c,
        native,
        "a count of unread bytes",
        usize::try_from(unread.0).ok(),
    )?;
    let timeout = handed(c, native, "a deadline", deadline(timeout_ms))?;
    Some(Upgrade {
        alpn,
        unread,
        timeout,
    })
}

/// The ticket of the next connection a listener takes, as its handle; `0` once the run has
/// stopped accepting.
pub unsafe extern "C" fn rt_net_accept(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    listener: I64,
) -> I64 {
    asked(ctx, "net_accept", WRITES, I64(0), |c: &mut Ctx| {
        let Some((net, label)) = labelled(c, unsafe { lent(label_at, label_n) }) else {
            return I64(0);
        };
        let job = net.accept(label, listener.0, c.site());
        waited(c, "net_accept", "`net.accept`", job)
    })
}

/// The ticket of at most `max` bytes of a connection found, none for the deadline.
pub unsafe extern "C" fn rt_net_recv(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    conn: I64,
    max: I64,
    timeout_ms: I64,
) -> I64 {
    asked(ctx, "net_recv", WRITES, I64(0), |c: &mut Ctx| {
        let Some((net, label)) = labelled(c, unsafe { lent(label_at, label_n) }) else {
            return I64(0);
        };
        let (Some(max), Some(timeout)) = (
            handed(c, "net_recv", "a byte count", count(max)),
            handed(c, "net_recv", "a deadline", deadline(timeout_ms)),
        ) else {
            return I64(0);
        };
        let job = net.recv(label, conn.0, max.min(MAX_RECV), timeout, c.site());
        waited(c, "net_recv", "`net.recv`", job)
    })
}

/// The ticket of a write of some prefix of the payload at `payload_at`: how many bytes it took
/// found, none for the deadline.
pub unsafe extern "C" fn rt_net_send(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    conn: I64,
    payload_at: Addr,
    payload_n: I64,
    timeout_ms: I64,
) -> I64 {
    asked(ctx, "net_send", WRITES, I64(0), |c: &mut Ctx| {
        let Some((net, label)) = labelled(c, unsafe { lent(label_at, label_n) }) else {
            return I64(0);
        };
        let Some(timeout) = handed(c, "net_send", "a deadline", deadline(timeout_ms)) else {
            return I64(0);
        };
        let payload = unsafe { lent(payload_at, payload_n) }.to_vec();
        let job = net.send(label, conn.0, payload, timeout, c.site());
        waited(c, "net_send", "`net.send`", job)
    })
}

/// The ticket of a write of every byte of the credential `framed`, byte 1 where they all went.
pub unsafe extern "C" fn rt_net_send_secret(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    conn: I64,
    framed: SecretHandle,
    timeout_ms: I64,
) -> I64 {
    asked(ctx, "net_send_secret", WRITES, I64(0), |c: &mut Ctx| {
        let Some((net, label)) = labelled(c, unsafe { lent(label_at, label_n) }) else {
            return I64(0);
        };
        let Some(framed) = secret_of(c, &framed) else {
            return I64(0);
        };
        let Some(timeout) = handed(c, "net_send_secret", "a deadline", deadline(timeout_ms)) else {
            return I64(0);
        };
        let job = net.send_secret(label, conn.0, opened(&framed), timeout, c.site());
        waited(c, "net_send_secret", "`net.send_secret`", job)
    })
}

/// Ends a connection's sending.
pub unsafe extern "C" fn rt_net_close_write(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    conn: I64,
) {
    asked(ctx, "net_close_write", WRITES, (), |c: &mut Ctx| {
        let Some((net, label)) = labelled(c, unsafe { lent(label_at, label_n) }) else {
            return;
        };
        let closed = net.close_write(label, conn.0, c.site());
        done(c, closed);
    });
}

/// Closes a socket of any kind.
pub unsafe extern "C" fn rt_net_close(ctx: *mut Ctx, label_at: Addr, label_n: I64, socket: I64) {
    asked(ctx, "net_close", WRITES, (), |c: &mut Ctx| {
        let Some((net, label)) = labelled(c, unsafe { lent(label_at, label_n) }) else {
            return;
        };
        let closed = net.close(label, socket.0, c.site());
        done(c, closed);
    });
}

/// Sets the option at `option_at` on a socket: nothing, or why not.
pub unsafe extern "C" fn rt_net_set_option(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    socket: I64,
    option_at: Addr,
    option_n: I64,
) -> TakenBytes {
    asked(
        ctx,
        "net_set_option",
        WRITES,
        TakenBytes(0),
        |c: &mut Ctx| {
            set_option(
                c,
                "net_set_option",
                label_at,
                label_n,
                socket,
                option_at,
                option_n,
                false,
            )
        },
    )
}

/// The option at `option_at` set on a socket, a datagram one where `datagram` says it must be.
#[allow(clippy::too_many_arguments)]
fn set_option(
    c: &mut Ctx,
    native: &str,
    label_at: Addr,
    label_n: I64,
    socket: I64,
    option_at: Addr,
    option_n: I64,
    datagram: bool,
) -> TakenBytes {
    let Some((net, label)) = labelled(c, unsafe { lent(label_at, label_n) }) else {
        return TakenBytes(0);
    };
    let option = SocketOption::read(unsafe { lent(option_at, option_n) });
    let Some(option) = handed(c, native, "a socket option", option) else {
        return TakenBytes(0);
    };
    let span = c.site();
    let answer = held_datagram(&net, label, socket, datagram, span)
        .and_then(|()| net.set_option(label, socket.0, option, span));
    given(c, answer)
}

/// Holds `socket` to a datagram socket where `datagram` says it must be one.
fn held_datagram(
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

/// The port this end of a socket is bound to, or -1 for a listener the drain has closed.
pub unsafe extern "C" fn rt_net_local_port(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    socket: I64,
) -> I64 {
    asked(ctx, "net_local_port", READS, I64(-1), |c: &mut Ctx| {
        let Some((net, label)) = labelled(c, unsafe { lent(label_at, label_n) }) else {
            return I64(-1);
        };
        match net.local_port(label, socket.0, c.site()) {
            Ok(at) => I64(at.map_or(-1, i64::from)),
            Err(d) => {
                c.fail(d);
                I64(-1)
            }
        }
    })
}

/// What a native that reads what a socket is answers, under `label` and of `socket`.
type Described = fn(&TcpHost, &str, i64, ply_core::Span) -> Result<Vec<u8>, Diagnostic>;

/// What `read` answers of a socket, a datagram one where `datagram` says it must be: what a socket
/// is, read as it stands, which reading again changes nothing of.
fn described(
    ctx: *mut Ctx,
    native: &'static str,
    label_at: Addr,
    label_n: I64,
    socket: I64,
    datagram: bool,
    read: Described,
) -> TakenBytes {
    asked(ctx, native, READS, TakenBytes(0), |c: &mut Ctx| {
        let Some((net, label)) = labelled(c, unsafe { lent(label_at, label_n) }) else {
            return TakenBytes(0);
        };
        let span = c.site();
        let answer = held_datagram(&net, label, socket, datagram, span)
            .and_then(|()| read(&net, label, socket.0, span));
        given(c, answer)
    })
}

/// The address this end of a socket is bound to, found.
pub unsafe extern "C" fn rt_net_local_address(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    socket: I64,
) -> TakenBytes {
    described(
        ctx,
        "net_local_address",
        label_at,
        label_n,
        socket,
        false,
        TcpHost::local_address,
    )
}

/// The address of a connection's far end, found.
pub unsafe extern "C" fn rt_net_peer_address(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    conn: I64,
) -> TakenBytes {
    described(
        ctx,
        "net_peer_address",
        label_at,
        label_n,
        conn,
        false,
        TcpHost::peer_address,
    )
}

/// Who holds the far end of a Unix socket, found.
pub unsafe extern "C" fn rt_net_peer_credentials(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    conn: I64,
) -> TakenBytes {
    described(
        ctx,
        "net_peer_credentials",
        label_at,
        label_n,
        conn,
        false,
        TcpHost::peer_credentials,
    )
}

/// The far end's certificate on a secured connection, found.
pub unsafe extern "C" fn rt_net_peer_certificate(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    conn: I64,
) -> TakenBytes {
    described(
        ctx,
        "net_peer_certificate",
        label_at,
        label_n,
        conn,
        false,
        TcpHost::peer_certificate,
    )
}

/// The application protocol a secured connection's handshake agreed, found.
pub unsafe extern "C" fn rt_net_protocol(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    conn: I64,
) -> TakenBytes {
    described(
        ctx,
        "net_protocol",
        label_at,
        label_n,
        conn,
        false,
        TcpHost::protocol,
    )
}

/// Each option a socket has, as parts.
pub unsafe extern "C" fn rt_net_options(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    socket: I64,
) -> TakenBytes {
    described(
        ctx,
        "net_options",
        label_at,
        label_n,
        socket,
        false,
        TcpHost::options,
    )
}

/// A datagram socket bound at the address at `to_at`: its handle, or why not.
pub unsafe extern "C" fn rt_udp_bind(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    to_at: Addr,
    to_n: I64,
    options_at: Addr,
    options_n: I64,
) -> TakenBytes {
    asked(ctx, "udp_bind", WRITES, TakenBytes(0), |c: &mut Ctx| {
        let Some((net, label)) = labelled(c, unsafe { lent(label_at, label_n) }) else {
            return TakenBytes(0);
        };
        let to = Endpoint::read(unsafe { lent(to_at, to_n) });
        let options = SocketOption::list(unsafe { lent(options_at, options_n) });
        let (Some(to), Some(options)) = (
            handed(c, "udp_bind", "an address", to),
            handed(c, "udp_bind", "socket options", options),
        ) else {
            return TakenBytes(0);
        };
        c.given(ply_host::udp::bind(&net, label, &to, &options))
    })
}

/// A datagram socket's far end fixed at the address at `to_at`: nothing, or why not.
pub unsafe extern "C" fn rt_udp_connect(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    socket: I64,
    to_at: Addr,
    to_n: I64,
) -> TakenBytes {
    asked(ctx, "udp_connect", WRITES, TakenBytes(0), |c: &mut Ctx| {
        let Some((net, label)) = labelled(c, unsafe { lent(label_at, label_n) }) else {
            return TakenBytes(0);
        };
        let to = Endpoint::read(unsafe { lent(to_at, to_n) });
        let Some(to) = handed(c, "udp_connect", "an address", to) else {
            return TakenBytes(0);
        };
        let answer = ply_host::udp::connect(&net, label, socket.0, &to, c.site());
        given(c, answer)
    })
}

/// The ticket of the payload at `payload_at` sent as one datagram to the address at `to_at`: its
/// length, or why not.
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_udp_send_to(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    socket: I64,
    to_at: Addr,
    to_n: I64,
    payload_at: Addr,
    payload_n: I64,
    timeout_ms: I64,
) -> I64 {
    asked(ctx, "udp_send_to", WRITES, I64(0), |c: &mut Ctx| {
        let Some((net, label)) = labelled(c, unsafe { lent(label_at, label_n) }) else {
            return I64(0);
        };
        let to = Endpoint::read(unsafe { lent(to_at, to_n) });
        let (Some(to), Some(timeout)) = (
            handed(c, "udp_send_to", "an address", to),
            handed(c, "udp_send_to", "a deadline", deadline(timeout_ms)),
        ) else {
            return I64(0);
        };
        let payload = unsafe { lent(payload_at, payload_n) }.to_vec();
        let job = ply_host::udp::send_to(&net, label, socket.0, to, payload, timeout, c.site());
        waited(c, "udp_send_to", "`udp.send_to`", job)
    })
}

/// The ticket of the payload at `payload_at` sent as one datagram to the far end `connect` fixed:
/// its length, or why not.
pub unsafe extern "C" fn rt_udp_send(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    socket: I64,
    payload_at: Addr,
    payload_n: I64,
    timeout_ms: I64,
) -> I64 {
    asked(ctx, "udp_send", WRITES, I64(0), |c: &mut Ctx| {
        let Some((net, label)) = labelled(c, unsafe { lent(label_at, label_n) }) else {
            return I64(0);
        };
        let Some(timeout) = handed(c, "udp_send", "a deadline", deadline(timeout_ms)) else {
            return I64(0);
        };
        let payload = unsafe { lent(payload_at, payload_n) }.to_vec();
        let job = ply_host::udp::send(&net, label, socket.0, payload, timeout, c.site());
        waited(c, "udp_send", "`udp.send`", job)
    })
}

/// The ticket of the next datagram, at most `max` bytes of it: the datagram, or why not.
pub unsafe extern "C" fn rt_udp_recv_from(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    socket: I64,
    max: I64,
    timeout_ms: I64,
) -> I64 {
    asked(ctx, "udp_recv_from", WRITES, I64(0), |c: &mut Ctx| {
        let Some((net, label)) = labelled(c, unsafe { lent(label_at, label_n) }) else {
            return I64(0);
        };
        let (Some(max), Some(timeout)) = (
            handed(c, "udp_recv_from", "a byte count", count(max)),
            handed(c, "udp_recv_from", "a deadline", deadline(timeout_ms)),
        ) else {
            return I64(0);
        };
        let job = ply_host::udp::recv_from(&net, label, socket.0, max, timeout, c.site());
        waited(c, "udp_recv_from", "`udp.recv_from`", job)
    })
}

/// Sets the option at `option_at` on a datagram socket: nothing, or why not.
pub unsafe extern "C" fn rt_udp_set_option(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    socket: I64,
    option_at: Addr,
    option_n: I64,
) -> TakenBytes {
    asked(
        ctx,
        "udp_set_option",
        WRITES,
        TakenBytes(0),
        |c: &mut Ctx| {
            set_option(
                c,
                "udp_set_option",
                label_at,
                label_n,
                socket,
                option_at,
                option_n,
                true,
            )
        },
    )
}

/// Closes a datagram socket.
pub unsafe extern "C" fn rt_udp_close(ctx: *mut Ctx, label_at: Addr, label_n: I64, socket: I64) {
    asked(ctx, "udp_close", WRITES, (), |c: &mut Ctx| {
        let Some((net, label)) = labelled(c, unsafe { lent(label_at, label_n) }) else {
            return;
        };
        let span = c.site();
        let closed = held_datagram(&net, label, socket, true, span)
            .and_then(|()| net.close(label, socket.0, span));
        done(c, closed);
    });
}

/// The address a datagram socket is bound to, found.
pub unsafe extern "C" fn rt_udp_local_address(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    socket: I64,
) -> TakenBytes {
    described(
        ctx,
        "udp_local_address",
        label_at,
        label_n,
        socket,
        true,
        TcpHost::local_address,
    )
}

/// Each option a datagram socket has, as parts.
pub unsafe extern "C" fn rt_udp_options(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    socket: I64,
) -> TakenBytes {
    described(
        ctx,
        "udp_options",
        label_at,
        label_n,
        socket,
        true,
        TcpHost::options,
    )
}
