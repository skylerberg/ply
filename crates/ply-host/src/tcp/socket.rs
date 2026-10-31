use super::wire::{self, Credentials, Endpoint, NetError, Probing, SocketOption};
use super::{
    Handles, Upgrade, bind, err_datagram_for_stream_socket, not_a_datagram_socket, not_a_listener,
    not_a_stream, not_upgradable, unknown_handle,
};
use crate::fs;
use crate::jobs::Job;
use crate::native::{found, int};
use crate::tls::{self, Credentials as TlsCredentials, HandshakeStats};
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

const BACKLOG: i32 = 128;

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

    fn raw(&self) -> SockRef<'_> {
        match self {
            Plain::Tcp(s) => SockRef::from(&**s),
            #[cfg(unix)]
            Plain::Unix(s) => SockRef::from(&**s),
        }
    }
}

#[derive(Clone)]
enum ListenerKind {
    Tcp(Arc<TcpListener>, Option<Arc<ServerConfig>>),
    #[cfg(unix)]
    Unix(Arc<UnixListener>, Arc<PathBuf>),
}

impl ListenerKind {
    fn accept(&self, handshakes: &Arc<HandshakeStats>) -> io::Result<Socket> {
        match self {
            ListenerKind::Tcp(listener, config) => {
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
            ListenerKind::Unix(listener, _) => {
                let (stream, _) = listener.accept()?;
                Ok(Socket::Stream(Plain::Unix(Arc::new(stream))))
            }
        }
    }
}

enum Socket {
    Listener(ListenerKind),
    Stream(Plain),
    Tls(Arc<tls::Session>),
    Datagram(Arc<UdpSocket>),
    Finished,
}

enum Connection {
    Plain(Plain),
    Tls(Arc<tls::Session>),
}

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

    fn listener(&self, handle: i64, label: &str, span: Span) -> Result<ListenerKind, Diagnostic> {
        self.handles.check(handle, label, span)?;
        match lock(&self.open).get(&handle) {
            Some(Socket::Listener(listening)) => Ok(listening.clone()),
            Some(Socket::Stream(_) | Socket::Tls(_)) => Err(not_a_listener(handle, span)),
            Some(Socket::Datagram(_)) => Err(err_datagram_for_stream_socket(handle, span)),
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
            Some(Socket::Datagram(_)) => Err(err_datagram_for_stream_socket(handle, span)),
            None => Err(unknown_handle(handle, span)),
        }
    }

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
            Some(Socket::Datagram(_)) => return Err(err_datagram_for_stream_socket(handle, span)),
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
    credentials: TlsCredentials,
    handshakes: Arc<HandshakeStats>,
    roots: Mutex<fs::Roots>,
    stopping: Arc<AtomicBool>,
    accepts: Arc<AtomicUsize>,
    closed_listener_addresses: Mutex<Vec<SocketAddr>>,
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
            handshakes: Arc::new(HandshakeStats::default()),
            roots: Mutex::new(fs::Roots::new()),
            stopping: Arc::new(AtomicBool::new(false)),
            accepts: Arc::new(AtomicUsize::new(0)),
            closed_listener_addresses: Mutex::new(Vec::new()),
            presenting: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub fn rooted(&self, roots: fs::Roots) {
        *lock(&self.roots) = roots;
    }

    pub fn credentials(&self) -> &TlsCredentials {
        &self.credentials
    }

    pub fn handshakes(&self) -> tls::HandshakeCounts {
        self.handshakes.snapshot()
    }

    pub fn local_addr(&self, handle: i64) -> Option<SocketAddr> {
        match lock(&self.sockets.open).get(&handle) {
            Some(Socket::Listener(ListenerKind::Tcp(l, _))) => l.local_addr().ok(),
            _ => None,
        }
    }

    pub(crate) fn open_datagram(&self, label: &str, socket: UdpSocket) -> i64 {
        self.sockets
            .insert(Some(label), Socket::Datagram(Arc::new(socket)))
    }

    pub fn datagram(
        &self,
        handle: i64,
        label: &str,
        span: Span,
    ) -> Result<Arc<UdpSocket>, Diagnostic> {
        self.sockets.datagram(handle, label, span)
    }

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
        Ok(fs::confine(base, path, span)?.unwrap_or_else(|| base.join(path)))
    }

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

    pub fn listen(&self, label: &str, port: u16, span: Span) -> Result<i64, Diagnostic> {
        let listener = bind("`net.listen`", port, span)?;
        Ok(self.sockets.insert(
            Some(label),
            Socket::Listener(ListenerKind::Tcp(Arc::new(listener), None)),
        ))
    }

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
            Socket::Listener(ListenerKind::Tcp(Arc::new(listener), Some(config))),
        ))
    }

    pub fn listen_on(&self, label: &str, to: &Endpoint, options: &[SocketOption]) -> Vec<u8> {
        wire::result(bound(to, options).map(|listener| {
            let handle = self.sockets.insert(
                Some(label),
                Socket::Listener(ListenerKind::Tcp(Arc::new(listener), None)),
            );
            int(handle).to_vec()
        }))
    }

    #[cfg(unix)]
    pub fn listen_unix(
        &self,
        label: &str,
        root: &str,
        path: &str,
        span: Span,
    ) -> Result<Vec<u8>, Diagnostic> {
        let path = self.unix_path("`net.listen_unix`", root, path, span)?;
        Ok(wire::result(match UnixListener::bind(&path) {
            Ok(listener) => {
                let handle = self.sockets.insert(
                    Some(label),
                    Socket::Listener(ListenerKind::Unix(Arc::new(listener), Arc::new(path))),
                );
                Ok(int(handle).to_vec())
            }
            Err(e) => Err(NetError::of(&e)),
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
        Ok(wire::result(Err(no_unix_sockets())))
    }

    pub fn connect(&self, label: &str, host: String, port: u16, timeout: Duration) -> Job {
        let sockets = Arc::clone(&self.sockets);
        let label = label.to_string();
        Box::new(move || {
            Ok(found(reach(&host, port, timeout).map(|stream| {
                int(sockets.insert(Some(&label), Socket::Stream(Plain::Tcp(Arc::new(stream)))))
            })))
        })
    }

    pub fn connect_tls(&self, label: &str, host: String, port: u16, timeout: Duration) -> Job {
        let sockets = Arc::clone(&self.sockets);
        let handshakes = Arc::clone(&self.handshakes);
        let config = self.credentials.client();
        let label = label.to_string();
        Box::new(move || {
            let Ok(name) = ServerName::try_from(host.clone()) else {
                return Ok(found(None::<[u8; 8]>));
            };
            Ok(found(reach(&host, port, timeout).map(|stream| {
                let session = tls::Session::connect(config, name, Arc::new(stream), handshakes);
                int(sockets.insert(Some(&label), Socket::Tls(Arc::new(session))))
            })))
        })
    }

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
            Ok(wire::result(connect_tcp(&to, &options, timeout).map(
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
            Ok(wire::result(reach_unix(&path, timeout).map(|stream| {
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
        Ok(Box::new(|| Ok(wire::result(Err(no_unix_sockets())))))
    }

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
            Some((tls::REASON_EARLY_PLAINTEXT, NetError::Injected))
        } else if server.is_err() {
            Some((
                tls::REASON_INVALID_SERVER_NAME,
                NetError::Handshake(tls::REASON_INVALID_SERVER_NAME.to_string()),
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
        let config = tls::client_config_offering_protocols(&client, &upgrade.alpn);
        let handshakes = Arc::clone(&self.handshakes);
        let (stream, session) =
            self.sockets
                .upgraded("`net.start_tls`", conn, label, span, move |stream| {
                    Arc::new(match (reason, server) {
                        (None, Ok(server)) => {
                            tls::Session::connect(config, server, stream, handshakes)
                        }
                        (reason, _) => tls::Session::rejected(
                            stream,
                            handshakes,
                            reason.unwrap_or(tls::REASON_INVALID_SERVER_NAME),
                        ),
                    })
                })?;
        if let Some((_, rejection)) = early {
            return Ok(Box::new(move || Ok(wire::result(Err(rejection)))));
        }
        Ok(Box::new(move || {
            if tls::plaintext_waiting(&stream) {
                session.refuse(tls::REASON_EARLY_PLAINTEXT);
                return Ok(wire::result(Err(NetError::Injected)));
            }
            Ok(finished(&session, upgrade.timeout))
        }))
    }

    pub fn serve_tls(
        &self,
        label: &str,
        conn: i64,
        credential: &str,
        upgrade: Upgrade,
        span: Span,
    ) -> Result<Job, Diagnostic> {
        let credential = self.credentials.resolve(credential, span)?;
        let config = tls::server_config_accepting_protocols(&credential, &upgrade.alpn);
        let handshakes = Arc::clone(&self.handshakes);
        let injected = upgrade.unread > 0;
        let (_, session) =
            self.sockets
                .upgraded("`net.serve_tls`", conn, label, span, move |stream| {
                    Arc::new(if injected {
                        tls::Session::rejected(stream, handshakes, tls::REASON_EARLY_PLAINTEXT)
                    } else {
                        tls::Session::new(config, stream, handshakes)
                    })
                })?;
        if injected {
            return Ok(Box::new(|| Ok(wire::result(Err(NetError::Injected)))));
        }
        Ok(Box::new(move || Ok(finished(&session, upgrade.timeout))))
    }

    pub fn accept(&self, label: &str, listener: i64, span: Span) -> Result<Job, Diagnostic> {
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
                stream.read_within(timeout);
                let mut buffer = vec![0u8; max + 1];
                buffer[0] = 1;
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
            Connection::Tls(session) => {
                session.deadline(timeout);
                Ok(found(session.read(max)))
            }
        }))
    }

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

    pub fn close_write(&self, label: &str, conn: i64, span: Span) -> Result<(), Diagnostic> {
        match self.sockets.stream(conn, label, span)? {
            Connection::Plain(stream) => stream.shutdown(Shutdown::Write),
            Connection::Tls(session) => session.close_write(),
        }
        Ok(())
    }

    pub fn close(&self, label: &str, socket: i64, span: Span) -> Result<(), Diagnostic> {
        self.sockets.handles.check(socket, label, span)?;
        let sock = lock(&self.sockets.open).remove(&socket);
        self.sockets.handles.close(socket);
        lock(&self.presenting).remove(&socket);
        match sock {
            Some(Socket::Stream(s)) => s.shutdown(Shutdown::Both),
            Some(Socket::Tls(s)) => s.close(),
            #[cfg(unix)]
            Some(Socket::Listener(ListenerKind::Unix(_, path))) => {
                let _ = std::fs::remove_file(&*path);
            }
            Some(
                Socket::Listener(ListenerKind::Tcp(..)) | Socket::Datagram(_) | Socket::Finished,
            ) => {}
            None => return Err(unknown_handle(socket, span)),
        }
        Ok(())
    }

    pub fn set_option(
        &self,
        label: &str,
        socket: i64,
        option: SocketOption,
        span: Span,
    ) -> Result<Vec<u8>, Diagnostic> {
        let set = self.described(socket, label, span, |sock| match raw_of(sock) {
            Some(raw) => apply(&raw, &option).map_err(|e| NetError::of(&e)),
            None => Err(NetError::Reset),
        })?;
        if let (Ok(()), SocketOption::Presenting(name)) = (&set, &option) {
            lock(&self.presenting).insert(socket, name.clone());
        }
        Ok(wire::result(set.map(|()| Vec::new())))
    }

    pub fn local_port(
        &self,
        label: &str,
        socket: i64,
        span: Span,
    ) -> Result<Option<u16>, Diagnostic> {
        let address = self.described(socket, label, span, local_of)?;
        Ok(address.map(|a| a.port()))
    }

    pub fn local_address(
        &self,
        label: &str,
        socket: i64,
        span: Span,
    ) -> Result<Vec<u8>, Diagnostic> {
        let address = self.described(socket, label, span, local_of)?;
        Ok(found(address.map(|a| Endpoint::of(a).written())))
    }

    pub fn peer_address(&self, label: &str, conn: i64, span: Span) -> Result<Vec<u8>, Diagnostic> {
        let address = self.described(conn, label, span, |sock| match sock {
            Socket::Stream(Plain::Tcp(s)) => s.peer_addr().ok(),
            Socket::Tls(s) => s.peer_addr().ok(),
            _ => None,
        })?;
        Ok(found(address.map(|a| Endpoint::of(a).written())))
    }

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

    pub fn protocol(&self, label: &str, conn: i64, span: Span) -> Result<Vec<u8>, Diagnostic> {
        let agreed = self.described(conn, label, span, |sock| match sock {
            Socket::Tls(s) => s.protocol(),
            _ => None,
        })?;
        Ok(found(agreed))
    }

    pub fn options(&self, label: &str, socket: i64, span: Span) -> Result<Vec<u8>, Diagnostic> {
        let options = self.described(socket, label, span, |sock| {
            raw_of(sock).map_or_else(Vec::new, |raw| standing(&raw))
        })?;
        Ok(SocketOption::listed(&options))
    }
}

fn presented(options: &[SocketOption]) -> Option<String> {
    options.iter().rev().find_map(|o| match o {
        SocketOption::Presenting(name) => Some(name.clone()),
        _ => None,
    })
}

fn finished(session: &tls::Session, timeout: Duration) -> Vec<u8> {
    let outcome = session
        .finish_within(timeout)
        .map_err(|reason| match reason {
            tls::ShutdownFailure::Expired => NetError::TimedOut,
            tls::ShutdownFailure::Rejected(tls::REASON_CERTIFICATE) => NetError::Untrusted,
            tls::ShutdownFailure::Rejected(tls::REASON_GONE) => NetError::Reset,
            tls::ShutdownFailure::Rejected(reason) => NetError::Handshake(reason.to_string()),
        });
    wire::result(outcome.map(found))
}

fn accepted(
    listening: &ListenerKind,
    sockets: &Sockets,
    handshakes: &Arc<HandshakeStats>,
    stopping: &AtomicBool,
) -> i64 {
    for _ in 0..=ACCEPT_RETRIES {
        if stopping.load(Ordering::Acquire) {
            return 0;
        }
        match listening.accept(handshakes) {
            Ok(_) if stopping.load(Ordering::Acquire) => return 0,
            Ok(sock) => return sockets.insert(None, sock),
            Err(e) if transient(&e) => continue,
            Err(_) => return 0,
        }
    }
    0
}

impl crate::signal::Accepting for TcpHost {
    fn stop_accepting(&self) -> usize {
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
                Some(Socket::Listener(ListenerKind::Tcp(l, _))) => {
                    if let Ok(address) = l.local_addr() {
                        closed.push(address);
                    }
                }
                #[cfg(unix)]
                Some(Socket::Listener(ListenerKind::Unix(_, path))) => {
                    drop(UnixStream::connect(&*path));
                    let _ = std::fs::remove_file(&*path);
                }
                _ => {}
            }
        }
        *lock(&self.closed_listener_addresses) = closed;
        listeners.len()
    }

    fn listening_at(&self) -> Vec<SocketAddr> {
        let mut addresses: Vec<SocketAddr> = lock(&self.sockets.open)
            .values()
            .filter_map(|s| match s {
                Socket::Listener(ListenerKind::Tcp(l, _)) => l.local_addr().ok(),
                _ => None,
            })
            .collect();
        addresses.extend(lock(&self.closed_listener_addresses).iter().copied());
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

const ACCEPT_RETRIES: usize = 16;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

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

fn connect_tcp(
    to: &Endpoint,
    options: &[SocketOption],
    timeout: Duration,
) -> Result<TcpStream, NetError> {
    let address = to.socket_addr()?;
    let rejection = |e: io::Error| NetError::of(&e);
    let socket = RawSocket::new(
        Domain::for_address(address),
        Type::STREAM,
        Some(Protocol::TCP),
    )
    .map_err(rejection)?;
    socket.set_tcp_nodelay(true).map_err(rejection)?;
    for option in options {
        apply(&socket, option).map_err(rejection)?;
    }
    socket
        .connect_timeout(&SockAddr::from(address), timeout)
        .map_err(rejection)?;
    Ok(socket.into())
}

#[cfg(unix)]
fn reach_unix(path: &Path, timeout: Duration) -> Result<UnixStream, NetError> {
    let rejection = |e: io::Error| NetError::of(&e);
    let address = SockAddr::unix(path).map_err(rejection)?;
    let socket = RawSocket::new(Domain::UNIX, Type::STREAM, None).map_err(rejection)?;
    socket.set_write_timeout(Some(timeout)).map_err(rejection)?;
    socket.connect(&address).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            NetError::Rejected
        } else {
            NetError::of(&e)
        }
    })?;
    Ok(socket.into())
}

#[cfg(not(unix))]
fn no_unix_sockets() -> NetError {
    NetError::Other(0, "this platform has no Unix sockets".to_string())
}

fn bound(to: &Endpoint, options: &[SocketOption]) -> Result<TcpListener, NetError> {
    let address = to.socket_addr()?;
    let rejection = |e: io::Error| NetError::of(&e);
    let socket = RawSocket::new(
        Domain::for_address(address),
        Type::STREAM,
        Some(Protocol::TCP),
    )
    .map_err(rejection)?;
    #[cfg(unix)]
    socket.set_reuse_address(true).map_err(rejection)?;
    for option in options {
        apply(&socket, option).map_err(rejection)?;
    }
    socket.bind(&SockAddr::from(address)).map_err(rejection)?;
    socket.listen(BACKLOG).map_err(rejection)?;
    Ok(socket.into())
}

fn raw_of(sock: &Socket) -> Option<SockRef<'_>> {
    match sock {
        Socket::Listener(ListenerKind::Tcp(l, _)) => Some(SockRef::from(&**l)),
        #[cfg(unix)]
        Socket::Listener(ListenerKind::Unix(l, _)) => Some(SockRef::from(&**l)),
        Socket::Stream(s) => Some(s.raw()),
        Socket::Tls(s) => Some(SockRef::from(s.socket())),
        Socket::Datagram(s) => Some(SockRef::from(&**s)),
        Socket::Finished => None,
    }
}

fn local_of(sock: &Socket) -> Option<SocketAddr> {
    match sock {
        Socket::Listener(ListenerKind::Tcp(l, _)) => l.local_addr().ok(),
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
        SocketOption::Presenting(_) => Ok(()),
    }
}

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
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &raw mut user, &raw mut group) } != 0 {
        return None;
    }
    let mut process: libc::pid_t = 0;
    let mut len = size_of::<libc::pid_t>() as libc::socklen_t;
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
