//! The socket table over TCP and Unix sockets, plaintext or TLS, and datagram sockets.

use super::wire::{self, Credentials, Endpoint, Probing, Refusal, SocketOption};
use super::{
    Handles, Upgrade, a_datagram_socket, bind, not_a_datagram_socket, not_a_listener, not_a_stream,
    not_upgradable, unknown_handle,
};
use crate::fs;
use crate::jobs::Job;
use crate::native::{found, int};
use crate::tls::{self, Credentials as TlsCredentials, Handshakes};
use ply_core::{Diagnostic, Resource, Span, Symbol, codes};
use rustls::pki_types::ServerName;
use rustls::server::ServerConfig;
use socket2::{
    Domain, InterfaceIndexOrAddress, Protocol, SockAddr, SockRef, Socket as RawSocket, Type,
};
use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream, UdpSocket};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use zeroize::Zeroizing;

/// How many connections a listener bound here queues before the kernel refuses more.
const BACKLOG: i32 = 128;

/// A plaintext byte stream.
#[derive(Clone)]
enum Plain {
    Tcp(Arc<TcpStream>),
    #[cfg(unix)]
    Unix(Arc<UnixStream>),
}

impl Plain {
    fn read(&self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Plain::Tcp(s) => (&**s).read(buffer),
            #[cfg(unix)]
            Plain::Unix(s) => (&**s).read(buffer),
        }
    }

    fn write(&self, payload: &[u8]) -> io::Result<usize> {
        match self {
            Plain::Tcp(s) => (&**s).write(payload),
            #[cfg(unix)]
            Plain::Unix(s) => (&**s).write(payload),
        }
    }

    fn write_all(&self, payload: &[u8]) -> io::Result<()> {
        match self {
            Plain::Tcp(s) => (&**s).write_all(payload),
            #[cfg(unix)]
            Plain::Unix(s) => (&**s).write_all(payload),
        }
    }

    fn read_within(&self, timeout: Duration) {
        let _ = match self {
            Plain::Tcp(s) => s.set_read_timeout(Some(timeout)),
            #[cfg(unix)]
            Plain::Unix(s) => s.set_read_timeout(Some(timeout)),
        };
    }

    fn write_within(&self, timeout: Duration) {
        let _ = match self {
            Plain::Tcp(s) => s.set_write_timeout(Some(timeout)),
            #[cfg(unix)]
            Plain::Unix(s) => s.set_write_timeout(Some(timeout)),
        };
    }

    fn shutdown(&self, how: Shutdown) {
        let _ = match self {
            Plain::Tcp(s) => s.shutdown(how),
            #[cfg(unix)]
            Plain::Unix(s) => s.shutdown(how),
        };
    }

    /// The socket's options, whichever kind of socket it is.
    fn raw(&self) -> SockRef<'_> {
        match self {
            Plain::Tcp(s) => SockRef::from(&**s),
            #[cfg(unix)]
            Plain::Unix(s) => SockRef::from(&**s),
        }
    }
}

/// A socket that takes connections, and what each one it takes becomes.
#[derive(Clone)]
enum Listening {
    /// `None` for plaintext; otherwise the config every accepted connection is terminated with.
    Tcp(Arc<TcpListener>, Option<Arc<ServerConfig>>),
    /// With the path it bound, which its close removes.
    #[cfg(unix)]
    Unix(Arc<UnixListener>, Arc<PathBuf>),
}

impl Listening {
    /// The next connection, with the listener's transport: a path that dropped TLS would serve in
    /// the clear.
    fn accept(&self, handshakes: &Arc<Handshakes>) -> io::Result<Socket> {
        match self {
            Listening::Tcp(listener, config) => {
                let (stream, _) = listener.accept()?;
                let _ = stream.set_nodelay(true);
                let stream = Arc::new(stream);
                Ok(match config {
                    Some(config) => Socket::Tls(Arc::new(tls::Session::new(
                        Arc::clone(config),
                        stream,
                        Arc::clone(handshakes),
                    ))),
                    None => Socket::Stream(Plain::Tcp(stream)),
                })
            }
            #[cfg(unix)]
            Listening::Unix(listener, _) => {
                let (stream, _) = listener.accept()?;
                Ok(Socket::Stream(Plain::Unix(Arc::new(stream))))
            }
        }
    }
}

enum Socket {
    Listener(Listening),
    Stream(Plain),
    Tls(Arc<tls::Session>),
    /// `std.udp`'s, which shares this table so one handle space and one label rule serve both.
    Datagram(Arc<UdpSocket>),
    /// A listener the drain closed.
    Finished,
}

enum Connection {
    Plain(Plain),
    Tls(Arc<tls::Session>),
}

/// The socket table and handle allocator, together because a completing `accept` inserts into both.
struct Sockets {
    open: Mutex<BTreeMap<i64, Socket>>,
    handles: Handles,
}

impl Sockets {
    fn insert(&self, label: Option<&str>, sock: Socket) -> i64 {
        let handle = self.handles.open(label);
        lock(&self.open).insert(handle, sock);
        handle
    }

    fn listener(&self, handle: i64, label: &str, span: Span) -> Result<Listening, Diagnostic> {
        self.handles.check(handle, label, span)?;
        match lock(&self.open).get(&handle) {
            Some(Socket::Listener(listening)) => Ok(listening.clone()),
            Some(Socket::Stream(_) | Socket::Tls(_)) => Err(not_a_listener(handle, span)),
            Some(Socket::Datagram(_)) => Err(a_datagram_socket(handle, span)),
            // Unreachable: `accept` checks the stop flag, set before any listener is swapped.
            Some(Socket::Finished) => Err(not_a_listener(handle, span)),
            None => Err(unknown_handle(handle, span)),
        }
    }

    fn datagram(&self, handle: i64, label: &str, span: Span) -> Result<Arc<UdpSocket>, Diagnostic> {
        self.handles.check(handle, label, span)?;
        match lock(&self.open).get(&handle) {
            Some(Socket::Datagram(socket)) => Ok(Arc::clone(socket)),
            Some(_) => Err(not_a_datagram_socket(handle, span)),
            None => Err(unknown_handle(handle, span)),
        }
    }

    fn stream(&self, handle: i64, label: &str, span: Span) -> Result<Connection, Diagnostic> {
        self.handles.check(handle, label, span)?;
        match lock(&self.open).get(&handle) {
            Some(Socket::Stream(s)) => Ok(Connection::Plain(s.clone())),
            Some(Socket::Tls(s)) => Ok(Connection::Tls(Arc::clone(s))),
            Some(Socket::Listener(..) | Socket::Finished) => Err(not_a_stream(handle, span)),
            Some(Socket::Datagram(_)) => Err(a_datagram_socket(handle, span)),
            None => Err(unknown_handle(handle, span)),
        }
    }

    /// The plaintext TCP connection an upgrade secures, with the session `secured` makes of it put
    /// in its place: every later operation on the handle goes through the session. A connection
    /// another operation still holds is refused, since that operation would read or write the
    /// plaintext under the session.
    fn upgraded(
        &self,
        what: &str,
        handle: i64,
        label: &str,
        span: Span,
        secured: impl FnOnce(Arc<TcpStream>) -> Arc<tls::Session>,
    ) -> Result<(Arc<TcpStream>, Arc<tls::Session>), Diagnostic> {
        self.handles.check(handle, label, span)?;
        let mut open = lock(&self.open);
        let stream = match open.get(&handle) {
            Some(Socket::Stream(Plain::Tcp(stream))) => {
                if Arc::strong_count(stream) > 1 {
                    return Err(not_upgradable(
                        what,
                        handle,
                        "another operation is still waiting on it",
                        span,
                    ));
                }
                Arc::clone(stream)
            }
            #[cfg(unix)]
            Some(Socket::Stream(Plain::Unix(_))) => {
                return Err(not_upgradable(what, handle, "it is a Unix socket", span));
            }
            Some(Socket::Tls(_)) => {
                return Err(not_upgradable(what, handle, "it is already TLS", span));
            }
            Some(Socket::Listener(..) | Socket::Finished) => {
                return Err(not_a_stream(handle, span));
            }
            Some(Socket::Datagram(_)) => return Err(a_datagram_socket(handle, span)),
            None => return Err(unknown_handle(handle, span)),
        };
        let session = secured(Arc::clone(&stream));
        open.insert(handle, Socket::Tls(Arc::clone(&session)));
        Ok((stream, session))
    }

    fn connections(&self) -> usize {
        lock(&self.open)
            .values()
            .filter(|s| matches!(s, Socket::Stream(_) | Socket::Tls(_)))
            .count()
    }
}

/// One of the parked `accept`s the drain wakes, counted for as long as its job is held.
struct Accepting(Arc<AtomicUsize>);

impl Accepting {
    fn counted(accepts: &Arc<AtomicUsize>) -> Accepting {
        accepts.fetch_add(1, Ordering::AcqRel);
        Accepting(Arc::clone(accepts))
    }
}

impl Drop for Accepting {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

pub struct TcpHost {
    sockets: Arc<Sockets>,
    /// What `net.listen_tls` resolves a credential name against.
    credentials: TlsCredentials,
    handshakes: Arc<Handshakes>,
    /// The roots a Unix socket's path is confined to.
    roots: Mutex<fs::Roots>,
    /// Set at the stop.
    stopping: Arc<AtomicBool>,
    /// `accept` operations parked on a pool thread.
    accepts: Arc<AtomicUsize>,
    /// Where the listeners the stop closed were bound.
    closed_at: Mutex<Vec<SocketAddr>>,
    /// The credential each connection's `start_tls` presents, where `Presenting` named one.
    presenting: Arc<Mutex<BTreeMap<i64, String>>>,
}

impl Default for TcpHost {
    fn default() -> TcpHost {
        TcpHost::new()
    }
}

impl TcpHost {
    pub fn new() -> TcpHost {
        TcpHost::with_credentials(TlsCredentials::empty())
    }

    pub fn with_credentials(credentials: TlsCredentials) -> TcpHost {
        TcpHost {
            sockets: Arc::new(Sockets {
                open: Mutex::new(BTreeMap::new()),
                handles: Handles::new(),
            }),
            credentials,
            handshakes: Arc::new(Handshakes::default()),
            roots: Mutex::new(fs::Roots::new()),
            stopping: Arc::new(AtomicBool::new(false)),
            accepts: Arc::new(AtomicUsize::new(0)),
            closed_at: Mutex::new(Vec::new()),
            presenting: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// The roots `--fs` bound, which is where a Unix socket's path may be.
    pub fn rooted(&self, roots: fs::Roots) {
        *lock(&self.roots) = roots;
    }

    pub fn credentials(&self) -> &TlsCredentials {
        &self.credentials
    }

    pub fn handshakes(&self) -> tls::HandshakeCounts {
        self.handshakes.snapshot()
    }

    /// The address a listening handle actually bound.
    pub fn local_addr(&self, handle: i64) -> Option<SocketAddr> {
        match lock(&self.sockets.open).get(&handle) {
            Some(Socket::Listener(Listening::Tcp(l, _))) => l.local_addr().ok(),
            _ => None,
        }
    }

    /// A datagram socket put in this host's table under `label`, answering its handle.
    pub(crate) fn open_datagram(&self, label: &str, socket: UdpSocket) -> i64 {
        self.sockets
            .insert(Some(label), Socket::Datagram(Arc::new(socket)))
    }

    /// The datagram socket a handle names under `label`.
    pub fn datagram(
        &self,
        handle: i64,
        label: &str,
        span: Span,
    ) -> Result<Arc<UdpSocket>, Diagnostic> {
        self.sockets.datagram(handle, label, span)
    }

    /// Where the Unix socket `path` under the root named `root` is, on this machine.
    fn unix_path(
        &self,
        what: &str,
        root: &str,
        path: &str,
        span: Span,
    ) -> Result<PathBuf, Diagnostic> {
        let roots = lock(&self.roots);
        let Some(base) = roots.get(&Resource::Named(Symbol::new(root))) else {
            return Err(Diagnostic::error(
                codes::FS_ROOT_UNBOUND,
                format!("{what} names the root `{root}`, and no root is bound to it"),
            )
            .primary(span, format!("`{root}` has no root"))
            .note(format!("bind one beside the run: `--fs {root}=<directory>`"))
            .note("a Unix socket is a path, and a path is reached only under a root the run was given"));
        };
        // A path that leads nowhere goes to the system as written, which answers that nothing is there.
        Ok(fs::confine(base, path, span)?.unwrap_or_else(|| base.join(path)))
    }

    /// The socket a handle names, for an operation that asks what it is.
    fn described<T>(
        &self,
        socket: i64,
        label: &str,
        span: Span,
        read: impl FnOnce(&Socket) -> T,
    ) -> Result<T, Diagnostic> {
        self.sockets.handles.check(socket, label, span)?;
        match lock(&self.sockets.open).get(&socket) {
            Some(sock) => Ok(read(sock)),
            None => Err(unknown_handle(socket, span)),
        }
    }

    /// A listener on `port` of the loopback address.
    pub fn listen(&self, label: &str, port: u16, span: Span) -> Result<i64, Diagnostic> {
        let listener = bind("`net.listen`", port, span)?;
        Ok(self.sockets.insert(
            Some(label),
            Socket::Listener(Listening::Tcp(Arc::new(listener), None)),
        ))
    }

    /// A listener on `port` of the loopback address that serves TLS with `credential`; one the
    /// run lacks is `E0429`.
    pub fn listen_tls(
        &self,
        label: &str,
        port: u16,
        credential: &str,
        span: Span,
    ) -> Result<i64, Diagnostic> {
        let config = self.credentials.resolve(credential, span)?;
        let listener = bind("`net.listen_tls`", port, span)?;
        Ok(self.sockets.insert(
            Some(label),
            Socket::Listener(Listening::Tcp(Arc::new(listener), Some(config))),
        ))
    }

    /// A listener at exactly `to`, with `options` set before the bind: its handle, or why not.
    pub fn listen_on(&self, label: &str, to: &Endpoint, options: &[SocketOption]) -> Vec<u8> {
        wire::answer(bound(to, options).map(|listener| {
            let handle = self.sockets.insert(
                Some(label),
                Socket::Listener(Listening::Tcp(Arc::new(listener), None)),
            );
            int(handle).to_vec()
        }))
    }

    /// A listener on the Unix socket at `path` under the root named `root`: its handle, or why
    /// not.
    #[cfg(unix)]
    pub fn listen_unix(
        &self,
        label: &str,
        root: &str,
        path: &str,
        span: Span,
    ) -> Result<Vec<u8>, Diagnostic> {
        let path = self.unix_path("`net.listen_unix`", root, path, span)?;
        Ok(wire::answer(match UnixListener::bind(&path) {
            Ok(listener) => {
                let handle = self.sockets.insert(
                    Some(label),
                    Socket::Listener(Listening::Unix(Arc::new(listener), Arc::new(path))),
                );
                Ok(int(handle).to_vec())
            }
            Err(e) => Err(Refusal::of(&e)),
        }))
    }

    #[cfg(not(unix))]
    pub fn listen_unix(
        &self,
        _label: &str,
        _root: &str,
        _path: &str,
        _span: Span,
    ) -> Result<Vec<u8>, Diagnostic> {
        Ok(wire::answer(Err(no_unix_sockets())))
    }

    /// A connection to `host`, its resolution and each address it resolves to tried in turn on the
    /// job's thread: its handle found, or none where the host was not reached before the deadline.
    pub fn connect(&self, label: &str, host: String, port: u16, timeout: Duration) -> Job {
        let sockets = Arc::clone(&self.sockets);
        let label = label.to_string();
        Box::new(move || {
            Ok(found(reach(&host, port, timeout).map(|stream| {
                int(sockets.insert(Some(&label), Socket::Stream(Plain::Tcp(Arc::new(stream)))))
            })))
        })
    }

    /// `connect`, then TLS over it verifying `host`; the handshake is left to the first read or
    /// write.
    pub fn connect_tls(&self, label: &str, host: String, port: u16, timeout: Duration) -> Job {
        let sockets = Arc::clone(&self.sockets);
        let handshakes = Arc::clone(&self.handshakes);
        let config = self.credentials.client();
        let label = label.to_string();
        Box::new(move || {
            // A host that is not a DNS name or an IP address is one no server can be verified as.
            let Ok(name) = ServerName::try_from(host.clone()) else {
                return Ok(found(None::<[u8; 8]>));
            };
            Ok(found(reach(&host, port, timeout).map(|stream| {
                let session = tls::Session::connect(config, name, Arc::new(stream), handshakes);
                int(sockets.insert(Some(&label), Socket::Tls(Arc::new(session))))
            })))
        })
    }

    /// A connection to exactly `to`, no name looked up, with `options` set before it connects:
    /// its handle, or why not.
    pub fn connect_to(
        &self,
        label: &str,
        to: Endpoint,
        options: Vec<SocketOption>,
        timeout: Duration,
    ) -> Job {
        let sockets = Arc::clone(&self.sockets);
        let presenting = Arc::clone(&self.presenting);
        let label = label.to_string();
        Box::new(move || {
            Ok(wire::answer(reach_to(&to, &options, timeout).map(
                |stream| {
                    let handle =
                        sockets.insert(Some(&label), Socket::Stream(Plain::Tcp(Arc::new(stream))));
                    if let Some(name) = presented(&options) {
                        lock(&presenting).insert(handle, name);
                    }
                    int(handle).to_vec()
                },
            )))
        })
    }

    /// A connection to the Unix socket at `path` under the root named `root`: its handle, or why
    /// not.
    #[cfg(unix)]
    pub fn connect_unix(
        &self,
        label: &str,
        root: &str,
        path: &str,
        timeout: Duration,
        span: Span,
    ) -> Result<Job, Diagnostic> {
        let path = self.unix_path("`net.connect_unix`", root, path, span)?;
        let sockets = Arc::clone(&self.sockets);
        let label = label.to_string();
        Ok(Box::new(move || {
            Ok(wire::answer(reach_unix(&path, timeout).map(|stream| {
                let handle =
                    sockets.insert(Some(&label), Socket::Stream(Plain::Unix(Arc::new(stream))));
                int(handle).to_vec()
            })))
        }))
    }

    #[cfg(not(unix))]
    pub fn connect_unix(
        &self,
        _label: &str,
        _root: &str,
        _path: &str,
        _timeout: Duration,
        _span: Span,
    ) -> Result<Job, Diagnostic> {
        Ok(Box::new(|| Ok(wire::answer(Err(no_unix_sockets())))))
    }

    /// The TLS handshake, completed now rather than when a request needs it: the microseconds it
    /// took found, or none for a connection with no handshake to complete. No handshake happens
    /// on its own: a session handshakes on its first read or write, so a peer that connects and
    /// says nothing costs nothing.
    pub fn handshake(&self, label: &str, conn: i64, span: Span) -> Result<Job, Diagnostic> {
        let session = match self.sockets.stream(conn, label, span)? {
            Connection::Tls(session) => Some(session),
            Connection::Plain(_) => None,
        };
        Ok(Box::new(move || {
            Ok(found(
                session
                    .and_then(|session| session.handshake())
                    .map(|us| int(us as i64)),
            ))
        }))
    }

    /// An open plaintext connection secured as the client's end, verifying the server as `name`:
    /// the protocol agreed found, or why not. The session takes the handle before the handshake
    /// starts, so nothing reaches the plaintext while it runs. Plaintext the caller holds unread,
    /// or the socket does, is a refusal: a TLS server never speaks first, so it was sent behind
    /// the go-ahead to be read as if secured.
    pub fn start_tls(
        &self,
        label: &str,
        conn: i64,
        name: &str,
        upgrade: Upgrade,
        span: Span,
    ) -> Result<Job, Diagnostic> {
        let server = ServerName::try_from(name.to_string());
        let early = if upgrade.unread > 0 {
            Some((tls::REASON_INJECTED, Refusal::Injected))
        } else if server.is_err() {
            Some((
                tls::REASON_NAME,
                Refusal::Handshake(tls::REASON_NAME.to_string()),
            ))
        } else {
            None
        };
        let reason = early.as_ref().map(|(reason, _)| *reason);
        let named = lock(&self.presenting).get(&conn).cloned();
        let client = match named {
            Some(name) => self.credentials.presenting(&name, span)?,
            None => self.credentials.client(),
        };
        let config = tls::client_offering(&client, &upgrade.alpn);
        let handshakes = Arc::clone(&self.handshakes);
        let (stream, session) =
            self.sockets
                .upgraded("`net.start_tls`", conn, label, span, move |stream| {
                    Arc::new(match (reason, server) {
                        (None, Ok(server)) => {
                            tls::Session::connect(config, server, stream, handshakes)
                        }
                        (reason, _) => tls::Session::refused(
                            stream,
                            handshakes,
                            reason.unwrap_or(tls::REASON_NAME),
                        ),
                    })
                })?;
        if let Some((_, refusal)) = early {
            return Ok(Box::new(move || Ok(wire::answer(Err(refusal)))));
        }
        Ok(Box::new(move || {
            if tls::plaintext_waiting(&stream) {
                session.refuse(tls::REASON_INJECTED);
                return Ok(wire::answer(Err(Refusal::Injected)));
            }
            Ok(settled(&session, upgrade.timeout))
        }))
    }

    /// The server's end of `start_tls`, with the credential named `credential`. Bytes the socket
    /// already holds are the client's handshake, which it may send as soon as it has the go-ahead,
    /// so only what the caller read past the request to upgrade is refused.
    pub fn serve_tls(
        &self,
        label: &str,
        conn: i64,
        credential: &str,
        upgrade: Upgrade,
        span: Span,
    ) -> Result<Job, Diagnostic> {
        let credential = self.credentials.resolve(credential, span)?;
        let config = tls::server_accepting(&credential, &upgrade.alpn);
        let handshakes = Arc::clone(&self.handshakes);
        let injected = upgrade.unread > 0;
        let (_, session) =
            self.sockets
                .upgraded("`net.serve_tls`", conn, label, span, move |stream| {
                    Arc::new(if injected {
                        tls::Session::refused(stream, handshakes, tls::REASON_INJECTED)
                    } else {
                        tls::Session::new(config, stream, handshakes)
                    })
                })?;
        if injected {
            return Ok(Box::new(|| Ok(wire::answer(Err(Refusal::Injected)))));
        }
        Ok(Box::new(move || Ok(settled(&session, upgrade.timeout))))
    }

    /// The next connection a listener takes, as its handle; `0` once the run has stopped
    /// accepting.
    pub fn accept(&self, label: &str, listener: i64, span: Span) -> Result<Job, Diagnostic> {
        // Before the lookup, because `stop_accepting` has already swapped the listener out.
        if self.stopping.load(Ordering::Acquire) {
            self.sockets.handles.check(listener, label, span)?;
            return Ok(Box::new(|| Ok(int(0).to_vec())));
        }
        let listening = self.sockets.listener(listener, label, span)?;
        let sockets = Arc::clone(&self.sockets);
        let handshakes = Arc::clone(&self.handshakes);
        let stopping = Arc::clone(&self.stopping);
        let parked = Accepting::counted(&self.accepts);
        Ok(Box::new(move || {
            let handle = accepted(&listening, &sockets, &handshakes, &stopping);
            drop(parked);
            Ok(int(handle).to_vec())
        }))
    }

    /// At most `max` bytes found, as soon as any arrive: none is the deadline, and an empty answer
    /// the peer done sending.
    pub fn recv(
        &self,
        label: &str,
        conn: i64,
        max: usize,
        timeout: Duration,
        span: Span,
    ) -> Result<Job, Diagnostic> {
        let conn = self.sockets.stream(conn, label, span)?;
        Ok(Box::new(move || match conn {
            Connection::Plain(stream) => {
                // This job owns the socket for its duration, so setting its timeout is safe.
                stream.read_within(timeout);
                // Read behind the byte that says it was found, so the bytes are not copied again.
                let mut buffer = vec![0u8; max + 1];
                buffer[0] = 1;
                // One `read`: a loop would hide short reads and closes from the program.
                match stream.read(&mut buffer[1..]) {
                    Ok(n) => {
                        buffer.truncate(n + 1);
                        Ok(buffer)
                    }
                    Err(e) if expired(&e) => Ok(found(None::<Vec<u8>>)),
                    Err(e) if peer_gone(&e) => Ok(found(Some(b""))),
                    Err(e) => Err(e.to_string()),
                }
            }
            // Never a failure: any TLS failure is "the peer went away", so the accept loop survives.
            Connection::Tls(session) => {
                session.deadline(timeout);
                Ok(found(session.read(max)))
            }
        }))
    }

    /// Writes some prefix of `payload`: how many bytes it took found, none for the deadline and
    /// `0` for the peer gone.
    pub fn send(
        &self,
        label: &str,
        conn: i64,
        payload: Vec<u8>,
        timeout: Duration,
        span: Span,
    ) -> Result<Job, Diagnostic> {
        let conn = self.sockets.stream(conn, label, span)?;
        Ok(Box::new(move || match conn {
            Connection::Plain(stream) => {
                stream.write_within(timeout);
                // One `write`, which may be short under backpressure; `std.net.send_all` loops.
                match stream.write(&payload) {
                    Ok(n) => Ok(found(Some(int(n as i64)))),
                    Err(e) if expired(&e) => Ok(found(None::<[u8; 8]>)),
                    Err(e) if peer_gone(&e) => Ok(found(Some(int(0)))),
                    Err(e) => Err(e.to_string()),
                }
            }
            Connection::Tls(session) => {
                session.deadline(timeout);
                Ok(found(Some(int(session.write(&payload) as i64))))
            }
        }))
    }

    /// Every byte of `payload`, which holds a credential: byte 1 once the peer took them all, byte
    /// 0 for a deadline or a peer gone before it did. The job's copy is wiped when it is dropped.
    pub fn send_secret(
        &self,
        label: &str,
        conn: i64,
        payload: &[u8],
        timeout: Duration,
        span: Span,
    ) -> Result<Job, Diagnostic> {
        let conn = self.sockets.stream(conn, label, span)?;
        let payload = Zeroizing::new(payload.to_vec());
        Ok(Box::new(move || {
            let went = match conn {
                Connection::Plain(stream) => {
                    stream.write_within(timeout);
                    stream.write_all(&payload).is_ok()
                }
                Connection::Tls(session) => {
                    session.deadline(timeout);
                    session.write(&payload) == payload.len()
                }
            };
            Ok(vec![u8::from(went)])
        }))
    }

    /// Ends this end's sending and leaves its reading open.
    pub fn close_write(&self, label: &str, conn: i64, span: Span) -> Result<(), Diagnostic> {
        match self.sockets.stream(conn, label, span)? {
            Connection::Plain(stream) => stream.shutdown(Shutdown::Write),
            Connection::Tls(session) => session.close_write(),
        }
        Ok(())
    }

    /// Closes a socket of any kind; its handle names nothing afterwards.
    pub fn close(&self, label: &str, socket: i64, span: Span) -> Result<(), Diagnostic> {
        self.sockets.handles.check(socket, label, span)?;
        let sock = lock(&self.sockets.open).remove(&socket);
        self.sockets.handles.close(socket);
        lock(&self.presenting).remove(&socket);
        match sock {
            // Shut down, not just dropped: a `recv` parked on another `Arc` returns only then.
            Some(Socket::Stream(s)) => s.shutdown(Shutdown::Both),
            // `close_notify` first, so the peer sees a clean end rather than a truncation.
            Some(Socket::Tls(s)) => s.close(),
            // The file is the listener's, so it goes with it; a path left behind refuses the
            // next bind.
            #[cfg(unix)]
            Some(Socket::Listener(Listening::Unix(_, path))) => {
                let _ = std::fs::remove_file(&*path);
            }
            Some(Socket::Listener(Listening::Tcp(..)) | Socket::Datagram(_) | Socket::Finished) => {
            }
            None => return Err(unknown_handle(socket, span)),
        }
        Ok(())
    }

    /// Sets one option of a socket of any kind: nothing, or why not.
    pub fn set_option(
        &self,
        label: &str,
        socket: i64,
        option: SocketOption,
        span: Span,
    ) -> Result<Vec<u8>, Diagnostic> {
        let set = self.described(socket, label, span, |sock| match raw_of(sock) {
            Some(raw) => apply(&raw, &option).map_err(|e| Refusal::of(&e)),
            None => Err(Refusal::Reset),
        })?;
        if let (Ok(()), SocketOption::Presenting(name)) = (&set, &option) {
            lock(&self.presenting).insert(socket, name.clone());
        }
        Ok(wire::answer(set.map(|()| Vec::new())))
    }

    /// The port this end of a socket is bound to; `None` for a listener the drain has closed.
    pub fn local_port(
        &self,
        label: &str,
        socket: i64,
        span: Span,
    ) -> Result<Option<u16>, Diagnostic> {
        let address = self.described(socket, label, span, local_of)?;
        Ok(address.map(|a| a.port()))
    }

    /// The address this end of a socket is bound to, found; none for one with no IP address.
    pub fn local_address(
        &self,
        label: &str,
        socket: i64,
        span: Span,
    ) -> Result<Vec<u8>, Diagnostic> {
        let address = self.described(socket, label, span, local_of)?;
        Ok(found(address.map(|a| Endpoint::of(a).written())))
    }

    /// The address of a connection's far end, found; none for a listener and for a Unix socket.
    pub fn peer_address(&self, label: &str, conn: i64, span: Span) -> Result<Vec<u8>, Diagnostic> {
        let address = self.described(conn, label, span, |sock| match sock {
            Socket::Stream(Plain::Tcp(s)) => s.peer_addr().ok(),
            Socket::Tls(s) => s.peer_addr().ok(),
            _ => None,
        })?;
        Ok(found(address.map(|a| Endpoint::of(a).written())))
    }

    /// Who holds the far end of a Unix socket, found; none for any other.
    pub fn peer_credentials(
        &self,
        label: &str,
        conn: i64,
        span: Span,
    ) -> Result<Vec<u8>, Diagnostic> {
        let credentials = self.described(conn, label, span, |sock| match sock {
            #[cfg(unix)]
            Socket::Stream(Plain::Unix(s)) => credentials_of(s),
            _ => None,
        })?;
        Ok(found(credentials.map(|c| c.written())))
    }

    /// The far end's certificate on a secured connection once its handshake completed, found: the
    /// server's to a client, the client's to a server where it presented one; none for any other.
    pub fn peer_certificate(
        &self,
        label: &str,
        conn: i64,
        span: Span,
    ) -> Result<Vec<u8>, Diagnostic> {
        let leaf = self.described(conn, label, span, |sock| match sock {
            Socket::Tls(s) => s.peer_certificate(),
            _ => None,
        })?;
        Ok(found(leaf))
    }

    /// The application protocol a secured connection's handshake agreed once it completed, found;
    /// none for any other.
    pub fn protocol(&self, label: &str, conn: i64, span: Span) -> Result<Vec<u8>, Diagnostic> {
        let agreed = self.described(conn, label, span, |sock| match sock {
            Socket::Tls(s) => s.protocol(),
            _ => None,
        })?;
        Ok(found(agreed))
    }

    /// Each option the socket has, as it stands, as parts.
    pub fn options(&self, label: &str, socket: i64, span: Span) -> Result<Vec<u8>, Diagnostic> {
        let options = self.described(socket, label, span, |sock| {
            raw_of(sock).map_or_else(Vec::new, |raw| standing(&raw))
        })?;
        Ok(SocketOption::listed(&options))
    }
}

/// The credential `options` name for a later `start_tls` to present, the last where several do.
fn presented(options: &[SocketOption]) -> Option<String> {
    options.iter().rev().find_map(|o| match o {
        SocketOption::Presenting(name) => Some(name.clone()),
        _ => None,
    })
}

/// The handshake of an upgrade, completed within its deadline: the protocol agreed found, or why
/// not.
fn settled(session: &tls::Session, timeout: Duration) -> Vec<u8> {
    let outcome = session.settle_within(timeout).map_err(|why| match why {
        tls::Unsettled::Expired => Refusal::TimedOut,
        tls::Unsettled::Refused(tls::REASON_CERTIFICATE) => Refusal::Untrusted,
        tls::Unsettled::Refused(tls::REASON_GONE) => Refusal::Reset,
        tls::Unsettled::Refused(reason) => Refusal::Handshake(reason.to_string()),
    });
    wire::answer(outcome.map(found))
}

/// The next connection a listener takes, as its handle; `0` once the run has stopped accepting or
/// the listener failed. A peer that aborts between the SYN and the accept is passed over, up to
/// [`ACCEPT_RETRIES`] of them.
fn accepted(
    listening: &Listening,
    sockets: &Sockets,
    handshakes: &Arc<Handshakes>,
    stopping: &AtomicBool,
) -> i64 {
    for _ in 0..=ACCEPT_RETRIES {
        if stopping.load(Ordering::Acquire) {
            return 0;
        }
        match listening.accept(handshakes) {
            // Taken as the run stopped accepting: the drain's wake dial, or a client racing it.
            Ok(_) if stopping.load(Ordering::Acquire) => return 0,
            // No label: the connection takes whichever one the program first uses it under.
            Ok(sock) => return sockets.insert(None, sock),
            Err(e) if transient(&e) => continue,
            // `0` is never a live handle: handles ascend from 1 and are never reused.
            Err(_) => return 0,
        }
    }
    0
}

impl crate::signal::Accepting for TcpHost {
    fn stop_accepting(&self) -> usize {
        // Flag before swap, or `accept` could find its listener gone and raise `E0502`.
        self.stopping.store(true, Ordering::Release);
        let mut open = lock(&self.sockets.open);
        let listeners: Vec<i64> = open
            .iter()
            .filter(|(_, s)| matches!(s, Socket::Listener(..)))
            .map(|(handle, _)| *handle)
            .collect();
        let mut closed = Vec::new();
        for handle in &listeners {
            match open.insert(*handle, Socket::Finished) {
                Some(Socket::Listener(Listening::Tcp(l, _))) => {
                    if let Ok(address) = l.local_addr() {
                        closed.push(address);
                    }
                }
                // An `accept` parked on it returns for a connection, so it is given one, and the
                // file goes as a close would take it.
                #[cfg(unix)]
                Some(Socket::Listener(Listening::Unix(_, path))) => {
                    drop(UnixStream::connect(&*path));
                    let _ = std::fs::remove_file(&*path);
                }
                _ => {}
            }
        }
        *lock(&self.closed_at) = closed;
        // The fd closes when the parked `accept` job drops its `Arc`, shortly after this returns.
        listeners.len()
    }

    fn listening_at(&self) -> Vec<SocketAddr> {
        let mut addresses: Vec<SocketAddr> = lock(&self.sockets.open)
            .values()
            .filter_map(|s| match s {
                Socket::Listener(Listening::Tcp(l, _)) => l.local_addr().ok(),
                _ => None,
            })
            .collect();
        addresses.extend(lock(&self.closed_at).iter().copied());
        addresses.sort();
        addresses.dedup();
        addresses
    }

    fn connections_in_flight(&self) -> usize {
        self.sockets.connections()
    }

    fn accepts_in_flight(&self) -> usize {
        self.accepts.load(Ordering::Acquire)
    }
}

fn expired(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
    )
}

/// An ordinary outcome, not the program's error: end of stream for a read, `0` for a write.
fn peer_gone(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::NotConnected
            | io::ErrorKind::UnexpectedEof
    )
}

fn transient(e: &io::Error) -> bool {
    expired(e) || matches!(e.kind(), io::ErrorKind::ConnectionAborted)
}

/// How many peers may abort between the SYN and the accept before the loop gives up on this call.
const ACCEPT_RETRIES: usize = 16;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Resolution and the connect on the caller's thread; each address the name resolves to gets the
/// deadline in turn. A name is ASCII, as it is to `dns.lookup`, whatever the system's resolver
/// would make of another.
fn reach(host: &str, port: u16, timeout: Duration) -> Option<TcpStream> {
    use std::net::ToSocketAddrs;
    if !host.is_ascii() {
        return None;
    }
    let addrs = (host, port).to_socket_addrs().ok()?;
    let stream = addrs
        .into_iter()
        .find_map(|addr| TcpStream::connect_timeout(&addr, timeout).ok())?;
    let _ = stream.set_nodelay(true);
    Some(stream)
}

/// A connection to one address, its options set before it connects: a buffer's size is part of
/// what the two ends agree on as they do.
fn reach_to(
    to: &Endpoint,
    options: &[SocketOption],
    timeout: Duration,
) -> Result<TcpStream, Refusal> {
    let address = to.socket_addr()?;
    let refusal = |e: io::Error| Refusal::of(&e);
    let socket = RawSocket::new(
        Domain::for_address(address),
        Type::STREAM,
        Some(Protocol::TCP),
    )
    .map_err(refusal)?;
    socket.set_tcp_nodelay(true).map_err(refusal)?;
    for option in options {
        apply(&socket, option).map_err(refusal)?;
    }
    socket
        .connect_timeout(&SockAddr::from(address), timeout)
        .map_err(refusal)?;
    Ok(socket.into())
}

#[cfg(unix)]
fn reach_unix(path: &Path, timeout: Duration) -> Result<UnixStream, Refusal> {
    let refusal = |e: io::Error| Refusal::of(&e);
    let address = SockAddr::unix(path).map_err(refusal)?;
    let socket = RawSocket::new(Domain::UNIX, Type::STREAM, None).map_err(refusal)?;
    // Linux polls a non-blocking connect to a full queue as ready; a blocking one waits this out.
    socket.set_write_timeout(Some(timeout)).map_err(refusal)?;
    socket.connect(&address).map_err(|e| {
        // Nothing at the path is nothing listening there.
        if e.kind() == io::ErrorKind::NotFound {
            Refusal::Refused
        } else {
            Refusal::of(&e)
        }
    })?;
    Ok(socket.into())
}

#[cfg(not(unix))]
fn no_unix_sockets() -> Refusal {
    Refusal::Other(0, "this platform has no Unix sockets".to_string())
}

/// A listener at `to` with `options` set before the bind, which is when the reuse options count.
/// An address is reused as `std`'s own listener reuses it unless an option says otherwise.
fn bound(to: &Endpoint, options: &[SocketOption]) -> Result<TcpListener, Refusal> {
    let address = to.socket_addr()?;
    let refusal = |e: io::Error| Refusal::of(&e);
    let socket = RawSocket::new(
        Domain::for_address(address),
        Type::STREAM,
        Some(Protocol::TCP),
    )
    .map_err(refusal)?;
    #[cfg(unix)]
    socket.set_reuse_address(true).map_err(refusal)?;
    for option in options {
        apply(&socket, option).map_err(refusal)?;
    }
    socket.bind(&SockAddr::from(address)).map_err(refusal)?;
    socket.listen(BACKLOG).map_err(refusal)?;
    Ok(socket.into())
}

/// The socket under a handle, for its options; a session's are its socket's.
fn raw_of(sock: &Socket) -> Option<SockRef<'_>> {
    match sock {
        Socket::Listener(Listening::Tcp(l, _)) => Some(SockRef::from(&**l)),
        #[cfg(unix)]
        Socket::Listener(Listening::Unix(l, _)) => Some(SockRef::from(&**l)),
        Socket::Stream(s) => Some(s.raw()),
        Socket::Tls(s) => Some(SockRef::from(s.socket())),
        Socket::Datagram(s) => Some(SockRef::from(&**s)),
        Socket::Finished => None,
    }
}

fn local_of(sock: &Socket) -> Option<SocketAddr> {
    match sock {
        Socket::Listener(Listening::Tcp(l, _)) => l.local_addr().ok(),
        Socket::Stream(Plain::Tcp(s)) => s.local_addr().ok(),
        Socket::Tls(s) => s.local_addr().ok(),
        Socket::Datagram(s) => s.local_addr().ok(),
        _ => None,
    }
}

pub(crate) fn apply(socket: &RawSocket, option: &SocketOption) -> io::Result<()> {
    match option {
        SocketOption::NoDelay(on) => socket.set_tcp_nodelay(*on),
        SocketOption::KeepAlive(None) => socket.set_keepalive(false),
        SocketOption::KeepAlive(Some(probing)) => socket.set_tcp_keepalive(&keepalive(probing)),
        SocketOption::ReuseAddress(on) => socket.set_reuse_address(*on),
        SocketOption::ReusePort(on) => reuse_port(socket, *on),
        SocketOption::SendBuffer(bytes) => socket.set_send_buffer_size(*bytes),
        SocketOption::ReceiveBuffer(bytes) => socket.set_recv_buffer_size(*bytes),
        SocketOption::Linger(d) => socket.set_linger(*d),
        SocketOption::Broadcast(on) => socket.set_broadcast(*on),
        SocketOption::MulticastLoop(on) => {
            if socket.local_addr()?.is_ipv6() {
                socket.set_multicast_loop_v6(*on)
            } else {
                socket.set_multicast_loop_v4(*on)
            }
        }
        SocketOption::JoinGroup(group, interface) => membership(socket, group, interface, true),
        SocketOption::LeaveGroup(group, interface) => membership(socket, group, interface, false),
        // A credential to present is the TLS session's, set at `start_tls`.
        SocketOption::Presenting(_) => Ok(()),
    }
}

/// Joins or leaves a multicast group on the interface named, or on the one the system picks.
fn membership(
    socket: &RawSocket,
    group: &IpAddr,
    interface: &Option<String>,
    join: bool,
) -> io::Result<()> {
    let index = match interface {
        Some(name) => wire::zone_index(name).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("`{name}` names no interface of this machine"),
            )
        })?,
        None => 0,
    };
    match (group, join) {
        (IpAddr::V4(group), true) => {
            socket.join_multicast_v4_n(group, &InterfaceIndexOrAddress::Index(index))
        }
        (IpAddr::V4(group), false) => {
            socket.leave_multicast_v4_n(group, &InterfaceIndexOrAddress::Index(index))
        }
        (IpAddr::V6(group), true) => socket.join_multicast_v6(group, index),
        (IpAddr::V6(group), false) => socket.leave_multicast_v6(group, index),
    }
}

/// The interval and the count are set where the platform has them and left to it elsewhere.
fn keepalive(probing: &Probing) -> socket2::TcpKeepalive {
    let params = socket2::TcpKeepalive::new().with_time(probing.idle);
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let params = {
        let params = match probing.interval {
            Some(interval) => params.with_interval(interval),
            None => params,
        };
        match probing.count {
            Some(count) => params.with_retries(count),
            None => params,
        }
    };
    params
}

#[cfg(unix)]
fn reuse_port(socket: &RawSocket, on: bool) -> io::Result<()> {
    socket.set_reuse_port(on)
}

#[cfg(not(unix))]
fn reuse_port(_: &RawSocket, _: bool) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "this platform has no port reuse",
    ))
}

/// Each option the socket answers for; one it does not have (a Unix socket's `NoDelay`) is left
/// out.
fn standing(socket: &RawSocket) -> Vec<SocketOption> {
    let mut options = Vec::new();
    if matches!(socket.r#type(), Ok(Type::DGRAM)) {
        if let Ok(on) = socket.broadcast() {
            options.push(SocketOption::Broadcast(on));
        }
        let looped = match socket.local_addr() {
            Ok(address) if address.is_ipv6() => socket.multicast_loop_v6(),
            _ => socket.multicast_loop_v4(),
        };
        if let Ok(on) = looped {
            options.push(SocketOption::MulticastLoop(on));
        }
    } else {
        if let Ok(on) = socket.tcp_nodelay() {
            options.push(SocketOption::NoDelay(on));
        }
        if let Ok(on) = socket.keepalive() {
            options.push(SocketOption::KeepAlive(on.then(|| probing(socket))));
        }
        if let Ok(linger) = socket.linger() {
            options.push(SocketOption::Linger(linger));
        }
    }
    if let Ok(on) = socket.reuse_address() {
        options.push(SocketOption::ReuseAddress(on));
    }
    #[cfg(unix)]
    if let Ok(on) = socket.reuse_port() {
        options.push(SocketOption::ReusePort(on));
    }
    if let Ok(bytes) = socket.send_buffer_size() {
        options.push(SocketOption::SendBuffer(bytes));
    }
    if let Ok(bytes) = socket.recv_buffer_size() {
        options.push(SocketOption::ReceiveBuffer(bytes));
    }
    options
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn probing(socket: &RawSocket) -> Probing {
    Probing {
        idle: socket.tcp_keepalive_time().unwrap_or_default(),
        interval: socket.tcp_keepalive_interval().ok(),
        count: socket.tcp_keepalive_retries().ok(),
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn probing(_: &RawSocket) -> Probing {
    Probing {
        idle: Duration::ZERO,
        interval: None,
        count: None,
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn credentials_of(stream: &UnixStream) -> Option<Credentials> {
    use std::os::fd::AsRawFd;
    let mut peer = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `peer` is plain data of the `len` bytes the call may write.
    let read = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut peer).cast(),
            &raw mut len,
        )
    };
    (read == 0).then(|| Credentials {
        user: i64::from(peer.uid),
        group: i64::from(peer.gid),
        process: Some(i64::from(peer.pid)),
    })
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn credentials_of(stream: &UnixStream) -> Option<Credentials> {
    use std::os::fd::AsRawFd;
    let (mut user, mut group): (libc::uid_t, libc::gid_t) = (0, 0);
    // SAFETY: both are plain integers the call writes.
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &raw mut user, &raw mut group) } != 0 {
        return None;
    }
    let mut process: libc::pid_t = 0;
    let mut len = size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: `process` is a plain integer of the `len` bytes the call may write.
    let read = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&raw mut process).cast(),
            &raw mut len,
        )
    };
    Some(Credentials {
        user: i64::from(user),
        group: i64::from(group),
        process: (read == 0).then(|| i64::from(process)),
    })
}

#[cfg(all(
    unix,
    not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))
))]
fn credentials_of(_: &UnixStream) -> Option<Credentials> {
    None
}
