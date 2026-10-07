//! The simulated twin: `net` over a script instead of a socket.

use super::wire::{self, Endpoint, Refusal, SocketOption};
use super::{
    Handles, Net, Op, Upgrade, no_connection_scripted, not_a_listener, not_a_stream,
    not_upgradable, unknown_handle,
};
use crate::tls;
use ply_eval::{Diagnostic, HostAnswer, HostRuntime, Pending, Resource, Span, Value, codes};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;
use zeroize::Zeroizing;

/// Where the ports the twin assigns start: the first of the dynamic range.
const EPHEMERAL: u16 = 49152;

/// Where the ports of the clients the twin's `accept` hands out start.
const CALLERS: u16 = 32768;

fn some(v: Value) -> Value {
    Value::ctor("Some", vec![v])
}

#[derive(Default)]
struct SimState {
    listeners: Vec<i64>,
    /// The connections `accept` hands out, in order, whichever listener asks.
    inbound: VecDeque<VecDeque<Vec<u8>>>,
    conns: BTreeMap<i64, VecDeque<Vec<u8>>>,
    sent: BTreeMap<i64, Vec<u8>>,
    /// Each open socket's own port: a listener's, which an accepted connection shares, or one
    /// assigned as the kernel would assign it.
    ports: BTreeMap<i64, u16>,
    /// Where the far end of each connection is.
    peers: BTreeMap<i64, Endpoint>,
    /// The connections an upgrade has secured.
    secured: BTreeSet<i64>,
    /// The connections whose sending this end has ended.
    ended: BTreeSet<i64>,
    /// The options set on each socket, the latest setting of each last.
    options: BTreeMap<i64, Vec<SocketOption>>,
    assigned: u16,
    /// How many clients `accept` has handed out, each from a port of its own.
    accepted: u16,
}

impl SimState {
    /// Ports from [`EPHEMERAL`] up, in the order they are asked for, wrapping within the range.
    fn assign(&mut self) -> u16 {
        let port = EPHEMERAL + self.assigned % (u16::MAX - EPHEMERAL + 1);
        self.assigned = self.assigned.wrapping_add(1);
        port
    }

    /// Where the next accepted client calls from: counted apart from this end's own ports, so a
    /// program's ports do not move with how many clients it has taken.
    fn caller(&mut self) -> Endpoint {
        let port = CALLERS + self.accepted % (EPHEMERAL - CALLERS);
        self.accepted = self.accepted.wrapping_add(1);
        loopback(port)
    }
}

pub struct SimNet {
    state: Mutex<SimState>,
    handles: Handles,
    /// Stands in for the credentials `--tls` gives the socket handler.
    credentials: BTreeSet<String>,
}

impl SimNet {
    /// Each connection `accept` hands out, as the chunks `recv` answers before end of stream.
    pub fn new(connections: Vec<Vec<Vec<u8>>>) -> SimNet {
        SimNet::with_credentials(connections, Vec::<String>::new())
    }

    pub fn with_credentials(
        connections: Vec<Vec<Vec<u8>>>,
        credentials: Vec<impl Into<String>>,
    ) -> SimNet {
        SimNet {
            state: Mutex::new(SimState {
                inbound: connections
                    .into_iter()
                    .map(|chunks| chunks.into_iter().collect())
                    .collect(),
                ..SimState::default()
            }),
            handles: Handles::new(),
            credentials: credentials.into_iter().map(Into::into).collect(),
        }
    }

    fn bind(&self, at: &Resource, port: u16) -> i64 {
        let handle = self.handles.open(Some(at));
        let mut state = lock(&self.state);
        state.listeners.push(handle);
        let port = if port == 0 { state.assign() } else { port };
        state.ports.insert(handle, port);
        handle
    }

    /// The next scripted connection opened under `at`, its far end at `peer`.
    fn opened(&self, at: &Resource, peer: Endpoint) -> Option<i64> {
        let mut state = lock(&self.state);
        let chunks = state.inbound.pop_front()?;
        let handle = self.handles.open(Some(at));
        state.conns.insert(handle, chunks);
        let port = state.assign();
        state.ports.insert(handle, port);
        state.peers.insert(handle, peer);
        Some(handle)
    }

    /// The twin has no transport to secure, so an upgrade is its rule alone: plaintext the caller
    /// holds unread is refused, and anything else agrees on the first protocol offered.
    fn upgraded(
        &self,
        op: Op,
        at: &Resource,
        conn: i64,
        upgrade: Upgrade,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        self.handles.check(conn, at, span)?;
        let mut state = lock(&self.state);
        if !state.conns.contains_key(&conn) {
            return Err(not_a_stream(conn, span));
        }
        if state.secured.contains(&conn) {
            return Err(not_upgradable(op, conn, "it is already TLS", span));
        }
        if upgrade.unread > 0 {
            state.conns.insert(conn, VecDeque::new());
            state.ended.insert(conn);
            return Ok(HostAnswer::Value(wire::err(&Refusal::Injected)));
        }
        state.secured.insert(conn);
        Ok(HostAnswer::Value(wire::ok(wire::secured(
            upgrade.alpn.into_iter().next(),
        ))))
    }

    pub fn sent(&self, conn: i64) -> Vec<u8> {
        lock(&self.state)
            .sent
            .get(&conn)
            .cloned()
            .unwrap_or_default()
    }
}

impl Net for SimNet {
    fn waits(&self) -> bool {
        false
    }

    fn path(&self, op: Op) -> &'static str {
        match op {
            Op::Listen => "ply_host::tcp::sim::listen",
            Op::ListenTls => "ply_host::tls::sim::listen",
            Op::ListenOn => "ply_host::tcp::sim::listen_on",
            Op::ListenUnix => "ply_host::tcp::sim::listen_unix",
            Op::Connect => "ply_host::tcp::sim::connect",
            Op::ConnectTls => "ply_host::tls::sim::connect",
            Op::ConnectTo => "ply_host::tcp::sim::connect_to",
            Op::ConnectUnix => "ply_host::tcp::sim::connect_unix",
            Op::Handshake => "ply_host::tls::sim::handshake",
            Op::StartTls => "ply_host::tls::sim::start",
            Op::ServeTls => "ply_host::tls::sim::serve",
            Op::Accept => "ply_host::tcp::sim::accept",
            Op::Recv => "ply_host::tcp::sim::recv",
            Op::Send => "ply_host::tcp::sim::send",
            Op::CloseWrite => "ply_host::tcp::sim::close_write",
            Op::Close => "ply_host::tcp::sim::close",
            Op::SetOption => "ply_host::tcp::sim::set_option",
            Op::LocalPort => "ply_host::tcp::sim::local_port",
            Op::LocalAddress => "ply_host::tcp::sim::local_address",
            Op::PeerAddress => "ply_host::tcp::sim::peer_address",
            Op::PeerCredentials => "ply_host::tcp::sim::peer_credentials",
            Op::PeerCertificate => "ply_host::tcp::sim::peer_certificate",
            Op::Protocol => "ply_host::tcp::sim::protocol",
            Op::Options => "ply_host::tcp::sim::options",
            Op::SendSecret => "ply_host::tcp::sim::send_secret",
        }
    }

    fn listen(&self, at: &Resource, port: u16, _span: Span) -> Result<HostAnswer, Diagnostic> {
        Ok(HostAnswer::Value(Value::Int(self.bind(at, port))))
    }

    /// Checks the credential, then binds an ordinary listener: TLS changes none of the bytes read.
    fn listen_tls(
        &self,
        at: &Resource,
        port: u16,
        credential: &str,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        if !self.credentials.contains(credential) {
            return Err(tls::unknown_credential(
                credential,
                self.credentials.iter().map(String::as_str),
                span,
            ));
        }
        Ok(HostAnswer::Value(Value::Int(self.bind(at, port))))
    }

    /// A listener at the port asked for, on loopback as the socket handler holds one to.
    fn listen_on(
        &self,
        at: &Resource,
        to: Endpoint,
        options: Vec<SocketOption>,
        _span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        if !to.ip.is_loopback() {
            return Ok(HostAnswer::Value(wire::err(&Refusal::PermissionDenied)));
        }
        let handle = self.bind(at, to.port);
        lock(&self.state).options.insert(handle, settings(options));
        Ok(HostAnswer::Value(wire::ok(Value::Int(handle))))
    }

    /// A listener with no port: the twin keeps no paths, so no root is asked for.
    fn listen_unix(
        &self,
        at: &Resource,
        _root: &str,
        _path: &str,
        _span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        let handle = self.handles.open(Some(at));
        lock(&self.state).listeners.push(handle);
        Ok(HostAnswer::Value(wire::ok(Value::Int(handle))))
    }

    /// The next scripted connection, as `accept` would hand it out; none left is a host not reached.
    fn connect(
        &self,
        at: &Resource,
        _host: &str,
        port: u16,
        _timeout: Duration,
        _span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        Ok(HostAnswer::Value(wire::option(
            self.opened(at, loopback(port)).map(Value::Int),
        )))
    }

    /// The next scripted connection, its far end where it was asked for; none left is one refused.
    fn connect_to(
        &self,
        at: &Resource,
        to: Endpoint,
        options: Vec<SocketOption>,
        _timeout: Duration,
        _span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        Ok(HostAnswer::Value(match self.opened(at, to) {
            Some(handle) => {
                lock(&self.state).options.insert(handle, settings(options));
                wire::ok(Value::Int(handle))
            }
            None => wire::err(&Refusal::Refused),
        }))
    }

    fn connect_unix(
        &self,
        at: &Resource,
        _root: &str,
        _path: &str,
        _timeout: Duration,
        _span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        let mut state = lock(&self.state);
        let Some(chunks) = state.inbound.pop_front() else {
            return Ok(HostAnswer::Value(wire::err(&Refusal::Refused)));
        };
        // Neither a port nor a far end's address: a Unix socket has no IP address.
        let handle = self.handles.open(Some(at));
        state.conns.insert(handle, chunks);
        Ok(HostAnswer::Value(wire::ok(Value::Int(handle))))
    }

    /// The next scripted connection, as `connect`: TLS changes none of the bytes read.
    fn connect_tls(
        &self,
        at: &Resource,
        host: &str,
        port: u16,
        timeout: Duration,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        self.connect(at, host, port, timeout, span)
    }

    /// The twin has no transport to handshake over, so a connection it hands out has none left to
    /// complete: it answers a zero-length one rather than pretending to have measured anything.
    fn handshake(&self, at: &Resource, conn: i64, span: Span) -> Result<HostAnswer, Diagnostic> {
        self.handles.check(conn, at, span)?;
        Ok(HostAnswer::Value(some(Value::Int(0))))
    }

    fn start_tls(
        &self,
        at: &Resource,
        conn: i64,
        _name: &str,
        upgrade: Upgrade,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        self.upgraded(Op::StartTls, at, conn, upgrade, span)
    }

    fn serve_tls(
        &self,
        at: &Resource,
        conn: i64,
        credential: &str,
        upgrade: Upgrade,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        if !self.credentials.contains(credential) {
            return Err(tls::unknown_credential(
                credential,
                self.credentials.iter().map(String::as_str),
                span,
            ));
        }
        self.upgraded(Op::ServeTls, at, conn, upgrade, span)
    }

    fn accept(&self, at: &Resource, listener: i64, span: Span) -> Result<HostAnswer, Diagnostic> {
        self.handles.check(listener, at, span)?;
        let mut state = lock(&self.state);
        if !state.listeners.contains(&listener) {
            return Err(not_a_listener(listener, span));
        }
        let Some(chunks) = state.inbound.pop_front() else {
            return Err(no_connection_scripted(span));
        };
        let handle = self.handles.open(None);
        state.conns.insert(handle, chunks);
        // A listener with no port is a Unix socket's, whose connections have no address either.
        if let Some(port) = state.ports.get(&listener).copied() {
            state.ports.insert(handle, port);
            let peer = state.caller();
            state.peers.insert(handle, peer);
        }
        Ok(HostAnswer::Value(Value::Int(handle)))
    }

    /// The twin never answers `None`.
    fn recv(
        &self,
        at: &Resource,
        conn: i64,
        max: usize,
        _timeout: Duration,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        self.handles.check(conn, at, span)?;
        let mut state = lock(&self.state);
        let Some(chunks) = state.conns.get_mut(&conn) else {
            return Err(not_a_stream(conn, span));
        };
        // An exhausted script is end of stream.
        let Some(mut chunk) = chunks.pop_front() else {
            return Ok(HostAnswer::Value(some(Value::bytes([]))));
        };
        if chunk.len() > max {
            chunks.push_front(chunk.split_off(max));
        }
        Ok(HostAnswer::Value(some(Value::bytes(chunk))))
    }

    fn send(
        &self,
        at: &Resource,
        conn: i64,
        payload: &[u8],
        _timeout: Duration,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        self.handles.check(conn, at, span)?;
        let mut state = lock(&self.state);
        if !state.conns.contains_key(&conn) {
            return Err(not_a_stream(conn, span));
        }
        if state.ended.contains(&conn) {
            return Ok(HostAnswer::Value(some(Value::Int(0))));
        }
        state
            .sent
            .entry(conn)
            .or_default()
            .extend_from_slice(payload);
        Ok(HostAnswer::Value(some(Value::Int(payload.len() as i64))))
    }

    /// What a test reads back of a credential is what the peer would: the twin keeps it with
    /// everything else sent.
    fn send_secret(
        &self,
        at: &Resource,
        conn: i64,
        payload: Zeroizing<Vec<u8>>,
        _timeout: Duration,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        self.handles.check(conn, at, span)?;
        let mut state = lock(&self.state);
        if !state.conns.contains_key(&conn) {
            return Err(not_a_stream(conn, span));
        }
        if state.ended.contains(&conn) {
            return Ok(HostAnswer::Value(Value::Bool(false)));
        }
        state
            .sent
            .entry(conn)
            .or_default()
            .extend_from_slice(&payload);
        Ok(HostAnswer::Value(Value::Bool(true)))
    }

    fn close_write(&self, at: &Resource, conn: i64, span: Span) -> Result<HostAnswer, Diagnostic> {
        self.handles.check(conn, at, span)?;
        let mut state = lock(&self.state);
        if !state.conns.contains_key(&conn) {
            return Err(not_a_stream(conn, span));
        }
        state.ended.insert(conn);
        Ok(HostAnswer::Value(Value::Unit))
    }

    fn close(&self, at: &Resource, socket: i64, span: Span) -> Result<HostAnswer, Diagnostic> {
        self.handles.check(socket, at, span)?;
        self.handles.close(socket);
        let mut state = lock(&self.state);
        state.ports.remove(&socket);
        state.peers.remove(&socket);
        state.secured.remove(&socket);
        state.ended.remove(&socket);
        state.options.remove(&socket);
        if state.conns.remove(&socket).is_some() {
            return Ok(HostAnswer::Value(Value::Unit));
        }
        match state.listeners.iter().position(|l| *l == socket) {
            Some(index) => {
                state.listeners.remove(index);
                Ok(HostAnswer::Value(Value::Unit))
            }
            None => Err(unknown_handle(socket, span)),
        }
    }

    /// Every option is taken and none changes what the twin does: it has no kernel to tune.
    fn set_option(
        &self,
        at: &Resource,
        socket: i64,
        option: SocketOption,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        self.handles.check(socket, at, span)?;
        let mut state = lock(&self.state);
        let set = state.options.entry(socket).or_default();
        if option.is_setting() {
            set.retain(|held| std::mem::discriminant(held) != std::mem::discriminant(&option));
            set.push(option);
        }
        Ok(HostAnswer::Value(wire::ok(Value::Unit)))
    }

    fn local_port(&self, at: &Resource, socket: i64, span: Span) -> Result<HostAnswer, Diagnostic> {
        self.handles.check(socket, at, span)?;
        Ok(HostAnswer::Value(wire::option(
            lock(&self.state)
                .ports
                .get(&socket)
                .map(|port| Value::Int(i64::from(*port))),
        )))
    }

    fn local_address(
        &self,
        at: &Resource,
        socket: i64,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        self.handles.check(socket, at, span)?;
        Ok(HostAnswer::Value(wire::option(
            lock(&self.state)
                .ports
                .get(&socket)
                .map(|port| loopback(*port).value()),
        )))
    }

    fn peer_address(&self, at: &Resource, conn: i64, span: Span) -> Result<HostAnswer, Diagnostic> {
        self.handles.check(conn, at, span)?;
        Ok(HostAnswer::Value(wire::option(
            lock(&self.state).peers.get(&conn).map(Endpoint::value),
        )))
    }

    /// The twin's sockets have no owner to report.
    fn peer_credentials(
        &self,
        at: &Resource,
        conn: i64,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        self.handles.check(conn, at, span)?;
        Ok(HostAnswer::Value(wire::none()))
    }

    /// The twin's connections are never secured, so none has a certificate to report.
    fn peer_certificate(
        &self,
        at: &Resource,
        conn: i64,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        self.handles.check(conn, at, span)?;
        Ok(HostAnswer::Value(wire::none()))
    }

    /// Nor a protocol one agreed.
    fn protocol(&self, at: &Resource, conn: i64, span: Span) -> Result<HostAnswer, Diagnostic> {
        self.handles.check(conn, at, span)?;
        Ok(HostAnswer::Value(wire::none()))
    }

    fn options(&self, at: &Resource, socket: i64, span: Span) -> Result<HostAnswer, Diagnostic> {
        self.handles.check(socket, at, span)?;
        Ok(HostAnswer::Value(Value::list(
            lock(&self.state)
                .options
                .get(&socket)
                .map(|set| set.iter().map(SocketOption::value).collect())
                .unwrap_or_default(),
        )))
    }
}

/// What the twin reads back of the options a socket opened with.
fn settings(options: Vec<SocketOption>) -> Vec<SocketOption> {
    options
        .into_iter()
        .filter(SocketOption::is_setting)
        .collect()
}

/// Where the twin puts a far end it was not told of.
fn loopback(port: u16) -> Endpoint {
    Endpoint {
        ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        port,
        zone: None,
    }
}

/// Mints no token, so any token it is handed belongs to another facility.
impl HostRuntime for SimNet {
    fn watch(&self, pending: &Pending) -> Result<(), Diagnostic> {
        Err(foreign_token(pending))
    }

    fn resolved(&self) -> Vec<(u64, Result<Value, Diagnostic>)> {
        Vec::new()
    }

    fn park(&self) -> Result<(), Diagnostic> {
        Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            "the simulated network was asked to wait, and it never has anything outstanding",
        ))
    }

    fn block_on(&self, pending: Pending) -> Result<Value, Diagnostic> {
        Err(foreign_token(&pending))
    }
}

#[cold]
fn foreign_token(pending: &Pending) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("the simulated network was asked about `{pending}`, which it did not mint"),
    )
    .note("every simulated answer is a value; a pending token here means two host facilities were composed and the wrong one was asked")
}

/// Poison is ignored: the maps have no invariant a panic can break.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
