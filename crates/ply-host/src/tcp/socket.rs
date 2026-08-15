//! The real one: `net` over loopback TCP, plaintext or TLS.
//!
//! `listen`, `listen_tls` and `close` are syscalls that do not wait, so they
//! answer [`HostAnswer::Value`]. `accept`, `recv` and `send` do wait, so they
//! answer [`HostAnswer::Pending`] over [`Pool`] and never occupy the thread the
//! scheduler is running tasks on.
//!
//! A TLS listener differs from a plaintext one in exactly two places: `accept`
//! wraps the connection in a [`tls::Session`] — doing no I/O, so one client
//! sending garbage cannot take down the accept loop — and `recv`/`send` go
//! through that session's record layer. Everything above the boundary is the
//! same code reading the same bytes, which is the whole point of TLS not being
//! a separate effect.

use super::pool::{Done, Pool};
use super::{Handles, Net, Op, not_a_listener, not_a_stream, unknown_handle};
use crate::tls::{self, Credentials, Handshakes};
use ply_core::ty::Resource;
use ply_eval::{HostAnswer, HostRuntime, Pending, Value};
use ply_span::{Diagnostic, Span};
use rustls::server::ServerConfig;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

enum Sock {
    /// `None` for a plaintext listener; a TLS listener carries the
    /// configuration every connection it accepts is terminated with.
    Listener(Arc<TcpListener>, Option<Arc<ServerConfig>>),
    Stream(Arc<TcpStream>),
    Tls(Arc<tls::Session>),
}

/// An accepted connection, whichever transport carries it. `recv` and `send`
/// are written once against this rather than twice against the two.
enum Conn {
    Plain(Arc<TcpStream>),
    Tls(Arc<tls::Session>),
}

/// The socket table and the handle allocator, in one [`Arc`] because a pool
/// thread completing an `accept` has to insert into both.
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
            None => Err(unknown_handle(handle, span)),
        }
    }

    fn stream(&self, handle: i64, at: &Resource, span: Span) -> Result<Conn, Diagnostic> {
        self.handles.check(handle, at, span)?;
        match lock(&self.open).get(&handle) {
            Some(Sock::Stream(s)) => Ok(Conn::Plain(Arc::clone(s))),
            Some(Sock::Tls(s)) => Ok(Conn::Tls(Arc::clone(s))),
            Some(Sock::Listener(..)) => Err(not_a_stream(handle, span)),
            None => Err(unknown_handle(handle, span)),
        }
    }
}

/// The TCP host handler's state: one socket table and one blocking pool.
///
/// Also the [`HostRuntime`] that resolves the tokens it mints. That is not where
/// the boundary's design puts the runtime, and the reason it is here is that
/// `HostRuntime` has no submission API: a handler that answers `Pending` has to
/// own the facility that resolves it. [`TcpHost::owns`] exists so a composed
/// runtime can route a token to whoever minted it.
pub struct TcpHost {
    sockets: Arc<Sockets>,
    pool: Pool,
    /// What `net.listen_tls` resolves a credential name against. Empty unless
    /// the run was given `--tls`, which is why an unconfigured name is `E0429`
    /// listing what there is rather than a socket that speaks plaintext.
    credentials: Credentials,
    handshakes: Arc<Handshakes>,
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
            pool: Pool::new(),
            credentials,
            handshakes: Arc::new(Handshakes::default()),
        }
    }

    pub fn credentials(&self) -> &Credentials {
        &self.credentials
    }

    /// What the run's `--host` summary reports about TLS: how many handshakes
    /// completed, how many were refused, and why.
    pub fn handshakes(&self) -> tls::HandshakeCounts {
        self.handshakes.snapshot()
    }

    /// The address a listening handle actually bound.
    ///
    /// Rust-side only: W1's `net` has no operation for it, so a Ply program that
    /// binds port 0 cannot discover the port it got. A test can, which is what
    /// this is for.
    pub fn local_addr(&self, handle: i64) -> Option<SocketAddr> {
        match lock(&self.sockets.open).get(&handle) {
            Some(Sock::Listener(l, _)) => l.local_addr().ok(),
            _ => None,
        }
    }

    /// Whether this host minted the token. A composed [`HostRuntime`] routes on
    /// it; polling the wrong facility loses a result rather than waiting for it.
    pub fn owns(&self, pending: &Pending) -> bool {
        self.pool.owns(pending)
    }

    pub fn outstanding(&self) -> usize {
        self.pool.outstanding()
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

    /// `net.recv` and `net.send` say `tcp` for both transports, because that is
    /// the handler the registry resolves; what routes a particular socket
    /// through rustls is which listener accepted it, and `ply hosts` makes that
    /// visible with its `transport` block rather than by splitting these rows.
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

    /// No handshake here, deliberately. A handshake inside `accept` is one
    /// client sending garbage taking down the accept loop, which is a denial of
    /// service delivered by design; [`tls::Session::new`] does no I/O and the
    /// first `recv` or `send` completes the handshake.
    fn accept(&self, at: &Resource, listener: i64, span: Span) -> Result<HostAnswer, Diagnostic> {
        let (listener, config) = self.sockets.listener(listener, at, span)?;
        let sockets = Arc::clone(&self.sockets);
        let handshakes = Arc::clone(&self.handshakes);
        self.waiting(span, "accept", Op::Accept.what(), move || {
            match listener.accept() {
                // No label: `accept` names the listener's, and the connection's
                // is whichever one the program first reads or writes it under.
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
                // A listener that is finished answers `0`, and handles ascend
                // from 1 and are never reused, so `0` is never a live socket.
                // A peer that aborted between the SYN and the accept is that
                // peer's business and not the accept loop's, so it is retried.
                Err(e) if transient(&e) => {
                    Done::Int(retry_accept(&listener, &sockets, &config, &handshakes))
                }
                Err(_) => Done::Int(0),
            }
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
                // The deadline, as one `setsockopt` on a socket this job owns
                // for its duration. That is the whole of ADR 0013 §7.2: no token
                // registry, and no race between a cancel and a completion.
                let _ = stream.set_read_timeout(Some(timeout));
                let mut buffer = vec![0u8; max];
                // One `read`, deliberately: a short answer is what a partial
                // read looks like and an empty one is what a peer's close looks
                // like, and a loop here would hide both from the program that
                // has to handle them.
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
            // Never `Failed`: a handshake that fails, a peer that resets and a
            // record that will not decrypt are all "the peer went away", which
            // is the path the server already has and the reason the accept loop
            // survives a client sending nonsense.
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
                // One `write`, which may take fewer bytes than it was given —
                // that is what backpressure looks like, and `std.net.send_all`
                // is where it is looped over. A `write_all` here would hide a
                // deadline that expired half way through a response.
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
            // Shut down rather than only dropping: another Arc of this stream may
            // be parked in a `recv` on a pool thread, and closing the fd is what
            // returns that thread.
            Some(Sock::Stream(s)) => {
                let _ = s.shutdown(Shutdown::Both);
                Ok(HostAnswer::Value(Value::Unit))
            }
            // A `close_notify` first, so a peer sees a clean end rather than a
            // truncation it is right to treat as an attack.
            Some(Sock::Tls(s)) => {
                s.close();
                Ok(HostAnswer::Value(Value::Unit))
            }
            Some(Sock::Listener(..)) => Ok(HostAnswer::Value(Value::Unit)),
            None => Err(unknown_handle(socket, span)),
        }
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

/// The deadline expired with nothing to show for it. A socket read timeout
/// surfaces as `WouldBlock` on some platforms and `TimedOut` on others, and both
/// mean the same thing to the program above.
fn expired(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::Interrupted
    )
}

/// The peer's misbehaviour, which is an ordinary outcome rather than the
/// program's error: end of stream for a read, `0` for a write.
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

/// How many peers may abort between the SYN and the accept before the loop gives
/// up on this call. Bounded rather than unbounded: an accept that spun forever
/// on a transient error would hold its pool thread exactly as a hang does.
const ACCEPT_RETRIES: usize = 16;

fn retry_accept(
    listener: &TcpListener,
    sockets: &Sockets,
    config: &Option<Arc<ServerConfig>>,
    handshakes: &Arc<Handshakes>,
) -> i64 {
    for _ in 0..ACCEPT_RETRIES {
        match listener.accept() {
            Ok((stream, _)) => {
                let stream = Arc::new(stream);
                // The listener's transport, not a plaintext default: a retry
                // that dropped TLS would serve one connection in the clear.
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
