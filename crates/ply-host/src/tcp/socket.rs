//! `net` over loopback TCP, plaintext or TLS.

use super::{Handles, Net, Op, not_a_listener, not_a_stream, unknown_handle};
use crate::pool::{Done, NET_FIRST_TOKEN, Pool};
use crate::tls::{self, Credentials, Handshakes};
use ply_eval::{HostAnswer, HostRuntime, Pending, Value};
use ply_span::{Diagnostic, Span};
use ply_ty::Resource;
use rustls::server::ServerConfig;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

enum Sock {
    /// `None` for plaintext; otherwise the config every accepted connection is terminated with.
    Listener(Arc<TcpListener>, Option<Arc<ServerConfig>>),
    Stream(Arc<TcpStream>),
    Tls(Arc<tls::Session>),
    /// A listener the drain closed.
    Finished,
}

enum Conn {
    Plain(Arc<TcpStream>),
    Tls(Arc<tls::Session>),
}

/// The socket table and handle allocator, together because a completing `accept` inserts into both.
struct Sockets {
    open: Mutex<BTreeMap<i64, Sock>>,
    handles: Handles,
}

impl Sockets {
    fn insert(&self, at: Option<&Resource>, sock: Sock) -> i64 {
        let handle = self.handles.open(at);
        lock(&self.open).insert(handle, sock);
        handle
    }

    fn listener(
        &self,
        handle: i64,
        at: &Resource,
        span: Span,
    ) -> Result<(Arc<TcpListener>, Option<Arc<ServerConfig>>), Diagnostic> {
        self.handles.check(handle, at, span)?;
        match lock(&self.open).get(&handle) {
            Some(Sock::Listener(l, tls)) => Ok((Arc::clone(l), tls.clone())),
            Some(Sock::Stream(_) | Sock::Tls(_)) => Err(not_a_listener(handle, span)),
            // Unreachable: `accept` checks the stop flag, set before any listener is swapped.
            Some(Sock::Finished) => Err(not_a_listener(handle, span)),
            None => Err(unknown_handle(handle, span)),
        }
    }

    fn stream(&self, handle: i64, at: &Resource, span: Span) -> Result<Conn, Diagnostic> {
        self.handles.check(handle, at, span)?;
        match lock(&self.open).get(&handle) {
            Some(Sock::Stream(s)) => Ok(Conn::Plain(Arc::clone(s))),
            Some(Sock::Tls(s)) => Ok(Conn::Tls(Arc::clone(s))),
            Some(Sock::Listener(..) | Sock::Finished) => Err(not_a_stream(handle, span)),
            None => Err(unknown_handle(handle, span)),
        }
    }

    fn connections(&self) -> usize {
        lock(&self.open)
            .values()
            .filter(|s| matches!(s, Sock::Stream(_) | Sock::Tls(_)))
            .count()
    }
}

pub struct TcpHost {
    sockets: Arc<Sockets>,
    pool: Pool,
    /// What `net.listen_tls` resolves a credential name against.
    credentials: Credentials,
    handshakes: Arc<Handshakes>,
    /// Set by phase 2 of the drain.
    stopping: Arc<AtomicBool>,
    /// `accept` operations parked on a pool thread.
    accepts: Arc<AtomicUsize>,
    /// Where the listeners phase 2 closed were bound.
    closed_at: Mutex<Vec<SocketAddr>>,
}

impl Default for TcpHost {
    fn default() -> TcpHost {
        TcpHost::new()
    }
}

impl TcpHost {
    pub fn new() -> TcpHost {
        TcpHost::with_credentials(Credentials::empty())
    }

    pub fn with_credentials(credentials: Credentials) -> TcpHost {
        TcpHost {
            sockets: Arc::new(Sockets {
                open: Mutex::new(BTreeMap::new()),
                handles: Handles::new(),
            }),
            pool: Pool::new(NET_FIRST_TOKEN),
            credentials,
            handshakes: Arc::new(Handshakes::default()),
            stopping: Arc::new(AtomicBool::new(false)),
            accepts: Arc::new(AtomicUsize::new(0)),
            closed_at: Mutex::new(Vec::new()),
        }
    }

    pub fn credentials(&self) -> &Credentials {
        &self.credentials
    }

    pub fn handshakes(&self) -> tls::HandshakeCounts {
        self.handshakes.snapshot()
    }

    /// The address a listening handle actually bound.
    pub fn local_addr(&self, handle: i64) -> Option<SocketAddr> {
        match lock(&self.sockets.open).get(&handle) {
            Some(Sock::Listener(l, _)) => l.local_addr().ok(),
            _ => None,
        }
    }

    pub fn owns(&self, pending: &Pending) -> bool {
        self.pool.owns(pending)
    }

    pub fn outstanding(&self) -> usize {
        self.pool.outstanding()
    }

    pub fn park_until(&self, bound: Duration) -> Result<(), Diagnostic> {
        self.pool.park_until(bound)
    }

    fn waiting(
        &self,
        span: Span,
        label: &'static str,
        what: &'static str,
        job: impl FnOnce() -> Done + Send + 'static,
    ) -> Result<HostAnswer, Diagnostic> {
        self.pool
            .submit(span, label, what, Box::new(job))
            .map(HostAnswer::Pending)
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
            Op::Accept => "ply_host::tcp::accept",
            Op::Recv => "ply_host::tcp::recv",
            Op::Send => "ply_host::tcp::send",
            Op::Close => "ply_host::tcp::close",
        }
    }

    fn listen(&self, at: &Resource, port: u16, span: Span) -> Result<HostAnswer, Diagnostic> {
        let listener = super::bind(Op::Listen.what(), port, span)?;
        Ok(HostAnswer::Value(Value::Int(self.sockets.insert(
            Some(at),
            Sock::Listener(Arc::new(listener), None),
        ))))
    }

    fn listen_tls(
        &self,
        at: &Resource,
        port: u16,
        credential: &str,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        let (listener, config) = tls::listen(&self.credentials, credential, port, span)?;
        Ok(HostAnswer::Value(Value::Int(self.sockets.insert(
            Some(at),
            Sock::Listener(Arc::new(listener), Some(config)),
        ))))
    }

    /// No handshake here, deliberately: the session handshakes on its first read or write.
    fn accept(&self, at: &Resource, listener: i64, span: Span) -> Result<HostAnswer, Diagnostic> {
        // Before the lookup, because `stop_accepting` has already swapped the listener out.
        if self.stopping.load(Ordering::Acquire) {
            self.sockets.handles.check(listener, at, span)?;
            return self.waiting(span, "accept", Op::Accept.what(), || Done::Int(0));
        }
        let (listener, config) = self.sockets.listener(listener, at, span)?;
        let sockets = Arc::clone(&self.sockets);
        let handshakes = Arc::clone(&self.handshakes);
        let stopping = Arc::clone(&self.stopping);
        let accepts = Arc::clone(&self.accepts);
        accepts.fetch_add(1, Ordering::AcqRel);
        self.waiting(span, "accept", Op::Accept.what(), move || {
            let done = match listener.accept() {
                // Taken as the run stopped accepting: the drain's wake dial, or a client racing it.
                Ok(_) if stopping.load(Ordering::Acquire) => Done::Int(0),
                // No label: the connection takes whichever one the program first uses it under.
                Ok((stream, _)) => {
                    let stream = Arc::new(stream);
                    let sock = match config {
                        Some(config) => {
                            Sock::Tls(Arc::new(tls::Session::new(config, stream, handshakes)))
                        }
                        None => Sock::Stream(stream),
                    };
                    Done::Int(sockets.insert(None, sock))
                }
                // `0` is never a live handle: handles ascend from 1 and are never reused.
                Err(e) if transient(&e) => Done::Int(retry_accept(
                    &listener,
                    &sockets,
                    &config,
                    &handshakes,
                    &stopping,
                )),
                Err(_) => Done::Int(0),
            };
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
            Conn::Plain(stream) => {
                // This job owns the socket for its duration, so setting its timeout is safe.
                let _ = stream.set_read_timeout(Some(timeout));
                let mut buffer = vec![0u8; max];
                // One `read`: a loop would hide short reads and closes from the program.
                match (&*stream).read(&mut buffer) {
                    Ok(n) => {
                        buffer.truncate(n);
                        Done::MaybeBytes(Some(buffer))
                    }
                    Err(e) if expired(&e) => Done::MaybeBytes(None),
                    Err(e) if peer_gone(&e) => Done::MaybeBytes(Some(Vec::new())),
                    Err(e) => Done::Failed(e.to_string()),
                }
            }
            // Never `Failed`: any TLS failure is "the peer went away", so the accept loop survives.
            Conn::Tls(session) => {
                session.deadline(timeout);
                Done::MaybeBytes(session.read(max))
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
            Conn::Plain(stream) => {
                let _ = stream.set_write_timeout(Some(timeout));
                // One `write`, which may be short under backpressure; `std.net.send_all` loops.
                match (&*stream).write(&payload) {
                    Ok(n) => Done::MaybeInt(Some(n as i64)),
                    Err(e) if expired(&e) => Done::MaybeInt(None),
                    Err(e) if peer_gone(&e) => Done::MaybeInt(Some(0)),
                    Err(e) => Done::Failed(e.to_string()),
                }
            }
            Conn::Tls(session) => {
                session.deadline(timeout);
                Done::MaybeInt(Some(session.write(&payload) as i64))
            }
        })
    }

    fn close(&self, at: &Resource, socket: i64, span: Span) -> Result<HostAnswer, Diagnostic> {
        self.sockets.handles.check(socket, at, span)?;
        let sock = lock(&self.sockets.open).remove(&socket);
        self.sockets.handles.close(socket);
        match sock {
            // Shut down, not just dropped: a `recv` parked on another `Arc` returns only then.
            Some(Sock::Stream(s)) => {
                let _ = s.shutdown(Shutdown::Both);
                Ok(HostAnswer::Value(Value::Unit))
            }
            // `close_notify` first, so the peer sees a clean end rather than a truncation.
            Some(Sock::Tls(s)) => {
                s.close();
                Ok(HostAnswer::Value(Value::Unit))
            }
            Some(Sock::Listener(..) | Sock::Finished) => Ok(HostAnswer::Value(Value::Unit)),
            None => Err(unknown_handle(socket, span)),
        }
    }
}

impl crate::signal::Accepting for TcpHost {
    fn stop_accepting(&self) -> usize {
        // Flag before swap, or `accept` could find its listener gone and raise `E0502`.
        self.stopping.store(true, Ordering::Release);
        let mut open = lock(&self.sockets.open);
        let listeners: Vec<i64> = open
            .iter()
            .filter(|(_, s)| matches!(s, Sock::Listener(..)))
            .map(|(handle, _)| *handle)
            .collect();
        let mut closed = Vec::new();
        for handle in &listeners {
            if let Some(Sock::Listener(l, _)) = open.get(handle)
                && let Ok(address) = l.local_addr()
            {
                closed.push(address);
            }
            open.insert(*handle, Sock::Finished);
        }
        *lock(&self.closed_at) = closed;
        // The fd closes when the parked `accept` job drops its `Arc`, shortly after this returns.
        listeners.len()
    }

    fn listening_at(&self) -> Vec<SocketAddr> {
        let mut addresses: Vec<SocketAddr> = lock(&self.sockets.open)
            .values()
            .filter_map(|s| match s {
                Sock::Listener(l, _) => l.local_addr().ok(),
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
    fn poll(&self, pending: &Pending) -> Result<Option<Value>, Diagnostic> {
        self.pool.poll(pending)
    }

    fn park(&self) -> Result<(), Diagnostic> {
        self.pool.park()
    }

    fn block_on(&self, pending: Pending) -> Result<Value, Diagnostic> {
        self.pool.block_on(pending)
    }
}

fn expired(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::Interrupted
    )
}

/// An ordinary outcome, not the program's error: end of stream for a read, `0` for a write.
fn peer_gone(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::NotConnected
            | std::io::ErrorKind::UnexpectedEof
    )
}

fn transient(e: &std::io::Error) -> bool {
    expired(e) || matches!(e.kind(), std::io::ErrorKind::ConnectionAborted)
}

/// How many peers may abort between the SYN and the accept before the loop gives up on this call.
const ACCEPT_RETRIES: usize = 16;

fn retry_accept(
    listener: &TcpListener,
    sockets: &Sockets,
    config: &Option<Arc<ServerConfig>>,
    handshakes: &Arc<Handshakes>,
    stopping: &AtomicBool,
) -> i64 {
    for _ in 0..ACCEPT_RETRIES {
        if stopping.load(Ordering::Acquire) {
            return 0;
        }
        match listener.accept() {
            Ok(_) if stopping.load(Ordering::Acquire) => return 0,
            Ok((stream, _)) => {
                let stream = Arc::new(stream);
                // The listener's transport: a retry that dropped TLS would serve in the clear.
                let sock = match config {
                    Some(config) => Sock::Tls(Arc::new(tls::Session::new(
                        Arc::clone(config),
                        stream,
                        Arc::clone(handshakes),
                    ))),
                    None => Sock::Stream(stream),
                };
                return sockets.insert(None, sock);
            }
            Err(e) if transient(&e) => continue,
            Err(_) => return 0,
        }
    }
    0
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
