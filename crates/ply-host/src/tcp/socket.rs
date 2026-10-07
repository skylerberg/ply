//! `net` over TCP and Unix sockets, plaintext or TLS.

use super::wire::{self, Credentials, Endpoint, Probing, Refusal, SocketOption};
use super::{
    Handles, Net, Op, Upgrade, a_datagram_socket, not_a_datagram_socket, not_a_listener,
    not_a_stream, not_upgradable, unknown_handle,
};
use crate::fs;
use crate::pool::{Inbox, JobOutput, Pool, Pooled};
use crate::tls::{self, Credentials as TlsCredentials, Handshakes};
use ply_eval::{
    Diagnostic, HostAnswer, HostRuntime, Pending, Resource, Span, Symbol, Value, codes,
};
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
    fn insert(&self, at: Option<&Resource>, sock: Socket) -> i64 {
        let handle = self.handles.open(at);
        lock(&self.open).insert(handle, sock);
        handle
    }

    fn listener(&self, handle: i64, at: &Resource, span: Span) -> Result<Listening, Diagnostic> {
        self.handles.check(handle, at, span)?;
        match lock(&self.open).get(&handle) {
            Some(Socket::Listener(listening)) => Ok(listening.clone()),
            Some(Socket::Stream(_) | Socket::Tls(_)) => Err(not_a_listener(handle, span)),
            Some(Socket::Datagram(_)) => Err(a_datagram_socket(handle, span)),
            // Unreachable: `accept` checks the stop flag, set before any listener is swapped.
            Some(Socket::Finished) => Err(not_a_listener(handle, span)),
            None => Err(unknown_handle(handle, span)),
        }
    }

    fn datagram(
        &self,
        handle: i64,
        at: &Resource,
        span: Span,
    ) -> Result<Arc<UdpSocket>, Diagnostic> {
        self.handles.check(handle, at, span)?;
        match lock(&self.open).get(&handle) {
            Some(Socket::Datagram(socket)) => Ok(Arc::clone(socket)),
            Some(_) => Err(not_a_datagram_socket(handle, span)),
            None => Err(unknown_handle(handle, span)),
        }
    }

    fn stream(&self, handle: i64, at: &Resource, span: Span) -> Result<Connection, Diagnostic> {
        self.handles.check(handle, at, span)?;
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
        op: Op,
        handle: i64,
        at: &Resource,
        span: Span,
        secured: impl FnOnce(Arc<TcpStream>) -> Arc<tls::Session>,
    ) -> Result<(Arc<TcpStream>, Arc<tls::Session>), Diagnostic> {
        self.handles.check(handle, at, span)?;
        let mut open = lock(&self.open);
        let stream = match open.get(&handle) {
            Some(Socket::Stream(Plain::Tcp(stream))) => {
                if Arc::strong_count(stream) > 1 {
                    return Err(not_upgradable(
                        op,
                        handle,
                        "another operation is still waiting on it",
                        span,
                    ));
                }
                Arc::clone(stream)
            }
            #[cfg(unix)]
            Some(Socket::Stream(Plain::Unix(_))) => {
                return Err(not_upgradable(op, handle, "it is a Unix socket", span));
            }
            Some(Socket::Tls(_)) => {
                return Err(not_upgradable(op, handle, "it is already TLS", span));
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

pub struct TcpHost {
    sockets: Arc<Sockets>,
    pool: Pool,
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
    /// The tokens a machine parks on when this host is itself its runtime.
    inbox: Arc<Inbox>,
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
            pool: Pool::new(),
            credentials,
            handshakes: Arc::new(Handshakes::default()),
            roots: Mutex::new(fs::Roots::new()),
            stopping: Arc::new(AtomicBool::new(false)),
            accepts: Arc::new(AtomicUsize::new(0)),
            closed_at: Mutex::new(Vec::new()),
            inbox: Arc::new(Inbox::default()),
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

    /// A datagram socket put in this host's table under `at`, answering its handle.
    pub(crate) fn open_datagram(&self, at: &Resource, socket: UdpSocket) -> i64 {
        self.sockets
            .insert(Some(at), Socket::Datagram(Arc::new(socket)))
    }

    /// The datagram socket a handle names under `at`.
    pub(crate) fn datagram(
        &self,
        handle: i64,
        at: &Resource,
        span: Span,
    ) -> Result<Arc<UdpSocket>, Diagnostic> {
        self.sockets.datagram(handle, at, span)
    }

    /// A job on this host's pool, for a facility that shares its sockets' threads.
    pub(crate) fn waiting(
        &self,
        span: Span,
        label: &'static str,
        what: &'static str,
        job: impl FnOnce() -> JobOutput + Send + 'static,
    ) -> Result<HostAnswer, Diagnostic> {
        self.pool
            .submit(span, label, what, Box::new(job))
            .map(HostAnswer::Pending)
    }

    /// Where the Unix socket `path` under the root named `root` is, on this machine.
    fn unix_path(&self, op: Op, root: &str, path: &str, span: Span) -> Result<PathBuf, Diagnostic> {
        let roots = lock(&self.roots);
        let Some(base) = roots.get(&Resource::Named(Symbol::new(root))) else {
            return Err(Diagnostic::error(
                codes::FS_ROOT_UNBOUND,
                format!("{} names the root `{root}`, and no root is bound to it", op.what()),
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
        at: &Resource,
        span: Span,
        read: impl FnOnce(&Socket) -> T,
    ) -> Result<T, Diagnostic> {
        self.sockets.handles.check(socket, at, span)?;
        match lock(&self.sockets.open).get(&socket) {
            Some(sock) => Ok(read(sock)),
            None => Err(unknown_handle(socket, span)),
        }
    }
}

fn answered(value: Value) -> Result<HostAnswer, Diagnostic> {
    Ok(HostAnswer::Value(value))
}

fn refused(refusal: Refusal) -> Result<HostAnswer, Diagnostic> {
    answered(wire::err(&refusal))
}

impl Pooled for TcpHost {
    fn pool(&self) -> &Pool {
        &self.pool
    }
}

impl Net for TcpHost {
    fn waits(&self) -> bool {
        true
    }

    /// `tcp` for both transports: the accepting listener, not the op, decides whether rustls runs.
    fn path(&self, op: Op) -> &'static str {
        match op {
            Op::Listen => "ply_host::tcp::listen",
            Op::ListenTls => tls::HANDLER,
            Op::ListenOn => "ply_host::tcp::listen_on",
            Op::ListenUnix => "ply_host::tcp::listen_unix",
            Op::Connect => "ply_host::tcp::connect",
            Op::ConnectTls => tls::CONNECT_HANDLER,
            Op::ConnectTo => "ply_host::tcp::connect_to",
            Op::ConnectUnix => "ply_host::tcp::connect_unix",
            Op::Handshake => tls::HANDSHAKE_HANDLER,
            Op::StartTls => tls::START_HANDLER,
            Op::ServeTls => tls::SERVE_HANDLER,
            Op::Accept => "ply_host::tcp::accept",
            Op::Recv => "ply_host::tcp::recv",
            Op::Send => "ply_host::tcp::send",
            Op::CloseWrite => "ply_host::tcp::close_write",
            Op::Close => "ply_host::tcp::close",
            Op::SetOption => "ply_host::tcp::set_option",
            Op::LocalPort => "ply_host::tcp::local_port",
            Op::LocalAddress => "ply_host::tcp::local_address",
            Op::PeerAddress => "ply_host::tcp::peer_address",
            Op::PeerCredentials => "ply_host::tcp::peer_credentials",
            Op::PeerCertificate => "ply_host::tls::peer_certificate",
            Op::Protocol => "ply_host::tls::protocol",
            Op::Options => "ply_host::tcp::options",
            Op::SendSecret => "ply_host::tcp::send_secret",
        }
    }

    fn listen(&self, at: &Resource, port: u16, span: Span) -> Result<HostAnswer, Diagnostic> {
        let listener = super::bind(Op::Listen.what(), port, span)?;
        answered(Value::Int(self.sockets.insert(
            Some(at),
            Socket::Listener(Listening::Tcp(Arc::new(listener), None)),
        )))
    }

    fn listen_tls(
        &self,
        at: &Resource,
        port: u16,
        credential: &str,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        let (listener, config) = tls::listen(&self.credentials, credential, port, span)?;
        answered(Value::Int(self.sockets.insert(
            Some(at),
            Socket::Listener(Listening::Tcp(Arc::new(listener), Some(config))),
        )))
    }

    /// Loopback only: a listener the rest of a network can reach is not a run's to open.
    fn listen_on(
        &self,
        at: &Resource,
        to: Endpoint,
        options: Vec<SocketOption>,
        _span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        if !to.ip.is_loopback() {
            return refused(Refusal::PermissionDenied);
        }
        match bound(&to, &options) {
            Ok(listener) => answered(wire::ok(Value::Int(self.sockets.insert(
                Some(at),
                Socket::Listener(Listening::Tcp(Arc::new(listener), None)),
            )))),
            Err(refusal) => refused(refusal),
        }
    }

    #[cfg(unix)]
    fn listen_unix(
        &self,
        at: &Resource,
        root: &str,
        path: &str,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        let path = self.unix_path(Op::ListenUnix, root, path, span)?;
        match UnixListener::bind(&path) {
            Ok(listener) => answered(wire::ok(Value::Int(self.sockets.insert(
                Some(at),
                Socket::Listener(Listening::Unix(Arc::new(listener), Arc::new(path))),
            )))),
            Err(e) => refused(Refusal::of(&e)),
        }
    }

    #[cfg(not(unix))]
    fn listen_unix(
        &self,
        _at: &Resource,
        _root: &str,
        _path: &str,
        _span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        refused(no_unix_sockets())
    }

    /// Resolution and the connect both happen on the job's thread; the deadline covers the connect
    /// to each address the name resolves to in turn.
    fn connect(
        &self,
        at: &Resource,
        host: &str,
        port: u16,
        timeout: Duration,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        let sockets = Arc::clone(&self.sockets);
        let host = host.to_string();
        let at = at.clone();
        self.waiting(span, "connect", Op::Connect.what(), move || {
            JobOutput::MaybeInt(reach(&host, port, timeout).map(|stream| {
                sockets.insert(Some(&at), Socket::Stream(Plain::Tcp(Arc::new(stream))))
            }))
        })
    }

    fn connect_tls(
        &self,
        at: &Resource,
        host: &str,
        port: u16,
        timeout: Duration,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        let sockets = Arc::clone(&self.sockets);
        let handshakes = Arc::clone(&self.handshakes);
        let config = self.credentials.client();
        let host = host.to_string();
        let at = at.clone();
        self.waiting(span, "connect_tls", Op::ConnectTls.what(), move || {
            // A host that is not a DNS name or an IP address is one no server can be verified as.
            let Ok(name) = ServerName::try_from(host.clone()) else {
                return JobOutput::MaybeInt(None);
            };
            JobOutput::MaybeInt(reach(&host, port, timeout).map(|stream| {
                let session = tls::Session::connect(config, name, Arc::new(stream), handshakes);
                sockets.insert(Some(&at), Socket::Tls(Arc::new(session)))
            }))
        })
    }

    fn connect_to(
        &self,
        at: &Resource,
        to: Endpoint,
        options: Vec<SocketOption>,
        timeout: Duration,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        let sockets = Arc::clone(&self.sockets);
        let presenting = Arc::clone(&self.presenting);
        let at = at.clone();
        self.waiting(span, "connect_to", Op::ConnectTo.what(), move || {
            let reached = reach_to(&to, &options, timeout).map(|stream| {
                let handle =
                    sockets.insert(Some(&at), Socket::Stream(Plain::Tcp(Arc::new(stream))));
                if let Some(name) = presented(&options) {
                    lock(&presenting).insert(handle, name);
                }
                handle
            });
            JobOutput::built(move || match reached {
                Ok(handle) => wire::ok(Value::Int(handle)),
                Err(refusal) => wire::err(&refusal),
            })
        })
    }

    #[cfg(unix)]
    fn connect_unix(
        &self,
        at: &Resource,
        root: &str,
        path: &str,
        timeout: Duration,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        let path = self.unix_path(Op::ConnectUnix, root, path, span)?;
        let sockets = Arc::clone(&self.sockets);
        let at = at.clone();
        self.waiting(span, "connect_unix", Op::ConnectUnix.what(), move || {
            let reached = reach_unix(&path, timeout).map(|stream| {
                sockets.insert(Some(&at), Socket::Stream(Plain::Unix(Arc::new(stream))))
            });
            JobOutput::built(move || match reached {
                Ok(handle) => wire::ok(Value::Int(handle)),
                Err(refusal) => wire::err(&refusal),
            })
        })
    }

    #[cfg(not(unix))]
    fn connect_unix(
        &self,
        _at: &Resource,
        _root: &str,
        _path: &str,
        _timeout: Duration,
        _span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        refused(no_unix_sockets())
    }

    /// No handshake here, deliberately: a session handshakes on its first read or write, so a peer
    /// that connects and says nothing costs nothing. A client that wants it earlier asks with
    /// `net.handshake`.
    fn handshake(&self, at: &Resource, conn: i64, span: Span) -> Result<HostAnswer, Diagnostic> {
        let session = match self.sockets.stream(conn, at, span)? {
            Connection::Tls(session) => Some(session),
            Connection::Plain(_) => None,
        };
        self.waiting(span, "handshake", Op::Handshake.what(), move || {
            JobOutput::MaybeInt(
                session
                    .and_then(|session| session.handshake())
                    .map(|us| us as i64),
            )
        })
    }

    /// The session takes the handle before the handshake starts, so nothing reaches the plaintext
    /// while it runs. Plaintext the caller holds unread, or the socket does, is a refusal: a TLS
    /// server never speaks first, so it was sent behind the go-ahead to be read as if secured.
    fn start_tls(
        &self,
        at: &Resource,
        conn: i64,
        name: &str,
        upgrade: Upgrade,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
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
                .upgraded(Op::StartTls, conn, at, span, move |stream| {
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
            return refused(refusal);
        }
        self.waiting(span, "start_tls", Op::StartTls.what(), move || {
            if tls::plaintext_waiting(&stream) {
                session.refuse(tls::REASON_INJECTED);
                return JobOutput::built(|| wire::err(&Refusal::Injected));
            }
            settled(&session, upgrade.timeout)
        })
    }

    /// The server's end. Bytes the socket already holds are the client's handshake, which it may
    /// send as soon as it has the go-ahead, so only what the caller read past the request to
    /// upgrade is refused.
    fn serve_tls(
        &self,
        at: &Resource,
        conn: i64,
        credential: &str,
        upgrade: Upgrade,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        let credential = self.credentials.resolve(credential, span)?;
        let config = tls::server_accepting(&credential, &upgrade.alpn);
        let handshakes = Arc::clone(&self.handshakes);
        let injected = upgrade.unread > 0;
        let (_, session) = self
            .sockets
            .upgraded(Op::ServeTls, conn, at, span, move |stream| {
                Arc::new(if injected {
                    tls::Session::refused(stream, handshakes, tls::REASON_INJECTED)
                } else {
                    tls::Session::new(config, stream, handshakes)
                })
            })?;
        if injected {
            return refused(Refusal::Injected);
        }
        self.waiting(span, "serve_tls", Op::ServeTls.what(), move || {
            settled(&session, upgrade.timeout)
        })
    }

    fn accept(&self, at: &Resource, listener: i64, span: Span) -> Result<HostAnswer, Diagnostic> {
        // Before the lookup, because `stop_accepting` has already swapped the listener out.
        if self.stopping.load(Ordering::Acquire) {
            self.sockets.handles.check(listener, at, span)?;
            return self.waiting(span, "accept", Op::Accept.what(), || JobOutput::Int(0));
        }
        let listening = self.sockets.listener(listener, at, span)?;
        let sockets = Arc::clone(&self.sockets);
        let handshakes = Arc::clone(&self.handshakes);
        let stopping = Arc::clone(&self.stopping);
        let accepts = Arc::clone(&self.accepts);
        accepts.fetch_add(1, Ordering::AcqRel);
        self.waiting(span, "accept", Op::Accept.what(), move || {
            let done = JobOutput::Int(accepted(&listening, &sockets, &handshakes, &stopping));
            accepts.fetch_sub(1, Ordering::AcqRel);
            done
        })
    }

    fn recv(
        &self,
        at: &Resource,
        conn: i64,
        max: usize,
        timeout: Duration,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        let conn = self.sockets.stream(conn, at, span)?;
        self.waiting(span, "recv", Op::Recv.what(), move || match conn {
            Connection::Plain(stream) => {
                // This job owns the socket for its duration, so setting its timeout is safe.
                stream.read_within(timeout);
                let mut buffer = vec![0u8; max];
                // One `read`: a loop would hide short reads and closes from the program.
                match stream.read(&mut buffer) {
                    Ok(n) => {
                        buffer.truncate(n);
                        JobOutput::MaybeBytes(Some(buffer))
                    }
                    Err(e) if expired(&e) => JobOutput::MaybeBytes(None),
                    Err(e) if peer_gone(&e) => JobOutput::MaybeBytes(Some(Vec::new())),
                    Err(e) => JobOutput::Failed(e.to_string()),
                }
            }
            // Never `Failed`: any TLS failure is "the peer went away", so the accept loop survives.
            Connection::Tls(session) => {
                session.deadline(timeout);
                JobOutput::MaybeBytes(session.read(max))
            }
        })
    }

    fn send(
        &self,
        at: &Resource,
        conn: i64,
        payload: &[u8],
        timeout: Duration,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        let conn = self.sockets.stream(conn, at, span)?;
        let payload = payload.to_vec();
        self.waiting(span, "send", Op::Send.what(), move || match conn {
            Connection::Plain(stream) => {
                stream.write_within(timeout);
                // One `write`, which may be short under backpressure; `std.net.send_all` loops.
                match stream.write(&payload) {
                    Ok(n) => JobOutput::MaybeInt(Some(n as i64)),
                    Err(e) if expired(&e) => JobOutput::MaybeInt(None),
                    Err(e) if peer_gone(&e) => JobOutput::MaybeInt(Some(0)),
                    Err(e) => JobOutput::Failed(e.to_string()),
                }
            }
            Connection::Tls(session) => {
                session.deadline(timeout);
                JobOutput::MaybeInt(Some(session.write(&payload) as i64))
            }
        })
    }

    fn send_secret(
        &self,
        at: &Resource,
        conn: i64,
        payload: Zeroizing<Vec<u8>>,
        timeout: Duration,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        let conn = self.sockets.stream(conn, at, span)?;
        self.waiting(span, "send_secret", Op::SendSecret.what(), move || {
            JobOutput::Bool(match conn {
                Connection::Plain(stream) => {
                    stream.write_within(timeout);
                    stream.write_all(&payload).is_ok()
                }
                Connection::Tls(session) => {
                    session.deadline(timeout);
                    session.write(&payload) == payload.len()
                }
            })
        })
    }

    fn close_write(&self, at: &Resource, conn: i64, span: Span) -> Result<HostAnswer, Diagnostic> {
        match self.sockets.stream(conn, at, span)? {
            Connection::Plain(stream) => stream.shutdown(Shutdown::Write),
            Connection::Tls(session) => session.close_write(),
        }
        answered(Value::Unit)
    }

    fn close(&self, at: &Resource, socket: i64, span: Span) -> Result<HostAnswer, Diagnostic> {
        self.sockets.handles.check(socket, at, span)?;
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
        answered(Value::Unit)
    }

    fn set_option(
        &self,
        at: &Resource,
        socket: i64,
        option: SocketOption,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        let set = self.described(socket, at, span, |sock| match raw_of(sock) {
            Some(raw) => apply(&raw, &option).map_err(|e| Refusal::of(&e)),
            None => Err(Refusal::Reset),
        })?;
        if let (Ok(()), SocketOption::Presenting(name)) = (&set, &option) {
            lock(&self.presenting).insert(socket, name.clone());
        }
        answered(match set {
            Ok(()) => wire::ok(Value::Unit),
            Err(refusal) => wire::err(&refusal),
        })
    }

    fn local_port(&self, at: &Resource, socket: i64, span: Span) -> Result<HostAnswer, Diagnostic> {
        let address = self.described(socket, at, span, local_of)?;
        answered(wire::option(
            address.map(|a| Value::Int(i64::from(a.port()))),
        ))
    }

    fn local_address(
        &self,
        at: &Resource,
        socket: i64,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        let address = self.described(socket, at, span, local_of)?;
        answered(wire::option(address.map(|a| Endpoint::of(a).value())))
    }

    fn peer_address(&self, at: &Resource, conn: i64, span: Span) -> Result<HostAnswer, Diagnostic> {
        let address = self.described(conn, at, span, |sock| match sock {
            Socket::Stream(Plain::Tcp(s)) => s.peer_addr().ok(),
            Socket::Tls(s) => s.peer_addr().ok(),
            _ => None,
        })?;
        answered(wire::option(address.map(|a| Endpoint::of(a).value())))
    }

    fn peer_credentials(
        &self,
        at: &Resource,
        conn: i64,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        let credentials = self.described(conn, at, span, |sock| match sock {
            #[cfg(unix)]
            Socket::Stream(Plain::Unix(s)) => credentials_of(s),
            _ => None,
        })?;
        answered(wire::option(credentials.map(|c| c.value())))
    }

    fn peer_certificate(
        &self,
        at: &Resource,
        conn: i64,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        let leaf = self.described(conn, at, span, |sock| match sock {
            Socket::Tls(s) => s.peer_certificate(),
            _ => None,
        })?;
        answered(wire::option(leaf.map(Value::bytes)))
    }

    fn protocol(&self, at: &Resource, conn: i64, span: Span) -> Result<HostAnswer, Diagnostic> {
        let agreed = self.described(conn, at, span, |sock| match sock {
            Socket::Tls(s) => s.protocol(),
            _ => None,
        })?;
        answered(wire::option(agreed.map(Value::str)))
    }

    fn options(&self, at: &Resource, socket: i64, span: Span) -> Result<HostAnswer, Diagnostic> {
        let options = self.described(socket, at, span, |sock| {
            raw_of(sock).map_or_else(Vec::new, |raw| standing(&raw))
        })?;
        answered(Value::list(
            options.iter().map(SocketOption::value).collect(),
        ))
    }
}

/// The credential `options` name for a later `start_tls` to present, the last where several do.
fn presented(options: &[SocketOption]) -> Option<String> {
    options.iter().rev().find_map(|o| match o {
        SocketOption::Presenting(name) => Some(name.clone()),
        _ => None,
    })
}

/// The handshake of an upgrade, completed within its deadline or refused with why.
fn settled(session: &tls::Session, timeout: Duration) -> JobOutput {
    let outcome = session.settle_within(timeout).map_err(|why| match why {
        tls::Unsettled::Expired => Refusal::TimedOut,
        tls::Unsettled::Refused(tls::REASON_CERTIFICATE) => Refusal::Untrusted,
        tls::Unsettled::Refused(tls::REASON_GONE) => Refusal::Reset,
        tls::Unsettled::Refused(reason) => Refusal::Handshake(reason.to_string()),
    });
    JobOutput::built(move || match outcome {
        Ok(protocol) => wire::ok(wire::secured(protocol)),
        Err(refusal) => wire::err(&refusal),
    })
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

impl HostRuntime for TcpHost {
    fn watch(&self, pending: &Pending) -> Result<(), Diagnostic> {
        self.pool.watch(pending, &self.inbox)
    }

    fn resolved(&self) -> Vec<(u64, Result<Value, Diagnostic>)> {
        self.pool.collect(&self.inbox)
    }

    fn park(&self) -> Result<(), Diagnostic> {
        self.pool.park()
    }

    fn block_on(&self, pending: Pending) -> Result<Value, Diagnostic> {
        self.pool.block_on(pending)
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
